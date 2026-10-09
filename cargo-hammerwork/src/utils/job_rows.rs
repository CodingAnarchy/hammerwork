//! Whole rows of `hammerwork_jobs`, for `backup create` and `backup restore`.
//!
//! The columns are read from `information_schema`, so every column a migration adds is
//! backed up and restored without a list to keep in sync: encrypted payloads and their
//! encryption metadata, cron schedules, timeouts, retry strategies, dependencies,
//! workflows, results, tracing and retention all round-trip. A column whose type this
//! module does not know is an error, never silently dropped.
//!
//! Values are written in one encoding for both backends, so a backup taken from
//! PostgreSQL can be restored into MySQL and the other way round:
//!
//! | Column type                                   | Backup value                        |
//! |-----------------------------------------------|-------------------------------------|
//! | `uuid`, `char`/`varchar`/`text`               | string                              |
//! | integers                                      | number                              |
//! | `boolean`, MySQL `tinyint(1)`                 | `true` / `false`                    |
//! | `timestamptz`, `timestamp`, `datetime`        | RFC 3339 string, microseconds, UTC  |
//! | `jsonb`, `json`                               | the JSON value                      |
//! | `bytea`, `blob` (encrypted payloads)          | base64 string                       |
//! | PostgreSQL `uuid[]`, `text[]`                 | array of strings                    |
//!
//! SQL `NULL` is `null`.

use anyhow::{Result, anyhow, bail};
use base64::Engine;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value};
use sqlx::Row;
use uuid::Uuid;

use crate::utils::database::DatabasePool;
use crate::utils::sql::Backend;

/// How the values of a column are read, written to a backup, and bound back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    Uuid,
    Text,
    Int,
    Bool,
    Timestamp,
    Json,
    Bytes,
    UuidArray,
    TextArray,
}

impl ColumnKind {
    /// The kind of a PostgreSQL column, from its `information_schema` `udt_name`.
    pub fn postgres(udt_name: &str) -> Option<Self> {
        Some(match udt_name {
            "uuid" => Self::Uuid,
            "varchar" | "text" | "bpchar" => Self::Text,
            "int2" | "int4" | "int8" => Self::Int,
            "bool" => Self::Bool,
            "timestamptz" | "timestamp" => Self::Timestamp,
            "jsonb" | "json" => Self::Json,
            "bytea" => Self::Bytes,
            "_uuid" => Self::UuidArray,
            "_text" | "_varchar" => Self::TextArray,
            _ => return None,
        })
    }

    /// The kind of a MySQL column, from its `information_schema` `DATA_TYPE` and
    /// `COLUMN_TYPE`.
    pub fn mysql(data_type: &str, column_type: &str) -> Option<Self> {
        Some(match data_type {
            "tinyint" if column_type.starts_with("tinyint(1)") => Self::Bool,
            "tinyint" | "smallint" | "mediumint" | "int" | "bigint" => Self::Int,
            "char" | "varchar" | "tinytext" | "text" | "mediumtext" | "longtext" => Self::Text,
            "timestamp" | "datetime" => Self::Timestamp,
            "json" => Self::Json,
            "tinyblob" | "blob" | "mediumblob" | "longblob" | "binary" | "varbinary" => Self::Bytes,
            _ => return None,
        })
    }
}

/// A column of `hammerwork_jobs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub kind: ColumnKind,
}

/// The columns of `hammerwork_jobs`, in table order.
///
/// Fails on a column whose type backups cannot copy, so a column added by a future
/// migration is reported instead of being left out of backups.
pub async fn job_columns(pool: &DatabasePool) -> Result<Vec<Column>> {
    let described: Vec<(String, Option<ColumnKind>, String)> = match pool {
        DatabasePool::Postgres(p) => {
            let rows: Vec<(String, String)> = sqlx::query_as(
                "SELECT column_name::text, udt_name::text FROM information_schema.columns \
                 WHERE table_schema = current_schema() AND table_name = 'hammerwork_jobs' \
                 ORDER BY ordinal_position",
            )
            .fetch_all(p)
            .await?;
            rows.into_iter()
                .map(|(name, udt)| (name, ColumnKind::postgres(&udt), udt))
                .collect()
        }
        DatabasePool::MySQL(p) => {
            let rows: Vec<(String, String, String)> = sqlx::query_as(
                "SELECT CAST(COLUMN_NAME AS CHAR), CAST(DATA_TYPE AS CHAR), \
                 CAST(COLUMN_TYPE AS CHAR) FROM information_schema.columns \
                 WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'hammerwork_jobs' \
                 ORDER BY ORDINAL_POSITION",
            )
            .fetch_all(p)
            .await?;
            rows.into_iter()
                .map(|(name, data_type, column_type)| {
                    let kind = ColumnKind::mysql(&data_type, &column_type);
                    (name, kind, column_type)
                })
                .collect()
        }
    };
    if described.is_empty() {
        bail!("Table hammerwork_jobs not found; run `cargo hammerwork migration run` first");
    }
    described
        .into_iter()
        .map(|(name, kind, sql_type)| {
            let kind = kind.ok_or_else(|| {
                anyhow!(
                    "Column hammerwork_jobs.{} has type {}, which backups cannot copy yet",
                    name,
                    sql_type
                )
            })?;
            Ok(Column { name, kind })
        })
        .collect()
}

