//! Migration 021 converts the MySQL `TIMESTAMP` columns to `DATETIME(6)` (#91).
//!
//! The test builds a database at migration 020 (the 2.x schema), fills it, runs migration
//! 021 and checks that every value, nullability and default survived, that jobs scheduled
//! beyond 2038 can be stored, and that running the migration again changes nothing.

#![cfg(feature = "mysql")]

mod test_utils;

use chrono::{DateTime, Utc};
use hammerwork::{
    Job, JobQueue,
    migrations::{Migration, MigrationManager, MigrationRunner, mysql::MySqlMigrationRunner},
    queue::DatabaseQueue,
};
use serde_json::json;
use sqlx::{
    Connection, Executor, MySql, MySqlConnection, Pool, Row,
    mysql::{MySqlConnectOptions, MySqlPoolOptions},
};
use std::str::FromStr;

const SCRATCH_DATABASE: &str = "hammerwork_dt_test";
const MIGRATION_ID: &str = "021_mysql_datetime";

/// Every column of every table: type, nullability, default and extra, in table order.
async fn column_snapshot(pool: &Pool<MySql>) -> Vec<(String, String, String, String, String)> {
    sqlx::query(
        "SELECT CAST(table_name AS CHAR) AS t, CAST(column_name AS CHAR) AS c,
                CAST(data_type AS CHAR) AS d, CAST(is_nullable AS CHAR) AS n,
                CONCAT(IFNULL(column_default, '<null>'), '|', IFNULL(extra, '')) AS e
         FROM information_schema.columns WHERE table_schema = DATABASE()
         ORDER BY table_name, ordinal_position",
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .iter()
    .map(|row| {
        (
            row.get("t"),
            row.get("c"),
            row.get("d"),
            row.get("n"),
            row.get("e"),
        )
    })
    .collect()
}

/// The text of every `TIMESTAMP`/`DATETIME` value in the hammerwork tables, to compare
/// before and after the conversion (the session time zone is UTC).
async fn value_snapshot(pool: &Pool<MySql>) -> Vec<String> {
    let columns = sqlx::query(
        "SELECT CAST(table_name AS CHAR) AS t, CAST(column_name AS CHAR) AS c
         FROM information_schema.columns
         WHERE table_schema = DATABASE() AND data_type IN ('timestamp', 'datetime')
           AND table_name <> 'hammerwork_migrations'
         ORDER BY table_name, ordinal_position",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let mut values = Vec::new();
    for column in columns {
        let table: String = column.get("t");
        let column: String = column.get("c");
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT CAST(DATE_FORMAT(`{column}`, '%Y-%m-%d %H:%i:%s.%f') AS CHAR) AS v
             FROM `{table}` ORDER BY 1"
        )))
        .fetch_all(pool)
        .await
        .unwrap();
        for row in rows {
            let value: Option<String> = row.get("v");
            values.push(format!("{table}.{column}={value:?}"));
        }
    }
    values
}

/// Run the pending migrations through a fresh manager.
async fn migrate(pool: &Pool<MySql>) {
    let runner = Box::new(MySqlMigrationRunner::new(pool.clone()));
    MigrationManager::new(runner)
        .run_migrations()
        .await
        .expect("migrations");
}

/// Normalise a default or `extra` for comparison: `CURRENT_TIMESTAMP` gains `(6)`.
fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .replace("current_timestamp(6)", "current_timestamp")
        .replace("current_timestamp()", "current_timestamp")
}

