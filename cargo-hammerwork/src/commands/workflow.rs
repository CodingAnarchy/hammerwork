use anyhow::{Context, Result, anyhow};
use clap::Subcommand;
use hammerwork::queue::DatabaseQueue;
use hammerwork::{FailurePolicy, Job, JobGroup};
use serde_json::Value;
use sqlx::Row;
use std::collections::{HashMap, HashSet, VecDeque};
use tracing::info;
use uuid::Uuid;

use crate::config::Config;
use crate::utils::database::{DatabasePool, JobQueueWrapper};
use crate::utils::sql::{SqlParams, bind_mysql, bind_pg};
use crate::utils::validation::{validate_json_payload, validate_priority};

#[derive(Debug, Clone)]
pub struct JobNode {
    pub id: String,
    pub queue_name: String,
    pub status: String,
    pub dependency_status: String,
    pub depends_on: Vec<String>,
    pub dependents: Vec<String>,
    pub workflow_id: Option<String>,
    pub workflow_name: Option<String>,
}

#[derive(Subcommand)]
pub enum WorkflowCommand {
    #[command(about = "List workflows")]
    List {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short, long, help = "Maximum number of workflows to display")]
        limit: Option<u32>,
        #[arg(long, help = "Show only running workflows")]
        running: bool,
        #[arg(long, help = "Show only completed workflows")]
        completed: bool,
        #[arg(long, help = "Show only failed workflows")]
        failed: bool,
    },
    #[command(about = "Show details of a specific workflow")]
    Show {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(help = "Workflow ID")]
        workflow_id: String,
        #[arg(long, help = "Show dependency graph")]
        dependencies: bool,
    },
    #[command(
        about = "Create a workflow from a JSON file of jobs",
        long_about = "Create a workflow and enqueue its jobs in one transaction.\n\n\
            The jobs file holds a JSON array. Each element is an object with a \"queue\" and a \
            \"payload\", and optionally a \"priority\" and \"depends_on\", a list of indexes \
            of earlier jobs in the array that must complete first.\n\n\
            Example: [{\"queue\": \"etl\", \"payload\": {\"step\": \"extract\"}}, \
            {\"queue\": \"etl\", \"payload\": {\"step\": \"load\"}, \"depends_on\": [0]}]"
    )]
    Create {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', long, help = "Workflow name")]
        name: String,
        #[arg(
            short = 'f',
            long,
            help = "JSON file with the workflow's jobs (see --help)"
        )]
        jobs_file: String,
        #[arg(long, help = "Failure policy (fail_fast, continue_on_failure, manual)")]
        failure_policy: Option<String>,
        #[arg(long, help = "Workflow metadata as JSON")]
        metadata: Option<String>,
    },
    #[command(about = "Cancel a running workflow")]
    Cancel {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(help = "Workflow ID")]
        workflow_id: String,
        #[arg(long, help = "Force cancel even if jobs are running")]
        force: bool,
    },
    #[command(about = "Show job dependencies")]
    Dependencies {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(help = "Job ID")]
        job_id: String,
        #[arg(long, help = "Show dependency tree")]
        tree: bool,
        #[arg(long, help = "Show jobs that depend on this job")]
        dependents: bool,
    },
    #[command(about = "Visualize workflow dependency graph")]
    Graph {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(help = "Workflow ID")]
        workflow_id: String,
        #[arg(long, help = "Output format (text, dot, mermaid, json)")]
        format: Option<String>,
    },
}

/// The first eight characters of an id, as shown in graphs (never splits a character).
fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// A row of `workflow list`.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowSummary {
    pub id: String,
    pub name: String,
    pub status: String,
    pub total_jobs: i32,
    pub completed_jobs: i32,
    pub failed_jobs: i32,
    pub failure_policy: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Workflows, newest first, optionally only those with one of `statuses`
/// (`running`, `completed`, `failed`, `cancelled`).
pub async fn fetch_workflows(
    pool: &DatabasePool,
    limit: u32,
    statuses: &[&str],
) -> Result<Vec<WorkflowSummary>> {
    let mut params = SqlParams::new(pool.backend());
    let id_expr = match pool {
        DatabasePool::Postgres(_) => "CAST(id AS TEXT)",
        DatabasePool::MySQL(_) => "id",
    };
    let mut sql = format!(
        "SELECT {id_expr} AS id, name, status, total_jobs, completed_jobs, failed_jobs, \
         failure_policy, created_at FROM hammerwork_workflows"
    );
    if !statuses.is_empty() {
        let list = statuses
            .iter()
            .map(|s| params.text(s))
            .collect::<Vec<_>>()
            .join(", ");
        sql.push_str(&format!(" WHERE status IN ({list})"));
    }
    sql.push_str(&format!(
        " ORDER BY created_at DESC, id {}",
        params.limit(limit)
    ));

    macro_rules! summaries {
        ($rows:expr) => {{
            let mut out = Vec::new();
            for row in $rows {
                out.push(WorkflowSummary {
                    id: row.try_get("id")?,
                    name: row.try_get("name")?,
                    status: row.try_get("status")?,
                    total_jobs: row.try_get("total_jobs")?,
                    completed_jobs: row.try_get("completed_jobs")?,
                    failed_jobs: row.try_get("failed_jobs")?,
                    failure_policy: row.try_get("failure_policy")?,
                    created_at: row.try_get("created_at")?,
                });
            }
            out
        }};
    }
    Ok(match pool {
        DatabasePool::Postgres(p) => {
            summaries!(
                bind_pg(sqlx::query(&sql), params.binds())
                    .fetch_all(p)
                    .await?
            )
        }
        DatabasePool::MySQL(p) => {
            summaries!(
                bind_mysql(sqlx::query(&sql), params.binds())
                    .fetch_all(p)
                    .await?
            )
        }
    })
}

pub fn render_workflow_list(workflows: &[WorkflowSummary]) -> String {
    if workflows.is_empty() {
        return "No workflows found.".to_string();
    }
    let mut table = comfy_table::Table::new();
    table.set_header(vec![
        "ID",
        "Name",
        "Status",
        "Jobs",
        "Completed",
        "Failed",
        "Policy",
        "Created",
    ]);
    for w in workflows {
        table.add_row(vec![
            w.id.clone(),
            w.name.clone(),
            w.status.clone(),
            w.total_jobs.to_string(),
            w.completed_jobs.to_string(),
            w.failed_jobs.to_string(),
            w.failure_policy.clone(),
            w.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
        ]);
    }
    table.to_string()
}

/// Parse the jobs file of `workflow create` into a workflow.
///
/// Every entry needs a `queue` and a `payload`; `depends_on` lists indexes of earlier entries.
pub fn build_workflow(
    name: &str,
    policy: FailurePolicy,
    metadata: Value,
    jobs: &Value,
) -> Result<JobGroup> {
    let entries = jobs
        .as_array()
        .ok_or_else(|| anyhow!("the jobs file must contain a JSON array"))?;
    if entries.is_empty() {
        return Err(anyhow!("the jobs file must contain at least one job"));
    }

    let mut workflow = JobGroup::new(name)
        .with_failure_policy(policy)
        .with_metadata(metadata);
    let mut ids = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let at = |msg: String| anyhow!("job {}: {}", index, msg);
        let object = entry
            .as_object()
            .ok_or_else(|| at("must be a JSON object".into()))?;
        let queue = object
            .get("queue")
            .and_then(Value::as_str)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| at("missing 'queue'".into()))?;
        let payload = object
            .get("payload")
            .ok_or_else(|| at("missing 'payload'".into()))?
            .clone();
        let mut job = Job::new(queue.to_string(), payload);
        if let Some(priority) = object.get("priority") {
            let priority = priority
                .as_str()
                .ok_or_else(|| at("'priority' must be a string".into()))?;
            job = job.with_priority(validate_priority(priority).map_err(|e| at(e.to_string()))?);
        }
        let mut dependencies = Vec::new();
        if let Some(deps) = object.get("depends_on") {
            for dep in deps
                .as_array()
                .ok_or_else(|| at("'depends_on' must be an array of job indexes".into()))?
            {
                let dep_index = dep
                    .as_u64()
                    .and_then(|i| usize::try_from(i).ok())
                    .ok_or_else(|| at("'depends_on' entries must be job indexes".into()))?;
                let dep_id = ids.get(dep_index).copied().ok_or_else(|| {
                    at(format!(
                        "depends_on {} must refer to an earlier job",
                        dep_index
                    ))
                })?;
                dependencies.push(dep_id);
            }
        }
        job = job.depends_on_jobs(&dependencies);
        ids.push(job.id);
        if !dependencies.is_empty() {
            workflow.dependencies.insert(job.id, dependencies);
        }
        workflow = workflow.add_job(job);
    }
    Ok(workflow)
}