/// A column name quoted as an identifier.
fn quoted(backend: Backend, name: &str) -> String {
    match backend {
        Backend::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
        Backend::MySql => format!("`{}`", name.replace('`', "``")),
    }
}

/// The `SELECT` list reading `columns` in the form [`row_to_json`] decodes.
pub fn select_list(backend: Backend, columns: &[Column]) -> String {
    columns
        .iter()
        .map(|column| {
            let name = quoted(backend, &column.name);
            match (backend, column.kind) {
                (Backend::Postgres, ColumnKind::Int) => format!("{name}::bigint AS {name}"),
                (Backend::Postgres, ColumnKind::Timestamp) => {
                    format!("{name}::timestamptz AS {name}")
                }
                (Backend::MySql, ColumnKind::Int) => format!("CAST({name} AS SIGNED) AS {name}"),
                _ => name,
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn timestamp_value(t: Option<DateTime<Utc>>) -> Value {
    t.map_or(Value::Null, |t| {
        Value::String(t.to_rfc3339_opts(SecondsFormat::Micros, true))
    })
}

fn strings_value<T: ToString>(items: Option<Vec<T>>) -> Value {
    items.map_or(Value::Null, |items| {
        Value::Array(
            items
                .iter()
                .map(|item| Value::String(item.to_string()))
                .collect(),
        )
    })
}

fn bytes_value(bytes: Option<Vec<u8>>) -> Value {
    bytes.map_or(Value::Null, |bytes| {
        Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
    })
}

/// A PostgreSQL row selected with [`select_list`], as a backup object.
pub fn pg_row_to_json(
    row: &sqlx::postgres::PgRow,
    columns: &[Column],
) -> Result<Map<String, Value>> {
    let mut object = Map::new();
    for column in columns {
        let name = column.name.as_str();
        let value = match column.kind {
            ColumnKind::Uuid => row
                .try_get::<Option<Uuid>, _>(name)?
                .map_or(Value::Null, |id| Value::String(id.to_string())),
            ColumnKind::Text => row
                .try_get::<Option<String>, _>(name)?
                .map_or(Value::Null, Value::String),
            ColumnKind::Int => row
                .try_get::<Option<i64>, _>(name)?
                .map_or(Value::Null, Value::from),
            ColumnKind::Bool => row
                .try_get::<Option<bool>, _>(name)?
                .map_or(Value::Null, Value::Bool),
            ColumnKind::Timestamp => timestamp_value(row.try_get(name)?),
            ColumnKind::Json => row
                .try_get::<Option<Value>, _>(name)?
                .unwrap_or(Value::Null),
            ColumnKind::Bytes => bytes_value(row.try_get(name)?),
            ColumnKind::UuidArray => strings_value(row.try_get::<Option<Vec<Uuid>>, _>(name)?),
            ColumnKind::TextArray => strings_value(row.try_get::<Option<Vec<String>>, _>(name)?),
        };
        object.insert(column.name.clone(), value);
    }
    Ok(object)
}

/// A MySQL row selected with [`select_list`], as a backup object.
pub fn mysql_row_to_json(
    row: &sqlx::mysql::MySqlRow,
    columns: &[Column],
) -> Result<Map<String, Value>> {
    let mut object = Map::new();
    for column in columns {
        let name = column.name.as_str();
        let value = match column.kind {
            ColumnKind::Uuid | ColumnKind::Text => row
                .try_get::<Option<String>, _>(name)?
                .map_or(Value::Null, Value::String),
            ColumnKind::Int => row
                .try_get::<Option<i64>, _>(name)?
                .map_or(Value::Null, Value::from),
            ColumnKind::Bool => row
                .try_get::<Option<bool>, _>(name)?
                .map_or(Value::Null, Value::Bool),
            ColumnKind::Timestamp => timestamp_value(row.try_get(name)?),
            ColumnKind::Json | ColumnKind::UuidArray | ColumnKind::TextArray => row
                .try_get::<Option<Value>, _>(name)?
                .unwrap_or(Value::Null),
            ColumnKind::Bytes => bytes_value(row.try_get(name)?),
        };
        object.insert(column.name.clone(), value);
    }
    Ok(object)
}

/// A backup value converted for binding to a column.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    /// SQL `NULL`, typed for the column (PostgreSQL rejects untyped parameters).
    Null(ColumnKind),
    Uuid(Uuid),
    Text(String),
    Int(i64),
    Bool(bool),
    Time(DateTime<Utc>),
    Json(Value),
    Bytes(Vec<u8>),
    UuidArray(Vec<Uuid>),
    TextArray(Vec<String>),
}

impl Param {
    /// Convert the backup value of column `column` for binding (see the module docs for
    /// the encoding).
    pub fn from_backup(column: &Column, value: &Value) -> Result<Self> {
        let invalid = |expected: &str| {
            anyhow!(
                "backup column '{}' must be {}, got {}",
                column.name,
                expected,
                value
            )
        };
        let strings = |value: &Value| -> Result<Vec<String>> {
            value
                .as_array()
                .ok_or_else(|| invalid("an array of strings"))?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| invalid("an array of strings"))
                })
                .collect()
        };
        if value.is_null() {
            return Ok(Self::Null(column.kind));
        }
        Ok(match column.kind {
            ColumnKind::Uuid => {
                let text = value.as_str().ok_or_else(|| invalid("a UUID string"))?;
                Self::Uuid(Uuid::parse_str(text).map_err(|_| invalid("a UUID string"))?)
            }
            ColumnKind::Text => Self::Text(
                value
                    .as_str()
                    .ok_or_else(|| invalid("a string"))?
                    .to_string(),
            ),
            ColumnKind::Int => Self::Int(value.as_i64().ok_or_else(|| invalid("an integer"))?),
            ColumnKind::Bool => Self::Bool(match value {
                Value::Bool(b) => *b,
                Value::Number(n) if n.as_i64() == Some(0) => false,
                Value::Number(n) if n.as_i64() == Some(1) => true,
                _ => return Err(invalid("a boolean")),
            }),
            ColumnKind::Timestamp => {
                let text = value
                    .as_str()
                    .ok_or_else(|| invalid("an RFC 3339 timestamp"))?;
                Self::Time(
                    DateTime::parse_from_rfc3339(text)
                        .map_err(|_| invalid("an RFC 3339 timestamp"))?
                        .with_timezone(&Utc),
                )
            }
            ColumnKind::Json => Self::Json(value.clone()),
            ColumnKind::Bytes => {
                let text = value.as_str().ok_or_else(|| invalid("a base64 string"))?;
                Self::Bytes(
                    base64::engine::general_purpose::STANDARD
                        .decode(text)
                        .map_err(|_| invalid("a base64 string"))?,
                )
            }
            ColumnKind::UuidArray => Self::UuidArray(
                strings(value)?
                    .iter()
                    .map(|id| Uuid::parse_str(id).map_err(|_| invalid("an array of UUIDs")))
                    .collect::<Result<_>>()?,
            ),
            ColumnKind::TextArray => Self::TextArray(strings(value)?),
        })
    }
}