#[tokio::test]
#[ignore] // Requires a MySQL server on which the user may CREATE DATABASE
async fn test_mysql_migration_021_converts_timestamps_to_datetime() {
    let base = MySqlConnectOptions::from_str(&test_utils::mysql_url()).unwrap();
    let mut admin = MySqlConnection::connect_with(&base).await.unwrap();
    for sql in [
        format!("DROP DATABASE IF EXISTS `{SCRATCH_DATABASE}`"),
        format!("CREATE DATABASE `{SCRATCH_DATABASE}`"),
    ] {
        admin.execute(sqlx::AssertSqlSafe(sql)).await.unwrap();
    }

    // Run the checks in a task so the scratch database is dropped even if they panic.
    let options = base.clone().database(SCRATCH_DATABASE);
    let outcome = tokio::spawn(async move {
        let pool = MySqlPoolOptions::new()
            .max_connections(4)
            .connect_with(options.clone())
            .await
            .unwrap();

        // Build the 2.x schema: record 021 as applied so only 001..020 run.
        let runner = MySqlMigrationRunner::new(pool.clone());
        runner.create_migration_table().await.unwrap();
        runner
            .record_migration(
                &Migration::new(MIGRATION_ID, String::new(), 21),
                0,
            )
            .await
            .unwrap();
        migrate(&pool).await;
        let before_columns = column_snapshot(&pool).await;
        assert!(
            before_columns
                .iter()
                .any(|(_, _, data_type, _, _)| data_type == "timestamp"),
            "the 2.x schema must have TIMESTAMP columns"
        );

        // Data written by 2.x: through the library, and rows with values at both ends of
        // the TIMESTAMP range.
        let queue = JobQueue::new(pool.clone());
        let queue_name = "dt_upgrade";
        let early: DateTime<Utc> = "2024-03-05T10:11:12.345678Z".parse().unwrap();
        let mut scheduled = Job::new(queue_name.to_string(), json!({"n": 1}));
        scheduled.scheduled_at = early;
        scheduled.created_at = early;
        let scheduled_id = queue.enqueue(scheduled).await.unwrap();
        let job = queue.enqueue(Job::new(queue_name.to_string(), json!({"n": 2}))).await;
        let running_id = job.unwrap();
        let claimed = queue.dequeue_one(queue_name).await.unwrap().expect("a due job");
        assert!(claimed.id == running_id || claimed.id == scheduled_id);
        queue.pause_queue("dt_paused", Some("tester")).await.unwrap();
        for (id, created, scheduled, completed) in [
            ("00000000-0000-0000-0000-000000000001", "1970-01-01 00:00:01.000000", "2038-01-19 03:14:07.999999", None),
            ("00000000-0000-0000-0000-000000000002", "2001-09-09 01:46:40.000001", "2024-02-29 23:59:59.999999", Some("2025-01-01 00:00:00.500000")),
        ] {
            sqlx::query(
                "INSERT INTO hammerwork_jobs_archive
                 (id, queue_name, payload, status, created_at, scheduled_at, completed_at, archived_at)
                 VALUES (?, 'dt_archive', 'x', 'Completed', ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(created)
            .bind(scheduled)
            .bind(completed)
            .bind("2026-10-11 08:09:10.111213")
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO hammerwork_batches
             (id, batch_name, total_jobs, status, failure_mode, created_at, completed_at)
             VALUES ('00000000-0000-0000-0000-0000000000b1', 'b', 1, 'Completed', 'ContinueOnFailure',
                     '2022-12-31 23:59:59.999999', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO hammerwork_workflows (id, name, created_at, completed_at)
             VALUES ('00000000-0000-0000-0000-0000000000f1', 'old', '2023-04-05 06:07:08', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let values_before = value_snapshot(&pool).await;
        assert!(values_before.len() > 15, "{values_before:?}");

        // Run migration 021 from a session whose time zone is not UTC: the migration
        // must convert under UTC all the same.
        let offset_pool = MySqlPoolOptions::new()
            .max_connections(4)
            .connect_with(options.clone().timezone(Some("+05:00".to_string())))
            .await
            .unwrap();
        sqlx::query("DELETE FROM hammerwork_migrations WHERE migration_id = ?")
            .bind(MIGRATION_ID)
            .execute(&pool)
            .await
            .unwrap();
        migrate(&offset_pool).await;
        offset_pool.close().await;

        let after_columns = column_snapshot(&pool).await;
        assert_eq!(before_columns.len(), after_columns.len());
        let mut converted = 0;
        for (before, after) in before_columns.iter().zip(&after_columns) {
            assert_eq!((&before.0, &before.1), (&after.0, &after.1));
            if before.2 == "timestamp" || (after.0 == "hammerwork_workflows" && before.2 == "datetime") {
                converted += 1;
                assert_eq!(after.2, "datetime", "{} {}", after.0, after.1);
                assert_eq!(before.3, after.3, "nullability of {} {}", after.0, after.1);
                assert_eq!(
                    normalize(&before.4),
                    normalize(&after.4),
                    "default of {} {}",
                    after.0,
                    after.1
                );
            } else {
                assert_eq!(before, after);
            }
        }
        assert_eq!(converted, 42, "converted columns (39 TIMESTAMP + 3 workflow DATETIME)");
        assert!(after_columns.iter().all(|c| c.2 != "timestamp"));
        let precisions: Vec<i64> = sqlx::query_scalar(
            "SELECT CAST(datetime_precision AS SIGNED) FROM information_schema.columns
             WHERE table_schema = DATABASE() AND data_type = 'datetime'
",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(precisions.iter().all(|p| *p == 6), "{precisions:?}");
        println!("converted {converted} TIMESTAMP columns");

        assert_eq!(value_snapshot(&pool).await, values_before);

        // Workflow times keep microseconds when written after the migration
        sqlx::query(
            "INSERT INTO hammerwork_workflows (id, name, created_at, completed_at, failed_at)
             VALUES ('00000000-0000-0000-0000-0000000000f2', 'new',
                     '2100-01-02 03:04:05.678901', '2100-01-02 03:04:06.123456', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let workflow_times: Vec<String> = sqlx::query_scalar(
            "SELECT CONCAT_WS('|', DATE_FORMAT(created_at, '%Y-%m-%d %H:%i:%s.%f'),
                    DATE_FORMAT(completed_at, '%Y-%m-%d %H:%i:%s.%f'))
             FROM hammerwork_workflows ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            workflow_times,
            [
                "2023-04-05 06:07:08.000000".to_string(),
                "2100-01-02 03:04:05.678901|2100-01-02 03:04:06.123456".to_string()
            ]
        );
        sqlx::query("DELETE FROM hammerwork_workflows WHERE id LIKE '%f2'")
            .execute(&pool)
            .await
            .unwrap();

        // The library reads the converted rows, and a job beyond 2038 is stored
        let read = queue.get_job(scheduled_id).await.unwrap().unwrap();
        assert!(read.created_at == early || read.scheduled_at == early);
        let far: DateTime<Utc> = "2100-06-15T12:30:45.123456Z".parse().unwrap();
        let mut future = Job::new(queue_name.to_string(), json!({"n": 3}));
        future.scheduled_at = far;
        let future_id = queue.enqueue(future).await.unwrap();
        let stored = queue.get_job(future_id).await.unwrap().unwrap();
        assert_eq!(stored.scheduled_at, far);
        let scheduled_for_later: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM hammerwork_jobs WHERE scheduled_at > UTC_TIMESTAMP(6)
             AND queue_name = ? AND id = ?",
        )
        .bind(queue_name)
        .bind(future_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(scheduled_for_later, 1);
        // hammerwork_queue_pause.updated_at keeps its ON UPDATE clause
        assert!(queue.is_queue_paused("dt_paused").await.unwrap());
        sqlx::query("UPDATE hammerwork_queue_pause SET updated_at = '2020-01-01 00:00:00'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE hammerwork_queue_pause SET reason = 'changed'")
            .execute(&pool)
            .await
            .unwrap();
        let touched: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM hammerwork_queue_pause WHERE updated_at > '2025-01-01'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(touched, 1);

        // Running the migration again changes nothing
        sqlx::query("DELETE FROM hammerwork_migrations WHERE migration_id = ?")
            .bind(MIGRATION_ID)
            .execute(&pool)
            .await
            .unwrap();
        let values_after = value_snapshot(&pool).await;
        migrate(&pool).await;
        assert_eq!(column_snapshot(&pool).await, after_columns);
        assert_eq!(value_snapshot(&pool).await, values_after);

        pool.close().await;
    })
    .await;

    admin
        .execute(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS `{SCRATCH_DATABASE}`"
        )))
        .await
        .unwrap();
    outcome.unwrap();
}
