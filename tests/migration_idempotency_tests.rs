//! Migrations must be safe to re-run after a partial failure.
//!
//! MySQL DDL auto-commits, so a migration that fails part-way leaves its earlier statements
//! applied without being recorded in `hammerwork_migrations`; the next run then replays the
//! whole migration. Each test migrates a scratch database, forgets every recorded migration,
//! migrates again (which replays every migration over the finished schema) and checks that the
//! schema did not change.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::test_utils;
    use hammerwork::migrations::{MigrationManager, mysql::MySqlMigrationRunner};
    use sqlx::{
        Connection, Executor, MySql, MySqlConnection, Pool, Row,
        mysql::{MySqlConnectOptions, MySqlPoolOptions},
    };
    use std::str::FromStr;

    async fn snapshot(pool: &Pool<MySql>) -> Vec<String> {
        let queries = [
            "SELECT CAST(CONCAT_WS('|', 'column', table_name, ordinal_position, column_name,
                    column_type, is_nullable, IFNULL(column_default, '<null>'),
                    IFNULL(extra, ''), IFNULL(character_set_name, ''),
                    IFNULL(collation_name, '')) AS CHAR) AS s
             FROM information_schema.columns WHERE table_schema = DATABASE()",
            "SELECT CAST(CONCAT_WS('|', 'index', table_name, index_name, seq_in_index,
                    non_unique, IFNULL(column_name, '<expr>'), IFNULL(collation, ''),
                    index_type) AS CHAR) AS s
             FROM information_schema.statistics WHERE table_schema = DATABASE()",
            "SELECT CAST(CONCAT_WS('|', 'constraint', tc.table_name, tc.constraint_name,
                    tc.constraint_type, IFNULL(cc.check_clause, '')) AS CHAR) AS s
             FROM information_schema.table_constraints tc
             LEFT JOIN information_schema.check_constraints cc
               ON cc.constraint_schema = tc.constraint_schema
              AND cc.constraint_name = tc.constraint_name
             WHERE tc.table_schema = DATABASE()",
        ];
        let mut lines = Vec::new();
        for query in queries {
            for row in sqlx::query(query).fetch_all(pool).await.unwrap() {
                lines.push(row.get::<String, _>("s"));
            }
        }
        lines.sort();
        lines
    }

    #[tokio::test]
    #[ignore] // Requires a MySQL server on which the user may CREATE DATABASE
    async fn test_migrations_are_idempotent_mysql() {
        let base = MySqlConnectOptions::from_str(&test_utils::mysql_url()).unwrap();
        let scratch = format!("hw_idem_{}", uuid::Uuid::new_v4().simple());

        let mut admin = MySqlConnection::connect_with(&base).await.unwrap();
        admin
            .execute(format!("CREATE DATABASE `{scratch}`").as_str())
            .await
            .unwrap();

        // Run the checks in a task so the scratch database is dropped even if they panic.
        let options = base.clone().database(&scratch);
        let outcome = tokio::spawn(async move {
            let pool = MySqlPoolOptions::new()
                .max_connections(2)
                .connect_with(options)
                .await
                .unwrap();
            let migrate = || async {
                let runner = Box::new(MySqlMigrationRunner::new(pool.clone()));
                MigrationManager::new(runner).run_migrations().await
            };

            migrate().await.expect("first migration run");
            let first = snapshot(&pool).await;
            assert!(first.len() > 100, "snapshot looks empty: {}", first.len());

            sqlx::query("DELETE FROM hammerwork_migrations")
                .execute(&pool)
                .await
                .unwrap();
            migrate()
                .await
                .expect("re-running every migration must succeed");
            let second = snapshot(&pool).await;
            assert_eq!(first, second, "schema changed after re-running migrations");

            pool.close().await;
        })
        .await;

        admin
            .execute(format!("DROP DATABASE IF EXISTS `{scratch}`").as_str())
            .await
            .unwrap();
        outcome.unwrap();
    }
}

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::test_utils;
    use hammerwork::migrations::{MigrationManager, postgres::PostgresMigrationRunner};
    use sqlx::{
        Connection, Executor, PgConnection, Pool, Postgres, Row,
        postgres::{PgConnectOptions, PgPoolOptions},
    };
    use std::str::FromStr;

    async fn snapshot(pool: &Pool<Postgres>) -> Vec<String> {
        let queries = [
            "SELECT concat_ws('|', 'column', table_name, ordinal_position, column_name,
                    data_type, udt_name, is_nullable, coalesce(column_default, '<null>'),
                    coalesce(character_maximum_length::text, '')) AS s
             FROM information_schema.columns WHERE table_schema = 'public'",
            "SELECT concat_ws('|', 'index', tablename, indexname, indexdef) AS s
             FROM pg_indexes WHERE schemaname = 'public'",
            "SELECT concat_ws('|', 'constraint', conrelid::regclass::text, conname,
                    pg_get_constraintdef(oid)) AS s
             FROM pg_constraint
             WHERE connamespace = 'public'::regnamespace",
            "SELECT concat_ws('|', 'trigger', tgrelid::regclass::text, tgname) AS s
             FROM pg_trigger WHERE NOT tgisinternal",
        ];
        let mut lines = Vec::new();
        for query in queries {
            for row in sqlx::query(query).fetch_all(pool).await.unwrap() {
                lines.push(row.get::<String, _>("s"));
            }
        }
        lines.sort();
        lines
    }

    #[tokio::test]
    #[ignore] // Requires a PostgreSQL server on which the user may CREATE DATABASE
    async fn test_migrations_are_idempotent_postgres() {
        let base = PgConnectOptions::from_str(&test_utils::postgres_url()).unwrap();
        let scratch = format!("hw_idem_{}", uuid::Uuid::new_v4().simple());

        let mut admin = PgConnection::connect_with(&base).await.unwrap();
        admin
            .execute(format!("CREATE DATABASE \"{scratch}\"").as_str())
            .await
            .unwrap();

        let options = base.clone().database(&scratch);
        let outcome = tokio::spawn(async move {
            let pool = PgPoolOptions::new()
                .max_connections(2)
                .connect_with(options)
                .await
                .unwrap();
            let migrate = || async {
                let runner = Box::new(PostgresMigrationRunner::new(pool.clone()));
                MigrationManager::new(runner).run_migrations().await
            };

            migrate().await.expect("first migration run");
            let first = snapshot(&pool).await;
            assert!(first.len() > 100, "snapshot looks empty: {}", first.len());

            sqlx::query("DELETE FROM hammerwork_migrations")
                .execute(&pool)
                .await
                .unwrap();
            migrate()
                .await
                .expect("re-running every migration must succeed");
            let second = snapshot(&pool).await;
            assert_eq!(first, second, "schema changed after re-running migrations");

            pool.close().await;
        })
        .await;

        admin
            .execute(format!("DROP DATABASE IF EXISTS \"{scratch}\" WITH (FORCE)").as_str())
            .await
            .unwrap();
        outcome.unwrap();
    }
}