/// An `INSERT` of one backed-up job: the columns it sets and their values.
#[derive(Debug, Clone, PartialEq)]
pub struct RowInsert {
    pub id: String,
    pub columns: Vec<Column>,
    pub params: Vec<Param>,
}

impl RowInsert {
    /// The statement inserting this row; an existing row with the same id is left
    /// alone (PostgreSQL `ON CONFLICT (id) DO NOTHING`; on MySQL the duplicate key error
    /// is caught by [`insert_rows`]).
    pub fn sql(&self, backend: Backend) -> String {
        let names = self
            .columns
            .iter()
            .map(|column| quoted(backend, &column.name))
            .collect::<Vec<_>>()
            .join(", ");
        match backend {
            Backend::Postgres => {
                let placeholders = (1..=self.params.len())
                    .map(|i| format!("${i}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "INSERT INTO hammerwork_jobs ({names}) VALUES ({placeholders}) \
                     ON CONFLICT (id) DO NOTHING"
                )
            }
            Backend::MySql => {
                let placeholders = vec!["?"; self.params.len()].join(", ");
                format!("INSERT INTO hammerwork_jobs ({names}) VALUES ({placeholders})")
            }
        }
    }
}

fn bind_pg<'q>(
    mut query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    params: &'q [Param],
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    for param in params {
        query = match param {
            Param::Null(kind) => match kind {
                ColumnKind::Uuid => query.bind(None::<Uuid>),
                ColumnKind::Text => query.bind(None::<String>),
                ColumnKind::Int => query.bind(None::<i64>),
                ColumnKind::Bool => query.bind(None::<bool>),
                ColumnKind::Timestamp => query.bind(None::<DateTime<Utc>>),
                ColumnKind::Json => query.bind(None::<Value>),
                ColumnKind::Bytes => query.bind(None::<Vec<u8>>),
                ColumnKind::UuidArray => query.bind(None::<Vec<Uuid>>),
                ColumnKind::TextArray => query.bind(None::<Vec<String>>),
            },
            Param::Uuid(v) => query.bind(*v),
            Param::Text(v) => query.bind(v.as_str()),
            Param::Int(v) => query.bind(*v),
            Param::Bool(v) => query.bind(*v),
            Param::Time(v) => query.bind(*v),
            Param::Json(v) => query.bind(v),
            Param::Bytes(v) => query.bind(v.as_slice()),
            Param::UuidArray(v) => query.bind(v.as_slice()),
            Param::TextArray(v) => query.bind(v.as_slice()),
        };
    }
    query
}

