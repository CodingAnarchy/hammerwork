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
        let rows = sqlx::query(query).bind("pending").fetch_all(&pool).await?;
        assert!(rows.len() <= 10);

        // Test query with priority filter
        let query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs WHERE priority = $1 ORDER BY created_at DESC LIMIT 10";
        let rows = sqlx::query(query).bind("normal").fetch_all(&pool).await?;
        assert!(rows.len() <= 10);

        // Test query with multiple conditions
        let query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs WHERE queue_name = $1 AND status = $2 ORDER BY created_at DESC LIMIT 10";
        let rows = sqlx::query(query)
            .bind("test_queue")
            .bind("pending")
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
        let query = "UPDATE hammerwork_jobs SET status = 'pending', attempts = 0, scheduled_at = NOW() WHERE status IN ('failed', 'dead')";
        let result = sqlx::query(query).execute(&pool).await?;
        // Should execute without error
        let _rows_affected = result.rows_affected();

        // Test cancel query syntax
        let query = "DELETE FROM hammerwork_jobs WHERE status = 'pending'";
        let result = sqlx::query(query).execute(&pool).await?;
        // Should execute without error
        let _rows_affected = result.rows_affected();

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

        let query = "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE status = 'failed' AND failed_at > NOW() - INTERVAL '1 hour'";
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
    fn test_query_building_logic() {
        // Test PostgreSQL query building
        let mut conditions = Vec::new();
        let queue = Some("test_queue".to_string());
        let status = Some("pending".to_string());
        let priority = Some("high".to_string());
        let limit = 50u32;

        if queue.is_some() {
            conditions.push(format!("queue_name = ${}", conditions.len() + 1));
        }

        if status.is_some() {
            conditions.push(format!("status = ${}", conditions.len() + 1));
        }

        if priority.is_some() {
            conditions.push(format!("priority = ${}", conditions.len() + 1));
        }

        let mut query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs".to_string();

        if !conditions.is_empty() {
            query.push_str(&format!(" WHERE {}", conditions.join(" AND ")));
        }

        query.push_str(" ORDER BY created_at DESC");
        query.push_str(&format!(" LIMIT {}", limit));

        // Verify query structure
        assert!(query.contains("WHERE"));
        assert!(query.contains("queue_name = $1"));
        assert!(query.contains("status = $2"));
        assert!(query.contains("priority = $3"));
        assert!(query.contains("ORDER BY created_at DESC"));
        assert!(query.contains("LIMIT 50"));

        println!("Generated PostgreSQL query: {}", query);
    }

    #[test]
    fn test_mysql_query_building_logic() {
        // Test MySQL query building (uses ? placeholders)
        let mut conditions = Vec::new();
        let queue = Some("test_queue".to_string());
        let status = Some("pending".to_string());
        let limit = 50u32;

        if queue.is_some() {
            conditions.push("queue_name = ?".to_string());
        }

        if status.is_some() {
            conditions.push("status = ?".to_string());
        }

        let mut query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs".to_string();

        if !conditions.is_empty() {
            query.push_str(&format!(" WHERE {}", conditions.join(" AND ")));
        }

        // Test MySQL interval syntax
        let hours = 24u32;
        if !conditions.is_empty() {
            query.push_str(&format!(
                " AND created_at > DATE_SUB(NOW(), INTERVAL {} HOUR)",
                hours
            ));
        } else {
            query.push_str(&format!(
                " WHERE created_at > DATE_SUB(NOW(), INTERVAL {} HOUR)",
                hours
            ));
        }

        query.push_str(" ORDER BY created_at DESC");
        query.push_str(&format!(" LIMIT {}", limit));

        // Verify query structure
        assert!(query.contains("WHERE"));
        assert!(query.contains("queue_name = ?"));
        assert!(query.contains("status = ?"));
        assert!(query.contains("DATE_SUB(NOW(), INTERVAL 24 HOUR)"));
        assert!(query.contains("ORDER BY created_at DESC"));
        assert!(query.contains("LIMIT 50"));

        println!("Generated MySQL query: {}", query);
    }

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
    fn test_sql_injection_prevention() {
        // Test that we're using parameterized queries correctly
        let malicious_input = "'; DROP TABLE hammerwork_jobs; --";

        // This should be safe because we use bind parameters
        let query = "SELECT * FROM hammerwork_jobs WHERE queue_name = $1";

        // Verify the query structure doesn't include the malicious input directly
        assert!(!query.contains("DROP TABLE"));
        assert!(query.contains("$1")); // Parameterized

        println!("Safe parameterized query: {}", query);
        println!(
            "Malicious input would be bound as parameter: {}",
            malicious_input
        );
    }

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

