use anyhow::Result;
use sqlx::{MySqlPool, PgPool, Row};

/// Tests for SQL query validation and correctness
/// These tests validate that our dynamic SQL queries are syntactically correct
/// and produce expected results
#[cfg(test)]
mod postgres_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_job_list_queries() -> Result<()> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:hammerwork@localhost:5433/hammerwork".to_string()
        });

        let pool = PgPool::connect(&database_url).await?;

        // Test basic job listing query
        let query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs ORDER BY created_at DESC LIMIT 10";
        let rows = sqlx::query(query).fetch_all(&pool).await?;
        assert!(rows.len() <= 10);

        // Test query with queue filter
        let query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs WHERE queue_name = $1 ORDER BY created_at DESC LIMIT 10";
        let rows = sqlx::query(query)
            .bind("test_queue")
            .fetch_all(&pool)
            .await?;
        // Should not error even if no results
        assert!(rows.len() <= 10);

        // Test query with status filter
        let query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs WHERE status = $1 ORDER BY created_at DESC LIMIT 10";
        // The library stores capitalized status names
        let rows = sqlx::query(query).bind("Pending").fetch_all(&pool).await?;
        assert!(rows.len() <= 10);

        // Test query with priority filter
        let query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs WHERE priority = $1 ORDER BY created_at DESC LIMIT 10";
        // priority is an INTEGER column (JobPriority::Normal = 2)
        let rows = sqlx::query(query).bind(2_i32).fetch_all(&pool).await?;
        assert!(rows.len() <= 10);

        // Test query with multiple conditions
        let query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs WHERE queue_name = $1 AND status = $2 ORDER BY created_at DESC LIMIT 10";
        let rows = sqlx::query(query)
            .bind("test_queue")
            .bind("Pending")
            .fetch_all(&pool)
            .await?;
        assert!(rows.len() <= 10);

        // Test query with time-based filter
        let query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs WHERE created_at > NOW() - INTERVAL '1 hours' ORDER BY created_at DESC LIMIT 10";
        let rows = sqlx::query(query).fetch_all(&pool).await?;
        assert!(rows.len() <= 10);

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_job_operations() -> Result<()> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:hammerwork@localhost:5433/hammerwork".to_string()
        });

        let pool = PgPool::connect(&database_url).await?;

        // Test retry query syntax
        let query = "SELECT id FROM hammerwork_jobs WHERE status IN ('Failed', 'Dead', 'TimedOut')";
        let result = sqlx::query(query).fetch_all(&pool).await?;
        // Should execute without error
        let _rows_affected = result.len();

        // Test cancel query syntax
        let query = "SELECT id FROM hammerwork_jobs WHERE status = 'Pending'";
        let result = sqlx::query(query).fetch_all(&pool).await?;
        // Should execute without error
        let _rows_affected = result.len();

        // Test job detail query
        let test_uuid = uuid::Uuid::new_v4();
        let query = "SELECT * FROM hammerwork_jobs WHERE id = $1";
        let result = sqlx::query(query)
            .bind(test_uuid)
            .fetch_optional(&pool)
            .await?;
        // Should not error even if no result
        assert!(result.is_none());

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_monitoring_queries() -> Result<()> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:hammerwork@localhost:5433/hammerwork".to_string()
        });

        let pool = PgPool::connect(&database_url).await?;

        // Test connectivity check
        let result = sqlx::query("SELECT 1").fetch_one(&pool).await?;
        assert!(result.len() > 0);

        // Test stuck jobs query
        let query = "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE status = 'running' AND started_at < NOW() - INTERVAL '1 hour'";
        let result = sqlx::query(query).fetch_one(&pool).await?;
        let count: i64 = result.try_get("count")?;
        assert!(count >= 0);

        // Test failure rate queries
        let query = "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE created_at > NOW() - INTERVAL '1 hour'";
        let result = sqlx::query(query).fetch_one(&pool).await?;
        let count: i64 = result.try_get("count")?;
        assert!(count >= 0);

        let query = "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE status = 'Failed' AND failed_at > NOW() - INTERVAL '1 hour'";
        let result = sqlx::query(query).fetch_one(&pool).await?;
        let count: i64 = result.try_get("count")?;
        assert!(count >= 0);

        Ok(())
    }
}

