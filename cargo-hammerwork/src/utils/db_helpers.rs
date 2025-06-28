use anyhow::Result;
use sqlx::Row;
use crate::utils::database::DatabasePool;

// Helper function to execute count queries that return i64
pub async fn execute_count_query(pool: &DatabasePool, query: &str) -> Result<i64> {
    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let row = sqlx::query(query).fetch_one(pg_pool).await?;
            Ok(row.try_get("count")?)
        }
        DatabasePool::MySQL(mysql_pool) => {
            let row = sqlx::query(query).fetch_one(mysql_pool).await?;
            Ok(row.try_get("count")?)
        }
    }
}

// Helper function to execute update/delete queries that return rows affected
pub async fn execute_update_query(pool: &DatabasePool, query: &str) -> Result<u64> {
    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let result = sqlx::query(query).execute(pg_pool).await?;
            Ok(result.rows_affected())
        }
        DatabasePool::MySQL(mysql_pool) => {
            let result = sqlx::query(query).execute(mysql_pool).await?;
            Ok(result.rows_affected())
        }
    }
}

// Helper function to execute parameterized count queries
pub async fn execute_count_query_with_param(pool: &DatabasePool, query: &str, param: &str) -> Result<i64> {
    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let pg_query = query.replace("?", "$1");
            let row = sqlx::query(&pg_query).bind(param).fetch_one(pg_pool).await?;
            Ok(row.try_get("count")?)
        }
        DatabasePool::MySQL(mysql_pool) => {
            let row = sqlx::query(query).bind(param).fetch_one(mysql_pool).await?;
            Ok(row.try_get("count")?)
        }
    }
}

// Helper function to execute parameterized update queries
pub async fn execute_update_query_with_param(pool: &DatabasePool, query: &str, param: &str) -> Result<u64> {
    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let pg_query = query.replace("?", "$1");
            let result = sqlx::query(&pg_query).bind(param).execute(pg_pool).await?;
            Ok(result.rows_affected())
        }
        DatabasePool::MySQL(mysql_pool) => {
            let result = sqlx::query(query).bind(param).execute(mysql_pool).await?;
            Ok(result.rows_affected())
        }
    }
}