//! Test utilities for setting up databases with migrations

#[cfg(feature = "postgres")]
pub async fn setup_postgres_queue() -> Arc<JobQueue<sqlx::Postgres>> {
    use hammerwork::migrations::postgres::PostgresMigrationRunner;
    use sqlx::{Pool, Postgres};

    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgres://postgres:hammerwork@localhost:5433/hammerwork_test".to_string()
    });

    let pool = Pool::<Postgres>::connect(&database_url)
        .await
        .expect("Failed to connect to Postgres");

    let queue = Arc::new(JobQueue::new(pool.clone()));

    // Run migrations to set up tables - the migration system handles duplicates gracefully
    let runner = Box::new(PostgresMigrationRunner::new(pool));
    let manager = MigrationManager::new(runner);
    manager
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    queue
}

#[cfg(feature = "mysql")]
#[allow(dead_code)]
pub async fn setup_mysql_queue() -> Arc<JobQueue<sqlx::MySql>> {
    use hammerwork::migrations::mysql::MySqlMigrationRunner;
    use sqlx::{MySql, Pool};

    let database_url = std::env::var("MYSQL_DATABASE_URL")
        .unwrap_or_else(|_| "mysql://root:hammerwork@localhost:3307/hammerwork_test".to_string());

    let pool = Pool::<MySql>::connect(&database_url)
        .await
        .expect("Failed to connect to MySQL");

    let queue = Arc::new(JobQueue::new(pool.clone()));

    // Run migrations to set up tables - the migration system handles duplicates gracefully
    let runner = Box::new(MySqlMigrationRunner::new(pool));
    let manager = MigrationManager::new(runner);
    manager
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    queue
}