#[cfg(test)]
mod mysql_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_job_list_queries() -> Result<()> {
        let database_url = std::env::var("MYSQL_DATABASE_URL")
            .unwrap_or_else(|_| "mysql://root:password@localhost:3306/hammerwork".to_string());

        let pool = MySqlPool::connect(&database_url).await?;

        // Test basic job listing query
        let query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs ORDER BY created_at DESC LIMIT 10";
        let rows = sqlx::query(query).fetch_all(&pool).await?;
        assert!(rows.len() <= 10);

        // Test query with time-based filter (MySQL syntax)
        let query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs WHERE created_at > DATE_SUB(NOW(), INTERVAL 1 HOUR) ORDER BY created_at DESC LIMIT 10";
        let rows = sqlx::query(query).fetch_all(&pool).await?;
        assert!(rows.len() <= 10);

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_monitoring_queries() -> Result<()> {
        let database_url = std::env::var("MYSQL_DATABASE_URL")
            .unwrap_or_else(|_| "mysql://root:password@localhost:3306/hammerwork".to_string());

        let pool = MySqlPool::connect(&database_url).await?;

        // Test connectivity check
        let result = sqlx::query("SELECT 1").fetch_one(&pool).await?;
        assert!(result.len() > 0);

        // Test stuck jobs query (MySQL syntax)
        let query = "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE status = 'running' AND started_at < DATE_SUB(NOW(), INTERVAL 1 HOUR)";
        let result = sqlx::query(query).fetch_one(&pool).await?;
        let count: i64 = result.try_get("count")?;
        assert!(count >= 0);

        Ok(())
    }
}

#[cfg(test)]
mod unit_tests {

    #[test]
    fn test_query_validation_functions() {
        // Test status validation
        assert!(is_valid_status("pending"));
        assert!(is_valid_status("running"));
        assert!(is_valid_status("completed"));
        assert!(is_valid_status("failed"));
        assert!(is_valid_status("retrying"));
        assert!(is_valid_status("dead"));
        assert!(!is_valid_status("invalid"));
        assert!(!is_valid_status(""));

        // Test priority validation
        assert!(is_valid_priority("background"));
        assert!(is_valid_priority("low"));
        assert!(is_valid_priority("normal"));
        assert!(is_valid_priority("high"));
        assert!(is_valid_priority("critical"));
        assert!(!is_valid_priority("invalid"));
        assert!(!is_valid_priority(""));

        // Test queue name validation
        assert!(is_valid_queue_name("emails"));
        assert!(is_valid_queue_name("background-jobs"));
        assert!(is_valid_queue_name("queue_1"));
        assert!(!is_valid_queue_name(""));
        assert!(!is_valid_queue_name("queue with spaces"));
        assert!(!is_valid_queue_name("queue/with/slashes"));
    }

    fn is_valid_status(status: &str) -> bool {
        matches!(
            status,
            "pending" | "running" | "completed" | "failed" | "retrying" | "dead"
        )
    }

    fn is_valid_priority(priority: &str) -> bool {
        matches!(
            priority,
            "background" | "low" | "normal" | "high" | "critical"
        )
    }

    fn is_valid_queue_name(name: &str) -> bool {
        !name.is_empty()
            && !name.contains(' ')
            && !name.contains('/')
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    }
}

#[cfg(test)]
mod error_handling_tests {

    #[test]
    fn test_limit_validation() {
        // Test reasonable limits
        let limit = 1000u32;
        assert!(limit <= 10000); // Reasonable upper bound

        let limit = 0u32;
        let safe_limit = if limit == 0 { 50 } else { limit };
        assert_eq!(safe_limit, 50);
    }
}

/// Validates the CLI's real query builders (not copies of their SQL): every user-supplied
/// value must be a bound parameter, placeholders must match the binds, and the statements must
/// run on the database with a hostile queue name.
#[cfg(test)]
mod builder_tests {
    use super::*;
    use cargo_hammerwork::commands::backup::build_backup_query;
    use cargo_hammerwork::commands::cron::build_cron_select_query;
    use cargo_hammerwork::commands::job::{build_list_jobs_query, build_purge_query};
    use cargo_hammerwork::commands::maintenance::{VacuumKind, build_vacuum_query};
    use cargo_hammerwork::commands::monitor::{
        build_avg_time_query, build_recent_jobs_query, build_status_counts_query,
        build_throughput_query,
    };
    use cargo_hammerwork::commands::queue::{
        HealthMetric, build_detailed_stats_query, build_health_query, build_queue_total_query,
    };
    use cargo_hammerwork::commands::spawn::{
        build_pending_spawns_query, build_spawn_list_query, build_spawn_stats_breakdown_query,
        build_spawn_stats_total_query,
    };
    use cargo_hammerwork::utils::job_ops::JobSelector;
    use cargo_hammerwork::utils::sql::{Backend, Bind, bind_mysql, bind_pg};
    use hammerwork::{JobPriority, JobStatus};