impl WorkflowCommand {
    pub async fn execute(&self, config: Config) -> Result<()> {
        let db_url = self.get_database_url(&config)?;
        let pool = DatabasePool::connect_with_config(&db_url, &config).await?;

        match self {
            WorkflowCommand::List {
                limit,
                running,
                completed,
                failed,
                ..
            } => {
                self.list_workflows(pool, *limit, *running, *completed, *failed)
                    .await
            }
            WorkflowCommand::Show {
                workflow_id,
                dependencies,
                ..
            } => self.show_workflow(pool, workflow_id, *dependencies).await,
            WorkflowCommand::Create {
                name,
                jobs_file,
                failure_policy,
                metadata,
                ..
            } => {
                self.create_workflow(
                    pool,
                    &config.encryption_settings()?,
                    name,
                    jobs_file,
                    failure_policy.as_deref(),
                    metadata.as_deref(),
                )
                .await
            }
            WorkflowCommand::Cancel {
                workflow_id, force, ..
            } => self.cancel_workflow(pool, workflow_id, *force).await,
            WorkflowCommand::Dependencies {
                job_id,
                tree,
                dependents,
                ..
            } => {
                self.show_dependencies(pool, job_id, *tree, *dependents)
                    .await
            }
            WorkflowCommand::Graph {
                workflow_id,
                format,
                ..
            } => self.show_graph(pool, workflow_id, format.as_deref()).await,
        }
    }

    pub fn get_database_url(&self, config: &Config) -> Result<String> {
        let url_option = match self {
            WorkflowCommand::List { database_url, .. } => database_url,
            WorkflowCommand::Show { database_url, .. } => database_url,
            WorkflowCommand::Create { database_url, .. } => database_url,
            WorkflowCommand::Cancel { database_url, .. } => database_url,
            WorkflowCommand::Dependencies { database_url, .. } => database_url,
            WorkflowCommand::Graph { database_url, .. } => database_url,
        };

        url_option
            .as_ref()
            .map(|s| s.as_str())
            .or(config.get_database_url())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Database URL is required"))
    }

    async fn list_workflows(
        &self,
        pool: DatabasePool,
        limit: Option<u32>,
        running: bool,
        completed: bool,
        failed: bool,
    ) -> Result<()> {
        let mut statuses = Vec::new();
        if running {
            statuses.push("running");
        }
        if completed {
            statuses.push("completed");
        }
        if failed {
            statuses.push("failed");
        }
        let workflows = fetch_workflows(&pool, limit.unwrap_or(50), &statuses).await?;
        println!("{}", render_workflow_list(&workflows));
        Ok(())
    }

    async fn show_workflow(
        &self,
        pool: DatabasePool,
        workflow_id: &str,
        show_dependencies: bool,
    ) -> Result<()> {
        let id = Uuid::parse_str(workflow_id)
            .map_err(|e| anyhow!("Invalid workflow ID '{}': {}", workflow_id, e))?;
        let queue = pool.clone().create_job_queue();
        let workflow = match &queue {
            JobQueueWrapper::Postgres(q) => q.get_workflow_status(id).await?,
            JobQueueWrapper::MySQL(q) => q.get_workflow_status(id).await?,
        }
        .ok_or_else(|| anyhow!("Workflow not found: {}", workflow_id))?;
        let jobs = self.get_workflow_jobs(&pool, workflow_id).await?;

        println!("{}", render_workflow_details(&workflow, &jobs));
        if show_dependencies && !jobs.is_empty() {
            println!("{}", render_text_graph(self, &jobs));
        }
        Ok(())
    }

