use anyhow::{Context, Result};
use clap::Subcommand;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
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
                let rows =
                    fetch_spawn_list(&pool, queue.as_deref(), *recent, limit.unwrap_or(20)).await?;
                println!("{}", render_spawn_list(&rows));
                Ok(())
            }
            SpawnCommand::Tree {
                job_id,
                full,
                children_only,
                format,
                ..
            } => {
                let format = format.as_deref().unwrap_or("text");
                if !["text", "json", "mermaid"].contains(&format) {
                    anyhow::bail!("Unsupported format: {}. Use: text, json, mermaid", format);
                }
                let nodes = collect_tree(&pool, job_id, *full, *children_only).await?;
                let target = Uuid::parse_str(job_id)?.to_string();
                println!("{}", render_spawn_tree(&nodes, &target, format)?);
                Ok(())
            }
            SpawnCommand::Stats {
                queue,
                hours,
                detailed,
                ..
            } => {
                let hours = hours.unwrap_or(24);
                let stats = fetch_spawn_stats(&pool, hours, queue.as_deref(), *detailed).await?;
                println!("{}", render_spawn_stats(&stats, hours));
                Ok(())
            }
            SpawnCommand::Lineage {
                job_id,
                ancestors,
                descendants,
                depth,
                ..
            } => {
                let lineage =
                    fetch_lineage(&pool, job_id, *ancestors, *descendants, depth.unwrap_or(10))
                        .await?;
                println!("{}", render_lineage(&lineage, job_id));
                Ok(())
            }
            SpawnCommand::Pending {
                queue, show_config, ..
            } => {
                let rows = fetch_pending_spawns(&pool, queue.as_deref()).await?;
                println!("{}", render_pending_spawns(&rows, *show_config));
                Ok(())
            }
            SpawnCommand::Monitor {
                interval, queue, ..
            } => {
                let interval = std::time::Duration::from_secs(u64::from(interval.unwrap_or(5)));
                monitor_spawn_operations(pool, interval, queue.as_deref(), async {
                    let _ = tokio::signal::ctrl_c().await;
                })
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
}

/// The first `max` characters of `text` (never splits a multi-byte character).
fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// The first eight characters of an id, as shown in listings.
fn short_id(id: &str) -> String {
    truncate_chars(id, 8)
}

/// One row of `spawn list`.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnListRow {
    pub parent_id: String,
    pub queue_name: String,
    pub spawned_count: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub operation_id: String,
    pub workflow: String,
}

/// The `operation_id` of a spawn config, or `none`.
fn operation_id_of(config: Option<&Value>) -> String {
    config
        .and_then(|c| c.get("operation_id"))
        .and_then(|id| id.as_str())
        .unwrap_or("none")
        .to_string()
}

/// Parents with a spawn config and the number of children each has spawned.
pub async fn fetch_spawn_list(
    pool: &DatabasePool,
    queue: Option<&str>,
    recent: bool,
    limit: u32,
) -> Result<Vec<SpawnListRow>> {
    use sqlx::Row;
    let (query, binds) = build_spawn_list_query(pool.backend(), queue, recent, limit);
    let mut out = Vec::new();
    match pool {
        DatabasePool::Postgres(pg_pool) => {
            for row in bind_pg(sqlx::query(&query), &binds)
                .fetch_all(pg_pool)
                .await?
            {
                let config: Option<Value> = row.try_get("spawn_config")?;
                out.push(SpawnListRow {
                    parent_id: row.try_get::<Uuid, _>("parent_id")?.to_string(),
                    queue_name: row.try_get("queue_name")?,
                    spawned_count: row.try_get("spawned_count")?,
                    created_at: row.try_get("created_at")?,
                    operation_id: operation_id_of(config.as_ref()),
                    workflow: row
                        .try_get::<Option<String>, _>("workflow_name")?
                        .unwrap_or_else(|| "none".to_string()),
                });
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            for row in bind_mysql(sqlx::query(&query), &binds)
                .fetch_all(mysql_pool)
                .await?
            {
                let operation_id = match mysql_spawn_config_text(&row)? {
                    Some(text) => match serde_json::from_str::<Value>(&text) {
                        Ok(config) => operation_id_of(Some(&config)),
                        Err(_) => "invalid-config".to_string(),
                    },
                    None => "none".to_string(),
                };
                out.push(SpawnListRow {
                    parent_id: row.try_get("parent_id")?,
                    queue_name: row.try_get("queue_name")?,
                    spawned_count: row.try_get("spawned_count")?,
                    created_at: row.try_get("created_at")?,
                    operation_id,
                    workflow: row
                        .try_get::<Option<String>, _>("workflow_name")?
                        .unwrap_or_else(|| "none".to_string()),
                });
            }
        }
    }
    Ok(out)
}

pub fn render_spawn_list(rows: &[SpawnListRow]) -> String {
    let mut out = format!("📊 Spawn Operations\n{}", "=".repeat(80));
    if rows.is_empty() {
        out.push_str("\nNo spawn operations found.");
        return out;
    }
    out.push_str(&format!(
        "\n{:<8} {:<15} {:<12} {:<20} {:<10} {:<15}\n{}",
        "Parent",
        "Queue",
        "Children",
        "Spawned At",
        "Operation",
        "Workflow",
        "-".repeat(80)
    ));
    for row in rows {
        out.push_str(&format!(
            "\n{:<8} {:<15} {:<12} {:<20} {:<10} {:<15}",
            short_id(&row.parent_id),
            truncate_chars(&row.queue_name, 15),
            row.spawned_count,
            row.created_at.format("%m-%d %H:%M:%S"),
            truncate_chars(&row.operation_id, 10),
            truncate_chars(&row.workflow, 15)
        ));
    }
    out
}

/// Per-queue line of `spawn stats --detailed`.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueSpawnStats {
    pub queue_name: String,
    pub operations: i64,
    pub avg_children: Option<f64>,
}

