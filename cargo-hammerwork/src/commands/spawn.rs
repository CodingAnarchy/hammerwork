use anyhow::{Context, Result};
use clap::Subcommand;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use tracing::warn;
use uuid::Uuid;

use crate::config::Config;
use crate::utils::database::DatabasePool;
use crate::utils::sql::{Backend, Bind, IntervalUnit, SqlParams, bind_mysql, bind_pg};

#[derive(Debug, Clone)]
pub struct SpawnNode {
    pub id: String,
    pub queue_name: String,
    pub status: String,
    pub depends_on: Vec<String>,
    pub spawn_config: Option<Value>,
    pub created_at: String,
    pub workflow_id: Option<String>,
    pub workflow_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SpawnOperation {
    pub parent_job_id: String,
    pub spawned_jobs: Vec<String>,
    pub spawned_at: String,
    pub operation_id: Option<String>,
    pub config: Option<Value>,
}

/// The `WHERE` condition that marks a parent job as having a spawn config.
fn spawn_config_condition(backend: Backend, column: &str) -> String {
    match backend {
        Backend::Postgres => format!("{column} ? '_spawn_config'"),
        Backend::MySql => format!("JSON_EXTRACT({column}, '$._spawn_config') IS NOT NULL"),
    }
}

/// `child` rows that depend on `parent`.
fn spawn_child_join(backend: Backend) -> &'static str {
    match backend {
        Backend::Postgres => "child.depends_on @> ARRAY[parent.id]",
        Backend::MySql => "JSON_CONTAINS(child.depends_on, CONCAT('\"', parent.id, '\"'))",
    }
}

/// ` AND parent.queue_name = <bound>` when a queue filter is given. Always qualified with
/// `parent.` because the queries join `hammerwork_jobs` to itself.
fn spawn_queue_clause(params: &mut SqlParams, queue: Option<&str>) -> String {
    match queue {
        Some(q) => format!(" AND parent.queue_name = {}", params.text(q)),
        None => String::new(),
    }
}

/// `spawn list`: parents with a spawn config and their child counts. Window, queue name and
/// limit are all bound, in that order.
pub fn build_spawn_list_query(
    backend: Backend,
    queue: Option<&str>,
    recent: bool,
    limit: u32,
) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let config_cond = spawn_config_condition(backend, "parent.payload");
    let config_select = match backend {
        Backend::Postgres => "parent.payload->'_spawn_config'",
        Backend::MySql => "JSON_EXTRACT(parent.payload, '$._spawn_config')",
    };
    let join = spawn_child_join(backend);
    let recent_clause = if recent {
        format!(
            " AND parent.created_at > {}",
            params.ago(1, IntervalUnit::Hour)
        )
    } else {
        String::new()
    };
    let queue_clause = spawn_queue_clause(&mut params, queue);
    let limit_clause = params.limit(limit);
    let sql = format!(
        "SELECT parent.id as parent_id, parent.queue_name, parent.created_at, \
         {config_select} as spawn_config, \
         COUNT(child.id) as spawned_count, \
         parent.workflow_id, parent.workflow_name \
         FROM hammerwork_jobs parent \
         LEFT JOIN hammerwork_jobs child ON {join} \
         WHERE {config_cond} AND parent.status IN ('Completed', 'Running'){recent_clause}{queue_clause} \
         GROUP BY parent.id, parent.queue_name, parent.created_at, parent.payload, parent.workflow_id, parent.workflow_name \
         ORDER BY parent.created_at DESC {limit_clause}"
    );
    (sql, params.into_binds())
}

/// `spawn stats`: totals over the last `hours`. Window, then queue name, are bound.
pub fn build_spawn_stats_total_query(
    backend: Backend,
    hours: u32,
    queue: Option<&str>,
) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let config_cond = spawn_config_condition(backend, "parent.payload");
    let join = spawn_child_join(backend);
    let avg = match backend {
        Backend::Postgres => "CAST(AVG(spawned_count) AS DOUBLE PRECISION)",
        Backend::MySql => "CAST(AVG(spawned_count) AS DOUBLE)",
    };
    let since = params.ago(hours, IntervalUnit::Hour);
    let queue_clause = spawn_queue_clause(&mut params, queue);
    let sql = format!(
        "SELECT COUNT(*) as total_spawn_ops, {avg} as avg_children, MAX(spawned_count) as max_children \
         FROM ( \
           SELECT parent.id, COUNT(child.id) as spawned_count \
           FROM hammerwork_jobs parent \
           LEFT JOIN hammerwork_jobs child ON {join} \
           WHERE {config_cond} AND parent.created_at > {since}{queue_clause} \
           GROUP BY parent.id \
         ) spawn_stats"
    );
    (sql, params.into_binds())
}

/// `spawn stats --detailed`: per-queue breakdown over the last `hours`.
pub fn build_spawn_stats_breakdown_query(
    backend: Backend,
    hours: u32,
    queue: Option<&str>,
) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let config_cond = spawn_config_condition(backend, "parent.payload");
    let join = spawn_child_join(backend);
    let avg = match backend {
        Backend::Postgres => "CAST(AVG(spawned_count) AS DOUBLE PRECISION)",
        Backend::MySql => "CAST(AVG(spawned_count) AS DOUBLE)",
    };
    let since = params.ago(hours, IntervalUnit::Hour);
    let queue_clause = spawn_queue_clause(&mut params, queue);
    let sql = format!(
        "SELECT queue_name, COUNT(*) as spawn_count, {avg} as avg_children \
         FROM ( \
           SELECT parent.queue_name, COUNT(child.id) as spawned_count \
           FROM hammerwork_jobs parent \
           LEFT JOIN hammerwork_jobs child ON {join} \
           WHERE {config_cond} AND parent.created_at > {since}{queue_clause} \
           GROUP BY parent.id, parent.queue_name \
         ) spawn_breakdown \
         GROUP BY queue_name ORDER BY spawn_count DESC"
    );
    (sql, params.into_binds())
}

/// `spawn pending`: running or pending jobs that carry a spawn config (at most 50).
pub fn build_pending_spawns_query(backend: Backend, queue: Option<&str>) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let config_cond = spawn_config_condition(backend, "payload");
    let config_select = match backend {
        Backend::Postgres => "payload->'_spawn_config'",
        Backend::MySql => "JSON_EXTRACT(payload, '$._spawn_config')",
    };
    let queue_clause = match queue {
        Some(q) => format!(" AND queue_name = {}", params.text(q)),
        None => String::new(),
    };
    let sql = format!(
        "SELECT id, queue_name, status, created_at, {config_select} as spawn_config \
         FROM hammerwork_jobs \
         WHERE {config_cond} AND status IN ('Running', 'Pending'){queue_clause} \
         ORDER BY created_at DESC LIMIT 50"
    );
    (sql, params.into_binds())
}

/// The `spawn_config` column of a MySQL row as JSON text. `JSON_EXTRACT` yields a native
/// JSON value, which sqlx will not decode as a string, so try both.
fn mysql_spawn_config_text(row: &sqlx::mysql::MySqlRow) -> Result<Option<String>> {
    use sqlx::Row;
    match row.try_get::<Option<Value>, _>("spawn_config") {
        Ok(value) => Ok(value.map(|v| v.to_string())),
        Err(_) => Ok(row.try_get::<Option<String>, _>("spawn_config")?),
    }
}

