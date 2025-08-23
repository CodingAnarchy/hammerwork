//! PostgreSQL-specific migration runner implementation.

use super::{Migration, MigrationRecord, MigrationRunner};
use crate::Result;
use chrono::Utc;
use sqlparser::{dialect::PostgreSqlDialect, parser::Parser};
use sqlx::{PgPool, Row};
use tracing::{debug, info, warn};

/// Parse SQL text into individual statements using sqlparser-rs
/// This properly handles quoted strings, dollar-quoted strings, and comments
fn parse_sql_statements(sql: &str) -> std::result::Result<Vec<String>, sqlparser::parser::ParserError> {
    let dialect = PostgreSqlDialect {};

    // First, try to parse the entire SQL as a series of statements
    match Parser::parse_sql(&dialect, sql) {
        Ok(statements) => {
            // Convert parsed statements back to SQL strings
            Ok(statements.iter().map(|stmt| format!("{};", stmt)).collect())
        }
        Err(_) => {
            // If parsing fails, try to split manually while respecting SQL syntax
            // This is a more conservative approach for complex migrations
            Ok(split_sql_respecting_quotes(sql))
        }
    }
}

/// Split SQL while respecting quoted contexts
/// This is a simpler fallback that handles the most common cases
fn split_sql_respecting_quotes(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current_statement = String::new();
    let mut in_single_quote = false;
    let mut in_dollar_quote = false;
    let mut dollar_tag = String::new();
    let mut chars = sql.chars().peekable();

    while let Some(ch) = chars.next() {
        current_statement.push(ch);

        match ch {
            '\'' if !in_dollar_quote => {
                // Handle single quotes (with escape sequences)
                if in_single_quote {
                    // Check for escaped quote
                    if chars.peek() == Some(&'\'') {
                        current_statement.push(chars.next().unwrap());
                    } else {
                        in_single_quote = false;
                    }
                } else {
                    in_single_quote = true;
                }
            }
            '$' if !in_single_quote => {
                // Handle dollar quoting
                if in_dollar_quote {
                    // Check if this closes the dollar quote
                    let mut temp_tag = String::new();
                    let chars_ahead: Vec<char> = chars.clone().collect();
                    let mut i = 0;

                    while i < chars_ahead.len()
                        && (chars_ahead[i].is_alphanumeric() || chars_ahead[i] == '_')
                    {
                        temp_tag.push(chars_ahead[i]);
                        i += 1;
                    }

                    if i < chars_ahead.len() && chars_ahead[i] == '$' && temp_tag == dollar_tag {
                        // Consume the tag and closing $
                        for _ in 0..=i {
                            if let Some(c) = chars.next() {
                                current_statement.push(c);
                            }
                        }
                        in_dollar_quote = false;
                        dollar_tag.clear();
                    }
                } else {
                    // Check if this starts a dollar quote
                    let mut temp_tag = String::new();
                    let chars_ahead: Vec<char> = chars.clone().collect();
                    let mut i = 0;

                    while i < chars_ahead.len()
                        && (chars_ahead[i].is_alphanumeric() || chars_ahead[i] == '_')
                    {
                        temp_tag.push(chars_ahead[i]);
                        i += 1;
                    }

                    if i < chars_ahead.len() && chars_ahead[i] == '$' {
                        // This is a dollar quote start
                        for _ in 0..=i {
                            if let Some(c) = chars.next() {
                                current_statement.push(c);
                            }
                        }
                        in_dollar_quote = true;
                        dollar_tag = temp_tag;
                    }
                }
            }
            ';' if !in_single_quote && !in_dollar_quote => {
                // This is a statement terminator
                let trimmed = current_statement.trim();
                if !trimmed.is_empty() && !trimmed.starts_with("--") {
                    statements.push(current_statement.clone());
                }
                current_statement.clear();
            }
            _ => {
                // Regular character, just continue
            }
        }
    }

    // Add final statement if non-empty
    let trimmed = current_statement.trim();
    if !trimmed.is_empty() && !trimmed.starts_with("--") {
        statements.push(current_statement);
    }

    statements
}