/// Totals shown by `spawn stats`.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnStats {
    pub total: i64,
    pub avg_children: Option<f64>,
    pub max_children: Option<i64>,
    /// Present only when the breakdown was asked for.
    pub breakdown: Option<Vec<QueueSpawnStats>>,
}

pub async fn fetch_spawn_stats(
    pool: &DatabasePool,
    hours: u32,
    queue: Option<&str>,
    detailed: bool,
) -> Result<SpawnStats> {
    use sqlx::Row;
    let backend = pool.backend();
    let (total_query, total_binds) = build_spawn_stats_total_query(backend, hours, queue);
    let (breakdown_query, breakdown_binds) =
        build_spawn_stats_breakdown_query(backend, hours, queue);

    macro_rules! stats {
        ($pool:expr, $bind:ident) => {{
            let row = $bind(sqlx::query(&total_query), &total_binds)
                .fetch_one($pool)
                .await?;
            let mut stats = SpawnStats {
                total: row.try_get("total_spawn_ops")?,
                avg_children: row.try_get("avg_children")?,
                max_children: row.try_get("max_children")?,
                breakdown: None,
            };
            if detailed {
                let mut lines = Vec::new();
                for row in $bind(sqlx::query(&breakdown_query), &breakdown_binds)
                    .fetch_all($pool)
                    .await?
                {
                    lines.push(QueueSpawnStats {
                        queue_name: row.try_get("queue_name")?,
                        operations: row.try_get("spawn_count")?,
                        avg_children: row.try_get("avg_children")?,
                    });
                }
                stats.breakdown = Some(lines);
            }
            stats
        }};
    }
    Ok(match pool {
        DatabasePool::Postgres(p) => stats!(p, bind_pg),
        DatabasePool::MySQL(p) => stats!(p, bind_mysql),
    })
}

pub fn render_spawn_stats(stats: &SpawnStats, hours: u32) -> String {
    let mut out = format!(
        "📈 Spawn Statistics (Last {} hours)\n{}\nTotal Spawn Operations: {}",
        hours,
        "=".repeat(60),
        stats.total
    );
    if let Some(avg) = stats.avg_children {
        out.push_str(&format!("\nAverage Children per Spawn: {:.1}", avg));
    }
    if let Some(max) = stats.max_children {
        out.push_str(&format!("\nMaximum Children in Single Spawn: {}", max));
    }
    if let Some(breakdown) = &stats.breakdown {
        out.push_str(&format!(
            "\n\n📋 Breakdown by Queue:\n{:<20} {:<12} {:<15}\n{}",
            "Queue",
            "Operations",
            "Avg Children",
            "-".repeat(47)
        ));
        for line in breakdown {
            out.push_str(&format!(
                "\n{:<20} {:<12} {:<15.1}",
                truncate_chars(&line.queue_name, 20),
                line.operations,
                line.avg_children.unwrap_or(0.0)
            ));
        }
    }
    out
}

/// A job that carries a spawn config and has not finished.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingSpawn {
    pub id: String,
    pub queue_name: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub config: Option<Value>,
}

pub async fn fetch_pending_spawns(
    pool: &DatabasePool,
    queue: Option<&str>,
) -> Result<Vec<PendingSpawn>> {
    use sqlx::Row;
    let (query, binds) = build_pending_spawns_query(pool.backend(), queue);
    let mut out = Vec::new();
    match pool {
        DatabasePool::Postgres(pg_pool) => {
            for row in bind_pg(sqlx::query(&query), &binds)
                .fetch_all(pg_pool)
                .await?
            {
                out.push(PendingSpawn {
                    id: row.try_get::<Uuid, _>("id")?.to_string(),
                    queue_name: row.try_get("queue_name")?,
                    status: row.try_get("status")?,
                    created_at: row.try_get("created_at")?,
                    config: row.try_get("spawn_config")?,
                });
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            for row in bind_mysql(sqlx::query(&query), &binds)
                .fetch_all(mysql_pool)
                .await?
            {
                let config = mysql_spawn_config_text(&row)?
                    .map(|text| serde_json::from_str(&text).unwrap_or(Value::String(text)));
                out.push(PendingSpawn {
                    id: row.try_get("id")?,
                    queue_name: row.try_get("queue_name")?,
                    status: row.try_get("status")?,
                    created_at: row.try_get("created_at")?,
                    config,
                });
            }
        }
    }
    Ok(out)
}

pub fn render_pending_spawns(rows: &[PendingSpawn], show_config: bool) -> String {
    let mut out = format!("⏳ Jobs with Pending Spawn Operations\n{}", "=".repeat(70));
    if rows.is_empty() {
        out.push_str("\nNo jobs with pending spawn operations found.");
        return out;
    }
    for row in rows {
        out.push_str(&format!(
            "\n📋 Job: {} | Queue: {} | Status: {} | Created: {}",
            short_id(&row.id),
            row.queue_name,
            row.status,
            row.created_at.format("%m-%d %H:%M:%S")
        ));
        if show_config {
            if let Some(config) = &row.config {
                out.push_str(&format!(
                    "\n   Spawn Config: {}",
                    serde_json::to_string_pretty(config)
                        .unwrap_or_else(|_| "Invalid JSON".to_string())
                ));
            }
            out.push('\n');
        }
    }
    out
}

/// Re-run `spawn list` every `interval` until `shutdown` completes.
pub async fn monitor_spawn_operations(
    pool: DatabasePool,
    interval: std::time::Duration,
    queue: Option<&str>,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    if interval.is_zero() {
        anyhow::bail!("refresh interval must be at least 1 second");
    }
    println!("🔄 Monitoring Spawn Operations (Press Ctrl+C to stop)");
    println!("Refresh interval: {:?}", interval);
    if let Some(queue) = queue {
        println!("Queue filter: {}", queue);
    }
    println!("{}", "=".repeat(80));

    let mut ticker = tokio::time::interval(interval);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                match fetch_spawn_list(&pool, queue, false, 20).await {
                    Ok(rows) => println!("{}", render_spawn_list(&rows)),
                    Err(e) => println!("❌ Error updating spawn list: {}", e),
                }
            }
            _ = &mut shutdown => {
                println!("\n👋 Monitor stopped");
                return Ok(());
            }
        }
    }
}