    const HOSTILE: &str = "it's; DROP TABLE hammerwork_jobs --";

    /// Every builder that takes a queue name, with options that exercise each clause.
    fn queue_scoped_queries(backend: Backend) -> Vec<(&'static str, String, Vec<Bind>)> {
        let q = Some(HOSTILE);
        let now = chrono::Utc::now();
        let mut out = Vec::new();
        let mut add = |name: &'static str, (sql, binds): (String, Vec<Bind>)| {
            out.push((name, sql, binds));
        };
        add(
            "cron_select",
            build_cron_select_query(backend, q, true, None),
        );
        add(
            "cron_select_by_id",
            build_cron_select_query(
                backend,
                q,
                false,
                Some("11111111-1111-1111-1111-111111111111"),
            ),
        );
        add("status_counts", build_status_counts_query(backend, q));
        add("recent_jobs", build_recent_jobs_query(backend, q));
        add("throughput", build_throughput_query(backend, 24, q));
        add("avg_time", build_avg_time_query(backend, 168, q));
        add("queue_total", build_queue_total_query(backend, q));
        add("detailed_stats", build_detailed_stats_query(backend, q));
        for metric in [
            HealthMetric::Total,
            HealthMetric::RecentFailures,
            HealthMetric::LongRunning,
        ] {
            add("health", build_health_query(backend, metric, q));
        }
        add("backup", build_backup_query(backend, q, false, false));
        add("spawn_list", build_spawn_list_query(backend, q, true, 20));
        add("spawn_stats", build_spawn_stats_total_query(backend, 24, q));
        add(
            "spawn_breakdown",
            build_spawn_stats_breakdown_query(backend, 24, q),
        );
        add("spawn_pending", build_pending_spawns_query(backend, q));
        add(
            "job_list",
            build_list_jobs_query(
                backend == Backend::Postgres,
                q,
                Some("pending"),
                Some(JobPriority::High),
                50,
                false,
                false,
                Some(24),
            ),
        );
        add(
            "job_purge",
            build_purge_query(backend, "status IN ('Completed')", q, Some(7)),
        );
        add(
            "job_selector",
            JobSelector {
                statuses: vec![JobStatus::Failed, JobStatus::Dead],
                queue: Some(HOSTILE.to_string()),
                failed_after: Some(now),
                created_before: Some(now),
                started_before: Some(now),
                never_started: true,
                attempts_exhausted: true,
            }
            .sql(backend),
        );
        out
    }

    /// Queries without a queue name that still take bound values.
    fn other_queries(backend: Backend) -> Vec<(&'static str, String, Vec<Bind>)> {
        let now = chrono::Utc::now();
        let mut out = Vec::new();
        for (kind, delete) in [
            (VacuumKind::Completed, false),
            (VacuumKind::Completed, true),
            (VacuumKind::Failed, false),
            (VacuumKind::Failed, true),
        ] {
            let (sql, binds) = build_vacuum_query(backend, kind, delete, now);
            out.push(("vacuum", sql, binds));
        }
        out
    }

    /// The `$n` placeholders in `sql`, as the highest index plus whether 1..=max are all used.
    fn postgres_placeholders(sql: &str) -> (usize, bool) {
        let mut seen = std::collections::BTreeSet::new();
        let bytes = sql.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'$' {
                let digits: String = sql[i + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(n) = digits.parse::<usize>() {
                    seen.insert(n);
                }
            }
            i += 1;
        }
        let max = seen.iter().next_back().copied().unwrap_or(0);
        (max, (1..=max).all(|n| seen.contains(&n)))
    }

    fn assert_parameterized(name: &str, backend: Backend, sql: &str, binds: &[Bind]) {
        match backend {
            Backend::Postgres => {
                let (max, contiguous) = postgres_placeholders(sql);
                assert_eq!(max, binds.len(), "{name}: $n vs binds in {sql}");
                assert!(contiguous, "{name}: gap in placeholders in {sql}");
            }
            Backend::MySql => {
                assert_eq!(
                    sql.matches('?').count(),
                    binds.len(),
                    "{name}: ? vs binds in {sql}"
                );
            }
        }
    }

    #[test]
    fn test_queue_names_are_bound_never_interpolated() {
        for backend in [Backend::Postgres, Backend::MySql] {
            for (name, sql, binds) in queue_scoped_queries(backend) {
                assert!(
                    !sql.contains("DROP"),
                    "{name}: queue name leaked into {sql}"
                );
                assert!(
                    !sql.contains("it's"),
                    "{name}: queue name leaked into {sql}"
                );
                assert!(
                    binds.contains(&Bind::Text(HOSTILE.to_string())),
                    "{name}: queue name is not bound ({backend:?})"
                );
                assert_parameterized(name, backend, &sql, &binds);
            }
            for (name, sql, binds) in other_queries(backend) {
                assert_parameterized(name, backend, &sql, &binds);
            }
        }
    }

    #[test]
    fn test_numbers_and_timestamps_are_bound_not_formatted() {
        // LIMIT and the interval amounts must be placeholders, even for plain integers.
        let (sql, binds) = build_spawn_list_query(Backend::Postgres, None, true, 20);
        assert!(sql.ends_with("LIMIT $2"), "{sql}");
        assert_eq!(binds, vec![Bind::Int(1), Bind::Int(20)]);

        let (sql, binds) = build_purge_query(Backend::MySql, "status IN ('Dead')", None, Some(30));
        assert!(sql.ends_with("INTERVAL ? DAY)"), "{sql}");
        assert_eq!(binds, vec![Bind::Int(30)]);

        let (sql, _) = build_list_jobs_query(true, None, None, None, 5, false, false, Some(3));
        assert!(sql.contains("make_interval(hours => $1::int)"), "{sql}");
        assert!(sql.ends_with("LIMIT $2"), "{sql}");
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_builders_run_with_hostile_queue_name() -> Result<()> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:hammerwork@localhost:5433/hammerwork".to_string()
        });
        let pool = PgPool::connect(&database_url).await?;

        let all = queue_scoped_queries(Backend::Postgres)
            .into_iter()
            .chain(other_queries(Backend::Postgres));
        for (name, sql, binds) in all {
            if name == "job_purge" || name == "vacuum" && sql.starts_with("DELETE") {
                // Writes: run them inside a rolled-back transaction
                let mut tx = pool.begin().await?;
                bind_pg(sqlx::query(&sql), &binds)
                    .execute(&mut *tx)
                    .await
                    .unwrap_or_else(|e| panic!("{name}: {e}\n{sql}"));
                tx.rollback().await?;
            } else {
                bind_pg(sqlx::query(&sql), &binds)
                    .fetch_all(&pool)
                    .await
                    .unwrap_or_else(|e| panic!("{name}: {e}\n{sql}"));
            }
        }

        // The table survived
        sqlx::query("SELECT 1 FROM hammerwork_jobs LIMIT 1")
            .fetch_all(&pool)
            .await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_builders_run_with_hostile_queue_name() -> Result<()> {
        let database_url = std::env::var("MYSQL_DATABASE_URL")
            .unwrap_or_else(|_| "mysql://root:password@localhost:3306/hammerwork".to_string());
        let pool = MySqlPool::connect(&database_url).await?;

        let all = queue_scoped_queries(Backend::MySql)
            .into_iter()
            .chain(other_queries(Backend::MySql));
        for (name, sql, binds) in all {
            if name == "job_purge" || name == "vacuum" && sql.starts_with("DELETE") {
                let mut tx = pool.begin().await?;
                bind_mysql(sqlx::query(&sql), &binds)
                    .execute(&mut *tx)
                    .await
                    .unwrap_or_else(|e| panic!("{name}: {e}\n{sql}"));
                tx.rollback().await?;
            } else {
                bind_mysql(sqlx::query(&sql), &binds)
                    .fetch_all(&pool)
                    .await
                    .unwrap_or_else(|e| panic!("{name}: {e}\n{sql}"));
            }
        }

        sqlx::query("SELECT 1 FROM hammerwork_jobs LIMIT 1")
            .fetch_all(&pool)
            .await?;
        Ok(())
    }
}