    async fn create_workflow(
        &self,
        pool: DatabasePool,
        encryption: &hammerwork::config::PayloadEncryptionConfig,
        name: &str,
        jobs_file: &str,
        failure_policy: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<()> {
        // Parse failure policy
        let policy = match failure_policy {
            Some("fail_fast") => FailurePolicy::FailFast,
            Some("continue_on_failure") => FailurePolicy::ContinueOnFailure,
            Some("manual") => FailurePolicy::Manual,
            Some(p) => {
                return Err(anyhow::anyhow!(
                    "Invalid failure policy: {}. Use: fail_fast, continue_on_failure, manual",
                    p
                ));
            }
            None => FailurePolicy::FailFast,
        };

        // Parse metadata
        let metadata_json = match metadata {
            Some(m) => validate_json_payload(m)?,
            None => serde_json::Value::Object(serde_json::Map::new()),
        };

        let text = std::fs::read_to_string(jobs_file)
            .with_context(|| format!("Cannot read jobs file {}", jobs_file))?;
        let jobs: Value = serde_json::from_str(&text)
            .map_err(|e| anyhow!("{} is not valid JSON: {}", jobs_file, e))?;
        let workflow = build_workflow(name, policy, metadata_json, &jobs)?;
        let (id, total, policy) = (
            workflow.id,
            workflow.total_jobs,
            format!("{:?}", workflow.failure_policy),
        );

        // Encrypts like the application, and never writes plaintext to encrypted queues
        match pool.create_enqueue_queue(encryption).await? {
            JobQueueWrapper::Postgres(q) => q.enqueue_workflow(workflow).await?,
            JobQueueWrapper::MySQL(q) => q.enqueue_workflow(workflow).await?,
        };

        println!("Workflow created:");
        println!("  ID: {}", id);
        println!("  Name: {}", name);
        println!("  Failure Policy: {}", policy);
        println!("  Jobs: {}", total);

        info!("Created workflow '{}' with ID {}", name, id);
        Ok(())
    }

    async fn cancel_workflow(
        &self,
        pool: DatabasePool,
        workflow_id: &str,
        force: bool,
    ) -> Result<()> {
        let id = Uuid::parse_str(workflow_id)
            .map_err(|e| anyhow!("Invalid workflow ID '{}': {}", workflow_id, e))?;
        let queue = pool.clone().create_job_queue();
        let workflow = match &queue {
            JobQueueWrapper::Postgres(q) => q.get_workflow_status(id).await?,
            JobQueueWrapper::MySQL(q) => q.get_workflow_status(id).await?,
        }
        .ok_or_else(|| anyhow!("Workflow not found: {}", workflow_id))?;

        let jobs = self.get_workflow_jobs(&pool, workflow_id).await?;
        let running = jobs.iter().filter(|j| j.status == "Running").count();
        if running > 0 && !force {
            return Err(anyhow!(
                "Workflow {} has {} running job(s); use --force to cancel it anyway",
                workflow_id,
                running
            ));
        }

        match &queue {
            JobQueueWrapper::Postgres(q) => q.cancel_workflow(id).await?,
            JobQueueWrapper::MySQL(q) => q.cancel_workflow(id).await?,
        }
        println!("Workflow '{}' ({}) cancelled", workflow.name, workflow_id);
        Ok(())
    }

    async fn show_dependencies(
        &self,
        pool: DatabasePool,
        job_id: &str,
        show_tree: bool,
        show_dependents: bool,
    ) -> Result<()> {
        let job_uuid = Uuid::parse_str(job_id)?;

        // Get the target job details
        let target_job = self
            .get_job_node(&pool, &job_uuid)
            .await?
            .ok_or_else(|| anyhow!("Job not found: {}", job_id))?;

        let mut out = format!(
            "Job Dependencies for {}\nQueue: {}\nStatus: {}\nDependency Status: {}",
            job_id, target_job.queue_name, target_job.status, target_job.dependency_status
        );
        if let Some(workflow_name) = &target_job.workflow_name {
            out.push_str(&format!("\nWorkflow: {}", workflow_name));
        }

        // Show immediate dependencies
        if !target_job.depends_on.is_empty() {
            out.push_str("\n\nDirect Dependencies:");
            for dep_id in &target_job.depends_on {
                out.push_str(&self.describe_neighbour(&pool, dep_id).await?);
            }
        } else {
            out.push_str("\n\nNo direct dependencies");
        }

        // Show immediate dependents if requested
        if show_dependents {
            if !target_job.dependents.is_empty() {
                out.push_str("\n\nDirect Dependents:");
                for dep_id in &target_job.dependents {
                    out.push_str(&self.describe_neighbour(&pool, dep_id).await?);
                }
            } else {
                out.push_str("\n\nNo direct dependents");
            }
        }

        // Show full dependency tree if requested
        if show_tree {
            out.push_str("\n\nDependency Tree:");

            // Build dependency graph for the workflow or just this job
            let jobs = if let Some(workflow_id) = &target_job.workflow_id {
                self.get_workflow_jobs(&pool, workflow_id).await?
            } else {
                // Collect all related jobs by traversing dependencies
                self.collect_related_jobs(&pool, &target_job).await?
            };

            if jobs.is_empty() {
                out.push_str("\n  No related jobs found");
            } else {
                out.push('\n');
                out.push_str(&render_dependency_tree(&jobs, &target_job.id));
            }
        }

        println!("{}", out);
        Ok(())
    }

    async fn describe_neighbour(&self, pool: &DatabasePool, id: &str) -> Result<String> {
        Ok(match self.get_job_node_by_string(pool, id).await? {
            Some(job) => format!("\n  ├─ {} ({})", id, job.status),
            None => format!("\n  ├─ {} (not found)", id),
        })
    }

    async fn show_graph(
        &self,
        pool: DatabasePool,
        workflow_id: &str,
        format: Option<&str>,
    ) -> Result<()> {
        let format = format.unwrap_or("text");
        if !["text", "dot", "mermaid", "json"].contains(&format) {
            return Err(anyhow!(
                "Unsupported format: {}. Use: text, dot, mermaid, json",
                format
            ));
        }

        // Get all jobs in the workflow
        let jobs = self.get_workflow_jobs(&pool, workflow_id).await?;

        if jobs.is_empty() {
            println!("No jobs found in workflow: {}", workflow_id);
            return Ok(());
        }

        println!("Workflow Graph: {}", workflow_id);
        println!("Format: {}", format);
        println!("Jobs: {}", jobs.len());
        println!("{}", render_graph(self, &jobs, workflow_id, format)?);
        Ok(())
    }

    pub fn print_json_graph(&self, jobs: &[JobNode]) -> Result<()> {
        println!("{}", render_json_graph(jobs)?);
        Ok(())
    }

    pub fn calculate_dependency_levels<'a>(
        &self,
        jobs: &'a [JobNode],
    ) -> Vec<(usize, Vec<&'a JobNode>)> {
        let job_map: HashMap<String, &JobNode> =
            jobs.iter().map(|job| (job.id.clone(), job)).collect();

        let mut levels = HashMap::new();
        let mut visited = HashSet::new();

        for job in jobs {
            if !visited.contains(&job.id) {
                Self::calculate_job_level(job, &job_map, &mut levels, &mut visited, 0);
            }
        }

        // Group jobs by level
        let mut result: HashMap<usize, Vec<&JobNode>> = HashMap::new();
        for (job_id, level) in levels {
            if let Some(job) = job_map.get(&job_id) {
                result.entry(level).or_default().push(job);
            }
        }

        let mut result: Vec<_> = result.into_iter().collect();
        result.sort_by_key(|(level, _)| *level);
        for (_, at_level) in &mut result {
            at_level.sort_by(|a, b| a.id.cmp(&b.id));
        }
        result
    }

