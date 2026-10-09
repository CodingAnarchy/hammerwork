//! `JobStatus` encoding and decoding through each database driver.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use hammerwork::JobStatus;

const ALL: [JobStatus; 8] = [
    JobStatus::Pending,
    JobStatus::Running,
    JobStatus::Completed,
    JobStatus::Failed,
    JobStatus::Dead,
    JobStatus::TimedOut,
    JobStatus::Retrying,
    JobStatus::Archived,
];

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_job_status_roundtrip() {
        let pool = sqlx::PgPool::connect(&test_utils::postgres_url())
            .await
            .unwrap();
        for status in ALL {
            let text: String = sqlx::query_scalar("SELECT $1::text")
                .bind(status)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(text, status.as_str());
            let decoded: JobStatus = sqlx::query_scalar("SELECT CAST($1 AS VARCHAR)")
                .bind(status.as_str())
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(decoded, status);
        }
        // Values written by old versions are JSON strings ("\"Pending\"").
        let decoded: JobStatus = sqlx::query_scalar("SELECT CAST('\"Pending\"' AS VARCHAR)")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(decoded, JobStatus::Pending);
        let err = sqlx::query_scalar::<_, JobStatus>("SELECT CAST('Exploded' AS VARCHAR)")
            .fetch_one(&pool)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Unknown job status: Exploded"),
            "{err}"
        );
    }
}

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_job_status_roundtrip() {
        let pool = sqlx::MySqlPool::connect(&test_utils::mysql_url())
            .await
            .unwrap();
        for status in ALL {
            let text: String = sqlx::query_scalar("SELECT CAST(? AS CHAR)")
                .bind(status)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(text, status.as_str());
            let decoded: JobStatus = sqlx::query_scalar("SELECT CAST(? AS CHAR)")
                .bind(status.as_str())
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(decoded, status);
        }
        let decoded: JobStatus = sqlx::query_scalar("SELECT CAST('\"Pending\"' AS CHAR)")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(decoded, JobStatus::Pending);
        let err = sqlx::query_scalar::<_, JobStatus>("SELECT CAST('Exploded' AS CHAR)")
            .fetch_one(&pool)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Unknown job status: Exploded"),
            "{err}"
        );
    }
}