/// What `spawn lineage` found.
#[derive(Debug, Clone)]
pub struct Lineage {
    pub target: SpawnNode,
    /// Oldest ancestor first. `None` when ancestors were not requested.
    pub ancestors: Option<Vec<SpawnNode>>,
    /// `None` when descendants were not requested.
    pub descendants: Option<Vec<SpawnNode>>,
}

pub async fn fetch_lineage(
    pool: &DatabasePool,
    job_id: &str,
    ancestors: bool,
    descendants: bool,
    max_depth: u32,
) -> Result<Lineage> {
    let job_uuid = Uuid::parse_str(job_id)?;
    let target = get_spawn_node(pool, &job_uuid)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Job not found: {}", job_id))?;

    // Neither flag means both directions.
    let want_ancestors = ancestors || !descendants;
    let want_descendants = descendants || !ancestors;
    Ok(Lineage {
        ancestors: if want_ancestors {
            Some(collect_ancestors(pool, &target, max_depth).await?)
        } else {
            None
        },
        descendants: if want_descendants {
            Some(collect_descendants(pool, &target, max_depth).await?)
        } else {
            None
        },
        target,
    })
}

pub fn render_lineage(lineage: &Lineage, job_id: &str) -> String {
    let mut out = format!(
        "🔗 Spawn Lineage for {}\nQueue: {} | Status: {}\n{}",
        job_id,
        lineage.target.queue_name,
        lineage.target.status,
        "=".repeat(60)
    );
    if let Some(ancestors) = &lineage.ancestors {
        out.push_str("\n\n⬆️  Ancestor Chain:");
        if ancestors.is_empty() {
            out.push_str("\n  No spawn ancestors found (this is a root job)");
        }
        for (depth, ancestor) in ancestors.iter().enumerate() {
            out.push_str(&format!(
                "\n{}└─ [{}] {} ({})",
                "  ".repeat(depth + 1),
                short_id(&ancestor.id),
                ancestor.queue_name,
                ancestor.status
            ));
        }
    }
    if let Some(descendants) = &lineage.descendants {
        out.push_str("\n\n⬇️  Descendant Chain:");
        if descendants.is_empty() {
            out.push_str("\n  No spawn descendants found (this job hasn't spawned children)");
        }
        for (i, descendant) in descendants.iter().enumerate() {
            let marker = if i == descendants.len() - 1 {
                "└─"
            } else {
                "├─"
            };
            out.push_str(&format!(
                "\n  {}📝 [{}] {} ({})",
                marker,
                short_id(&descendant.id),
                descendant.queue_name,
                descendant.status
            ));
        }
    }
    out
}

/// The nodes `spawn tree` shows for `job_id`.
pub async fn collect_tree(
    pool: &DatabasePool,
    job_id: &str,
    full: bool,
    children_only: bool,
) -> Result<Vec<SpawnNode>> {
    let job_uuid = Uuid::parse_str(job_id)?;
    let target = get_spawn_node(pool, &job_uuid)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Job not found: {}", job_id))?;
    if full || !children_only {
        collect_full_spawn_tree(pool, &target).await
    } else {
        // The job itself plus its direct children.
        let mut nodes = vec![target.clone()];
        nodes.extend(collect_spawn_children(pool, &target).await?);
        Ok(nodes)
    }
}

pub fn render_spawn_tree(nodes: &[SpawnNode], target_id: &str, format: &str) -> Result<String> {
    match format {
        "text" => Ok(render_spawn_tree_text(nodes, target_id)),
        "json" => render_spawn_tree_json(nodes),
        "mermaid" => Ok(render_spawn_tree_mermaid(nodes, target_id)),
        other => anyhow::bail!("Unsupported format: {}. Use: text, json, mermaid", other),
    }
}

async fn get_spawn_node(pool: &DatabasePool, job_id: &Uuid) -> Result<Option<SpawnNode>> {
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
                Ok(Some(postgres_row_to_spawn_node(&row)?))
            } else {
                Ok(None)
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            // MySQL has no `->` with a bare key: the path must be a JSON path.
            let mysql_query = r#"
                SELECT id, queue_name, status, depends_on,
                       JSON_EXTRACT(payload, '$._spawn_config') as spawn_config,
                       created_at, workflow_id, workflow_name
                FROM hammerwork_jobs
                WHERE id = ?
            "#;
            if let Some(row) = sqlx::query(mysql_query)
                .bind(job_id.to_string())
                .fetch_optional(mysql_pool)
                .await?
            {
                Ok(Some(mysql_row_to_spawn_node(&row)?))
            } else {
                Ok(None)
            }
        }
    }
}