#[derive(Subcommand)]
pub enum SpawnCommand {
    #[command(about = "List active spawn operations")]
    List {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short, long, help = "Maximum number of operations to display")]
        limit: Option<u32>,
        #[arg(long, help = "Show only recent spawn operations")]
        recent: bool,
        #[arg(long, help = "Show spawn operations for specific queue")]
        queue: Option<String>,
    },
    #[command(about = "Show spawn tree hierarchy for a job")]
    Tree {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(help = "Job ID (parent or child)")]
        job_id: String,
        #[arg(long, help = "Show full spawn tree (both up and down)")]
        full: bool,
        #[arg(long, help = "Show only children of this job")]
        children_only: bool,
        #[arg(long, help = "Output format (text, json, mermaid)")]
        format: Option<String>,
    },
    #[command(about = "Show spawn statistics")]
    Stats {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'Q', long, help = "Filter by queue name")]
        queue: Option<String>,
        #[arg(long, help = "Time period in hours (default: 24)")]
        hours: Option<u32>,
        #[arg(long, help = "Show detailed breakdown")]
        detailed: bool,
    },
    #[command(about = "Track spawn lineage for a job")]
    Lineage {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(help = "Job ID")]
        job_id: String,
        #[arg(long, help = "Show ancestor chain")]
        ancestors: bool,
        #[arg(long, help = "Show descendant chain")]
        descendants: bool,
        #[arg(long, help = "Maximum depth to traverse")]
        depth: Option<u32>,
    },
    #[command(about = "Show jobs waiting for spawn completion")]
    Pending {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'Q', long, help = "Filter by queue name")]
        queue: Option<String>,
        #[arg(long, help = "Show spawn configuration details")]
        show_config: bool,
    },
    #[command(about = "Monitor spawn operations in real-time")]
    Monitor {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(long, help = "Refresh interval in seconds (default: 5)")]
        interval: Option<u32>,
        #[arg(short = 'Q', long, help = "Filter by queue name")]
        queue: Option<String>,
    },
}

impl SpawnCommand {
    pub async fn execute(&self, config: Config) -> Result<()> {
        let db_url = self.get_database_url(&config)?;
        let pool = DatabasePool::connect(&db_url, config.get_connection_pool_size()).await?;

        match self {
            SpawnCommand::List {
                limit,
                recent,
                queue,
                ..
            } => {
                self.list_spawn_operations(pool, *limit, *recent, queue.as_deref())
                    .await
            }
            SpawnCommand::Tree {
                job_id,
                full,
                children_only,
                format,
                ..
            } => {
                self.show_spawn_tree(pool, job_id, *full, *children_only, format.as_deref())
                    .await
            }
            SpawnCommand::Stats {
                queue,
                hours,
                detailed,
                ..
            } => {
                self.show_spawn_stats(pool, queue.as_deref(), *hours, *detailed)
                    .await
            }
            SpawnCommand::Lineage {
                job_id,
                ancestors,
                descendants,
                depth,
                ..
            } => {
                self.show_spawn_lineage(pool, job_id, *ancestors, *descendants, *depth)
                    .await
            }
            SpawnCommand::Pending {
                queue, show_config, ..
            } => {
                self.show_pending_spawns(pool, queue.as_deref(), *show_config)
                    .await
            }
            SpawnCommand::Monitor {
                interval, queue, ..
            } => {
                self.monitor_spawn_operations(pool, *interval, queue.as_deref())
                    .await
            }
        }
    }

    pub fn get_database_url(&self, config: &Config) -> Result<String> {
        let url_option = match self {
            SpawnCommand::List { database_url, .. } => database_url,
            SpawnCommand::Tree { database_url, .. } => database_url,
            SpawnCommand::Stats { database_url, .. } => database_url,
            SpawnCommand::Lineage { database_url, .. } => database_url,
            SpawnCommand::Pending { database_url, .. } => database_url,
            SpawnCommand::Monitor { database_url, .. } => database_url,
        };

        url_option
            .as_ref()
            .map(|s| s.as_str())
            .or(config.get_database_url())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Database URL is required"))
    }

    async fn list_spawn_operations(
        &self,
        pool: DatabasePool,
        limit: Option<u32>,
        recent: bool,
        queue_filter: Option<&str>,
    ) -> Result<()> {
        let limit = limit.unwrap_or(20);
        let (query, binds) = build_spawn_list_query(pool.backend(), queue_filter, recent, limit);

        match pool {
            DatabasePool::Postgres(pg_pool) => {
                let rows = bind_pg(sqlx::query(&query), &binds)
                    .fetch_all(&pg_pool)
                    .await?;

                println!("📊 Spawn Operations");
                println!("{}", "=".repeat(80));

                if rows.is_empty() {
                    println!("No spawn operations found.");
                    return Ok(());
                }

                println!(
                    "{:<8} {:<15} {:<12} {:<20} {:<10} {:<15}",
                    "Parent", "Queue", "Children", "Spawned At", "Operation", "Workflow"
                );
                println!("{}", "-".repeat(80));

                for row in rows {
                    use sqlx::Row;
                    let parent_id: Uuid = row.try_get("parent_id")?;
                    let queue_name: String = row.try_get("queue_name")?;
                    let spawned_count: i64 = row.try_get("spawned_count")?;
                    let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;
                    let spawn_config: Option<Value> = row.try_get("spawn_config")?;

                    let operation_id = spawn_config
                        .as_ref()
                        .and_then(|c| c.get("operation_id"))
                        .and_then(|id| id.as_str())
                        .unwrap_or("none");

                    let workflow = row
                        .try_get::<Option<String>, _>("workflow_name")?
                        .unwrap_or_else(|| "none".to_string());

                    println!(
                        "{:<8} {:<15} {:<12} {:<20} {:<10} {:<15}",
                        &parent_id.to_string()[..8],
                        &queue_name[..std::cmp::min(15, queue_name.len())],
                        spawned_count,
                        created_at.format("%m-%d %H:%M:%S"),
                        &operation_id[..std::cmp::min(10, operation_id.len())],
                        &workflow[..std::cmp::min(15, workflow.len())]
                    );
                }
            }
            DatabasePool::MySQL(mysql_pool) => {
                let rows = bind_mysql(sqlx::query(&query), &binds)
                    .fetch_all(&mysql_pool)
                    .await?;

                println!("📊 Spawn Operations");
                println!("{}", "=".repeat(80));

                if rows.is_empty() {
                    println!("No spawn operations found.");
                    return Ok(());
                }

                println!(
                    "{:<8} {:<15} {:<12} {:<20} {:<10} {:<15}",
                    "Parent", "Queue", "Children", "Spawned At", "Operation", "Workflow"
                );
                println!("{}", "-".repeat(80));

                for row in rows {
                    use sqlx::Row;
                    let parent_id: String = row.try_get("parent_id")?;
                    let queue_name: String = row.try_get("queue_name")?;
                    let spawned_count: i64 = row.try_get("spawned_count")?;
                    let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;
                    let spawn_config_str: Option<String> = mysql_spawn_config_text(&row)?;

                    let operation_id = spawn_config_str
                        .as_ref()
                        .map(
                            |config_str| match serde_json::from_str::<Value>(config_str) {
                                Ok(config) => config
                                    .get("operation_id")
                                    .and_then(|id| id.as_str().map(|s| s.to_string()))
                                    .unwrap_or_else(|| "none".to_string()),
                                Err(_) => "invalid-config".to_string(),
                            },
                        )
                        .unwrap_or_else(|| "none".to_string());

                    let workflow = row
                        .try_get::<Option<String>, _>("workflow_name")?
                        .unwrap_or_else(|| "none".to_string());

                    println!(
                        "{:<8} {:<15} {:<12} {:<20} {:<10} {:<15}",
                        &parent_id[..std::cmp::min(8, parent_id.len())],
                        &queue_name[..std::cmp::min(15, queue_name.len())],
                        spawned_count,
                        created_at.format("%m-%d %H:%M:%S"),
                        &operation_id[..std::cmp::min(10, operation_id.len())],
                        &workflow[..std::cmp::min(15, workflow.len())]
                    );
                }
            }
        }

        Ok(())
    }

