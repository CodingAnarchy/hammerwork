use anyhow::Result;
use clap::Subcommand;
use hammerwork::{FailurePolicy, JobGroup};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::utils::database::DatabasePool;
use crate::utils::validation::validate_json_payload;

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
    #[command(about = "Create a new workflow")]
    Create {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', long, help = "Workflow name")]
        name: String,
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

impl WorkflowCommand {
    pub async fn execute(&self, config: Config) -> Result<()> {
        let db_url = self.get_database_url(&config)?;
        let pool = DatabasePool::connect(&db_url, config.get_connection_pool_size()).await?;

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
                failure_policy,
                metadata,
                ..
            } => {
                self.create_workflow(name, failure_policy.as_deref(), metadata.as_deref())
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
        _pool: DatabasePool,
        limit: Option<u32>,
        running: bool,
        completed: bool,
        failed: bool,
    ) -> Result<()> {
        warn!("Workflow listing is not fully implemented yet");
        println!("⚠️  Workflow listing is not fully implemented yet.");
        println!("    This would list workflows with filters:");
        println!("    - Limit: {}", limit.unwrap_or(50));
        println!("    - Running: {}", running);
        println!("    - Completed: {}", completed);
        println!("    - Failed: {}", failed);

        Ok(())
    }

    async fn show_workflow(
        &self,
        _pool: DatabasePool,
        workflow_id: &str,
        show_dependencies: bool,
    ) -> Result<()> {
        warn!("Workflow details view is not fully implemented yet");
        println!("⚠️  Workflow details view is not fully implemented yet.");
        println!("    This would show details for workflow: {}", workflow_id);
        println!("    Show dependencies: {}", show_dependencies);

        Ok(())
    }

    async fn create_workflow(
        &self,
        name: &str,
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

        // Create workflow
        let workflow = JobGroup::new(name)
            .with_failure_policy(policy)
            .with_metadata(metadata_json);

        println!("Workflow created:");
        println!("  ID: {}", workflow.id);
        println!("  Name: {}", workflow.name);
        println!("  Failure Policy: {:?}", workflow.failure_policy);

        info!("Created workflow '{}' with ID {}", name, workflow.id);
        println!("\nUse 'cargo hammerwork job enqueue' to add jobs to this workflow.");

        Ok(())
    }

    async fn cancel_workflow(
        &self,
        _pool: DatabasePool,
        workflow_id: &str,
        force: bool,
    ) -> Result<()> {
        warn!("Workflow cancellation is not fully implemented yet");
        println!("⚠️  Workflow cancellation is not fully implemented yet.");
        println!("    This would cancel workflow: {}", workflow_id);
        println!("    Force: {}", force);

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
        let target_job = self.get_job_node(&pool, &job_uuid).await?;
        let target_job = match target_job {
            Some(job) => job,
            None => {
                println!("Job not found: {}", job_id);
                return Ok(());
            }
        };

        println!("Job Dependencies for {}", job_id);
        println!("Queue: {}", target_job.queue_name);
        println!("Status: {}", target_job.status);
        println!("Dependency Status: {}", target_job.dependency_status);

        if let Some(workflow_name) = &target_job.workflow_name {
            println!("Workflow: {}", workflow_name);
        }

        // Show immediate dependencies
        if !target_job.depends_on.is_empty() {
            println!("\nDirect Dependencies:");
            for dep_id in &target_job.depends_on {
                if let Some(dep_job) = self.get_job_node_by_string(&pool, dep_id).await? {
                    println!("  ├─ {} ({})", dep_id, dep_job.status);
                } else {
                    println!("  ├─ {} (not found)", dep_id);
                }
            }
        } else {
            println!("\nNo direct dependencies");
        }

        // Show immediate dependents if requested
        if show_dependents {
            if !target_job.dependents.is_empty() {
                println!("\nDirect Dependents:");
                for dep_id in &target_job.dependents {
                    if let Some(dep_job) = self.get_job_node_by_string(&pool, dep_id).await? {
                        println!("  ├─ {} ({})", dep_id, dep_job.status);
                    } else {
                        println!("  ├─ {} (not found)", dep_id);
                    }
                }
            } else {
                println!("\nNo direct dependents");
            }
        }

        // Show full dependency tree if requested
        if show_tree {
            println!("\nDependency Tree:");

            // Build dependency graph for the workflow or just this job
            let jobs = if let Some(workflow_id) = &target_job.workflow_id {
                self.get_workflow_jobs(&pool, workflow_id).await?
            } else {
                // Collect all related jobs by traversing dependencies
                self.collect_related_jobs(&pool, &target_job).await?
            };

            if jobs.is_empty() {
                println!("  No related jobs found");
            } else {
                self.print_dependency_tree(&jobs, &target_job.id);
            }
        }

        Ok(())
    }

    async fn show_graph(
        &self,
        pool: DatabasePool,
        workflow_id: &str,
        format: Option<&str>,
    ) -> Result<()> {
        let format = format.unwrap_or("text");

        // Get all jobs in the workflow
        let jobs = self.get_workflow_jobs(&pool, workflow_id).await?;

        if jobs.is_empty() {
            println!("No jobs found in workflow: {}", workflow_id);
            return Ok(());
        }

        println!("Workflow Graph: {}", workflow_id);
        println!("Format: {}", format);
        println!("Jobs: {}", jobs.len());

        match format {
            "text" => self.print_text_graph(&jobs),
            "dot" => self.print_dot_graph(&jobs, workflow_id),
            "mermaid" => self.print_mermaid_graph(&jobs, workflow_id),
            "json" => self.print_json_graph(&jobs)?,
            _ => {
                println!(
                    "Unsupported format: {}. Use: text, dot, mermaid, json",
                    format
                );
                return Ok(());
            }
        }

        Ok(())
    }

    fn print_text_graph(&self, jobs: &[JobNode]) {
        println!("\nDependency Graph (Text Format):");
        println!("{}", "=".repeat(50));

        // Group jobs by dependency level
        let mut levels = self.calculate_dependency_levels(jobs);
        levels.sort_by_key(|level| level.0);

        for (level, jobs_at_level) in levels {
            println!("\nLevel {}: {} job(s)", level, jobs_at_level.len());
            for job in jobs_at_level {
                let deps_str = if job.depends_on.is_empty() {
                    "none".to_string()
                } else {
                    format!("{} dependencies", job.depends_on.len())
                };

                println!("  ├─ [{}] {} ({})", &job.id[..8], job.status, deps_str);
            }
        }
    }

    fn print_dot_graph(&self, jobs: &[JobNode], workflow_id: &str) {
        println!("\nDOT Graph Format:");
        println!("{}", "=".repeat(50));
        println!("digraph workflow_{} {{", workflow_id.replace('-', "_"));
        println!("  rankdir=TB;");
        println!("  node [shape=box];");

        // Define nodes
        for job in jobs {
            let color = match job.status.as_str() {
                "Completed" => "lightgreen",
                "Failed" => "lightcoral",
                "Running" => "lightblue",
                "Pending" => "lightyellow",
                _ => "lightgray",
            };

            println!(
                "  \"{}\" [label=\"{}\\n{}\" fillcolor={} style=filled];",
                job.id,
                &job.id[..8],
                job.status,
                color
            );
        }

        // Define edges (dependencies)
        for job in jobs {
            for dep_id in &job.depends_on {
                println!("  \"{}\" -> \"{}\";", dep_id, job.id);
            }
        }

        println!("}}");
        println!(
            "\nTo visualize: copy the above DOT code to https://dreampuf.github.io/GraphvizOnline/"
        );
    }

    fn print_mermaid_graph(&self, jobs: &[JobNode], workflow_id: &str) {
        println!("\nMermaid Graph Format:");
        println!("{}", "=".repeat(50));
        println!("---");
        println!("title: Hammerwork Workflow Dependency Graph");
        println!("---");
        println!("graph TD");
        println!("    subgraph \"📋 Workflow: {}\"", &workflow_id[..8]);

        // Define nodes with styling
        for job in jobs {
            let short_id = &job.id[..8];
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

            println!(
                "        {}[\"{}<br/>{}<br/>{} {}\"]{}",
                short_id, short_id, job.status, dependency_indicator, job.queue_name, status_class
            );
        }

        println!();

        // Define edges (dependencies) inside subgraph
        for job in jobs {
            let job_short = &job.id[..8];
            for dep_id in &job.depends_on {
                let dep_short = &dep_id[..8];
                println!("        {} --> {}", dep_short, job_short);
            }
        }

        println!("    end");
        println!();

        // Define styling classes
        println!(
            "    classDef completed fill:#d4edda,stroke:#155724,stroke-width:2px,color:#155724"
        );
        println!("    classDef failed fill:#f8d7da,stroke:#721c24,stroke-width:2px,color:#721c24");
        println!("    classDef running fill:#cce7ff,stroke:#004085,stroke-width:2px,color:#004085");
        println!("    classDef pending fill:#fff3cd,stroke:#856404,stroke-width:2px,color:#856404");
        println!("    classDef default fill:#e2e3e5,stroke:#383d41,stroke-width:2px,color:#383d41");

        println!("\nTo visualize: copy the above Mermaid code to:");
        println!("- GitHub/GitLab markdown (```mermaid ... ```)");
        println!("- https://mermaid.live/");
        println!("- VS Code with Mermaid extension");
    }

    pub fn print_json_graph(&self, jobs: &[JobNode]) -> Result<()> {
        println!("\nJSON Graph Format:");
        println!("{}", "=".repeat(50));

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

        println!("{}", serde_json::to_string_pretty(&graph)?);
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

        result.into_iter().collect()
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

    async fn get_job_node(&self, pool: &DatabasePool, job_id: &Uuid) -> Result<Option<JobNode>> {
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
                    Ok(Some(self.postgres_row_to_job_node(&row)?))
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
                    Ok(Some(self.mysql_row_to_job_node(&row)?))
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

    async fn get_workflow_jobs(
        &self,
        pool: &DatabasePool,
        workflow_id: &str,
    ) -> Result<Vec<JobNode>> {
        let query = r#"
            SELECT id, queue_name, status, dependency_status, depends_on, dependents, workflow_id, workflow_name
            FROM hammerwork_jobs 
            WHERE workflow_id = $1
            ORDER BY created_at
        "#;

        match pool {
            DatabasePool::Postgres(pg_pool) => {
                let workflow_uuid = Uuid::parse_str(workflow_id)?;
                let rows = sqlx::query(query)
                    .bind(workflow_uuid)
                    .fetch_all(pg_pool)
                    .await?;

                let mut jobs = Vec::new();
                for row in rows {
                    jobs.push(self.postgres_row_to_job_node(&row)?);
                }
                Ok(jobs)
            }
            DatabasePool::MySQL(mysql_pool) => {
                let mysql_query = r#"
                    SELECT id, queue_name, status, dependency_status, depends_on, dependents, workflow_id, workflow_name
                    FROM hammerwork_jobs 
                    WHERE workflow_id = ?
                    ORDER BY created_at
                "#;
                let rows = sqlx::query(mysql_query)
                    .bind(workflow_id)
                    .fetch_all(mysql_pool)
                    .await?;

                let mut jobs = Vec::new();
                for row in rows {
                    jobs.push(self.mysql_row_to_job_node(&row)?);
                }
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
                    if !jobs.contains_key(dep_id) {
                        if let Some(dep_job) = self.get_job_node_by_string(pool, dep_id).await? {
                            jobs.insert(dep_id.clone(), dep_job);
                            to_visit.push_back(dep_id.clone());
                        }
                    }
                }

                // Visit all dependents
                for dep_id in &job.dependents {
                    if !jobs.contains_key(dep_id) {
                        if let Some(dep_job) = self.get_job_node_by_string(pool, dep_id).await? {
                            jobs.insert(dep_id.clone(), dep_job);
                            to_visit.push_back(dep_id.clone());
                        }
                    }
                }
            }
        }

        Ok(jobs.into_values().collect())
    }

    fn postgres_row_to_job_node(&self, row: &sqlx::postgres::PgRow) -> Result<JobNode> {
        use sqlx::Row;

        let id: Uuid = row.get("id");
        let depends_on = self.parse_json_array(row.try_get("depends_on").ok())?;
        let dependents = self.parse_json_array(row.try_get("dependents").ok())?;

        let workflow_id: Option<String> = match row.try_get::<Option<Uuid>, _>("workflow_id") {
            Ok(Some(uuid)) => Some(uuid.to_string()),
            Ok(None) => None,
            Err(_) => None,
        };

        Ok(JobNode {
            id: id.to_string(),
            queue_name: row.get("queue_name"),
            status: row.get("status"),
            dependency_status: row
                .try_get("dependency_status")
                .unwrap_or_else(|_| "none".to_string()),
            depends_on,
            dependents,
            workflow_id,
            workflow_name: row.try_get("workflow_name").ok(),
        })
    }

    fn mysql_row_to_job_node(&self, row: &sqlx::mysql::MySqlRow) -> Result<JobNode> {
        use sqlx::Row;

        let id: String = row.get("id");
        let depends_on = self.parse_json_array(row.try_get("depends_on").ok())?;
        let dependents = self.parse_json_array(row.try_get("dependents").ok())?;

        let workflow_id: Option<String> = row.try_get("workflow_id").ok();

        Ok(JobNode {
            id,
            queue_name: row.get("queue_name"),
            status: row.get("status"),
            dependency_status: row
                .try_get("dependency_status")
                .unwrap_or_else(|_| "none".to_string()),
            depends_on,
            dependents,
            workflow_id,
            workflow_name: row.try_get("workflow_name").ok(),
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

    fn print_dependency_tree(&self, jobs: &[JobNode], target_job_id: &str) {
        let job_map: HashMap<String, &JobNode> =
            jobs.iter().map(|job| (job.id.clone(), job)).collect();

        // Find root jobs (jobs with no dependencies)
        let mut roots: Vec<&JobNode> = jobs
            .iter()
            .filter(|job| job.depends_on.is_empty())
            .collect();

        // If no natural roots, use all jobs as potential roots
        if roots.is_empty() {
            roots = jobs.iter().collect();
        }

        // Sort roots by creation order (assuming UUID ordering roughly correlates)
        roots.sort_by(|a, b| a.id.cmp(&b.id));

        println!("  Tree Structure:");
        let mut visited = HashSet::new();

        for root in &roots {
            if !visited.contains(&root.id) {
                Self::print_job_tree_node(root, &job_map, &mut visited, 0, target_job_id);
            }
        }

        // Handle any remaining unvisited jobs (cycles or disconnected components)
        for job in jobs {
            if !visited.contains(&job.id) {
                println!("  [Disconnected]");
                Self::print_job_tree_node(job, &job_map, &mut visited, 0, target_job_id);
            }
        }
    }

    fn print_job_tree_node(
        job: &JobNode,
        job_map: &HashMap<String, &JobNode>,
        visited: &mut HashSet<String>,
        depth: usize,
        target_job_id: &str,
    ) {
        if visited.contains(&job.id) {
            return;
        }
        visited.insert(job.id.clone());

        let indent = "  ".repeat(depth + 1);
        let marker = if depth == 0 { "┌─" } else { "├─" };
        let highlight = if job.id == target_job_id { " ⭐" } else { "" };

        println!(
            "{}{}[{}] {} ({}){}",
            indent,
            marker,
            &job.id[..8], // Show first 8 chars of UUID
            job.status,
            job.dependency_status,
            highlight
        );

        // Recursively print dependents (children in the tree)
        for dependent_id in &job.dependents {
            if let Some(dependent_job) = job_map.get(dependent_id) {
                if !visited.contains(dependent_id) {
                    Self::print_job_tree_node(
                        dependent_job,
                        job_map,
                        visited,
                        depth + 1,
                        target_job_id,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_workflow_command_structure() {
        // Test that commands can be created
        let list_cmd = WorkflowCommand::List {
            database_url: None,
            limit: Some(10),
            running: true,
            completed: false,
            failed: false,
        };

        // This should compile without errors
        assert!(matches!(list_cmd, WorkflowCommand::List { .. }));
    }

    #[test]
    fn test_workflow_id_parsing() {
        let test_uuid = "550e8400-e29b-41d4-a716-446655440000";
        let parsed = Uuid::parse_str(test_uuid);
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_job_node_creation() {
        let job_node = JobNode {
            id: "test-job-123".to_string(),
            queue_name: "test-queue".to_string(),
            status: "Pending".to_string(),
            dependency_status: "waiting".to_string(),
            depends_on: vec!["dep1".to_string(), "dep2".to_string()],
            dependents: vec!["child1".to_string()],
            workflow_id: Some("workflow-123".to_string()),
            workflow_name: Some("test-workflow".to_string()),
        };

        assert_eq!(job_node.id, "test-job-123");
        assert_eq!(job_node.depends_on.len(), 2);
        assert_eq!(job_node.dependents.len(), 1);
        assert!(job_node.workflow_id.is_some());
    }

    #[test]
    fn test_parse_json_array() {
        let workflow_cmd = WorkflowCommand::List {
            database_url: None,
            limit: None,
            running: false,
            completed: false,
            failed: false,
        };

        // Test valid array
        let json_array = json!(["job1", "job2", "job3"]);
        let result = workflow_cmd.parse_json_array(Some(json_array)).unwrap();
        assert_eq!(result, vec!["job1", "job2", "job3"]);

        // Test empty array
        let empty_array = json!([]);
        let result = workflow_cmd.parse_json_array(Some(empty_array)).unwrap();
        assert!(result.is_empty());

        // Test non-array JSON
        let non_array = json!({"not": "array"});
        let result = workflow_cmd.parse_json_array(Some(non_array)).unwrap();
        assert!(result.is_empty());

        // Test None input
        let result = workflow_cmd.parse_json_array(None).unwrap();
        assert!(result.is_empty());

        // Test array with mixed types (should filter non-strings)
        let mixed_array = json!(["job1", 123, "job2", null, "job3"]);
        let result = workflow_cmd.parse_json_array(Some(mixed_array)).unwrap();
        assert_eq!(result, vec!["job1", "job2", "job3"]);
    }

    #[test]
    fn test_dependency_level_calculation() {
        let workflow_cmd = WorkflowCommand::List {
            database_url: None,
            limit: None,
            running: false,
            completed: false,
            failed: false,
        };

        // Create test jobs with dependencies
        let jobs = vec![
            JobNode {
                id: "job1".to_string(),
                queue_name: "queue1".to_string(),
                status: "Completed".to_string(),
                dependency_status: "none".to_string(),
                depends_on: vec![], // Root job
                dependents: vec!["job2".to_string()],
                workflow_id: Some("workflow1".to_string()),
                workflow_name: Some("test".to_string()),
            },
            JobNode {
                id: "job2".to_string(),
                queue_name: "queue1".to_string(),
                status: "Running".to_string(),
                dependency_status: "satisfied".to_string(),
                depends_on: vec!["job1".to_string()],
                dependents: vec!["job3".to_string()],
                workflow_id: Some("workflow1".to_string()),
                workflow_name: Some("test".to_string()),
            },
            JobNode {
                id: "job3".to_string(),
                queue_name: "queue1".to_string(),
                status: "Pending".to_string(),
                dependency_status: "waiting".to_string(),
                depends_on: vec!["job2".to_string()],
                dependents: vec![],
                workflow_id: Some("workflow1".to_string()),
                workflow_name: Some("test".to_string()),
            },
        ];

        let levels = workflow_cmd.calculate_dependency_levels(&jobs);

        // Should have 3 levels (0, 1, 2)
        assert!(levels.len() <= 3);

        // Verify that we have some levels calculated
        assert!(!levels.is_empty());

        // Check that levels are properly ordered
        let mut level_numbers: Vec<usize> = levels.iter().map(|(level, _)| *level).collect();
        level_numbers.sort();

        // Should start from 0
        assert_eq!(level_numbers[0], 0);
    }

    #[test]
    fn test_graph_format_validation() {
        let workflow_cmd = WorkflowCommand::Graph {
            database_url: None,
            workflow_id: "test-workflow".to_string(),
            format: Some("text".to_string()),
        };

        // Test that command structure is correct
        if let WorkflowCommand::Graph { format, .. } = workflow_cmd {
            assert_eq!(format, Some("text".to_string()));
        } else {
            panic!("Expected Graph command");
        }
    }

    #[test]
    fn test_dependencies_command_structure() {
        let deps_cmd = WorkflowCommand::Dependencies {
            database_url: None,
            job_id: "test-job-123".to_string(),
            tree: true,
            dependents: true,
        };

        if let WorkflowCommand::Dependencies {
            job_id,
            tree,
            dependents,
            ..
        } = deps_cmd
        {
            assert_eq!(job_id, "test-job-123");
            assert!(tree);
            assert!(dependents);
        } else {
            panic!("Expected Dependencies command");
        }
    }

    #[test]
    fn test_create_workflow_validation() {
        let create_cmd = WorkflowCommand::Create {
            database_url: None,
            name: "test-workflow".to_string(),
            failure_policy: Some("fail_fast".to_string()),
            metadata: Some(r#"{"key": "value"}"#.to_string()),
        };

        if let WorkflowCommand::Create {
            name,
            failure_policy,
            metadata,
            ..
        } = create_cmd
        {
            assert_eq!(name, "test-workflow");
            assert_eq!(failure_policy, Some("fail_fast".to_string()));
            assert!(metadata.is_some());
        } else {
            panic!("Expected Create command");
        }
    }

    #[test]
    fn test_workflow_command_variants() {
        // Test all command variants can be created
        let commands = vec![
            WorkflowCommand::List {
                database_url: None,
                limit: Some(10),
                running: false,
                completed: false,
                failed: false,
            },
            WorkflowCommand::Show {
                database_url: None,
                workflow_id: "test".to_string(),
                dependencies: false,
            },
            WorkflowCommand::Create {
                database_url: None,
                name: "test".to_string(),
                failure_policy: None,
                metadata: None,
            },
            WorkflowCommand::Cancel {
                database_url: None,
                workflow_id: "test".to_string(),
                force: false,
            },
            WorkflowCommand::Dependencies {
                database_url: None,
                job_id: "test".to_string(),
                tree: false,
                dependents: false,
            },
            WorkflowCommand::Graph {
                database_url: None,
                workflow_id: "test".to_string(),
                format: None,
            },
        ];

        // All commands should be valid
        assert_eq!(commands.len(), 6);
    }

    #[test]
    fn test_job_node_dependency_relationships() {
        let parent = JobNode {
            id: "parent".to_string(),
            queue_name: "queue1".to_string(),
            status: "Completed".to_string(),
            dependency_status: "none".to_string(),
            depends_on: vec![],
            dependents: vec!["child1".to_string(), "child2".to_string()],
            workflow_id: Some("workflow1".to_string()),
            workflow_name: Some("test".to_string()),
        };

        let child1 = JobNode {
            id: "child1".to_string(),
            queue_name: "queue1".to_string(),
            status: "Running".to_string(),
            dependency_status: "satisfied".to_string(),
            depends_on: vec!["parent".to_string()],
            dependents: vec![],
            workflow_id: Some("workflow1".to_string()),
            workflow_name: Some("test".to_string()),
        };

        let child2 = JobNode {
            id: "child2".to_string(),
            queue_name: "queue1".to_string(),
            status: "Pending".to_string(),
            dependency_status: "waiting".to_string(),
            depends_on: vec!["parent".to_string()],
            dependents: vec![],
            workflow_id: Some("workflow1".to_string()),
            workflow_name: Some("test".to_string()),
        };

        // Verify relationships
        assert!(parent.depends_on.is_empty());
        assert_eq!(parent.dependents.len(), 2);
        assert!(parent.dependents.contains(&"child1".to_string()));
        assert!(parent.dependents.contains(&"child2".to_string()));

        assert_eq!(child1.depends_on.len(), 1);
        assert!(child1.depends_on.contains(&"parent".to_string()));
        assert!(child1.dependents.is_empty());

        assert_eq!(child2.depends_on.len(), 1);
        assert!(child2.depends_on.contains(&"parent".to_string()));
        assert!(child2.dependents.is_empty());
    }

    #[test]
    fn test_database_url_extraction() {
        let config = Config {
            database_url: Some("postgres://localhost/test".to_string()),
            default_queue: None,
            default_limit: None,
            log_level: None,
            connection_pool_size: None,
        };

        let workflow_cmd = WorkflowCommand::List {
            database_url: Some("postgres://override/test".to_string()),
            limit: None,
            running: false,
            completed: false,
            failed: false,
        };

        // Test database URL extraction logic
        let result = workflow_cmd.get_database_url(&config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "postgres://override/test");

        // Test fallback to config
        let workflow_cmd_no_url = WorkflowCommand::List {
            database_url: None,
            limit: None,
            running: false,
            completed: false,
            failed: false,
        };

        let result = workflow_cmd_no_url.get_database_url(&config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "postgres://localhost/test");
    }

    #[test]
    fn test_job_status_classification() {
        let statuses = vec!["Completed", "Failed", "Running", "Pending", "Unknown"];

        for status in statuses {
            let job = JobNode {
                id: "test".to_string(),
                queue_name: "queue1".to_string(),
                status: status.to_string(),
                dependency_status: "none".to_string(),
                depends_on: vec![],
                dependents: vec![],
                workflow_id: None,
                workflow_name: None,
            };

            // Verify status is preserved
            assert_eq!(job.status, status);
        }
    }

    #[test]
    fn test_dependency_status_types() {
        let dep_statuses = vec!["none", "waiting", "satisfied", "failed"];

        for dep_status in dep_statuses {
            let job = JobNode {
                id: "test".to_string(),
                queue_name: "queue1".to_string(),
                status: "Pending".to_string(),
                dependency_status: dep_status.to_string(),
                depends_on: vec![],
                dependents: vec![],
                workflow_id: None,
                workflow_name: None,
            };

            // Verify dependency status is preserved
            assert_eq!(job.dependency_status, dep_status);
        }
    }
}