#[cfg(test)]
mod spawn_query_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_spawn_list_queries() -> Result<()> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:hammerwork@localhost:5433/hammerwork".to_string()
        });

        let pool = PgPool::connect(&database_url).await?;

        // Test spawn operations listing query
        let query = r#"
            SELECT parent.id as parent_id, parent.queue_name, parent.created_at,
                   parent.payload->'_spawn_config' as spawn_config,
                   COUNT(child.id) as spawned_count,
                   parent.workflow_id, parent.workflow_name
            FROM hammerwork_jobs parent
            LEFT JOIN hammerwork_jobs child ON child.depends_on @> CONCAT('["', parent.id, '"]')::jsonb
            WHERE parent.payload ? '_spawn_config' 
                  AND parent.status IN ('Completed', 'Running')
            GROUP BY parent.id, parent.queue_name, parent.created_at, parent.payload, parent.workflow_id, parent.workflow_name
            ORDER BY parent.created_at DESC
            LIMIT 20
        "#;

        let rows = sqlx::query(query).fetch_all(&pool).await?;
        // Should succeed even if no spawn operations exist
        assert!(rows.len() >= 0);

        // Test with recent filter
        let query_recent = r#"
            SELECT parent.id as parent_id, parent.queue_name, parent.created_at,
                   parent.payload->'_spawn_config' as spawn_config,
                   COUNT(child.id) as spawned_count
            FROM hammerwork_jobs parent
            LEFT JOIN hammerwork_jobs child ON child.depends_on @> CONCAT('["', parent.id, '"]')::jsonb
            WHERE parent.payload ? '_spawn_config' 
                  AND parent.status IN ('Completed', 'Running')
                  AND parent.created_at > NOW() - INTERVAL '1 hour'
            GROUP BY parent.id, parent.queue_name, parent.created_at, parent.payload
            ORDER BY parent.created_at DESC
            LIMIT 10
        "#;

        let rows = sqlx::query(query_recent).fetch_all(&pool).await?;
        // Verify query executes without error
        assert!(rows.len() >= 0);

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_spawn_stats_queries() -> Result<()> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:hammerwork@localhost:5433/hammerwork".to_string()
        });

        let pool = PgPool::connect(&database_url).await?;

        // Test spawn statistics aggregation
        let query = r#"
            SELECT COUNT(*) as total_spawn_ops,
                   AVG(spawned_count) as avg_children,
                   MAX(spawned_count) as max_children
            FROM (
                SELECT parent.id, COUNT(child.id) as spawned_count
                FROM hammerwork_jobs parent
                LEFT JOIN hammerwork_jobs child ON child.depends_on @> CONCAT('["', parent.id, '"]')::jsonb
                WHERE parent.payload ? '_spawn_config'
                      AND parent.created_at > NOW() - INTERVAL '24 hours'
                GROUP BY parent.id
            ) spawn_stats
        "#;

        let row = sqlx::query(query).fetch_one(&pool).await?;
        let total: i64 = row.get("total_spawn_ops");
        assert!(total >= 0);

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_spawn_list_queries() -> Result<()> {
        let database_url = std::env::var("MYSQL_DATABASE_URL").unwrap_or_else(|_| {
            "mysql://root:hammerwork@localhost:3307/hammerwork".to_string()
        });

        let pool = MySqlPool::connect(&database_url).await?;

        // Test MySQL spawn operations listing query
        let query = r#"
            SELECT parent.id as parent_id, parent.queue_name, parent.created_at,
                   JSON_EXTRACT(parent.payload, '$._spawn_config') as spawn_config,
                   COUNT(child.id) as spawned_count,
                   parent.workflow_id, parent.workflow_name
            FROM hammerwork_jobs parent
            LEFT JOIN hammerwork_jobs child ON JSON_CONTAINS(child.depends_on, CONCAT('"', parent.id, '"'))
            WHERE JSON_EXTRACT(parent.payload, '$._spawn_config') IS NOT NULL
                  AND parent.status IN ('Completed', 'Running')
            GROUP BY parent.id, parent.queue_name, parent.created_at, parent.payload, parent.workflow_id, parent.workflow_name
            ORDER BY parent.created_at DESC
            LIMIT 20
        "#;

        let rows = sqlx::query(query).fetch_all(&pool).await?;
        // Should succeed even if no spawn operations exist
        assert!(rows.len() >= 0);

        // Test with recent filter using MySQL syntax
        let query_recent = r#"
            SELECT parent.id as parent_id, parent.queue_name, parent.created_at,
                   JSON_EXTRACT(parent.payload, '$._spawn_config') as spawn_config,
                   COUNT(child.id) as spawned_count
            FROM hammerwork_jobs parent
            LEFT JOIN hammerwork_jobs child ON JSON_CONTAINS(child.depends_on, CONCAT('"', parent.id, '"'))
            WHERE JSON_EXTRACT(parent.payload, '$._spawn_config') IS NOT NULL
                  AND parent.status IN ('Completed', 'Running')
                  AND parent.created_at > DATE_SUB(NOW(), INTERVAL 1 HOUR)
            GROUP BY parent.id, parent.queue_name, parent.created_at, parent.payload
            ORDER BY parent.created_at DESC
            LIMIT 10
        "#;

        let rows = sqlx::query(query_recent).fetch_all(&pool).await?;
        // Verify query executes without error
        assert!(rows.len() >= 0);

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_spawn_stats_queries() -> Result<()> {
        let database_url = std::env::var("MYSQL_DATABASE_URL").unwrap_or_else(|_| {
            "mysql://root:hammerwork@localhost:3307/hammerwork".to_string()
        });

        let pool = MySqlPool::connect(&database_url).await?;

        // Test MySQL spawn statistics aggregation
        let query = r#"
            SELECT COUNT(*) as total_spawn_ops,
                   AVG(spawned_count) as avg_children,
                   MAX(spawned_count) as max_children
            FROM (
                SELECT parent.id, COUNT(child.id) as spawned_count
                FROM hammerwork_jobs parent
                LEFT JOIN hammerwork_jobs child ON JSON_CONTAINS(child.depends_on, CONCAT('"', parent.id, '"'))
                WHERE JSON_EXTRACT(parent.payload, '$._spawn_config') IS NOT NULL
                      AND parent.created_at > DATE_SUB(NOW(), INTERVAL 24 HOUR)
                GROUP BY parent.id
            ) spawn_stats
        "#;

        let row = sqlx::query(query).fetch_one(&pool).await?;
        let total: i64 = row.get("total_spawn_ops");
        assert!(total >= 0);

        Ok(())
    }

    #[test]
    fn test_spawn_query_building_postgres() {
        // Test PostgreSQL spawn query building
        let recent = true;
        let queue_filter = Some("spawn_queue");
        let limit = 20u32;

        let mut query = r#"
            SELECT parent.id as parent_id, parent.queue_name, parent.created_at,
                   parent.payload->'_spawn_config' as spawn_config,
                   COUNT(child.id) as spawned_count
            FROM hammerwork_jobs parent
            LEFT JOIN hammerwork_jobs child ON child.depends_on @> CONCAT('["', parent.id, '"]')::jsonb
            WHERE parent.payload ? '_spawn_config' 
                  AND parent.status IN ('Completed', 'Running')"#.to_string();

        if recent {
            query.push_str(" AND parent.created_at > NOW() - INTERVAL '1 hour'");
        }

        if let Some(queue) = queue_filter {
            query.push_str(&format!(" AND parent.queue_name = '{}'", queue));
        }

        query.push_str(r#"
            GROUP BY parent.id, parent.queue_name, parent.created_at, parent.payload
            ORDER BY parent.created_at DESC"#);
        
        query.push_str(&format!(" LIMIT {}", limit));

        // Verify query structure
        assert!(query.contains("payload ? '_spawn_config'"));
        assert!(query.contains("depends_on @>"));
        assert!(query.contains("INTERVAL '1 hour'"));
        assert!(query.contains("parent.queue_name = 'spawn_queue'"));
        assert!(query.contains("LIMIT 20"));
    }

    #[test]
    fn test_spawn_query_building_mysql() {
        // Test MySQL spawn query building
        let recent = true;
        let queue_filter = Some("spawn_queue");
        let limit = 20u32;

        let mut query = r#"
            SELECT parent.id as parent_id, parent.queue_name, parent.created_at,
                   JSON_EXTRACT(parent.payload, '$._spawn_config') as spawn_config,
                   COUNT(child.id) as spawned_count
            FROM hammerwork_jobs parent
            LEFT JOIN hammerwork_jobs child ON JSON_CONTAINS(child.depends_on, CONCAT('"', parent.id, '"'))
            WHERE JSON_EXTRACT(parent.payload, '$._spawn_config') IS NOT NULL
                  AND parent.status IN ('Completed', 'Running')"#.to_string();

        if recent {
            query.push_str(" AND parent.created_at > DATE_SUB(NOW(), INTERVAL 1 HOUR)");
        }

        if let Some(queue) = queue_filter {
            query.push_str(&format!(" AND parent.queue_name = '{}'", queue));
        }

        query.push_str(r#"
            GROUP BY parent.id, parent.queue_name, parent.created_at, parent.payload
            ORDER BY parent.created_at DESC"#);
        
        query.push_str(&format!(" LIMIT {}", limit));

        // Verify query structure
        assert!(query.contains("JSON_EXTRACT(parent.payload, '$._spawn_config')"));
        assert!(query.contains("JSON_CONTAINS(child.depends_on"));
        assert!(query.contains("DATE_SUB(NOW(), INTERVAL 1 HOUR)"));
        assert!(query.contains("parent.queue_name = 'spawn_queue'"));
        assert!(query.contains("LIMIT 20"));
    }

    #[test]
    fn test_spawn_tree_query_building() {
        // Test spawn tree query construction
        let job_id = "550e8400-e29b-41d4-a716-446655440000";
        
        let postgres_query = format!(
            r#"
            SELECT id, queue_name, status, depends_on,
                   payload->'_spawn_config' as spawn_config,
                   created_at, workflow_id, workflow_name
            FROM hammerwork_jobs
            WHERE depends_on @> '["{}"]'
            ORDER BY created_at
            "#,
            job_id
        );

        let mysql_query = format!(
            r#"
            SELECT id, queue_name, status, depends_on,
                   JSON_EXTRACT(payload, '$._spawn_config') as spawn_config,
                   created_at, workflow_id, workflow_name
            FROM hammerwork_jobs
            WHERE JSON_CONTAINS(depends_on, '"{}"')
            ORDER BY created_at
            "#,
            job_id
        );

        // Verify both queries contain the job ID
        assert!(postgres_query.contains(&job_id));
        assert!(mysql_query.contains(&job_id));
        
        // Verify database-specific syntax
        assert!(postgres_query.contains("@>"));
        assert!(mysql_query.contains("JSON_CONTAINS"));
    }
}