    async fn show_spawn_tree(
        &self,
        pool: DatabasePool,
        job_id: &str,
        show_full: bool,
        children_only: bool,
        format: Option<&str>,
    ) -> Result<()> {
        let format = format.unwrap_or("text");
        let job_uuid = Uuid::parse_str(job_id)?;

        // Get the target job
        let target_job = self.get_spawn_node(&pool, &job_uuid).await?;
        let target_job = match target_job {
            Some(job) => job,
            None => {
                println!("Job not found: {}", job_id);
                return Ok(());
            }
        };

        // Collect spawn tree
        let spawn_tree = if show_full || !children_only {
            self.collect_full_spawn_tree(&pool, &target_job).await?
        } else {
            self.collect_spawn_children(&pool, &target_job).await?
        };

        match format {
            "text" => self.print_spawn_tree_text(&spawn_tree, &target_job.id),
            "json" => self.print_spawn_tree_json(&spawn_tree)?,
            "mermaid" => self.print_spawn_tree_mermaid(&spawn_tree, &target_job.id),
            _ => {
                println!("Unsupported format: {}. Use: text, json, mermaid", format);
                return Ok(());
            }
        }

        Ok(())
    }

    async fn show_spawn_stats(
        &self,
        pool: DatabasePool,
        queue_filter: Option<&str>,
        hours: Option<u32>,
        detailed: bool,
    ) -> Result<()> {
        let hours = hours.unwrap_or(24);

        println!("📈 Spawn Statistics (Last {} hours)", hours);
        println!("{}", "=".repeat(60));

        let pool_backend = pool.backend();
        match pool {
            DatabasePool::Postgres(pg_pool) => {
                // Total spawn operations
                let (total_query, total_binds) =
                    build_spawn_stats_total_query(pool_backend, hours, queue_filter);

                {
                    let row = bind_pg(sqlx::query(&total_query), &total_binds)
                        .fetch_one(&pg_pool)
                        .await?;
                    use sqlx::Row;
                    let total: i64 = row.try_get("total_spawn_ops")?;
                    let avg: Option<f64> = row.try_get("avg_children")?;
                    let max: Option<i64> = row.try_get("max_children")?;

                    println!("Total Spawn Operations: {}", total);
                    if let Some(avg) = avg {
                        println!("Average Children per Spawn: {:.1}", avg);
                    }
                    if let Some(max) = max {
                        println!("Maximum Children in Single Spawn: {}", max);
                    }
                }

                if detailed {
                    println!("\n📋 Breakdown by Queue:");
                    let (breakdown_query, breakdown_binds) =
                        build_spawn_stats_breakdown_query(pool_backend, hours, queue_filter);

                    let rows = bind_pg(sqlx::query(&breakdown_query), &breakdown_binds)
                        .fetch_all(&pg_pool)
                        .await?;

                    println!(
                        "{:<20} {:<12} {:<15}",
                        "Queue", "Operations", "Avg Children"
                    );
                    println!("{}", "-".repeat(47));

                    for row in rows {
                        use sqlx::Row;
                        let queue: String = row.try_get("queue_name")?;
                        let count: i64 = row.try_get("spawn_count")?;
                        let avg: Option<f64> = row.try_get("avg_children")?;

                        println!(
                            "{:<20} {:<12} {:<15.1}",
                            &queue[..std::cmp::min(20, queue.len())],
                            count,
                            avg.unwrap_or(0.0)
                        );
                    }
                }
            }
            DatabasePool::MySQL(mysql_pool) => {
                // Total spawn operations - MySQL compatible
                let (total_query, total_binds) =
                    build_spawn_stats_total_query(pool_backend, hours, queue_filter);

                {
                    let row = bind_mysql(sqlx::query(&total_query), &total_binds)
                        .fetch_one(&mysql_pool)
                        .await?;
                    use sqlx::Row;
                    let total: i64 = row.try_get("total_spawn_ops")?;
                    let avg: Option<f64> = row.try_get("avg_children")?;
                    let max: Option<i64> = row.try_get("max_children")?;

                    println!("Total Spawn Operations: {}", total);
                    if let Some(avg) = avg {
                        println!("Average Children per Spawn: {:.1}", avg);
                    }
                    if let Some(max) = max {
                        println!("Maximum Children in Single Spawn: {}", max);
                    }
                }

                if detailed {
                    println!("\n📋 Breakdown by Queue:");
                    let (breakdown_query, breakdown_binds) =
                        build_spawn_stats_breakdown_query(pool_backend, hours, queue_filter);

                    let rows = bind_mysql(sqlx::query(&breakdown_query), &breakdown_binds)
                        .fetch_all(&mysql_pool)
                        .await?;

                    println!(
                        "{:<20} {:<12} {:<15}",
                        "Queue", "Operations", "Avg Children"
                    );
                    println!("{}", "-".repeat(47));

                    for row in rows {
                        use sqlx::Row;
                        let queue: String = row.try_get("queue_name")?;
                        let count: i64 = row.try_get("spawn_count")?;
                        let avg: Option<f64> = row.try_get("avg_children")?;

                        println!(
                            "{:<20} {:<12} {:<15.1}",
                            &queue[..std::cmp::min(20, queue.len())],
                            count,
                            avg.unwrap_or(0.0)
                        );
                    }
                }
            }
        }

        Ok(())
    }

    async fn show_spawn_lineage(
        &self,
        pool: DatabasePool,
        job_id: &str,
        show_ancestors: bool,
        show_descendants: bool,
        max_depth: Option<u32>,
    ) -> Result<()> {
        let job_uuid = Uuid::parse_str(job_id)?;
        let max_depth = max_depth.unwrap_or(10);

        let target_job = self.get_spawn_node(&pool, &job_uuid).await?;
        let target_job = match target_job {
            Some(job) => job,
            None => {
                println!("Job not found: {}", job_id);
                return Ok(());
            }
        };

        println!("🔗 Spawn Lineage for {}", job_id);
        println!(
            "Queue: {} | Status: {}",
            target_job.queue_name, target_job.status
        );
        println!("{}", "=".repeat(60));

        // Show ancestors (parents, grandparents, etc.)
        if show_ancestors || (!show_descendants) {
            println!("\n⬆️  Ancestor Chain:");
            let ancestors = self
                .collect_ancestors(&pool, &target_job, max_depth)
                .await?;
            if ancestors.is_empty() {
                println!("  No spawn ancestors found (this is a root job)");
            } else {
                for (depth, ancestor) in ancestors.iter().enumerate() {
                    let indent = "  ".repeat(depth + 1);
                    println!(
                        "{}└─ [{}] {} ({})",
                        indent,
                        &ancestor.id[..8],
                        ancestor.queue_name,
                        ancestor.status
                    );
                }
            }
        }

        // Show descendants (children, grandchildren, etc.)
        if show_descendants || (!show_ancestors) {
            println!("\n⬇️  Descendant Chain:");
            let descendants = self
                .collect_descendants(&pool, &target_job, max_depth)
                .await?;
            if descendants.is_empty() {
                println!("  No spawn descendants found (this job hasn't spawned children)");
            } else {
                self.print_descendant_tree(&descendants, &target_job.id, 0);
            }
        }

        Ok(())
    }