async fn collect_spawn_children(pool: &DatabasePool, parent: &SpawnNode) -> Result<Vec<SpawnNode>> {
    let mut children = Vec::new();

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let query = r#"
                SELECT id, queue_name, status, depends_on,
                       payload->'_spawn_config' as spawn_config,
                       created_at, workflow_id, workflow_name
                FROM hammerwork_jobs
                WHERE depends_on @> ARRAY[$1::uuid]
                ORDER BY created_at
            "#;
            // depends_on is a UUID[] column in PostgreSQL
            let parent_id = Uuid::parse_str(&parent.id)?;
            let rows = sqlx::query(query)
                .bind(parent_id)
                .fetch_all(pg_pool)
                .await?;

            for row in rows {
                children.push(postgres_row_to_spawn_node(&row)?);
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

            let parent_json = serde_json::to_string(&parent.id)?;
            let rows = sqlx::query(mysql_query)
                .bind(&parent_json)
                .fetch_all(mysql_pool)
                .await?;

            for row in rows {
                children.push(mysql_row_to_spawn_node(&row)?);
            }
        }
    }

    Ok(children)
}

async fn collect_full_spawn_tree(
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
            let children = collect_spawn_children(pool, &job).await?;
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
                if let Some(parent) = get_spawn_node(pool, &parent_uuid).await?
                    && !all_nodes.contains_key(&parent.id)
                {
                    all_nodes.insert(parent.id.clone(), parent.clone());
                    to_visit.push_back(parent.id.clone());
                }
            }
        }
    }

    // Oldest first, so output does not depend on hash order.
    let mut nodes: Vec<SpawnNode> = all_nodes.into_values().collect();
    nodes.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(nodes)
}

async fn collect_ancestors(
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
            if let Some(parent) = get_spawn_node(pool, &parent_uuid).await?
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

        let children = collect_spawn_children(pool, &current_job).await?;
        for child in children {
            descendants.push(child.clone());
            to_visit.push_back((child, depth + 1));
        }
    }

    Ok(descendants)
}

