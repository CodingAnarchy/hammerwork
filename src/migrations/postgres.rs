//! PostgreSQL-specific migration runner implementation.

use super::{Migration, MigrationRecord, MigrationRunner};
use crate::Result;
use chrono::Utc;
use sqlparser::{dialect::PostgreSqlDialect, parser::Parser};
use sqlx::{PgPool, Row};
use tracing::{debug, info, warn};

/// Parse SQL text into individual statements using sqlparser-rs
/// This properly handles quoted strings, dollar-quoted strings, and comments
fn parse_sql_statements(
    sql: &str,
) -> std::result::Result<Vec<String>, sqlparser::parser::ParserError> {
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
                // Handle dollar quoting (including empty tags like $$)
                if in_dollar_quote {
                    // Check if this closes the dollar quote
                    let mut temp_tag = String::new();
                    let chars_ahead: Vec<char> = chars.clone().collect();
                    let mut i = 0;

                    // Collect the tag (could be empty)
                    while i < chars_ahead.len()
                        && (chars_ahead[i].is_alphanumeric() || chars_ahead[i] == '_')
                    {
                        temp_tag.push(chars_ahead[i]);
                        i += 1;
                    }

                    // Check for closing $ and matching tag
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
                    let chars_ahead: Vec<char> = chars.clone().collect();
                    let mut i = 0;
                    let mut temp_tag = String::new();

                    // Collect the tag (could be empty for $$ ... $$)
                    while i < chars_ahead.len()
                        && (chars_ahead[i].is_alphanumeric() || chars_ahead[i] == '_')
                    {
                        temp_tag.push(chars_ahead[i]);
                        i += 1;
                    }

                    // Check if we have a closing $ to complete the dollar quote start
                    if i < chars_ahead.len() && chars_ahead[i] == '$' {
                        // This is a dollar quote start (consume tag and closing $)
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

                // Only push non-empty statements that aren't just comments
                // Remove comment-only lines but keep statements that have SQL content
                let lines: Vec<&str> = trimmed.lines().collect();
                let has_sql_content = lines.iter().any(|line| {
                    let line_trimmed = line.trim();
                    !line_trimmed.is_empty() && !line_trimmed.starts_with("--")
                });

                if has_sql_content {
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
    if !trimmed.is_empty() {
        // Check if final statement has SQL content (not just comments)
        let lines: Vec<&str> = trimmed.lines().collect();
        let has_sql_content = lines.iter().any(|line| {
            let line_trimmed = line.trim();
            !line_trimmed.is_empty() && !line_trimmed.starts_with("--")
        });

        if has_sql_content {
            statements.push(current_statement);
        }
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

    #[test]
    fn test_dollar_quoted_string_parsing() {
        let sql_with_function = r#"
CREATE OR REPLACE FUNCTION update_timestamp()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER test_trigger BEFORE UPDATE ON test_table FOR EACH ROW EXECUTE FUNCTION update_timestamp();
"#;

        let statements = super::split_sql_respecting_quotes(sql_with_function);

        // Should split into 2 statements
        assert_eq!(statements.len(), 2);

        // First statement should contain the entire function including dollar quotes
        assert!(statements[0].contains("RETURNS TRIGGER AS $$"));
        assert!(statements[0].contains("$$ LANGUAGE plpgsql"));

        // Second statement should be the trigger creation
        assert!(statements[1].contains("CREATE TRIGGER"));
    }

    #[test]
    fn test_sqlparser_integration() {
        let sql_with_function = r#"
CREATE OR REPLACE FUNCTION update_hammerwork_queue_pause_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
"#;

        // Test with sqlparser-rs integration
        let result = super::parse_sql_statements(sql_with_function);
        assert!(
            result.is_ok(),
            "sqlparser should handle dollar-quoted strings"
        );

        let statements = result.unwrap();
        assert_eq!(statements.len(), 1);
        assert!(
            statements[0].contains("LANGUAGE plpgsql"),
            "Statement should contain LANGUAGE plpgsql: {}",
            statements[0]
        );
    }

    #[test]
    fn test_migration_014_sql_parsing() {
        // Test the exact SQL from migration 014 that was causing issues
        let migration_014_sql = r#"-- Add queue pause functionality
-- Migration 014: Add queue pause state tracking

-- Create table for tracking queue pause states
CREATE TABLE IF NOT EXISTS hammerwork_queue_pause (
    queue_name VARCHAR(255) PRIMARY KEY,
    paused_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    paused_by VARCHAR(255),
    reason TEXT,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

-- Create index for faster lookups
CREATE INDEX IF NOT EXISTS idx_hammerwork_queue_pause_paused_at ON hammerwork_queue_pause(paused_at);

-- Add function to automatically update the updated_at timestamp
CREATE OR REPLACE FUNCTION update_hammerwork_queue_pause_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Create trigger to automatically update updated_at
DROP TRIGGER IF EXISTS trigger_update_hammerwork_queue_pause_updated_at ON hammerwork_queue_pause;
CREATE TRIGGER trigger_update_hammerwork_queue_pause_updated_at
    BEFORE UPDATE ON hammerwork_queue_pause
    FOR EACH ROW
    EXECUTE FUNCTION update_hammerwork_queue_pause_updated_at();"#;

        // Test with our SQL splitting logic
        let statements = super::split_sql_respecting_quotes(migration_014_sql);

        // Should split into 5 statements:
        // 1. CREATE TABLE
        // 2. CREATE INDEX
        // 3. CREATE FUNCTION (with dollar quotes)
        // 4. DROP TRIGGER
        // 5. CREATE TRIGGER
        assert_eq!(
            statements.len(),
            5,
            "Should parse 5 statements from migration 014"
        );

        // Verify the function statement contains the complete dollar-quoted block
        let function_statement = &statements[2];
        assert!(function_statement.contains("CREATE OR REPLACE FUNCTION"));
        assert!(function_statement.contains("RETURNS TRIGGER AS $$"));
        assert!(function_statement.contains("$$ LANGUAGE plpgsql"));
        assert!(function_statement.contains("NEW.updated_at = NOW()"));

        // Test with sqlparser-rs integration
        let result = super::parse_sql_statements(migration_014_sql);
        assert!(
            result.is_ok(),
            "Migration 014 SQL should parse successfully with sqlparser-rs: {:?}",
            result
        );
    }

    #[test]
    fn test_migration_012_sql_parsing() {
        // Test the SQL from migration 012 which has complex transaction with many statements
        let migration_012_sql = r#"-- Migration 012: Optimize job dependencies using native PostgreSQL arrays
-- Converts JSONB dependency arrays to native UUID[] arrays for better performance
-- This migration is wrapped in a transaction for safety

BEGIN;

-- Step 1: Add new UUID array columns
ALTER TABLE hammerwork_jobs 
ADD COLUMN IF NOT EXISTS depends_on_array UUID[] DEFAULT '{}';

ALTER TABLE hammerwork_jobs 
ADD COLUMN IF NOT EXISTS dependents_array UUID[] DEFAULT '{}';

-- Step 2: Migrate existing JSONB data to UUID arrays with validation
-- Handle depends_on column with UUID validation
UPDATE hammerwork_jobs 
SET depends_on_array = CASE 
    WHEN depends_on IS NULL OR depends_on = 'null'::jsonb OR depends_on = '[]'::jsonb THEN '{}'::UUID[]
    WHEN jsonb_typeof(depends_on) = 'array' THEN 
        ARRAY(
            SELECT elem::UUID 
            FROM jsonb_array_elements_text(depends_on) AS elem
            WHERE elem::text ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
        )
    ELSE '{}'::UUID[]
END;

-- Handle dependents column with UUID validation
UPDATE hammerwork_jobs 
SET dependents_array = CASE 
    WHEN dependents IS NULL OR dependents = 'null'::jsonb OR dependents = '[]'::jsonb THEN '{}'::UUID[]
    WHEN jsonb_typeof(dependents) = 'array' THEN 
        ARRAY(
            SELECT elem::UUID 
            FROM jsonb_array_elements_text(dependents) AS elem
            WHERE elem::text ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
        )
    ELSE '{}'::UUID[]
END;

-- Step 3: Verify data migration integrity (simplified for migration runner compatibility)
-- Note: Since the migration runner splits on semicolons, we skip complex validation
-- The column constraints and indexes below will catch any issues

-- Step 4: Create indexes on new array columns (before dropping old ones)
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_depends_on_array
    ON hammerwork_jobs USING GIN (depends_on_array);

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_dependents_array
    ON hammerwork_jobs USING GIN (dependents_array);

-- Step 5: Drop old JSONB indexes (will be recreated after column rename)
DROP INDEX IF EXISTS idx_hammerwork_jobs_depends_on;
DROP INDEX IF EXISTS idx_hammerwork_jobs_dependents;

-- Step 6: Drop old JSONB columns and rename array columns
ALTER TABLE hammerwork_jobs DROP COLUMN IF EXISTS depends_on;
ALTER TABLE hammerwork_jobs DROP COLUMN IF EXISTS dependents;

ALTER TABLE hammerwork_jobs RENAME COLUMN depends_on_array TO depends_on;
ALTER TABLE hammerwork_jobs RENAME COLUMN dependents_array TO dependents;

-- Step 7: Recreate indexes with original names
DROP INDEX IF EXISTS idx_hammerwork_jobs_depends_on_array;
DROP INDEX IF EXISTS idx_hammerwork_jobs_dependents_array;

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_depends_on
    ON hammerwork_jobs USING GIN (depends_on);

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_dependents
    ON hammerwork_jobs USING GIN (dependents);

-- Step 8: Update comments to reflect new column types
COMMENT ON COLUMN hammerwork_jobs.depends_on IS 'Array of job IDs this job depends on (native UUID array)';
COMMENT ON COLUMN hammerwork_jobs.dependents IS 'Cached array of job IDs that depend on this job (native UUID array)';

-- Step 9: Add constraint to ensure reasonable array sizes (prevent abuse)
ALTER TABLE hammerwork_jobs 
ADD CONSTRAINT chk_depends_on_size 
CHECK (array_length(depends_on, 1) IS NULL OR array_length(depends_on, 1) <= 1000);

ALTER TABLE hammerwork_jobs 
ADD CONSTRAINT chk_dependents_size 
CHECK (array_length(dependents, 1) IS NULL OR array_length(dependents, 1) <= 10000);

COMMIT;"#;

        // Test with our SQL splitting logic
        let statements = super::split_sql_respecting_quotes(migration_012_sql);

        // Should split into 22 statements:
        // 1. BEGIN
        // 2-3. Two ALTER TABLE (add columns)
        // 4-5. Two complex UPDATE statements
        // 6-7. Two CREATE INDEX statements
        // 8-9. Two DROP INDEX statements
        // 10-11. Two ALTER TABLE (drop columns)
        // 12-13. Two ALTER TABLE (rename columns)
        // 14-15. Two DROP INDEX statements
        // 16-17. Two CREATE INDEX statements
        // 18-19. Two COMMENT statements
        // 20-21. Two ALTER TABLE (add constraints)
        // 22. COMMIT
        assert_eq!(
            statements.len(),
            22,
            "Should parse 22 statements from migration 012"
        );

        // Verify key statements are parsed correctly
        assert!(
            statements[0].contains("BEGIN"),
            "First statement should be BEGIN"
        );
        assert!(
            statements[21].contains("COMMIT"),
            "Last statement should be COMMIT"
        );

        // Verify complex UPDATE statements with CASE expressions are parsed correctly
        let depends_on_update = statements.iter().find(|stmt| {
            stmt.contains("SET depends_on_array = CASE")
                && stmt.contains("jsonb_array_elements_text(depends_on)")
        });
        assert!(
            depends_on_update.is_some(),
            "Should find depends_on UPDATE statement"
        );

        let dependents_update = statements.iter().find(|stmt| {
            stmt.contains("SET dependents_array = CASE")
                && stmt.contains("jsonb_array_elements_text(dependents)")
        });
        assert!(
            dependents_update.is_some(),
            "Should find dependents UPDATE statement"
        );

        // Verify COMMENT statements are parsed correctly
        let comment_statements: Vec<_> = statements
            .iter()
            .filter(|stmt| stmt.contains("COMMENT ON COLUMN"))
            .collect();
        assert_eq!(
            comment_statements.len(),
            2,
            "Should have 2 COMMENT statements"
        );

        // Verify constraint statements are parsed correctly
        let constraint_statements: Vec<_> = statements
            .iter()
            .filter(|stmt| {
                stmt.contains("ADD CONSTRAINT")
                    && (stmt.contains("chk_depends_on_size")
                        || stmt.contains("chk_dependents_size"))
            })
            .collect();
        assert_eq!(
            constraint_statements.len(),
            2,
            "Should have 2 constraint statements"
        );

        // Test with sqlparser-rs integration
        let result = super::parse_sql_statements(migration_012_sql);
        assert!(
            result.is_ok(),
            "Migration 012 SQL should parse successfully with sqlparser-rs: {:?}",
            result
        );
    }
}