    async fn show_pending_spawns(
        &self,
        pool: DatabasePool,
        queue_filter: Option<&str>,
        show_config: bool,
    ) -> Result<()> {
        let (query, binds) = build_pending_spawns_query(pool.backend(), queue_filter);

        println!("⏳ Jobs with Pending Spawn Operations");
        println!("{}", "=".repeat(70));

        match pool {
            DatabasePool::Postgres(pg_pool) => {
                let rows = bind_pg(sqlx::query(&query), &binds)
                    .fetch_all(&pg_pool)
                    .await?;

                if rows.is_empty() {
                    println!("No jobs with pending spawn operations found.");
                    return Ok(());
                }

                for row in rows {
                    use sqlx::Row;
                    let id: Uuid = row.try_get("id")?;
                    let queue_name: String = row.try_get("queue_name")?;
                    let status: String = row.try_get("status")?;
                    let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;
                    let spawn_config: Option<Value> = row.try_get("spawn_config")?;

                    println!(
                        "📋 Job: {} | Queue: {} | Status: {} | Created: {}",
                        &id.to_string()[..8],
                        queue_name,
                        status,
                        created_at.format("%m-%d %H:%M:%S")
                    );

                    if show_config {
                        if let Some(config) = spawn_config {
                            println!(
                                "   Spawn Config: {}",
                                serde_json::to_string_pretty(&config)
                                    .unwrap_or_else(|_| "Invalid JSON".to_string())
                            );
                        }
                        println!();
                    }
                }
            }
            DatabasePool::MySQL(mysql_pool) => {
                let rows = bind_mysql(sqlx::query(&query), &binds)
                    .fetch_all(&mysql_pool)
                    .await?;

                if rows.is_empty() {
                    println!("No jobs with pending spawn operations found.");
                    return Ok(());
                }

                for row in rows {
                    use sqlx::Row;
                    let id: String = row.try_get("id")?;
                    let queue_name: String = row.try_get("queue_name")?;
                    let status: String = row.try_get("status")?;
                    let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;
                    let spawn_config_str: Option<String> = mysql_spawn_config_text(&row)?;

                    println!(
                        "📋 Job: {} | Queue: {} | Status: {} | Created: {}",
                        &id[..std::cmp::min(8, id.len())],
                        queue_name,
                        status,
                        created_at.format("%m-%d %H:%M:%S")
                    );

                    if show_config {
                        if let Some(ref config_str) = spawn_config_str {
                            match serde_json::from_str::<Value>(config_str) {
                                Ok(config) => {
                                    println!(
                                        "   Spawn Config: {}",
                                        serde_json::to_string_pretty(&config)
                                            .unwrap_or_else(|_| "Invalid JSON".to_string())
                                    );
                                }
                                Err(_) => {
                                    println!("   Spawn Config: {}", config_str);
                                }
                            }
                        }
                        println!();
                    }
                }
            }
        }

        Ok(())
    }

    async fn monitor_spawn_operations(
        &self,
        pool: DatabasePool,
        interval: Option<u32>,
        queue_filter: Option<&str>,
    ) -> Result<()> {
        let interval = std::time::Duration::from_secs(interval.unwrap_or(5) as u64);

        println!("🔄 Monitoring Spawn Operations (Press Ctrl+C to stop)");
        println!("Refresh interval: {:?}", interval);
        if let Some(queue) = queue_filter {
            println!("Queue filter: {}", queue);
        }
        println!("{}", "=".repeat(80));

        // For monitoring, we need to reconnect each time since we can't clone the pool
        // This is acceptable for a monitoring command that runs periodically
        match &pool {
            DatabasePool::Postgres(_pg_pool) => {
                // Extract URL from pool options - this is a limitation,
                // in practice we'd store the URL when creating the pool
                warn!("Monitor mode requires reconnection for each refresh");
                Err(anyhow::anyhow!(
                    "Monitor mode needs database URL to reconnect. Use list/stats commands instead for one-time queries."
                ))
            }
            DatabasePool::MySQL(_) => {
                warn!("Monitor mode requires reconnection for each refresh");
                Err(anyhow::anyhow!(
                    "Monitor mode needs database URL to reconnect. Use list/stats commands instead for one-time queries."
                ))
            }
        }

        // Note: In a real implementation, we'd store the connection URL and pool size
        // in the DatabasePool struct to enable reconnection for monitoring
    }

    // Helper methods for data collection and display