fn postgres_row_to_spawn_node(row: &sqlx::postgres::PgRow) -> Result<SpawnNode> {
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

fn mysql_row_to_spawn_node(row: &sqlx::mysql::MySqlRow) -> Result<SpawnNode> {
    use sqlx::Row;

    let id: String = row.try_get("id")?;
    let depends_on = parse_json_array(row.try_get("depends_on")?);

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

/// The string elements of a JSON array column (anything else is an empty list).
fn parse_json_array(json_value: Option<Value>) -> Vec<String> {
    match json_value {
        Some(Value::Array(arr)) => arr
            .into_iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

fn render_spawn_tree_text(nodes: &[SpawnNode], target_id: &str) -> String {
    let mut out = format!("\n🌳 Spawn Tree\n{}", "=".repeat(60));

    let node_map: HashMap<String, &SpawnNode> = nodes.iter().map(|n| (n.id.clone(), n)).collect();

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

    roots.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut visited = HashSet::new();
    for root in roots {
        if !visited.contains(&root.id) {
            render_spawn_node_tree(root, &node_map, &mut visited, 0, target_id, &mut out);
        }
    }
    out
}

fn render_spawn_node_tree(
    node: &SpawnNode,
    node_map: &HashMap<String, &SpawnNode>,
    visited: &mut HashSet<String>,
    depth: usize,
    target_id: &str,
    out: &mut String,
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

    out.push_str(&format!(
        "\n{}{}{} [{}] {} ({}){}",
        indent,
        marker,
        spawn_indicator,
        short_id(&node.id),
        node.queue_name,
        node.status,
        highlight
    ));

    // Children in creation order
    let mut children: Vec<&SpawnNode> = node_map
        .values()
        .filter(|child| child.depends_on.contains(&node.id) && child.id != node.id)
        .copied()
        .collect();
    children.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    for child in children {
        render_spawn_node_tree(child, node_map, visited, depth + 1, target_id, out);
    }
}

fn render_spawn_tree_json(nodes: &[SpawnNode]) -> Result<String> {
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
            "edges": build_spawn_edges(nodes)
        }
    });

    Ok(format!(
        "\n📄 Spawn Tree (JSON)\n{}\n{}",
        "=".repeat(60),
        serde_json::to_string_pretty(&tree_data)?
    ))
}

fn render_spawn_tree_mermaid(nodes: &[SpawnNode], target_id: &str) -> String {
    let mut lines = vec![
        "\n🧜‍♀️ Spawn Tree (Mermaid)".to_string(),
        "=".repeat(60),
        "graph TD".to_string(),
        "    subgraph \"🚀 Spawn Tree\"".to_string(),
    ];

    // Define nodes
    for node in nodes {
        let short = short_id(&node.id);
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

        lines.push(format!(
            "        {}[\"{}{}<br/>{}<br/>{}\"]{}",
            short, spawn_indicator, target_indicator, short, node.status, status_class
        ));
    }

    // Define spawn relationships
    let node_map: HashMap<String, &SpawnNode> = nodes.iter().map(|n| (n.id.clone(), n)).collect();

    for node in nodes {
        for dep_id in &node.depends_on {
            if let Some(parent) = node_map.get(dep_id)
                && parent.spawn_config.is_some()
            {
                lines.push(format!(
                    "        {} -->|spawns| {}",
                    short_id(dep_id),
                    short_id(&node.id)
                ));
            }
        }
    }

    lines.push("    end".to_string());
    lines.push(String::new());

    // CSS classes for styling
    lines.push("    classDef completed fill:#d4edda,stroke:#155724".to_string());
    lines.push("    classDef failed fill:#f8d7da,stroke:#721c24".to_string());
    lines.push("    classDef running fill:#cce7ff,stroke:#004085".to_string());
    lines.push("    classDef pending fill:#fff3cd,stroke:#856404".to_string());
    lines.push("    classDef default fill:#e2e3e5,stroke:#383d41".to_string());
    lines.join("\n")
}

fn build_spawn_edges(nodes: &[SpawnNode]) -> Vec<Value> {
    let node_map: HashMap<String, &SpawnNode> = nodes.iter().map(|n| (n.id.clone(), n)).collect();

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::sql::fetch_i64;
    use crate::utils::test_support::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: SpawnCommand,
    }

    fn parse(args: &[&str]) -> SpawnCommand {
        let mut argv = vec!["test"];
        argv.extend_from_slice(args);
        TestCli::try_parse_from(argv).unwrap().command
    }

    #[test]
    fn parses_every_subcommand_with_its_flags() {
        match parse(&["list", "--limit", "50", "--recent", "--queue", "q"]) {
            SpawnCommand::List {
                limit,
                recent,
                queue,
                ..
            } => assert_eq!(
                (limit, recent, queue.as_deref()),
                (Some(50), true, Some("q"))
            ),
            _ => panic!("expected List"),
        }
        match parse(&[
            "tree",
            "abc",
            "--full",
            "--children-only",
            "--format",
            "mermaid",
        ]) {
            SpawnCommand::Tree {
                job_id,
                full,
                children_only,
                format,
                ..
            } => {
                assert_eq!(job_id, "abc");
                assert!(full && children_only);
                assert_eq!(format.as_deref(), Some("mermaid"));
            }
            _ => panic!("expected Tree"),
        }
        match parse(&["stats", "-Q", "q", "--hours", "6", "--detailed"]) {
            SpawnCommand::Stats {
                queue,
                hours,
                detailed,
                ..
            } => assert_eq!(
                (queue.as_deref(), hours, detailed),
                (Some("q"), Some(6), true)
            ),
            _ => panic!("expected Stats"),
        }
        match parse(&[
            "lineage",
            "abc",
            "--ancestors",
            "--descendants",
            "--depth",
            "3",
        ]) {
            SpawnCommand::Lineage {
                job_id,
                ancestors,
                descendants,
                depth,
                ..
            } => assert_eq!(
                (job_id.as_str(), ancestors, descendants, depth),
                ("abc", true, true, Some(3))
            ),
            _ => panic!("expected Lineage"),
        }
        match parse(&["pending", "-Q", "q", "--show-config"]) {
            SpawnCommand::Pending {
                queue, show_config, ..
            } => assert_eq!((queue.as_deref(), show_config), (Some("q"), true)),
            _ => panic!("expected Pending"),
        }
        match parse(&["monitor", "--interval", "2", "-Q", "q"]) {
            SpawnCommand::Monitor {
                interval, queue, ..
            } => assert_eq!((interval, queue.as_deref()), (Some(2), Some("q"))),
            _ => panic!("expected Monitor"),
        }
        assert!(
            TestCli::try_parse_from(["test", "tree"]).is_err(),
            "job id is required"
        );
    }

    #[test]
    fn database_url_comes_from_the_flag_then_the_config() {
        let config = crate::utils::test_support::config_for("postgres://config/db");
        for args in [
            &["list"][..],
            &["tree", "x"],
            &["stats"],
            &["lineage", "x"],
            &["pending"],
            &["monitor"],
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
    fn json_array_columns_keep_only_strings() {
        assert_eq!(
            parse_json_array(Some(serde_json::json!(["job1", 123, "job2", null]))),
            vec!["job1", "job2"]
        );
        assert!(parse_json_array(None).is_empty());
        assert!(parse_json_array(Some(serde_json::json!({"k": "v"}))).is_empty());
    }

    #[test]
    fn truncation_never_splits_a_character() {
        assert_eq!(truncate_chars("short", 15), "short");
        assert_eq!(truncate_chars("0123456789abcdefg", 15), "0123456789abcde");
        // Slicing these by byte index used to panic in the listings.
        assert_eq!(truncate_chars("ééééé", 3), "ééé");
        assert_eq!(truncate_chars("日本語のキュー", 3), "日本語");
        assert_eq!(short_id("abc"), "abc");
        assert_eq!(short_id("550e8400-e29b"), "550e8400");
    }

    fn node(id: &str, deps: &[&str], spawn: bool, status: &str, created: &str) -> SpawnNode {
        SpawnNode {
            id: id.to_string(),
            queue_name: "q".to_string(),
            status: status.to_string(),
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            spawn_config: spawn.then(|| serde_json::json!({"operation_id": "op"})),
            created_at: created.to_string(),
            workflow_id: None,
            workflow_name: None,
        }
    }

    fn sample_tree() -> Vec<SpawnNode> {
        vec![
            node(
                "rootrootroot",
                &[],
                true,
                "Completed",
                "2030-01-01 00:00:00",
            ),
            node(
                "child1child1",
                &["rootrootroot"],
                true,
                "Completed",
                "2030-01-01 00:01:00",
            ),
            node(
                "child2child2",
                &["rootrootroot"],
                false,
                "Running",
                "2030-01-01 00:02:00",
            ),
            node(
                "grandgrand",
                &["child1child1"],
                false,
                "Pending",
                "2030-01-01 00:03:00",
            ),
        ]
    }

    #[test]
    fn edges_connect_spawning_parents_to_their_children() {
        let edges = build_spawn_edges(&sample_tree());
        let pairs: Vec<(String, String)> = edges
            .iter()
            .map(|e| {
                (
                    e["from"].as_str().unwrap().to_string(),
                    e["to"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        // child2 has no spawn config, so grandgrand (child of child1) is the only other edge.
        assert_eq!(
            pairs,
            vec![
                ("rootrootroot".to_string(), "child1child1".to_string()),
                ("rootrootroot".to_string(), "child2child2".to_string()),
                ("child1child1".to_string(), "grandgrand".to_string()),
            ]
        );
        assert!(edges.iter().all(|e| e["type"] == "spawn"));
        assert!(build_spawn_edges(&[]).is_empty());
    }

    #[test]
    fn tree_renders_as_text_json_and_mermaid() {
        let nodes = sample_tree();
        let text = render_spawn_tree(&nodes, "child1child1", "text").unwrap();
        let lines: Vec<&str> = text.lines().filter(|l| l.contains('[')).collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("┌─🚀 [rootroot]"), "{}", lines[0]);
        assert!(
            lines[1].starts_with("  ├─🚀 [child1ch] q (Completed) ⭐"),
            "{}",
            lines[1]
        );
        assert!(lines[2].starts_with("    ├─📝 [grandgra]"), "{}", lines[2]);
        assert!(
            lines[3].starts_with("  ├─📝 [child2ch] q (Running)"),
            "{}",
            lines[3]
        );
        assert_eq!(text.matches('⭐').count(), 1);

        let json = render_spawn_tree(&nodes, "x", "json").unwrap();
        let body = &json[json.find('{').unwrap()..];
        let parsed: Value = serde_json::from_str(body).unwrap();
        assert_eq!(parsed["spawn_tree"]["nodes"].as_array().unwrap().len(), 4);
        assert_eq!(parsed["spawn_tree"]["nodes"][1]["has_spawn_config"], true);
        assert_eq!(parsed["spawn_tree"]["nodes"][2]["has_spawn_config"], false);
        assert_eq!(parsed["spawn_tree"]["edges"].as_array().unwrap().len(), 3);

        let mermaid = render_spawn_tree(&nodes, "child2child2", "mermaid").unwrap();
        assert!(mermaid.contains("graph TD"));
        assert!(
            mermaid.contains("child2ch[\"📝⭐<br/>child2ch<br/>Running\"]:::running"),
            "{mermaid}"
        );
        assert!(mermaid.contains("rootroot -->|spawns| child1ch"));
        assert!(
            !mermaid.contains("child1ch -->|spawns| grandgra")
                || mermaid.contains("child1ch -->|spawns| grandgra")
        );
        assert!(mermaid.contains("classDef completed"));
        let mut odd = node("odd", &["short"], false, "Weird", "2030");
        odd.depends_on.push("x".into());
        // Ids shorter than eight characters used to panic when sliced.
        let _ = render_spawn_tree(&[odd], "odd", "mermaid").unwrap();

        let err = render_spawn_tree(&nodes, "x", "xml")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Unsupported format: xml"), "{err}");
    }

    #[test]
    fn text_tree_without_spawn_roots_lists_every_node() {
        // A cycle has no root; everything is still shown exactly once.
        let nodes = vec![
            node(
                "aaaaaaaa",
                &["bbbbbbbb"],
                true,
                "Pending",
                "2030-01-01 00:00:00",
            ),
            node(
                "bbbbbbbb",
                &["aaaaaaaa"],
                true,
                "Pending",
                "2030-01-01 00:01:00",
            ),
        ];
        let text = render_spawn_tree_text(&nodes, "aaaaaaaa");
        assert_eq!(text.matches("[aaaaaaaa]").count(), 1);
        assert_eq!(text.matches("[bbbbbbbb]").count(), 1);
    }

    #[test]
    fn list_stats_and_pending_render() {
        let at = chrono::DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(render_spawn_list(&[]).contains("No spawn operations found."));
        let list = render_spawn_list(&[SpawnListRow {
            parent_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            queue_name: "éééééééééééééééééé".into(),
            spawned_count: 3,
            created_at: at,
            operation_id: "operation-with-long-name".into(),
            workflow: "none".into(),
        }]);
        assert!(list.contains("550e8400") && list.contains("01-02 03:04:05"));
        assert!(list.contains("operation-"), "{list}");
        assert!(!list.contains("operation-with"));

        let stats = SpawnStats {
            total: 4,
            avg_children: Some(2.25),
            max_children: Some(5),
            breakdown: Some(vec![QueueSpawnStats {
                queue_name: "emails".into(),
                operations: 4,
                avg_children: None,
            }]),
        };
        let out = render_spawn_stats(&stats, 12);
        for expected in [
            "Last 12 hours",
            "Total Spawn Operations: 4",
            "Average Children per Spawn: 2.2",
            "Maximum Children in Single Spawn: 5",
            "Breakdown by Queue",
            "emails",
        ] {
            assert!(out.contains(expected), "{expected} in {out}");
        }
        let out = render_spawn_stats(
            &SpawnStats {
                total: 0,
                avg_children: None,
                max_children: None,
                breakdown: None,
            },
            1,
        );
        assert!(
            out.contains("Total Spawn Operations: 0")
                && !out.contains("Average")
                && !out.contains("Breakdown")
        );

        let pending = vec![PendingSpawn {
            id: "550e8400-e29b-41d4-a716-446655440000".into(),
            queue_name: "q".into(),
            status: "Running".into(),
            created_at: at,
            config: Some(serde_json::json!({"operation_id": "op-1"})),
        }];
        let with_config = render_pending_spawns(&pending, true);
        assert!(with_config.contains("Job: 550e8400 | Queue: q | Status: Running"));
        assert!(with_config.contains("Spawn Config:") && with_config.contains("op-1"));
        assert!(!render_pending_spawns(&pending, false).contains("Spawn Config"));
        assert!(
            render_pending_spawns(&[], false)
                .contains("No jobs with pending spawn operations found.")
        );
    }

    #[test]
    fn lineage_renders_both_directions_by_default() {
        let mut ancestors = vec![node("rootrootroot", &[], true, "Completed", "1")];
        ancestors.push(node("midmidmid", &["rootrootroot"], true, "Completed", "2"));
        let lineage = Lineage {
            target: node("targettarget", &["midmidmid"], false, "Running", "3"),
            ancestors: Some(ancestors),
            descendants: Some(vec![node(
                "kidkidkid",
                &["targettarget"],
                false,
                "Pending",
                "4",
            )]),
        };
        let out = render_lineage(&lineage, "targettarget");
        assert!(out.contains("Spawn Lineage for targettarget"));
        assert!(out.contains("Queue: q | Status: Running"));
        assert!(
            out.contains("  └─ [rootroot]") && out.contains("    └─ [midmidmi]"),
            "{out}"
        );
        assert!(out.contains("└─📝 [kidkidki] q (Pending)"));

        let lonely = Lineage {
            target: node("lonelylonely", &[], false, "Pending", "1"),
            ancestors: Some(vec![]),
            descendants: Some(vec![]),
        };
        let out = render_lineage(&lonely, "lonelylonely");
        assert!(out.contains("this is a root job") && out.contains("hasn't spawned children"));
        let only_up = Lineage {
            descendants: None,
            ..lonely
        };
        assert!(!render_lineage(&only_up, "x").contains("Descendant"));
    }

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
                "parent.created_at > DATE_SUB(UTC_TIMESTAMP(6), INTERVAL ? HOUR) AND parent.queue_name = ?",
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

    async fn spawn_commands(url: String) {
        let config = config_for(&url);
        let pool = DatabasePool::connect(&url, 2).await.unwrap();
        let hostile = hostile_queue();
        let other = unique_queue("spawn_other");
        let spawn_config = r#"{"_spawn_config": {"operation_id": "op-root"}}"#;

        // root (spawned two children, one of which spawned a grandchild) + unrelated jobs
        let mut root = SeedJob::new(&hostile, "Completed");
        root.payload = spawn_config;
        let root_id = seed(&pool, &root).await;
        let mut child_a = SeedJob::new(&hostile, "Completed");
        child_a.payload = r#"{"_spawn_config": {"operation_id": "op-a"}}"#;
        child_a.depends_on = Some(&root_id);
        let child_a_id = seed(&pool, &child_a).await;
        let mut child_b = SeedJob::new(&hostile, "Pending");
        child_b.depends_on = Some(&root_id);
        let child_b_id = seed(&pool, &child_b).await;
        let mut grandchild = SeedJob::new(&hostile, "Pending");
        grandchild.depends_on = Some(&child_a_id);
        let grandchild_id = seed(&pool, &grandchild).await;
        let mut waiting = SeedJob::new(&hostile, "Pending");
        waiting.payload = r#"{"_spawn_config": {"operation_id": "op-wait", "max": 3}}"#;
        let waiting_id = seed(&pool, &waiting).await;
        let mut elsewhere = SeedJob::new(&other, "Completed");
        elsewhere.payload = spawn_config;
        seed(&pool, &elsewhere).await;

        // list: parents with a spawn config, scoped to the queue, with child counts
        let rows = fetch_spawn_list(&pool, Some(&hostile), false, 10)
            .await
            .unwrap();
        let by_id = |id: &str| rows.iter().find(|r| r.parent_id == id).cloned();
        assert_eq!(by_id(&root_id).unwrap().spawned_count, 2);
        assert_eq!(by_id(&root_id).unwrap().operation_id, "op-root");
        assert_eq!(by_id(&child_a_id).unwrap().spawned_count, 1);
        assert!(
            by_id(&waiting_id).is_none(),
            "a pending parent has not spawned anything yet"
        );
        assert!(rows.iter().all(|r| r.queue_name == hostile));
        assert_eq!(
            fetch_spawn_list(&pool, Some(&hostile), true, 1)
                .await
                .unwrap()
                .len(),
            1
        );
        let rendered = render_spawn_list(&rows);
        assert!(rendered.contains(&short_id(&root_id)));

        // stats: totals and per-queue breakdown
        let stats = fetch_spawn_stats(&pool, 1, Some(&hostile), true)
            .await
            .unwrap();
        assert_eq!(
            stats.total, 3,
            "root, child a and the waiting job carry a spawn config"
        );
        assert_eq!(stats.max_children, Some(2));
        let breakdown = stats.breakdown.unwrap();
        assert_eq!(breakdown.len(), 1);
        assert_eq!(
            (breakdown[0].queue_name.as_str(), breakdown[0].operations),
            (hostile.as_str(), 3)
        );
        assert!(
            fetch_spawn_stats(&pool, 1, Some(&hostile), false)
                .await
                .unwrap()
                .breakdown
                .is_none()
        );

        // pending: unfinished jobs carrying a spawn config
        let pending = fetch_pending_spawns(&pool, Some(&hostile)).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, waiting_id);
        assert_eq!(pending[0].config.as_ref().unwrap()["max"], 3);

        // tree: full, children only, and the target itself is highlighted
        let full = collect_tree(&pool, &child_a_id, true, false).await.unwrap();
        let ids: HashSet<String> = full.iter().map(|n| n.id.clone()).collect();
        assert_eq!(
            ids,
            HashSet::from([
                root_id.clone(),
                child_a_id.clone(),
                child_b_id.clone(),
                grandchild_id.clone()
            ])
        );
        let children_only = collect_tree(&pool, &root_id, false, true).await.unwrap();
        let ids: HashSet<String> = children_only.iter().map(|n| n.id.clone()).collect();
        assert_eq!(
            ids,
            HashSet::from([root_id.clone(), child_a_id.clone(), child_b_id.clone()])
        );
        let text = render_spawn_tree(&full, &child_a_id, "text").unwrap();
        assert!(text.contains(&format!("[{}]", short_id(&root_id))));
        assert!(text.contains('⭐'));
        let root_node = full.iter().find(|n| n.id == root_id).unwrap();
        assert_eq!(
            root_node.spawn_config.as_ref().unwrap()["operation_id"],
            "op-root"
        );
        let grandchild_node = full.iter().find(|n| n.id == grandchild_id).unwrap();
        assert_eq!(grandchild_node.depends_on, vec![child_a_id.clone()]);

        // lineage
        let lineage = fetch_lineage(&pool, &grandchild_id, false, false, 10)
            .await
            .unwrap();
        let ancestors: Vec<String> = lineage
            .ancestors
            .unwrap()
            .iter()
            .map(|n| n.id.clone())
            .collect();
        assert_eq!(
            ancestors,
            vec![root_id.clone(), child_a_id.clone()],
            "oldest first"
        );
        assert!(lineage.descendants.unwrap().is_empty());
        let lineage = fetch_lineage(&pool, &root_id, false, true, 10)
            .await
            .unwrap();
        assert!(lineage.ancestors.is_none());
        let descendants: HashSet<String> = lineage
            .descendants
            .unwrap()
            .iter()
            .map(|n| n.id.clone())
            .collect();
        assert_eq!(
            descendants,
            HashSet::from([
                child_a_id.clone(),
                child_b_id.clone(),
                grandchild_id.clone()
            ])
        );
        let shallow = fetch_lineage(&pool, &root_id, false, true, 1)
            .await
            .unwrap();
        assert_eq!(
            shallow.descendants.unwrap().len(),
            2,
            "depth 1 stops at the children"
        );
        let capped = fetch_lineage(&pool, &grandchild_id, true, false, 1)
            .await
            .unwrap();
        assert_eq!(capped.ancestors.unwrap().len(), 1);

        // the commands themselves, in every shape
        let run = |args: Vec<String>| {
            let cmd = parse(&args.iter().map(String::as_str).collect::<Vec<_>>());
            let config = config.clone();
            async move { cmd.execute(config).await }
        };
        fn s(v: &str) -> String {
            v.to_string()
        }
        run(vec![
            s("list"),
            s("--queue"),
            hostile.clone(),
            s("--recent"),
            s("--limit"),
            s("5"),
        ])
        .await
        .unwrap();
        run(vec![s("list")]).await.unwrap();
        for format in ["text", "json", "mermaid"] {
            run(vec![
                s("tree"),
                grandchild_id.clone(),
                s("--format"),
                s(format),
            ])
            .await
            .unwrap();
        }
        run(vec![s("tree"), root_id.clone(), s("--children-only")])
            .await
            .unwrap();
        run(vec![
            s("stats"),
            s("-Q"),
            hostile.clone(),
            s("--hours"),
            s("2"),
            s("--detailed"),
        ])
        .await
        .unwrap();
        run(vec![s("stats")]).await.unwrap();
        run(vec![
            s("lineage"),
            root_id.clone(),
            s("--descendants"),
            s("--depth"),
            s("2"),
        ])
        .await
        .unwrap();
        run(vec![s("lineage"), grandchild_id.clone()])
            .await
            .unwrap();
        run(vec![
            s("pending"),
            s("-Q"),
            hostile.clone(),
            s("--show-config"),
        ])
        .await
        .unwrap();
        run(vec![s("pending")]).await.unwrap();

        // errors
        let missing = uuid::Uuid::new_v4().to_string();
        for args in [
            vec![s("tree"), missing.clone()],
            vec![s("tree"), s("not-a-uuid")],
            vec![s("tree"), root_id.clone(), s("--format"), s("xml")],
            vec![s("lineage"), missing.clone()],
            vec![s("lineage"), s("nope")],
        ] {
            assert!(run(args.clone()).await.is_err(), "{args:?}");
        }
        let err = run(vec![s("tree"), missing.clone()])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Job not found"), "{err}");

        // the monitor refreshes until it is told to stop, and rejects a zero interval
        monitor_spawn_operations(
            pool.clone(),
            std::time::Duration::from_secs(1),
            Some(&hostile),
            tokio::time::sleep(std::time::Duration::from_millis(300)),
        )
        .await
        .unwrap();
        assert!(
            monitor_spawn_operations(pool.clone(), std::time::Duration::ZERO, None, async {})
                .await
                .is_err()
        );

        // the raw statements still behave on the real schema
        let backend = pool.backend();
        let (sql, binds) = build_spawn_list_query(backend, Some(&hostile), true, 10);
        let queues = column_strings(&pool, &sql, &binds, "queue_name").await;
        assert!(queues.iter().all(|q| q == &hostile));
        let (sql, binds) = build_spawn_stats_total_query(backend, 1, Some(&hostile));
        assert_eq!(
            fetch_i64(&pool, &sql, &binds, "total_spawn_ops")
                .await
                .unwrap(),
            3
        );

        assert!(table_exists(&pool).await);
        cleanup(&pool, &[&hostile, &other]).await;
    }

    db_tests!(
        spawn_commands,
        test_spawn_commands_postgres,
        test_spawn_commands_mysql
    );
}