    fn calculate_job_level(
        job: &JobNode,
        job_map: &HashMap<String, &JobNode>,
        levels: &mut HashMap<String, usize>,
        visited: &mut HashSet<String>,
        _current_level: usize,
    ) {
        if visited.contains(&job.id) {
            return;
        }

        visited.insert(job.id.clone());

        // Calculate max dependency level
        let max_dep_level = job
            .depends_on
            .iter()
            .filter_map(|dep_id| {
                if let Some(dep_job) = job_map.get(dep_id) {
                    if !visited.contains(dep_id) {
                        Self::calculate_job_level(
                            dep_job,
                            job_map,
                            levels,
                            visited,
                            _current_level,
                        );
                    }
                    levels.get(dep_id).copied()
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(0);

        let job_level = max_dep_level + if job.depends_on.is_empty() { 0 } else { 1 };
        levels.insert(job.id.clone(), job_level);
    }

    // Helper methods for dependency tree visualization

    pub async fn get_job_node(
        &self,
        pool: &DatabasePool,
        job_id: &Uuid,
    ) -> Result<Option<JobNode>> {
        let query = r#"
            SELECT id, queue_name, status, dependency_status, depends_on, dependents, workflow_id, workflow_name
            FROM hammerwork_jobs
            WHERE id = $1
        "#;

        match pool {
            DatabasePool::Postgres(pg_pool) => {
                if let Some(row) = sqlx::query(query)
                    .bind(job_id)
                    .fetch_optional(pg_pool)
                    .await?
                {
                    let mut node = self.postgres_row_to_job_node(&row)?;
                    merge_ids(&mut node.dependents, fetch_dependents(pool, job_id).await?);
                    Ok(Some(node))
                } else {
                    Ok(None)
                }
            }
            DatabasePool::MySQL(mysql_pool) => {
                let mysql_query = r#"
                    SELECT id, queue_name, status, dependency_status, depends_on, dependents, workflow_id, workflow_name
                    FROM hammerwork_jobs
                    WHERE id = ?
                "#;
                if let Some(row) = sqlx::query(mysql_query)
                    .bind(job_id.to_string())
                    .fetch_optional(mysql_pool)
                    .await?
                {
                    let mut node = self.mysql_row_to_job_node(&row)?;
                    merge_ids(&mut node.dependents, fetch_dependents(pool, job_id).await?);
                    Ok(Some(node))
                } else {
                    Ok(None)
                }
            }
        }
    }

    async fn get_job_node_by_string(
        &self,
        pool: &DatabasePool,
        job_id: &str,
    ) -> Result<Option<JobNode>> {
        let uuid = Uuid::parse_str(job_id)?;
        self.get_job_node(pool, &uuid).await
    }

    pub async fn get_workflow_jobs(
        &self,
        pool: &DatabasePool,
        workflow_id: &str,
    ) -> Result<Vec<JobNode>> {
        let query = r#"
            SELECT id, queue_name, status, dependency_status, depends_on, dependents, workflow_id, workflow_name
            FROM hammerwork_jobs
            WHERE workflow_id = $1
            ORDER BY created_at, id
        "#;

        let workflow_uuid = Uuid::parse_str(workflow_id)
            .map_err(|e| anyhow!("Invalid workflow ID '{}': {}", workflow_id, e))?;
        match pool {
            DatabasePool::Postgres(pg_pool) => {
                let rows = sqlx::query(query)
                    .bind(workflow_uuid)
                    .fetch_all(pg_pool)
                    .await?;

                let mut jobs = Vec::new();
                for row in rows {
                    jobs.push(self.postgres_row_to_job_node(&row)?);
                }
                fill_dependents(&mut jobs);
                Ok(jobs)
            }
            DatabasePool::MySQL(mysql_pool) => {
                let mysql_query = r#"
                    SELECT id, queue_name, status, dependency_status, depends_on, dependents, workflow_id, workflow_name
                    FROM hammerwork_jobs
                    WHERE workflow_id = ?
                    ORDER BY created_at, id
                "#;
                let rows = sqlx::query(mysql_query)
                    .bind(workflow_uuid.to_string())
                    .fetch_all(mysql_pool)
                    .await?;

                let mut jobs = Vec::new();
                for row in rows {
                    jobs.push(self.mysql_row_to_job_node(&row)?);
                }
                fill_dependents(&mut jobs);
                Ok(jobs)
            }
        }
    }

    async fn collect_related_jobs(
        &self,
        pool: &DatabasePool,
        target_job: &JobNode,
    ) -> Result<Vec<JobNode>> {
        let mut jobs = HashMap::new();
        let mut to_visit = VecDeque::new();
        let mut visited = HashSet::new();

        // Start with the target job
        jobs.insert(target_job.id.clone(), target_job.clone());
        to_visit.push_back(target_job.id.clone());

        // Traverse both dependencies and dependents
        while let Some(job_id) = to_visit.pop_front() {
            if visited.contains(&job_id) {
                continue;
            }
            visited.insert(job_id.clone());

            if let Some(job) = jobs.get(&job_id).cloned() {
                // Visit all dependencies
                for dep_id in &job.depends_on {
                    if !jobs.contains_key(dep_id)
                        && let Some(dep_job) = self.get_job_node_by_string(pool, dep_id).await?
                    {
                        jobs.insert(dep_id.clone(), dep_job);
                        to_visit.push_back(dep_id.clone());
                    }
                }

                // Visit all dependents
                for dep_id in &job.dependents {
                    if !jobs.contains_key(dep_id)
                        && let Some(dep_job) = self.get_job_node_by_string(pool, dep_id).await?
                    {
                        jobs.insert(dep_id.clone(), dep_job);
                        to_visit.push_back(dep_id.clone());
                    }
                }
            }
        }

        let mut jobs: Vec<JobNode> = jobs.into_values().collect();
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(jobs)
    }

    fn postgres_row_to_job_node(&self, row: &sqlx::postgres::PgRow) -> Result<JobNode> {
        let id: Uuid = row.try_get("id")?;
        // depends_on and dependents are UUID[] columns in PostgreSQL
        let uuid_array = |column: &str| -> Result<Vec<String>> {
            Ok(row
                .try_get::<Option<Vec<Uuid>>, _>(column)?
                .unwrap_or_default()
                .iter()
                .map(Uuid::to_string)
                .collect())
        };
        let depends_on = uuid_array("depends_on")?;
        let dependents = uuid_array("dependents")?;

        let workflow_id: Option<String> = row
            .try_get::<Option<Uuid>, _>("workflow_id")?
            .map(|uuid| uuid.to_string());

        Ok(JobNode {
            id: id.to_string(),
            queue_name: row.try_get("queue_name")?,
            status: row.try_get("status")?,
            dependency_status: row
                .try_get::<Option<String>, _>("dependency_status")?
                .unwrap_or_else(|| "none".to_string()),
            depends_on,
            dependents,
            workflow_id,
            workflow_name: row.try_get("workflow_name")?,
        })
    }

    fn mysql_row_to_job_node(&self, row: &sqlx::mysql::MySqlRow) -> Result<JobNode> {
        let id: String = row.try_get("id")?;
        let depends_on = self.parse_json_array(row.try_get("depends_on")?)?;
        let dependents = self.parse_json_array(row.try_get("dependents")?)?;

        let workflow_id: Option<String> = row.try_get("workflow_id")?;

        Ok(JobNode {
            id,
            queue_name: row.try_get("queue_name")?,
            status: row.try_get("status")?,
            dependency_status: row
                .try_get::<Option<String>, _>("dependency_status")?
                .unwrap_or_else(|| "none".to_string()),
            depends_on,
            dependents,
            workflow_id,
            workflow_name: row.try_get("workflow_name")?,
        })
    }

    pub fn parse_json_array(&self, json_value: Option<Value>) -> Result<Vec<String>> {
        match json_value {
            Some(Value::Array(arr)) => Ok(arr
                .into_iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()),
            _ => Ok(Vec::new()),
        }
    }
}

fn merge_ids(into: &mut Vec<String>, more: Vec<String>) {
    for id in more {
        if !into.contains(&id) {
            into.push(id);
        }
    }
}

/// The library records only `depends_on`; the `dependents` column is never written. Derive
/// each job's dependents from the dependencies of the jobs in `jobs`.
fn fill_dependents(jobs: &mut [JobNode]) {
    let edges: Vec<(String, String)> = jobs
        .iter()
        .flat_map(|job| {
            job.depends_on
                .iter()
                .map(move |dep| (dep.clone(), job.id.clone()))
        })
        .collect();
    for job in jobs.iter_mut() {
        let kids = edges
            .iter()
            .filter(|(dep, _)| *dep == job.id)
            .map(|(_, kid)| kid.clone())
            .collect();
        merge_ids(&mut job.dependents, kids);
    }
}

/// Ids of the jobs that list `job_id` in their `depends_on`.
async fn fetch_dependents(pool: &DatabasePool, job_id: &Uuid) -> Result<Vec<String>> {
    Ok(match pool {
        DatabasePool::Postgres(pg_pool) => sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM hammerwork_jobs WHERE depends_on @> ARRAY[$1::uuid] \
                 ORDER BY created_at, id",
        )
        .bind(job_id)
        .fetch_all(pg_pool)
        .await?
        .iter()
        .map(Uuid::to_string)
        .collect(),
        DatabasePool::MySQL(mysql_pool) => {
            sqlx::query_scalar::<_, String>(
                "SELECT id FROM hammerwork_jobs WHERE JSON_CONTAINS(depends_on, ?) \
             ORDER BY created_at, id",
            )
            .bind(serde_json::to_string(&job_id.to_string())?)
            .fetch_all(mysql_pool)
            .await?
        }
    })
}

/// The details printed by `workflow show`.
pub fn render_workflow_details(workflow: &JobGroup, jobs: &[JobNode]) -> String {
    let mut by_status: Vec<(String, usize)> = Vec::new();
    for job in jobs {
        match by_status.iter_mut().find(|(s, _)| *s == job.status) {
            Some((_, n)) => *n += 1,
            None => by_status.push((job.status.clone(), 1)),
        }
    }
    by_status.sort();

    let mut out = format!(
        "Workflow {}\n  Name: {}\n  Status: {}\n  Failure Policy: {:?}\n  Created: {}\n  Jobs: {} ({} completed, {} failed)",
        workflow.id,
        workflow.name,
        workflow.status.as_str(),
        workflow.failure_policy,
        workflow.created_at.format("%Y-%m-%d %H:%M:%S UTC"),
        workflow.total_jobs,
        workflow.completed_jobs,
        workflow.failed_jobs
    );
    if let Some(done) = workflow.completed_at {
        out.push_str(&format!(
            "\n  Completed: {}",
            done.format("%Y-%m-%d %H:%M:%S UTC")
        ));
    }
    if let Some(failed) = workflow.failed_at {
        out.push_str(&format!(
            "\n  Failed: {}",
            failed.format("%Y-%m-%d %H:%M:%S UTC")
        ));
    }
    if !by_status.is_empty() {
        out.push_str("\n\nJobs by status:");
        for (status, count) in &by_status {
            out.push_str(&format!("\n  {}: {}", status, count));
        }
    }
    if workflow.metadata.as_object().is_none_or(|m| !m.is_empty()) {
        out.push_str(&format!(
            "\n\nMetadata: {}",
            serde_json::to_string(&workflow.metadata).unwrap_or_default()
        ));
    }
    out
}

/// A graph of `jobs` in `format` (`text`, `dot`, `mermaid` or `json`).
pub fn render_graph(
    cmd: &WorkflowCommand,
    jobs: &[JobNode],
    workflow_id: &str,
    format: &str,
) -> Result<String> {
    match format {
        "text" => Ok(render_text_graph(cmd, jobs)),
        "dot" => Ok(render_dot_graph(jobs, workflow_id)),
        "mermaid" => Ok(render_mermaid_graph(jobs, workflow_id)),
        "json" => render_json_graph(jobs),
        other => Err(anyhow!(
            "Unsupported format: {}. Use: text, dot, mermaid, json",
            other
        )),
    }
}

fn render_text_graph(cmd: &WorkflowCommand, jobs: &[JobNode]) -> String {
    let mut out = format!("\nDependency Graph (Text Format):\n{}", "=".repeat(50));

    // Group jobs by dependency level
    for (level, jobs_at_level) in cmd.calculate_dependency_levels(jobs) {
        out.push_str(&format!(
            "\n\nLevel {}: {} job(s)",
            level,
            jobs_at_level.len()
        ));
        for job in jobs_at_level {
            let deps_str = if job.depends_on.is_empty() {
                "none".to_string()
            } else {
                format!("{} dependencies", job.depends_on.len())
            };

            out.push_str(&format!(
                "\n  ├─ [{}] {} ({})",
                short_id(&job.id),
                job.status,
                deps_str
            ));
        }
    }
    out
}

fn render_dot_graph(jobs: &[JobNode], workflow_id: &str) -> String {
    let mut lines = vec![
        "\nDOT Graph Format:".to_string(),
        "=".repeat(50),
        format!("digraph workflow_{} {{", workflow_id.replace('-', "_")),
        "  rankdir=TB;".to_string(),
        "  node [shape=box];".to_string(),
    ];

    // Define nodes
    for job in jobs {
        let color = match job.status.as_str() {
            "Completed" => "lightgreen",
            "Failed" => "lightcoral",
            "Running" => "lightblue",
            "Pending" => "lightyellow",
            _ => "lightgray",
        };

        lines.push(format!(
            "  \"{}\" [label=\"{}\\n{}\" fillcolor={} style=filled];",
            job.id,
            short_id(&job.id),
            job.status,
            color
        ));
    }

    // Define edges (dependencies)
    for job in jobs {
        for dep_id in &job.depends_on {
            lines.push(format!("  \"{}\" -> \"{}\";", dep_id, job.id));
        }
    }

    lines.push("}".to_string());
    lines.push(
        "\nTo visualize: copy the above DOT code to https://dreampuf.github.io/GraphvizOnline/"
            .to_string(),
    );
    lines.join("\n")
}

fn render_mermaid_graph(jobs: &[JobNode], workflow_id: &str) -> String {
    let mut lines = vec![
        "\nMermaid Graph Format:".to_string(),
        "=".repeat(50),
        "---".to_string(),
        "title: Hammerwork Workflow Dependency Graph".to_string(),
        "---".to_string(),
        "graph TD".to_string(),
        format!("    subgraph \"📋 Workflow: {}\"", short_id(workflow_id)),
    ];

    // Define nodes with styling
    for job in jobs {
        let short = short_id(&job.id);
        let status_class = match job.status.as_str() {
            "Completed" => ":::completed",
            "Failed" => ":::failed",
            "Running" => ":::running",
            "Pending" => ":::pending",
            _ => ":::default",
        };

        // Node definition with label inside subgraph
        let dependency_indicator = match job.dependency_status.as_str() {
            "waiting" => "⏳",
            "satisfied" => "✅",
            "failed" => "❌",
            _ => "🔵",
        };

        lines.push(format!(
            "        {}[\"{}<br/>{}<br/>{} {}\"]{}",
            short, short, job.status, dependency_indicator, job.queue_name, status_class
        ));
    }

    lines.push(String::new());

    // Define edges (dependencies) inside subgraph
    for job in jobs {
        for dep_id in &job.depends_on {
            lines.push(format!(
                "        {} --> {}",
                short_id(dep_id),
                short_id(&job.id)
            ));
        }
    }

    lines.push("    end".to_string());
    lines.push(String::new());

    // Define styling classes
    lines.push(
        "    classDef completed fill:#d4edda,stroke:#155724,stroke-width:2px,color:#155724"
            .to_string(),
    );
    lines.push(
        "    classDef failed fill:#f8d7da,stroke:#721c24,stroke-width:2px,color:#721c24"
            .to_string(),
    );
    lines.push(
        "    classDef running fill:#cce7ff,stroke:#004085,stroke-width:2px,color:#004085"
            .to_string(),
    );
    lines.push(
        "    classDef pending fill:#fff3cd,stroke:#856404,stroke-width:2px,color:#856404"
            .to_string(),
    );
    lines.push(
        "    classDef default fill:#e2e3e5,stroke:#383d41,stroke-width:2px,color:#383d41"
            .to_string(),
    );

    lines.push("\nTo visualize: copy the above Mermaid code to:".to_string());
    lines.push("- GitHub/GitLab markdown (```mermaid ... ```)".to_string());
    lines.push("- https://mermaid.live/".to_string());
    lines.push("- VS Code with Mermaid extension".to_string());
    lines.join("\n")
}

fn render_json_graph(jobs: &[JobNode]) -> Result<String> {
    let graph = serde_json::json!({
        "nodes": jobs.iter().map(|job| {
            serde_json::json!({
                "id": job.id,
                "queue": job.queue_name,
                "status": job.status,
                "dependency_status": job.dependency_status,
                "workflow_id": job.workflow_id,
                "workflow_name": job.workflow_name
            })
        }).collect::<Vec<_>>(),
        "edges": jobs.iter().flat_map(|job| {
            job.depends_on.iter().map(move |dep_id| {
                serde_json::json!({
                    "from": dep_id,
                    "to": job.id,
                    "type": "dependency"
                })
            })
        }).collect::<Vec<_>>()
    });

    Ok(format!(
        "\nJSON Graph Format:\n{}\n{}",
        "=".repeat(50),
        serde_json::to_string_pretty(&graph)?
    ))
}

/// The indented dependency tree of `jobs`, with `target_job_id` marked.
fn render_dependency_tree(jobs: &[JobNode], target_job_id: &str) -> String {
    let job_map: HashMap<String, &JobNode> = jobs.iter().map(|job| (job.id.clone(), job)).collect();

    // Find root jobs (jobs with no dependencies)
    let mut roots: Vec<&JobNode> = jobs
        .iter()
        .filter(|job| job.depends_on.is_empty())
        .collect();

    // If no natural roots, use all jobs as potential roots
    if roots.is_empty() {
        roots = jobs.iter().collect();
    }

    roots.sort_by(|a, b| a.id.cmp(&b.id));

    let mut out = String::from("  Tree Structure:");
    let mut visited = HashSet::new();

    for root in &roots {
        if !visited.contains(&root.id) {
            render_job_tree_node(root, &job_map, &mut visited, 0, target_job_id, &mut out);
        }
    }

    // Handle any remaining unvisited jobs (cycles or disconnected components)
    for job in jobs {
        if !visited.contains(&job.id) {
            out.push_str("\n  [Disconnected]");
            render_job_tree_node(job, &job_map, &mut visited, 0, target_job_id, &mut out);
        }
    }
    out
}

fn render_job_tree_node(
    job: &JobNode,
    job_map: &HashMap<String, &JobNode>,
    visited: &mut HashSet<String>,
    depth: usize,
    target_job_id: &str,
    out: &mut String,
) {
    if visited.contains(&job.id) {
        return;
    }
    visited.insert(job.id.clone());

    let indent = "  ".repeat(depth + 1);
    let marker = if depth == 0 { "┌─" } else { "├─" };
    let highlight = if job.id == target_job_id { " ⭐" } else { "" };

    out.push_str(&format!(
        "\n{}{}[{}] {} ({}){}",
        indent,
        marker,
        short_id(&job.id),
        job.status,
        job.dependency_status,
        highlight
    ));

    // Recursively print dependents (children in the tree)
    for dependent_id in &job.dependents {
        if let Some(dependent_job) = job_map.get(dependent_id)
            && !visited.contains(dependent_id)
        {
            render_job_tree_node(
                dependent_job,
                job_map,
                visited,
                depth + 1,
                target_job_id,
                out,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::*;
    use clap::Parser;
    use serde_json::json;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: WorkflowCommand,
    }

    fn parse(args: &[&str]) -> WorkflowCommand {
        let mut argv = vec!["test"];
        argv.extend_from_slice(args);
        TestCli::try_parse_from(argv).unwrap().command
    }

    #[test]
    fn parses_every_subcommand_with_its_flags() {
        match parse(&["list", "-l", "5", "--running", "--completed", "--failed"]) {
            WorkflowCommand::List {
                limit,
                running,
                completed,
                failed,
                ..
            } => assert_eq!(
                (limit, running, completed, failed),
                (Some(5), true, true, true)
            ),
            _ => panic!("expected List"),
        }
        match parse(&["show", "wf", "--dependencies"]) {
            WorkflowCommand::Show {
                workflow_id,
                dependencies,
                ..
            } => assert_eq!((workflow_id.as_str(), dependencies), ("wf", true)),
            _ => panic!("expected Show"),
        }
        match parse(&[
            "create",
            "-n",
            "etl",
            "-f",
            "jobs.json",
            "--failure-policy",
            "manual",
            "--metadata",
            "{}",
        ]) {
            WorkflowCommand::Create {
                name,
                jobs_file,
                failure_policy,
                metadata,
                ..
            } => {
                assert_eq!((name.as_str(), jobs_file.as_str()), ("etl", "jobs.json"));
                assert_eq!(failure_policy.as_deref(), Some("manual"));
                assert_eq!(metadata.as_deref(), Some("{}"));
            }
            _ => panic!("expected Create"),
        }
        assert!(
            TestCli::try_parse_from(["test", "create", "-n", "etl"]).is_err(),
            "a workflow without jobs cannot be created"
        );
        assert!(matches!(
            parse(&["cancel", "wf", "--force"]),
            WorkflowCommand::Cancel { force: true, .. }
        ));
        match parse(&["dependencies", "job", "--tree", "--dependents"]) {
            WorkflowCommand::Dependencies {
                job_id,
                tree,
                dependents,
                ..
            } => assert_eq!((job_id.as_str(), tree, dependents), ("job", true, true)),
            _ => panic!("expected Dependencies"),
        }
        assert!(matches!(
            parse(&["graph", "wf", "--format", "dot"]),
            WorkflowCommand::Graph { format: Some(f), .. } if f == "dot"
        ));
    }

    #[test]
    fn database_url_comes_from_the_flag_then_the_config() {
        let config = config_for("postgres://config/db");
        for args in [
            &["list"][..],
            &["show", "x"],
            &["create", "-n", "n", "-f", "f"],
            &["cancel", "x"],
            &["dependencies", "x"],
            &["graph", "x"],
        ] {
            assert_eq!(
                parse(args).get_database_url(&config).unwrap(),
                "postgres://config/db"
            );
            assert!(parse(args).get_database_url(&Config::default()).is_err());
        }
        assert_eq!(
            parse(&["list", "-u", "mysql://flag/db"])
                .get_database_url(&config)
                .unwrap(),
            "mysql://flag/db"
        );
    }

    #[test]
    fn workflow_files_become_dependent_jobs() {
        let workflow = build_workflow(
            "etl",
            FailurePolicy::ContinueOnFailure,
            json!({"owner": "me"}),
            &json!([
                {"queue": "extract", "payload": {"step": 1}},
                {"queue": "transform", "payload": {"step": 2}, "priority": "high", "depends_on": [0]},
                {"queue": "load", "payload": {"step": 3}, "depends_on": [0, 1]},
            ]),
        )
        .unwrap();
        assert_eq!(workflow.name, "etl");
        assert_eq!(workflow.total_jobs, 3);
        assert_eq!(workflow.metadata["owner"], "me");
        assert!(matches!(
            workflow.failure_policy,
            FailurePolicy::ContinueOnFailure
        ));
        let ids: Vec<_> = workflow.jobs.iter().map(|j| j.id).collect();
        assert!(workflow.jobs[0].depends_on.is_empty());
        assert_eq!(workflow.jobs[1].depends_on, vec![ids[0]]);
        assert_eq!(workflow.jobs[1].priority, hammerwork::JobPriority::High);
        assert_eq!(workflow.jobs[2].depends_on, vec![ids[0], ids[1]]);
        assert!(
            workflow
                .jobs
                .iter()
                .all(|j| j.workflow_id == Some(workflow.id))
        );
        assert_eq!(workflow.dependencies[&ids[2]], vec![ids[0], ids[1]]);
        workflow.validate().unwrap();
    }

    #[test]
    fn workflow_files_are_validated() {
        let build = |jobs: Value| {
            build_workflow("w", FailurePolicy::FailFast, json!({}), &jobs)
                .map(|_| ())
                .unwrap_err()
                .to_string()
        };
        assert!(build(json!({"queue": "q"})).contains("JSON array"));
        assert!(build(json!([])).contains("at least one job"));
        assert!(build(json!([5])).contains("job 0: must be a JSON object"));
        assert!(build(json!([{"payload": {}}])).contains("job 0: missing 'queue'"));
        assert!(build(json!([{"queue": "", "payload": {}}])).contains("missing 'queue'"));
        assert!(build(json!([{"queue": "q"}])).contains("missing 'payload'"));
        assert!(
            build(json!([{"queue": "q", "payload": {}, "priority": 3}]))
                .contains("must be a string")
        );
        assert!(
            build(json!([{"queue": "q", "payload": {}, "priority": "urgent"}]))
                .contains("Invalid priority")
        );
        assert!(
            build(json!([{"queue": "q", "payload": {}, "depends_on": 0}]))
                .contains("array of job indexes")
        );
        assert!(
            build(json!([{"queue": "q", "payload": {}, "depends_on": ["a"]}]))
                .contains("job indexes")
        );
        let msg = build(json!([{"queue": "q", "payload": {}, "depends_on": [0]}]));
        assert!(msg.contains("must refer to an earlier job"), "{msg}");
        let msg = build(json!([
            {"queue": "q", "payload": {}},
            {"queue": "q", "payload": {}, "depends_on": [5]}
        ]));
        assert!(
            msg.contains("job 1") && msg.contains("earlier job"),
            "{msg}"
        );
    }

    fn node(
        id: &str,
        queue: &str,
        status: &str,
        dep: &str,
        deps: &[&str],
        kids: &[&str],
    ) -> JobNode {
        JobNode {
            id: id.to_string(),
            queue_name: queue.to_string(),
            status: status.to_string(),
            dependency_status: dep.to_string(),
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            dependents: kids.iter().map(|d| d.to_string()).collect(),
            workflow_id: Some("wf".into()),
            workflow_name: Some("etl".into()),
        }
    }

    fn sample_jobs() -> Vec<JobNode> {
        vec![
            node(
                "aaaaaaaa-1",
                "extract",
                "Completed",
                "none",
                &[],
                &["bbbbbbbb-2", "cccccccc-3"],
            ),
            node(
                "bbbbbbbb-2",
                "transform",
                "Running",
                "satisfied",
                &["aaaaaaaa-1"],
                &["dddddddd-4"],
            ),
            node(
                "cccccccc-3",
                "transform",
                "Failed",
                "failed",
                &["aaaaaaaa-1"],
                &["dddddddd-4"],
            ),
            node(
                "dddddddd-4",
                "load",
                "Pending",
                "waiting",
                &["bbbbbbbb-2", "cccccccc-3"],
                &[],
            ),
        ]
    }

    #[test]
    fn dependency_levels_follow_the_longest_path() {
        let cmd = parse(&["list"]);
        let jobs = sample_jobs();
        let levels = cmd.calculate_dependency_levels(&jobs);
        let summary: Vec<(usize, Vec<&str>)> = levels
            .iter()
            .map(|(l, js)| (*l, js.iter().map(|j| j.id.as_str()).collect()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (0, vec!["aaaaaaaa-1"]),
                (1, vec!["bbbbbbbb-2", "cccccccc-3"]),
                (2, vec!["dddddddd-4"]),
            ]
        );
        assert!(cmd.calculate_dependency_levels(&[]).is_empty());
    }

    #[test]
    fn graphs_render_in_every_format() {
        let cmd = parse(&["list"]);
        let jobs = sample_jobs();

        let text = render_graph(&cmd, &jobs, "wf", "text").unwrap();
        assert!(text.contains("Level 0: 1 job(s)") && text.contains("Level 2: 1 job(s)"));
        assert!(text.contains("[aaaaaaaa] Completed (none)"));
        assert!(text.contains("[dddddddd] Pending (2 dependencies)"));

        let dot = render_graph(&cmd, &jobs, "work-flow-1", "dot").unwrap();
        assert!(dot.contains("digraph workflow_work_flow_1 {"));
        assert!(dot.contains("fillcolor=lightgreen") && dot.contains("fillcolor=lightcoral"));
        assert!(dot.contains("fillcolor=lightblue") && dot.contains("fillcolor=lightyellow"));
        assert!(dot.contains("\"aaaaaaaa-1\" -> \"bbbbbbbb-2\";"));
        assert_eq!(dot.matches(" -> ").count(), 4);

        let mermaid = render_graph(&cmd, &jobs, "wf", "mermaid").unwrap();
        assert!(mermaid.contains("subgraph \"📋 Workflow: wf\""));
        assert!(
            mermaid.contains("dddddddd[\"dddddddd<br/>Pending<br/>⏳ load\"]:::pending"),
            "{mermaid}"
        );
        assert!(mermaid.contains("bbbbbbbb[\"bbbbbbbb<br/>Running<br/>✅ transform\"]:::running"));
        assert!(mermaid.contains("cccccccc[\"cccccccc<br/>Failed<br/>❌ transform\"]:::failed"));
        assert!(mermaid.contains("aaaaaaaa --> bbbbbbbb"));

        let json = render_graph(&cmd, &jobs, "wf", "json").unwrap();
        let parsed: Value = serde_json::from_str(&json[json.find('{').unwrap()..]).unwrap();
        assert_eq!(parsed["nodes"].as_array().unwrap().len(), 4);
        assert_eq!(parsed["edges"].as_array().unwrap().len(), 4);
        assert_eq!(parsed["edges"][0]["type"], "dependency");

        let err = render_graph(&cmd, &jobs, "wf", "svg")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Unsupported format: svg"), "{err}");
        // Ids shorter than eight characters used to panic when sliced.
        let short = vec![node("ab", "q", "Pending", "none", &["x"], &[])];
        for format in ["text", "dot", "mermaid", "json"] {
            render_graph(&cmd, &short, "wf", format).unwrap();
        }
    }

    #[test]
    fn dependency_tree_marks_the_target_and_handles_cycles() {
        let tree = render_dependency_tree(&sample_jobs(), "bbbbbbbb-2");
        let lines: Vec<&str> = tree.lines().collect();
        assert_eq!(lines[0], "  Tree Structure:");
        assert!(
            lines[1].starts_with("  ┌─[aaaaaaaa] Completed (none)"),
            "{tree}"
        );
        assert!(tree.contains("[bbbbbbbb] Running (satisfied) ⭐"));
        assert_eq!(tree.matches("[dddddddd]").count(), 1, "shown once");

        let cyclic = vec![
            node(
                "aaaaaaaa-1",
                "q",
                "Pending",
                "waiting",
                &["bbbbbbbb-2"],
                &["bbbbbbbb-2"],
            ),
            node(
                "bbbbbbbb-2",
                "q",
                "Pending",
                "waiting",
                &["aaaaaaaa-1"],
                &["aaaaaaaa-1"],
            ),
        ];
        let tree = render_dependency_tree(&cyclic, "x");
        assert_eq!(tree.matches("[aaaaaaaa]").count(), 1);
        assert_eq!(tree.matches("[bbbbbbbb]").count(), 1);
    }

    #[test]
    fn workflow_list_and_details_render() {
        assert_eq!(render_workflow_list(&[]), "No workflows found.");
        let at = chrono::DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let list = render_workflow_list(&[WorkflowSummary {
            id: "wf-1".into(),
            name: "etl".into(),
            status: "running".into(),
            total_jobs: 3,
            completed_jobs: 1,
            failed_jobs: 0,
            failure_policy: "fail_fast".into(),
            created_at: at,
        }]);
        for expected in ["wf-1", "etl", "running", "fail_fast", "2030-01-02 03:04:05"] {
            assert!(list.contains(expected), "{expected} in {list}");
        }

        let mut group = JobGroup::new("etl").with_metadata(json!({"owner": "me"}));
        group.total_jobs = 4;
        group.completed_jobs = 1;
        group.failed_jobs = 1;
        let details = render_workflow_details(&group, &sample_jobs());
        for expected in [
            "Name: etl",
            "Status: running",
            "Jobs: 4 (1 completed, 1 failed)",
            "Completed: 1",
            "Failed: 1",
            "Pending: 1",
            "Running: 1",
            "Metadata: {\"owner\":\"me\"}",
        ] {
            assert!(details.contains(expected), "{expected} in {details}");
        }
        let bare = render_workflow_details(&JobGroup::new("empty"), &[]);
        assert!(!bare.contains("Jobs by status") && !bare.contains("Metadata"));
    }

    async fn has_workflow(pool: &DatabasePool, name: &str, statuses: &[&str]) -> bool {
        fetch_workflows(pool, 500, statuses)
            .await
            .unwrap()
            .iter()
            .any(|w| w.name == name)
    }

    async fn workflow_commands(url: String) {
        let config = config_for(&url);
        let pool = DatabasePool::connect(&url, 2).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let queue = unique_queue("wf");
        let name = format!("etl {}", uuid::Uuid::new_v4().simple());
        let run = |args: Vec<String>| {
            let cmd = parse(&args.iter().map(String::as_str).collect::<Vec<_>>());
            let config = config.clone();
            async move { cmd.execute(config).await }
        };
        fn s(v: &str) -> String {
            v.to_string()
        }

        // create stores the workflow and its dependent jobs in one go
        let file = dir.path().join("jobs.json");
        std::fs::write(
            &file,
            json!([
                {"queue": queue, "payload": {"step": "extract"}},
                {"queue": queue, "payload": {"step": "transform"}, "depends_on": [0], "priority": "high"},
                {"queue": queue, "payload": {"step": "load"}, "depends_on": [0, 1]},
            ])
            .to_string(),
        )
        .unwrap();
        let file = file.to_str().unwrap().to_string();
        run(vec![
            s("create"),
            s("-n"),
            name.clone(),
            s("-f"),
            file.clone(),
            s("--failure-policy"),
            s("continue_on_failure"),
            s("--metadata"),
            s(r#"{"owner": "ops"}"#),
        ])
        .await
        .unwrap();

        let workflows = fetch_workflows(&pool, 100, &[]).await.unwrap();
        let wf = workflows
            .iter()
            .find(|w| w.name == name)
            .expect("workflow stored");
        assert_eq!(
            (
                wf.status.as_str(),
                wf.total_jobs,
                wf.failure_policy.as_str()
            ),
            ("running", 3, "continue_on_failure")
        );
        let wf_id = wf.id.clone();
        let jobs = WorkflowCommand::List {
            database_url: None,
            limit: None,
            running: false,
            completed: false,
            failed: false,
        }
        .get_workflow_jobs(&pool, &wf_id)
        .await
        .unwrap();
        assert_eq!(jobs.len(), 3);
        let by_payload = |step: &str| -> JobNode {
            let idx = ["extract", "transform", "load"]
                .iter()
                .position(|s| *s == step)
                .unwrap();
            jobs[idx].clone()
        };
        let (extract, transform, load) = (
            by_payload("extract"),
            by_payload("transform"),
            by_payload("load"),
        );
        assert!(extract.depends_on.is_empty());
        assert_eq!(transform.depends_on, vec![extract.id.clone()]);
        assert_eq!(load.depends_on.len(), 2);
        assert_eq!(
            extract.dependents.len(),
            2,
            "dependents are maintained: {:?}",
            extract.dependents
        );
        assert_eq!(extract.workflow_name.as_deref(), Some(name.as_str()));
        assert_eq!(count_jobs(&pool, &queue, Some("Pending")).await, 3);
        assert_eq!(
            job_column(&pool, &transform.id, "priority")
                .await
                .as_deref(),
            Some("3")
        );

        // create errors leave nothing behind
        let bad = dir.path().join("bad.json");
        for (content, expected) in [
            ("not json", "is not valid JSON"),
            ("[]", "at least one job"),
            (r#"[{"queue": "q"}]"#, "missing 'payload'"),
        ] {
            std::fs::write(&bad, content).unwrap();
            let err = run(vec![
                s("create"),
                s("-n"),
                s("bad"),
                s("-f"),
                bad.to_str().unwrap().to_string(),
            ])
            .await
            .unwrap_err()
            .to_string();
            assert!(err.contains(expected), "{content}: {err}");
        }
        assert!(
            run(vec![
                s("create"),
                s("-n"),
                s("bad"),
                s("-f"),
                s("/no/such/file.json")
            ])
            .await
            .is_err()
        );
        assert!(
            run(vec![
                s("create"),
                s("-n"),
                s("bad"),
                s("-f"),
                file.clone(),
                s("--failure-policy"),
                s("whenever")
            ])
            .await
            .is_err()
        );
        assert!(
            run(vec![
                s("create"),
                s("-n"),
                s("bad"),
                s("-f"),
                file.clone(),
                s("--metadata"),
                s("{nope")
            ])
            .await
            .is_err()
        );
        assert!(
            fetch_workflows(&pool, 100, &[])
                .await
                .unwrap()
                .iter()
                .all(|w| w.name != "bad")
        );

        // list: filters by status and honours the limit
        assert!(has_workflow(&pool, &name, &["running"]).await);
        assert!(has_workflow(&pool, &name, &["running", "completed"]).await);
        assert!(!has_workflow(&pool, &name, &["completed"]).await);
        assert!(!has_workflow(&pool, &name, &["failed"]).await);
        assert_eq!(fetch_workflows(&pool, 1, &[]).await.unwrap().len(), 1);
        run(vec![s("list")]).await.unwrap();
        run(vec![s("list"), s("--running"), s("--limit"), s("3")])
            .await
            .unwrap();
        run(vec![s("list"), s("--completed"), s("--failed")])
            .await
            .unwrap();

        // show
        run(vec![s("show"), wf_id.clone()]).await.unwrap();
        run(vec![s("show"), wf_id.clone(), s("--dependencies")])
            .await
            .unwrap();
        let missing = uuid::Uuid::new_v4().to_string();
        let err = run(vec![s("show"), missing.clone()])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Workflow not found"), "{err}");
        assert!(run(vec![s("show"), s("nope")]).await.is_err());

        // graph in every format; unknown formats and empty workflows
        for format in ["text", "dot", "mermaid", "json"] {
            run(vec![s("graph"), wf_id.clone(), s("--format"), s(format)])
                .await
                .unwrap();
        }
        run(vec![s("graph"), wf_id.clone()]).await.unwrap();
        run(vec![s("graph"), missing.clone()]).await.unwrap(); // no jobs: says so
        assert!(
            run(vec![s("graph"), wf_id.clone(), s("--format"), s("svg")])
                .await
                .is_err()
        );
        assert!(run(vec![s("graph"), s("nope")]).await.is_err());

        // dependencies of a job: direct, dependents, tree (via its workflow)
        run(vec![s("dependencies"), load.id.clone()]).await.unwrap();
        run(vec![
            s("dependencies"),
            extract.id.clone(),
            s("--dependents"),
            s("--tree"),
        ])
        .await
        .unwrap();
        let err = run(vec![s("dependencies"), missing.clone()])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Job not found"), "{err}");
        assert!(run(vec![s("dependencies"), s("nope")]).await.is_err());
        // a job outside any workflow: the tree is found by walking its dependencies
        let lone_parent = seed(&pool, &SeedJob::new(&queue, "Completed")).await;
        let mut child = SeedJob::new(&queue, "Pending");
        child.depends_on = Some(&lone_parent);
        let lone_child = seed(&pool, &child).await;
        run(vec![s("dependencies"), lone_child.clone(), s("--tree")])
            .await
            .unwrap();
        let cmd = parse(&["list"]);
        let node = cmd
            .get_job_node(&pool, &uuid::Uuid::parse_str(&lone_child).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(node.depends_on, vec![lone_parent.clone()]);
        let related = cmd.collect_related_jobs(&pool, &node).await.unwrap();
        assert_eq!(related.len(), 2);

        // cancel: a running job needs --force; the library fails the unfinished jobs
        exec_sql(
            &pool,
            &format!(
                "UPDATE hammerwork_jobs SET status = 'Running', started_at = {} WHERE id = '{}'",
                match pool.backend() {
                    crate::utils::sql::Backend::Postgres => "NOW()",
                    crate::utils::sql::Backend::MySql => "UTC_TIMESTAMP(6)",
                },
                extract.id
            ),
        )
        .await;
        let err = run(vec![s("cancel"), wf_id.clone()])
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("1 running job") && err.contains("--force"),
            "{err}"
        );
        assert_eq!(job_status(&pool, &extract.id).await, "Running");
        run(vec![s("cancel"), wf_id.clone(), s("--force")])
            .await
            .unwrap();
        for job in [&extract, &transform, &load] {
            assert_eq!(
                job_status(&pool, &job.id).await,
                "Failed",
                "unfinished jobs fail"
            );
        }
        let wf = fetch_workflows(&pool, 500, &["cancelled"]).await.unwrap();
        assert!(wf.iter().any(|w| w.id == wf_id));
        assert!(run(vec![s("cancel"), missing.clone()]).await.is_err());
        assert!(run(vec![s("cancel"), s("nope")]).await.is_err());

        cleanup(&pool, &[&queue]).await;
        exec_sql(
            &pool,
            &format!("DELETE FROM hammerwork_workflows WHERE id = '{wf_id}'"),
        )
        .await;
    }

    db_tests!(
        workflow_commands,
        test_workflow_commands_postgres,
        test_workflow_commands_mysql
    );
}