    async fn get_spawn_node(
        &self,
        pool: &DatabasePool,
        job_id: &Uuid,
    ) -> Result<Option<SpawnNode>> {
        let query = r#"
            SELECT id, queue_name, status, depends_on, 
                   payload->'_spawn_config' as spawn_config,
                   created_at, workflow_id, workflow_name
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
                    Ok(Some(self.postgres_row_to_spawn_node(&row)?))
                } else {
                    Ok(None)
                }
            }
            DatabasePool::MySQL(mysql_pool) => {
                let mysql_query = query.replace("$1", "?");
                if let Some(row) = sqlx::query(&mysql_query)
                    .bind(job_id.to_string())
                    .fetch_optional(mysql_pool)
                    .await?
                {
                    Ok(Some(self.mysql_row_to_spawn_node(&row)?))
                } else {
                    Ok(None)
                }
            }
        }
    }

    async fn collect_spawn_children(
        &self,
        pool: &DatabasePool,
        parent: &SpawnNode,
    ) -> Result<Vec<SpawnNode>> {
        let query = r#"
            SELECT id, queue_name, status, depends_on,
                   payload->'_spawn_config' as spawn_config,
                   created_at, workflow_id, workflow_name
            FROM hammerwork_jobs
            WHERE depends_on @> ARRAY[$1::uuid]
            ORDER BY created_at
        "#;

        let mut children = Vec::new();

        match pool {
            DatabasePool::Postgres(pg_pool) => {
                // depends_on is a UUID[] column in PostgreSQL
                let parent_id = Uuid::parse_str(&parent.id)?;
                let rows = sqlx::query(query)
                    .bind(parent_id)
                    .fetch_all(pg_pool)
                    .await?;

                for row in rows {
                    children.push(self.postgres_row_to_spawn_node(&row)?);
                }
            }
            DatabasePool::MySQL(mysql_pool) => {
                let mysql_query = r#"
                    SELECT id, queue_name, status, depends_on,
                           JSON_EXTRACT(payload, '$._spawn_config') as spawn_config,
                           created_at, workflow_id, workflow_name
                    FROM hammerwork_jobs
                    WHERE JSON_CONTAINS(depends_on, ?)
                    ORDER BY created_at
                "#;

                let parent_json = format!("\"{}\"", parent.id);
                let rows = sqlx::query(mysql_query)
                    .bind(&parent_json)
                    .fetch_all(mysql_pool)
                    .await?;

                for row in rows {
                    children.push(self.mysql_row_to_spawn_node(&row)?);
                }
            }
        }

        Ok(children)
    }

    async fn collect_full_spawn_tree(
        &self,
        pool: &DatabasePool,
        target: &SpawnNode,
    ) -> Result<Vec<SpawnNode>> {
        let mut all_nodes = HashMap::new();
        let mut to_visit = VecDeque::new();
        let mut visited = HashSet::new();

        // Start with target job
        all_nodes.insert(target.id.clone(), target.clone());
        to_visit.push_back(target.id.clone());

        // Traverse both up (parents) and down (children)
        while let Some(job_id) = to_visit.pop_front() {
            if visited.contains(&job_id) {
                continue;
            }
            visited.insert(job_id.clone());

            if let Some(job) = all_nodes.get(&job_id).cloned() {
                // Get children
                let children = self.collect_spawn_children(pool, &job).await?;
                for child in children {
                    if !all_nodes.contains_key(&child.id) {
                        all_nodes.insert(child.id.clone(), child.clone());
                        to_visit.push_back(child.id.clone());
                    }
                }

                // Get parents (jobs this one depends on)
                for parent_id in &job.depends_on {
                    let parent_uuid = Uuid::parse_str(parent_id)
                        .with_context(|| format!("invalid dependency id '{}'", parent_id))?;
                    if let Some(parent) = self.get_spawn_node(pool, &parent_uuid).await?
                        && !all_nodes.contains_key(&parent.id)
                    {
                        all_nodes.insert(parent.id.clone(), parent.clone());
                        to_visit.push_back(parent.id.clone());
                    }
                }
            }
        }

        Ok(all_nodes.into_values().collect())
    }

    async fn collect_ancestors(
        &self,
        pool: &DatabasePool,
        job: &SpawnNode,
        max_depth: u32,
    ) -> Result<Vec<SpawnNode>> {
        let mut ancestors = Vec::new();
        let mut current = job.clone();
        let mut depth = 0;

        while depth < max_depth && !current.depends_on.is_empty() {
            // Find the spawn parent (first dependency that has spawn config)
            let mut parent_found = false;
            for parent_id in &current.depends_on {
                let parent_uuid = Uuid::parse_str(parent_id)
                    .with_context(|| format!("invalid dependency id '{}'", parent_id))?;
                if let Some(parent) = self.get_spawn_node(pool, &parent_uuid).await?
                    && parent.spawn_config.is_some()
                {
                    ancestors.push(parent.clone());
                    current = parent;
                    parent_found = true;
                    break;
                }
            }

            if !parent_found {
                break;
            }

            depth += 1;
        }

        ancestors.reverse(); // Show from oldest ancestor to immediate parent
        Ok(ancestors)
    }

    async fn collect_descendants(
        &self,
        pool: &DatabasePool,
        job: &SpawnNode,
        max_depth: u32,
    ) -> Result<Vec<SpawnNode>> {
        let mut descendants = Vec::new();
        let mut to_visit = VecDeque::new();
        let mut visited = HashSet::new();

        to_visit.push_back((job.clone(), 0));

        while let Some((current_job, depth)) = to_visit.pop_front() {
            if depth >= max_depth || visited.contains(&current_job.id) {
                continue;
            }
            visited.insert(current_job.id.clone());

            let children = self.collect_spawn_children(pool, &current_job).await?;
            for child in children {
                descendants.push(child.clone());
                to_visit.push_back((child, depth + 1));
            }
        }

        Ok(descendants)
    }

    fn postgres_row_to_spawn_node(&self, row: &sqlx::postgres::PgRow) -> Result<SpawnNode> {
        use sqlx::Row;

        let id: Uuid = row.try_get("id")?;
        // depends_on is a UUID[] column in PostgreSQL
        let depends_on = row
            .try_get::<Option<Vec<Uuid>>, _>("depends_on")?
            .unwrap_or_default()
            .iter()
            .map(Uuid::to_string)
            .collect();
        let spawn_config: Option<Value> = row.try_get("spawn_config")?;
        let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;

        let workflow_id: Option<String> = row
            .try_get::<Option<Uuid>, _>("workflow_id")?
            .map(|uuid| uuid.to_string());

        Ok(SpawnNode {
            id: id.to_string(),
            queue_name: row.try_get("queue_name")?,
            status: row.try_get("status")?,
            depends_on,
            spawn_config,
            created_at: created_at.to_string(),
            workflow_id,
            workflow_name: row.try_get("workflow_name")?,
        })
    }

    fn mysql_row_to_spawn_node(&self, row: &sqlx::mysql::MySqlRow) -> Result<SpawnNode> {
        use sqlx::Row;

        let id: String = row.try_get("id")?;
        let depends_on = self.parse_json_array(row.try_get("depends_on")?)?;

        // Handle spawn config which might be a JSON string in MySQL
        let spawn_config: Option<Value> = match row.try_get::<Option<String>, _>("spawn_config") {
            Ok(Some(config_str)) => Some(
                serde_json::from_str(&config_str)
                    .context("corrupt spawn_config JSON in hammerwork_jobs")?,
            ),
            Ok(None) => None,
            // Not a text column: decode it as a native JSON value instead.
            Err(_) => row.try_get::<Option<Value>, _>("spawn_config")?,
        };

        let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;

        Ok(SpawnNode {
            id,
            queue_name: row.try_get("queue_name")?,
            status: row.try_get("status")?,
            depends_on,
            spawn_config,
            created_at: created_at.to_string(),
            workflow_id: row.try_get("workflow_id")?,
            workflow_name: row.try_get("workflow_name")?,
        })
    }

    fn parse_json_array(&self, json_value: Option<Value>) -> Result<Vec<String>> {
        match json_value {
            Some(Value::Array(arr)) => Ok(arr
                .into_iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()),
            _ => Ok(Vec::new()),
        }
    }

    fn print_spawn_tree_text(&self, nodes: &[SpawnNode], target_id: &str) {
        println!("\n🌳 Spawn Tree");
        println!("{}", "=".repeat(60));

        let node_map: HashMap<String, &SpawnNode> =
            nodes.iter().map(|n| (n.id.clone(), n)).collect();

        // Find root nodes (nodes with no spawn parents)
        let mut roots: Vec<&SpawnNode> = nodes
            .iter()
            .filter(|node| {
                !node.depends_on.iter().any(|dep_id| {
                    node_map
                        .get(dep_id)
                        .is_some_and(|dep| dep.spawn_config.is_some())
                })
            })
            .collect();

        if roots.is_empty() {
            roots = nodes.iter().collect();
        }

        roots.sort_by(|a, b| a.created_at.cmp(&b.created_at));

        let mut visited = HashSet::new();
        for root in roots {
            if !visited.contains(&root.id) {
                self.print_spawn_node_tree(root, &node_map, &mut visited, 0, target_id);
            }
        }
    }

    #[allow(clippy::only_used_in_recursion)]
    fn print_spawn_node_tree(
        &self,
        node: &SpawnNode,
        node_map: &HashMap<String, &SpawnNode>,
        visited: &mut HashSet<String>,
        depth: usize,
        target_id: &str,
    ) {
        if visited.contains(&node.id) {
            return;
        }
        visited.insert(node.id.clone());

        let indent = "  ".repeat(depth);
        let marker = if depth == 0 { "┌─" } else { "├─" };
        let highlight = if node.id == target_id { " ⭐" } else { "" };
        let spawn_indicator = if node.spawn_config.is_some() {
            "🚀"
        } else {
            "📝"
        };

        println!(
            "{}{}{} [{}] {} ({}){}",
            indent,
            marker,
            spawn_indicator,
            &node.id[..8],
            node.queue_name,
            node.status,
            highlight
        );

        // Find and print children
        let children: Vec<&SpawnNode> = node_map
            .values()
            .filter(|child| child.depends_on.contains(&node.id) && child.id != node.id)
            .copied()
            .collect();

        for child in children {
            self.print_spawn_node_tree(child, node_map, visited, depth + 1, target_id);
        }
    }

    fn print_spawn_tree_json(&self, nodes: &[SpawnNode]) -> Result<()> {
        println!("\n📄 Spawn Tree (JSON)");
        println!("{}", "=".repeat(60));

        let tree_data = serde_json::json!({
            "spawn_tree": {
                "nodes": nodes.iter().map(|node| {
                    serde_json::json!({
                        "id": node.id,
                        "queue": node.queue_name,
                        "status": node.status,
                        "depends_on": node.depends_on,
                        "has_spawn_config": node.spawn_config.is_some(),
                        "spawn_config": node.spawn_config,
                        "created_at": node.created_at,
                        "workflow_id": node.workflow_id,
                        "workflow_name": node.workflow_name
                    })
                }).collect::<Vec<_>>(),
                "edges": self.build_spawn_edges(nodes)
            }
        });

        println!("{}", serde_json::to_string_pretty(&tree_data)?);
        Ok(())
    }

    fn print_spawn_tree_mermaid(&self, nodes: &[SpawnNode], target_id: &str) {
        println!("\n🧜‍♀️ Spawn Tree (Mermaid)");
        println!("{}", "=".repeat(60));
        println!("graph TD");
        println!("    subgraph \"🚀 Spawn Tree\"");

        // Define nodes
        for node in nodes {
            let short_id = &node.id[..8];
            let status_class = match node.status.as_str() {
                "Completed" => ":::completed",
                "Failed" => ":::failed",
                "Running" => ":::running",
                "Pending" => ":::pending",
                _ => ":::default",
            };

            let spawn_indicator = if node.spawn_config.is_some() {
                "🚀"
            } else {
                "📝"
            };
            let target_indicator = if node.id == target_id { "⭐" } else { "" };

            println!(
                "        {}[\"{}{}<br/>{}<br/>{}\"]{}",
                short_id, spawn_indicator, target_indicator, short_id, node.status, status_class
            );
        }

        // Define spawn relationships
        let node_map: HashMap<String, &SpawnNode> =
            nodes.iter().map(|n| (n.id.clone(), n)).collect();

        for node in nodes {
            for dep_id in &node.depends_on {
                if let Some(parent) = node_map.get(dep_id)
                    && parent.spawn_config.is_some()
                {
                    println!("        {} -->|spawns| {}", &dep_id[..8], &node.id[..8]);
                }
            }
        }

        println!("    end");
        println!();

        // CSS classes for styling
        println!("    classDef completed fill:#d4edda,stroke:#155724");
        println!("    classDef failed fill:#f8d7da,stroke:#721c24");
        println!("    classDef running fill:#cce7ff,stroke:#004085");
        println!("    classDef pending fill:#fff3cd,stroke:#856404");
        println!("    classDef default fill:#e2e3e5,stroke:#383d41");
    }

    fn build_spawn_edges(&self, nodes: &[SpawnNode]) -> Vec<Value> {
        let node_map: HashMap<String, &SpawnNode> =
            nodes.iter().map(|n| (n.id.clone(), n)).collect();

        let mut edges = Vec::new();

        for node in nodes {
            for dep_id in &node.depends_on {
                if let Some(parent) = node_map.get(dep_id)
                    && parent.spawn_config.is_some()
                {
                    edges.push(serde_json::json!({
                        "from": dep_id,
                        "to": node.id,
                        "type": "spawn",
                        "relationship": "parent_spawned_child"
                    }));
                }
            }
        }

        edges
    }

    fn print_descendant_tree(&self, descendants: &[SpawnNode], _target_id: &str, depth: usize) {
        if descendants.is_empty() {
            return;
        }

        for (i, descendant) in descendants.iter().enumerate() {
            let indent = "  ".repeat(depth + 1);
            let marker = if i == descendants.len() - 1 {
                "└─"
            } else {
                "├─"
            };

            println!(
                "{}{}📝 [{}] {} ({})",
                indent,
                marker,
                &descendant.id[..8],
                descendant.queue_name,
                descendant.status
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_spawn_command_variants() {
        // Test that all spawn command variants can be created
        let commands = [
            SpawnCommand::List {
                database_url: None,
                limit: Some(10),
                recent: false,
                queue: None,
            },
            SpawnCommand::Tree {
                database_url: None,
                job_id: "test-job".to_string(),
                full: false,
                children_only: false,
                format: None,
            },
            SpawnCommand::Stats {
                database_url: None,
                queue: None,
                hours: Some(24),
                detailed: false,
            },
            SpawnCommand::Lineage {
                database_url: None,
                job_id: "test-job".to_string(),
                ancestors: false,
                descendants: false,
                depth: None,
            },
            SpawnCommand::Pending {
                database_url: None,
                queue: None,
                show_config: false,
            },
            SpawnCommand::Monitor {
                database_url: None,
                interval: Some(5),
                queue: None,
            },
        ];

        // All commands should be valid
        assert_eq!(commands.len(), 6);
    }

    #[test]
    fn test_spawn_node_creation() {
        let spawn_node = SpawnNode {
            id: "test-spawn-123".to_string(),
            queue_name: "spawning-queue".to_string(),
            status: "Completed".to_string(),
            depends_on: vec!["parent-job".to_string()],
            spawn_config: Some(serde_json::json!({"max_spawn_count": 5})),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            workflow_id: Some("workflow-123".to_string()),
            workflow_name: Some("test-workflow".to_string()),
        };

        assert_eq!(spawn_node.id, "test-spawn-123");
        assert!(spawn_node.spawn_config.is_some());
        assert_eq!(spawn_node.depends_on.len(), 1);
    }

    #[test]
    fn test_spawn_operation_creation() {
        let spawn_op = SpawnOperation {
            parent_job_id: "parent-123".to_string(),
            spawned_jobs: vec!["child-1".to_string(), "child-2".to_string()],
            spawned_at: "2024-01-01T00:00:00Z".to_string(),
            operation_id: Some("op-123".to_string()),
            config: Some(serde_json::json!({"batch_size": 10})),
        };

        assert_eq!(spawn_op.parent_job_id, "parent-123");
        assert_eq!(spawn_op.spawned_jobs.len(), 2);
        assert!(spawn_op.operation_id.is_some());
        assert!(spawn_op.config.is_some());
    }

    #[test]
    fn test_json_array_parsing() {
        let spawn_cmd = SpawnCommand::List {
            database_url: None,
            limit: None,
            recent: false,
            queue: None,
        };

        // Test valid array parsing
        let json_array = serde_json::json!(["job1", "job2", "job3"]);
        let result = spawn_cmd.parse_json_array(Some(json_array)).unwrap();
        assert_eq!(result, vec!["job1", "job2", "job3"]);

        // Test empty input
        let result = spawn_cmd.parse_json_array(None).unwrap();
        assert!(result.is_empty());

        // Test non-array JSON
        let json_object = serde_json::json!({"key": "value"});
        let result = spawn_cmd.parse_json_array(Some(json_object)).unwrap();
        assert!(result.is_empty());

        // Test array with mixed types (should only extract strings)
        let mixed_array = serde_json::json!(["string1", 123, true, "string2", null]);
        let result = spawn_cmd.parse_json_array(Some(mixed_array)).unwrap();
        assert_eq!(result, vec!["string1", "string2"]);
    }

    #[test]
    fn test_get_database_url() {
        let config = Config::default();

        // Test with explicit database URL
        let cmd = SpawnCommand::List {
            database_url: Some("postgres://test".to_string()),
            limit: None,
            recent: false,
            queue: None,
        };
        let result = cmd.get_database_url(&config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "postgres://test");

        // Test without database URL (should fail with default config)
        let cmd = SpawnCommand::List {
            database_url: None,
            limit: None,
            recent: false,
            queue: None,
        };
        let result = cmd.get_database_url(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_spawn_command_parsing() {
        #[derive(Parser)]
        struct TestApp {
            #[command(subcommand)]
            command: SpawnCommand,
        }

        // Test list command parsing
        let app = TestApp::try_parse_from([
            "test",
            "list",
            "--limit",
            "50",
            "--recent",
            "--queue",
            "test-queue",
        ]);
        assert!(app.is_ok());
        match app.unwrap().command {
            SpawnCommand::List {
                limit,
                recent,
                queue,
                ..
            } => {
                assert_eq!(limit, Some(50));
                assert!(recent);
                assert_eq!(queue, Some("test-queue".to_string()));
            }
            _ => panic!("Wrong command variant"),
        }

        // Test tree command parsing
        let app = TestApp::try_parse_from([
            "test",
            "tree",
            "550e8400-e29b-41d4-a716-446655440000",
            "--full",
            "--format",
            "json",
        ]);
        assert!(app.is_ok());
        match app.unwrap().command {
            SpawnCommand::Tree {
                job_id,
                full,
                format,
                ..
            } => {
                assert_eq!(job_id, "550e8400-e29b-41d4-a716-446655440000");
                assert!(full);
                assert_eq!(format, Some("json".to_string()));
            }
            _ => panic!("Wrong command variant"),
        }

        // Test stats command parsing
        let app = TestApp::try_parse_from([
            "test",
            "stats",
            "--hours",
            "48",
            "--detailed",
            "--queue",
            "stats-queue",
        ]);
        assert!(app.is_ok());
        match app.unwrap().command {
            SpawnCommand::Stats {
                hours,
                detailed,
                queue,
                ..
            } => {
                assert_eq!(hours, Some(48));
                assert!(detailed);
                assert_eq!(queue, Some("stats-queue".to_string()));
            }
            _ => panic!("Wrong command variant"),
        }

        // Test lineage command parsing
        let app = TestApp::try_parse_from([
            "test",
            "lineage",
            "test-job-id",
            "--ancestors",
            "--descendants",
            "--depth",
            "5",
        ]);
        assert!(app.is_ok());
        match app.unwrap().command {
            SpawnCommand::Lineage {
                job_id,
                ancestors,
                descendants,
                depth,
                ..
            } => {
                assert_eq!(job_id, "test-job-id");
                assert!(ancestors);
                assert!(descendants);
                assert_eq!(depth, Some(5));
            }
            _ => panic!("Wrong command variant"),
        }

        // Test pending command parsing
        let app = TestApp::try_parse_from([
            "test",
            "pending",
            "--queue",
            "pending-queue",
            "--show-config",
        ]);
        assert!(app.is_ok());
        match app.unwrap().command {
            SpawnCommand::Pending {
                queue, show_config, ..
            } => {
                assert_eq!(queue, Some("pending-queue".to_string()));
                assert!(show_config);
            }
            _ => panic!("Wrong command variant"),
        }

        // Test monitor command parsing
        let app = TestApp::try_parse_from([
            "test",
            "monitor",
            "--interval",
            "10",
            "--queue",
            "monitor-queue",
        ]);
        assert!(app.is_ok());
        match app.unwrap().command {
            SpawnCommand::Monitor {
                interval, queue, ..
            } => {
                assert_eq!(interval, Some(10));
                assert_eq!(queue, Some("monitor-queue".to_string()));
            }
            _ => panic!("Wrong command variant"),
        }
    }

    #[test]
    fn test_build_spawn_edges() {
        let cmd = SpawnCommand::List {
            database_url: None,
            limit: None,
            recent: false,
            queue: None,
        };

        let nodes = vec![
            SpawnNode {
                id: "parent-1".to_string(),
                queue_name: "queue1".to_string(),
                status: "Completed".to_string(),
                depends_on: vec![],
                spawn_config: Some(serde_json::json!({"spawn": true})),
                created_at: "2024-01-01T00:00:00Z".to_string(),
                workflow_id: None,
                workflow_name: None,
            },
            SpawnNode {
                id: "child-1".to_string(),
                queue_name: "queue2".to_string(),
                status: "Running".to_string(),
                depends_on: vec!["parent-1".to_string()],
                spawn_config: None,
                created_at: "2024-01-01T00:01:00Z".to_string(),
                workflow_id: None,
                workflow_name: None,
            },
            SpawnNode {
                id: "child-2".to_string(),
                queue_name: "queue2".to_string(),
                status: "Pending".to_string(),
                depends_on: vec!["parent-1".to_string()],
                spawn_config: None,
                created_at: "2024-01-01T00:02:00Z".to_string(),
                workflow_id: None,
                workflow_name: None,
            },
        ];

        let edges = cmd.build_spawn_edges(&nodes);
        assert_eq!(edges.len(), 2);

        // Check first edge
        assert_eq!(edges[0]["from"], "parent-1");
        assert_eq!(edges[0]["to"], "child-1");
        assert_eq!(edges[0]["type"], "spawn");

        // Check second edge
        assert_eq!(edges[1]["from"], "parent-1");
        assert_eq!(edges[1]["to"], "child-2");
        assert_eq!(edges[1]["type"], "spawn");
    }

    #[test]
    fn test_spawn_node_with_no_dependencies() {
        let node = SpawnNode {
            id: "standalone-job".to_string(),
            queue_name: "solo-queue".to_string(),
            status: "Completed".to_string(),
            depends_on: vec![],
            spawn_config: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            workflow_id: None,
            workflow_name: None,
        };

        assert!(node.depends_on.is_empty());
        assert!(node.spawn_config.is_none());
        assert!(node.workflow_id.is_none());
    }

    #[test]
    fn test_spawn_node_with_workflow() {
        let node = SpawnNode {
            id: "workflow-job".to_string(),
            queue_name: "workflow-queue".to_string(),
            status: "Running".to_string(),
            depends_on: vec!["parent-1".to_string(), "parent-2".to_string()],
            spawn_config: Some(serde_json::json!({
                "operation_id": "op-123",
                "batch_size": 100
            })),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            workflow_id: Some("workflow-123".to_string()),
            workflow_name: Some("data-processing-workflow".to_string()),
        };

        assert_eq!(node.depends_on.len(), 2);
        assert!(node.spawn_config.is_some());
        assert_eq!(node.workflow_id, Some("workflow-123".to_string()));
        assert_eq!(
            node.workflow_name,
            Some("data-processing-workflow".to_string())
        );
    }

    #[test]
    fn test_spawn_operation_without_config() {
        let op = SpawnOperation {
            parent_job_id: "parent-456".to_string(),
            spawned_jobs: vec![],
            spawned_at: "2024-01-01T12:00:00Z".to_string(),
            operation_id: None,
            config: None,
        };

        assert!(op.spawned_jobs.is_empty());
        assert!(op.operation_id.is_none());
        assert!(op.config.is_none());
    }

    #[test]
    fn test_format_options() {
        // Test that all supported formats are recognized
        let formats = vec!["text", "json", "mermaid"];
        for format in formats {
            let cmd = SpawnCommand::Tree {
                database_url: None,
                job_id: "test".to_string(),
                full: false,
                children_only: false,
                format: Some(format.to_string()),
            };

            match cmd {
                SpawnCommand::Tree {
                    format: Some(f), ..
                } => {
                    assert!(["text", "json", "mermaid"].contains(&f.as_str()));
                }
                _ => panic!("Wrong command variant"),
            }
        }
    }

    #[test]
    fn test_spawn_config_parsing() {
        // Test various spawn config formats
        let configs = vec![
            serde_json::json!({"operation_id": "op-1"}),
            serde_json::json!({"max_spawn_count": 100, "batch_size": 10}),
            serde_json::json!({"spawn_after_seconds": 60}),
            serde_json::json!({}), // Empty config
        ];

        for config in configs {
            let node = SpawnNode {
                id: "test".to_string(),
                queue_name: "test".to_string(),
                status: "Pending".to_string(),
                depends_on: vec![],
                spawn_config: Some(config.clone()),
                created_at: "2024-01-01T00:00:00Z".to_string(),
                workflow_id: None,
                workflow_name: None,
            };

            assert!(node.spawn_config.is_some());
            assert_eq!(node.spawn_config.unwrap(), config);
        }
    }

    #[test]
    fn test_job_status_variants() {
        let statuses = vec![
            "Pending",
            "Running",
            "Completed",
            "Failed",
            "Retrying",
            "Dead",
            "TimedOut",
        ];

        for status in statuses {
            let node = SpawnNode {
                id: "test".to_string(),
                queue_name: "test".to_string(),
                status: status.to_string(),
                depends_on: vec![],
                spawn_config: None,
                created_at: "2024-01-01T00:00:00Z".to_string(),
                workflow_id: None,
                workflow_name: None,
            };

            assert!(!node.status.is_empty());
        }
    }

    #[test]
    fn test_command_defaults() {
        // Test that commands have sensible defaults
        let list_cmd = SpawnCommand::List {
            database_url: None,
            limit: None,
            recent: false,
            queue: None,
        };

        match list_cmd {
            SpawnCommand::List {
                limit,
                recent,
                queue,
                ..
            } => {
                assert!(limit.is_none());
                assert!(!recent);
                assert!(queue.is_none());
            }
            _ => panic!("Wrong variant"),
        }

        let monitor_cmd = SpawnCommand::Monitor {
            database_url: None,
            interval: None,
            queue: None,
        };

        match monitor_cmd {
            SpawnCommand::Monitor { interval, .. } => {
                assert!(interval.is_none());
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_complex_spawn_tree() {
        // Test a more complex spawn tree structure
        let nodes = vec![
            SpawnNode {
                id: "root".to_string(),
                queue_name: "root-queue".to_string(),
                status: "Completed".to_string(),
                depends_on: vec![],
                spawn_config: Some(serde_json::json!({"spawn": true})),
                created_at: "2024-01-01T00:00:00Z".to_string(),
                workflow_id: Some("wf-1".to_string()),
                workflow_name: Some("Main Workflow".to_string()),
            },
            SpawnNode {
                id: "level1-a".to_string(),
                queue_name: "processing".to_string(),
                status: "Completed".to_string(),
                depends_on: vec!["root".to_string()],
                spawn_config: Some(serde_json::json!({"spawn": true})),
                created_at: "2024-01-01T00:01:00Z".to_string(),
                workflow_id: Some("wf-1".to_string()),
                workflow_name: Some("Main Workflow".to_string()),
            },
            SpawnNode {
                id: "level1-b".to_string(),
                queue_name: "processing".to_string(),
                status: "Running".to_string(),
                depends_on: vec!["root".to_string()],
                spawn_config: None,
                created_at: "2024-01-01T00:02:00Z".to_string(),
                workflow_id: Some("wf-1".to_string()),
                workflow_name: Some("Main Workflow".to_string()),
            },
            SpawnNode {
                id: "level2-a".to_string(),
                queue_name: "finalize".to_string(),
                status: "Pending".to_string(),
                depends_on: vec!["level1-a".to_string()],
                spawn_config: None,
                created_at: "2024-01-01T00:03:00Z".to_string(),
                workflow_id: Some("wf-1".to_string()),
                workflow_name: Some("Main Workflow".to_string()),
            },
        ];

        let cmd = SpawnCommand::List {
            database_url: None,
            limit: None,
            recent: false,
            queue: None,
        };

        let edges = cmd.build_spawn_edges(&nodes);
        assert_eq!(edges.len(), 3); // root->level1-a, root->level1-b, level1-a->level2-a
    }

    use crate::utils::sql::fetch_i64;
    use crate::utils::test_support::*;

    #[test]
    fn test_spawn_list_query_binds_window_queue_and_limit() {
        let (sql, binds) = build_spawn_list_query(Backend::Postgres, Some(HOSTILE_QUEUE), true, 20);
        assert!(sql.contains(
            "parent.created_at > NOW() - make_interval(hours => $1::int) AND parent.queue_name = $2"
        ));
        assert!(sql.ends_with("ORDER BY parent.created_at DESC LIMIT $3"));
        assert!(sql.contains("parent.payload ? '_spawn_config'"));
        assert_eq!(
            binds,
            vec![
                Bind::Int(1),
                Bind::Text(HOSTILE_QUEUE.into()),
                Bind::Int(20)
            ]
        );
        assert!(!sql.contains("DROP"));

        let (sql, binds) = build_spawn_list_query(Backend::MySql, Some(HOSTILE_QUEUE), false, 5);
        assert!(sql.contains("AND parent.queue_name = ? GROUP BY"));
        assert!(sql.ends_with("LIMIT ?"));
        assert!(sql.contains("JSON_EXTRACT(parent.payload, '$._spawn_config') IS NOT NULL"));
        assert_eq!(binds, vec![Bind::Text(HOSTILE_QUEUE.into()), Bind::Int(5)]);
    }

    #[test]
    fn test_spawn_stats_queries_bind_window_then_queue() {
        for (backend, since) in [
            (
                Backend::Postgres,
                "parent.created_at > NOW() - make_interval(hours => $1::int) AND parent.queue_name = $2",
            ),
            (
                Backend::MySql,
                "parent.created_at > DATE_SUB(NOW(), INTERVAL ? HOUR) AND parent.queue_name = ?",
            ),
        ] {
            for (sql, binds) in [
                build_spawn_stats_total_query(backend, 24, Some(HOSTILE_QUEUE)),
                build_spawn_stats_breakdown_query(backend, 24, Some(HOSTILE_QUEUE)),
            ] {
                assert!(sql.contains(since), "{sql}");
                assert_eq!(binds, vec![Bind::Int(24), Bind::Text(HOSTILE_QUEUE.into())]);
            }
        }
        let (sql, binds) = build_spawn_stats_total_query(Backend::MySql, 24, None);
        assert!(!sql.contains("queue_name = ?"));
        assert_eq!(binds, vec![Bind::Int(24)]);
    }

    #[test]
    fn test_pending_spawns_query_binds_queue_name() {
        let (sql, binds) = build_pending_spawns_query(Backend::Postgres, Some(HOSTILE_QUEUE));
        assert!(sql.contains("status IN ('Running', 'Pending') AND queue_name = $1 ORDER BY"));
        assert_eq!(binds, vec![Bind::Text(HOSTILE_QUEUE.into())]);
        let (sql, binds) = build_pending_spawns_query(Backend::MySql, None);
        assert!(!sql.contains('?') && binds.is_empty());
    }

    async fn spawn_roundtrip(pool: DatabasePool) {
        let hostile = hostile_queue();
        let other = format!("other_{}", uuid::Uuid::new_v4().simple());
        let config = r#"{"_spawn_config": {"operation_id": "op1"}}"#;
        let mut parent = SeedJob::new(&hostile, "Completed");
        parent.payload = config;
        let parent_id = seed(&pool, &parent).await;
        let mut child = SeedJob::new(&hostile, "Pending");
        child.depends_on = Some(&parent_id);
        seed(&pool, &child).await;
        let mut pending = SeedJob::new(&hostile, "Pending");
        pending.payload = config;
        seed(&pool, &pending).await;
        let mut elsewhere = SeedJob::new(&other, "Completed");
        elsewhere.payload = config;
        seed(&pool, &elsewhere).await;

        let backend = pool.backend();
        let (sql, binds) = build_spawn_list_query(backend, Some(&hostile), true, 10);
        let queues = column_strings(&pool, &sql, &binds, "queue_name").await;
        assert_eq!(queues, vec![hostile.clone()]);
        assert_eq!(
            fetch_i64(&pool, &sql, &binds, "spawned_count")
                .await
                .unwrap(),
            1
        );

        let (sql, binds) = build_spawn_stats_total_query(backend, 1, Some(&hostile));
        assert_eq!(
            fetch_i64(&pool, &sql, &binds, "total_spawn_ops")
                .await
                .unwrap(),
            2
        );
        let (sql, binds) = build_spawn_stats_breakdown_query(backend, 1, Some(&hostile));
        assert_eq!(
            column_strings(&pool, &sql, &binds, "queue_name").await,
            vec![hostile.clone()]
        );

        let (sql, binds) = build_pending_spawns_query(backend, Some(&hostile));
        assert_eq!(
            column_strings(&pool, &sql, &binds, "queue_name").await,
            vec![hostile.clone()]
        );

        let cmd = SpawnCommand::Pending {
            database_url: None,
            queue: None,
            show_config: true,
        };
        cmd.list_spawn_operations(pool.clone(), Some(10), true, Some(&hostile))
            .await
            .unwrap();
        cmd.show_spawn_stats(pool.clone(), Some(&hostile), Some(1), true)
            .await
            .unwrap();
        cmd.show_pending_spawns(pool.clone(), Some(&hostile), true)
            .await
            .unwrap();

        assert!(table_exists(&pool).await);
        cleanup(&pool, &[&hostile, &other]).await;
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL (PostgreSQL)"]
    async fn test_spawn_queue_filter_is_injection_safe_postgres() {
        spawn_roundtrip(pg_pool().await).await;
    }

    #[tokio::test]
    #[ignore = "requires MYSQL_DATABASE_URL"]
    async fn test_spawn_queue_filter_is_injection_safe_mysql() {
        spawn_roundtrip(mysql_pool().await).await;
    }
}