fn bind_mysql<'q>(
    mut query: sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>,
    params: &'q [Param],
) -> sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments> {
    for param in params {
        query = match param {
            Param::Null(_) => query.bind(None::<String>),
            Param::Uuid(v) => query.bind(v.to_string()),
            Param::Text(v) => query.bind(v.as_str()),
            Param::Int(v) => query.bind(*v),
            Param::Bool(v) => query.bind(*v),
            Param::Time(v) => query.bind(*v),
            Param::Json(v) => query.bind(v),
            Param::Bytes(v) => query.bind(v.as_slice()),
            // MySQL stores these as JSON
            Param::UuidArray(v) => query.bind(Value::from(
                v.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            )),
            Param::TextArray(v) => query.bind(Value::from(v.clone())),
        };
    }
    query
}

/// Insert `rows` in one transaction. Returns how many were inserted; rows whose id
/// already exists are left untouched and not counted. On any other error nothing is
/// inserted.
pub async fn insert_rows(pool: &DatabasePool, rows: &[RowInsert]) -> Result<usize> {
    let mut inserted = 0;
    match pool {
        DatabasePool::Postgres(p) => {
            let mut tx = p.begin().await?;
            for row in rows {
                let sql = row.sql(Backend::Postgres);
                let affected = bind_pg(sqlx::query(&sql), &row.params)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| anyhow!("Cannot restore job {}: {}", row.id, e))?
                    .rows_affected();
                inserted += affected as usize;
            }
            tx.commit().await?;
        }
        DatabasePool::MySQL(p) => {
            let mut tx = p.begin().await?;
            for row in rows {
                let sql = row.sql(Backend::MySql);
                match bind_mysql(sqlx::query(&sql), &row.params)
                    .execute(&mut *tx)
                    .await
                {
                    Ok(result) => inserted += result.rows_affected() as usize,
                    // A failed statement does not end a MySQL transaction.
                    Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {}
                    Err(e) => return Err(anyhow!("Cannot restore job {}: {}", row.id, e)),
                }
            }
            tx.commit().await?;
        }
    }
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn column(name: &str, kind: ColumnKind) -> Column {
        Column {
            name: name.to_string(),
            kind,
        }
    }

    #[test]
    fn column_types_are_classified() {
        assert_eq!(ColumnKind::postgres("uuid"), Some(ColumnKind::Uuid));
        assert_eq!(ColumnKind::postgres("_uuid"), Some(ColumnKind::UuidArray));
        assert_eq!(ColumnKind::postgres("_text"), Some(ColumnKind::TextArray));
        assert_eq!(ColumnKind::postgres("bytea"), Some(ColumnKind::Bytes));
        assert_eq!(ColumnKind::postgres("int4"), Some(ColumnKind::Int));
        assert_eq!(ColumnKind::postgres("interval"), None);
        assert_eq!(
            ColumnKind::mysql("tinyint", "tinyint(1)"),
            Some(ColumnKind::Bool)
        );
        assert_eq!(
            ColumnKind::mysql("tinyint", "tinyint(4)"),
            Some(ColumnKind::Int)
        );
        assert_eq!(
            ColumnKind::mysql("longblob", "longblob"),
            Some(ColumnKind::Bytes)
        );
        assert_eq!(
            ColumnKind::mysql("char", "char(36)"),
            Some(ColumnKind::Text)
        );
        assert_eq!(ColumnKind::mysql("geometry", "geometry"), None);
    }

    #[test]
    fn select_list_casts_integers_and_quotes_names() {
        let columns = [
            column("id", ColumnKind::Uuid),
            column("attempts", ColumnKind::Int),
            column("created_at", ColumnKind::Timestamp),
        ];
        assert_eq!(
            select_list(Backend::Postgres, &columns),
            r#""id", "attempts"::bigint AS "attempts", "created_at"::timestamptz AS "created_at""#
        );
        assert_eq!(
            select_list(Backend::MySql, &columns),
            "`id`, CAST(`attempts` AS SIGNED) AS `attempts`, `created_at`"
        );
    }

    #[test]
    fn backup_values_convert_by_column_kind() {
        let id = "11111111-1111-1111-1111-111111111111";
        let cases = [
            (
                ColumnKind::Uuid,
                json!(id),
                Param::Uuid(Uuid::parse_str(id).unwrap()),
            ),
            (ColumnKind::Text, json!("q"), Param::Text("q".into())),
            (ColumnKind::Int, json!(7), Param::Int(7)),
            (ColumnKind::Bool, json!(true), Param::Bool(true)),
            (ColumnKind::Bool, json!(0), Param::Bool(false)),
            (
                ColumnKind::Timestamp,
                json!("2024-01-02T03:04:05.123456Z"),
                Param::Time(
                    DateTime::parse_from_rfc3339("2024-01-02T03:04:05.123456Z")
                        .unwrap()
                        .with_timezone(&Utc),
                ),
            ),
            (
                ColumnKind::Json,
                json!({"a": [1]}),
                Param::Json(json!({"a": [1]})),
            ),
            (
                ColumnKind::Bytes,
                json!("AAEC"),
                Param::Bytes(vec![0, 1, 2]),
            ),
            (
                ColumnKind::UuidArray,
                json!([id]),
                Param::UuidArray(vec![Uuid::parse_str(id).unwrap()]),
            ),
            (
                ColumnKind::TextArray,
                json!(["ssn"]),
                Param::TextArray(vec!["ssn".into()]),
            ),
            (
                ColumnKind::Bytes,
                Value::Null,
                Param::Null(ColumnKind::Bytes),
            ),
        ];
        for (kind, value, expected) in cases {
            assert_eq!(
                Param::from_backup(&column("c", kind), &value).unwrap(),
                expected,
                "{kind:?} {value}"
            );
        }

        for (kind, value) in [
            (ColumnKind::Uuid, json!("nope")),
            (ColumnKind::Text, json!(1)),
            (ColumnKind::Int, json!("1")),
            (ColumnKind::Bool, json!(2)),
            (ColumnKind::Timestamp, json!("yesterday")),
            (ColumnKind::Bytes, json!("not base64!")),
            (ColumnKind::UuidArray, json!(["x"])),
            (ColumnKind::TextArray, json!("ssn")),
        ] {
            let err = Param::from_backup(&column("c", kind), &value).unwrap_err();
            assert!(err.to_string().contains("backup column 'c'"), "{err}");
        }
    }

    #[test]
    fn timestamps_and_bytes_are_encoded_losslessly() {
        let t = DateTime::parse_from_rfc3339("2024-01-02T03:04:05.000007Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            timestamp_value(Some(t)),
            json!("2024-01-02T03:04:05.000007Z")
        );
        assert_eq!(timestamp_value(None), Value::Null);
        assert_eq!(bytes_value(Some(vec![0, 1, 2])), json!("AAEC"));
        assert_eq!(strings_value(Some(vec!["a", "b"])), json!(["a", "b"]));
    }

    #[test]
    fn insert_sql_names_every_column() {
        let row = RowInsert {
            id: "x".into(),
            columns: vec![
                column("id", ColumnKind::Uuid),
                column("payload", ColumnKind::Json),
            ],
            params: vec![Param::Null(ColumnKind::Uuid), Param::Json(json!({}))],
        };
        assert_eq!(
            row.sql(Backend::Postgres),
            r#"INSERT INTO hammerwork_jobs ("id", "payload") VALUES ($1, $2) ON CONFLICT (id) DO NOTHING"#
        );
        assert_eq!(
            row.sql(Backend::MySql),
            "INSERT INTO hammerwork_jobs (`id`, `payload`) VALUES (?, ?)"
        );
    }
}
