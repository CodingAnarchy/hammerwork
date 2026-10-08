//! Helpers for building parameterized SQL for both backends.
//!
//! Every value that originates outside the program (queue names, hours, limits, cutoff
//! timestamps) must reach the database as a bound parameter, never spliced into the SQL text.
//! [`SqlParams`] hands out the right placeholder for the backend (`$n` for PostgreSQL, `?` for
//! MySQL) and records the bind values in order, so the SQL text and its binds cannot drift
//! apart. Callers must request placeholders in the order they appear in the SQL text, which
//! is what building the statement left to right does naturally.

use crate::utils::database::DatabasePool;
use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::mysql::{MySql, MySqlArguments};
use sqlx::postgres::{PgArguments, Postgres};
use sqlx::query::Query;

/// Which SQL dialect a statement is built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Postgres,
    MySql,
}

impl DatabasePool {
    /// The dialect of this pool.
    pub fn backend(&self) -> Backend {
        match self {
            DatabasePool::Postgres(_) => Backend::Postgres,
            DatabasePool::MySQL(_) => Backend::MySql,
        }
    }
}

/// A value bound to a placeholder.
#[derive(Debug, Clone, PartialEq)]
pub enum Bind {
    Text(String),
    Int(i64),
    Time(DateTime<Utc>),
}

/// Time units supported by [`SqlParams::ago`]. A closed enum so the unit keyword in the SQL
/// text can never come from user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntervalUnit {
    Hour,
    Day,
}

/// Placeholder allocator and bind collector for one statement.
#[derive(Debug, Clone)]
pub struct SqlParams {
    backend: Backend,
    binds: Vec<Bind>,
}

impl SqlParams {
    pub fn new(backend: Backend) -> Self {
        Self {
            backend,
            binds: Vec::new(),
        }
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// Record `value` and return its placeholder.
    pub fn bind(&mut self, value: Bind) -> String {
        self.binds.push(value);
        match self.backend {
            Backend::Postgres => format!("${}", self.binds.len()),
            Backend::MySql => "?".to_string(),
        }
    }

    pub fn text(&mut self, value: &str) -> String {
        self.bind(Bind::Text(value.to_string()))
    }

    pub fn int(&mut self, value: i64) -> String {
        self.bind(Bind::Int(value))
    }

    pub fn time(&mut self, value: DateTime<Utc>) -> String {
        self.bind(Bind::Time(value))
    }

    /// `LIMIT <placeholder>`.
    pub fn limit(&mut self, limit: u32) -> String {
        format!("LIMIT {}", self.int(i64::from(limit)))
    }

    /// An expression for "`amount` `unit`s before now", with the amount bound.
    pub fn ago(&mut self, amount: u32, unit: IntervalUnit) -> String {
        let p = self.int(i64::from(amount));
        match (self.backend, unit) {
            (Backend::Postgres, IntervalUnit::Hour) => {
                format!("NOW() - make_interval(hours => {p}::int)")
            }
            (Backend::Postgres, IntervalUnit::Day) => {
                format!("NOW() - make_interval(days => {p}::int)")
            }
            (Backend::MySql, IntervalUnit::Hour) => format!("DATE_SUB(NOW(), INTERVAL {p} HOUR)"),
            (Backend::MySql, IntervalUnit::Day) => format!("DATE_SUB(NOW(), INTERVAL {p} DAY)"),
        }
    }

    pub fn binds(&self) -> &[Bind] {
        &self.binds
    }

    pub fn into_binds(self) -> Vec<Bind> {
        self.binds
    }
}

/// Apply `binds` in order to a PostgreSQL query.
pub fn bind_pg<'q>(
    mut query: Query<'q, Postgres, PgArguments>,
    binds: &'q [Bind],
) -> Query<'q, Postgres, PgArguments> {
    for bind in binds {
        query = match bind {
            Bind::Text(s) => query.bind(s.as_str()),
            Bind::Int(i) => query.bind(*i),
            Bind::Time(t) => query.bind(*t),
        };
    }
    query
}

/// Apply `binds` in order to a MySQL query.
pub fn bind_mysql<'q>(
    mut query: Query<'q, MySql, MySqlArguments>,
    binds: &'q [Bind],
) -> Query<'q, MySql, MySqlArguments> {
    for bind in binds {
        query = match bind {
            Bind::Text(s) => query.bind(s.as_str()),
            Bind::Int(i) => query.bind(*i),
            Bind::Time(t) => query.bind(*t),
        };
    }
    query
}

/// Run a statement returning one row with an integer column `column`.
pub async fn fetch_i64(
    pool: &DatabasePool,
    sql: &str,
    binds: &[Bind],
    column: &str,
) -> Result<i64> {
    match pool {
        DatabasePool::Postgres(p) => {
            let row = bind_pg(sqlx::query(sql), binds).fetch_one(p).await?;
            Ok(row.try_get(column)?)
        }
        DatabasePool::MySQL(p) => {
            let row = bind_mysql(sqlx::query(sql), binds).fetch_one(p).await?;
            Ok(row.try_get(column)?)
        }
    }
}

/// Run a write statement and return the number of affected rows.
pub async fn execute_binds(pool: &DatabasePool, sql: &str, binds: &[Bind]) -> Result<u64> {
    match pool {
        DatabasePool::Postgres(p) => Ok(bind_pg(sqlx::query(sql), binds)
            .execute(p)
            .await?
            .rows_affected()),
        DatabasePool::MySQL(p) => Ok(bind_mysql(sqlx::query(sql), binds)
            .execute(p)
            .await?
            .rows_affected()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::HOSTILE_QUEUE;

    #[test]
    fn postgres_placeholders_are_numbered_in_order() {
        let mut p = SqlParams::new(Backend::Postgres);
        assert_eq!(p.text("a"), "$1");
        assert_eq!(p.int(5), "$2");
        assert_eq!(p.limit(10), "LIMIT $3");
        assert_eq!(
            p.ago(2, IntervalUnit::Hour),
            "NOW() - make_interval(hours => $4::int)"
        );
        assert_eq!(p.binds().len(), 4);
        assert_eq!(p.binds()[0], Bind::Text("a".into()));
        assert_eq!(p.binds()[3], Bind::Int(2));
    }

    #[test]
    fn mysql_placeholders_are_question_marks() {
        let mut p = SqlParams::new(Backend::MySql);
        assert_eq!(p.text(HOSTILE_QUEUE), "?");
        assert_eq!(p.limit(10), "LIMIT ?");
        assert_eq!(
            p.ago(3, IntervalUnit::Day),
            "DATE_SUB(NOW(), INTERVAL ? DAY)"
        );
        assert_eq!(
            p.into_binds(),
            vec![
                Bind::Text(HOSTILE_QUEUE.into()),
                Bind::Int(10),
                Bind::Int(3)
            ]
        );
    }
}