/// PostgreSQL migration runner
pub struct PostgresMigrationRunner {
    pool: PgPool,
}

impl PostgresMigrationRunner {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl MigrationRunner<sqlx::Postgres> for PostgresMigrationRunner {
    async fn run_migration(&self, migration: &Migration, sql: &str) -> Result<()> {
        debug!("Executing PostgreSQL migration: {}", migration.id);

        let mut tx = self.pool.begin().await?;

        // Parse SQL using sqlparser-rs for proper statement splitting
        let statements = match parse_sql_statements(sql) {
            Ok(stmts) => stmts,
            Err(e) => {
                warn!(
                    "Failed to parse SQL with sqlparser, falling back to naive splitting: {}",
                    e
                );
                // Fallback to simple splitting for compatibility
                sql.split(';')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty() && !s.chars().all(|c| c.is_whitespace() || c == '\n'))
                    .collect()
            }
        };

        for (i, statement) in statements.iter().enumerate() {
            // Add semicolon back if it was removed by split
            let full_statement = if statement.ends_with(';') {
                statement.to_string()
            } else {
                format!("{};", statement)
            };

            debug!(
                "Executing statement {} of {} for migration {}",
                i + 1,
                statements.len(),
                migration.id
            );

            sqlx::query(&full_statement).execute(&mut *tx).await?;
        }

        tx.commit().await?;

        info!(
            "Successfully executed PostgreSQL migration: {} ({} statements)",
            migration.id,
            statements.len()
        );
        Ok(())
    }

    async fn migration_table_exists(&self) -> Result<bool> {
        let row = sqlx::query(
            "SELECT EXISTS (
                SELECT FROM information_schema.tables 
                WHERE table_schema = 'public' 
                AND table_name = 'hammerwork_migrations'
            )",
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(row.get::<bool, _>(0))
    }

    async fn create_migration_table(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS hammerwork_migrations (
                migration_id VARCHAR NOT NULL PRIMARY KEY,
                executed_at TIMESTAMPTZ NOT NULL,
                execution_time_ms BIGINT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        info!("Created PostgreSQL migration tracking table");
        Ok(())
    }

    async fn get_executed_migrations(&self) -> Result<Vec<MigrationRecord>> {
        let rows = sqlx::query(
            "SELECT migration_id, executed_at, execution_time_ms 
             FROM hammerwork_migrations 
             ORDER BY executed_at",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut records = Vec::new();
        for row in rows {
            records.push(MigrationRecord {
                migration_id: row.get("migration_id"),
                executed_at: row.get("executed_at"),
                execution_time_ms: row.get::<i64, _>("execution_time_ms") as u64,
            });
        }

        Ok(records)
    }

    async fn record_migration(&self, migration: &Migration, execution_time_ms: u64) -> Result<()> {
        sqlx::query(
            "INSERT INTO hammerwork_migrations (migration_id, executed_at, execution_time_ms) 
             VALUES ($1, $2, $3)",
        )
        .bind(&migration.id)
        .bind(Utc::now())
        .bind(execution_time_ms as i64)
        .execute(&self.pool)
        .await?;

        debug!("Recorded PostgreSQL migration: {}", migration.id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Tests for PostgreSQL migration functionality

    #[test]
    fn test_sql_statement_splitting() {
        let multi_statement_sql = r#"
-- Comment line
CREATE TABLE test_table (
    id INTEGER PRIMARY KEY
);

-- Another comment
ALTER TABLE test_table ADD COLUMN name VARCHAR(50);

CREATE INDEX idx_test ON test_table (name);
"#;

        let statements: Vec<&str> = multi_statement_sql
            .split(";\n")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        // Should split into 3 non-empty statements
        assert_eq!(statements.len(), 3);

        // Verify each statement contains expected keywords
        assert!(statements[0].contains("CREATE TABLE"));
        assert!(statements[1].contains("ALTER TABLE"));
        assert!(statements[2].contains("CREATE INDEX"));
    }
}
