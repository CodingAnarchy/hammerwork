//! `cargo hammerwork encryption`: encryption key operations.
//!
//! `encryption audit` reads the key audit log (`hammerwork_key_audit_log`), where the
//! library's `KeyManager` records key creation, access and rotation. It reads the table
//! directly, so it needs no master key.

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use clap::Subcommand;
use hammerwork::encryption::{KeyOperation, parse_key_operation};
use serde::Serialize;
use sqlx::Row;

use crate::config::Config;
use crate::utils::database::DatabasePool;
use crate::utils::sql::{Backend, Bind, SqlParams, bind_mysql, bind_pg};

#[derive(Subcommand)]
pub enum EncryptionCommand {
    #[command(
        about = "Show the key audit log (key creation, access and rotation)",
        long_about = "Show the records of the key audit log (hammerwork_key_audit_log), \
            newest first. The KeyManager records key creation, access and rotation there \
            when auditing is enabled. Filter by key, operation, outcome and time; page \
            with --limit and --offset. Needs no encryption key.\n\n\
            Examples:\n  \
            cargo hammerwork encryption audit --key-id payment-key --since 7d\n  \
            cargo hammerwork encryption audit --operation rotate --failed --format json"
    )]
    Audit {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'k', long, help = "Only records of this key")]
        key_id: Option<String>,
        #[arg(
            short = 'o',
            long,
            value_parser = parse_operation,
            help = "Only this operation: create, access, rotate, retire, revoke, delete or update"
        )]
        operation: Option<KeyOperation>,
        #[arg(
            long,
            value_parser = parse_time,
            help = "Only records at or after this time: an RFC 3339 timestamp \
                    (2026-10-01T00:00:00Z) or a time ago (30m, 24h, 7d)"
        )]
        since: Option<DateTime<Utc>>,
        #[arg(
            long,
            value_parser = parse_time,
            help = "Only records before this time (same forms as --since)"
        )]
        until: Option<DateTime<Utc>>,
        #[arg(long, conflicts_with = "failed", help = "Only successful operations")]
        success: bool,
        #[arg(long, help = "Only failed operations")]
        failed: bool,
        #[arg(
            short = 'l',
            long,
            default_value = "50",
            help = "Maximum number of records"
        )]
        limit: u32,
        #[arg(
            long,
            default_value = "0",
            help = "Skip this many records (newest first), for paging"
        )]
        offset: u32,
        #[arg(
            long,
            default_value = "table",
            value_parser = ["table", "json"],
            help = "Output format"
        )]
        format: String,
    },
}

impl EncryptionCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        let db_url = self.get_database_url(config)?;
        let pool = DatabasePool::connect_with_config(&db_url, config).await?;
        match self {
            EncryptionCommand::Audit { format, .. } => {
                let filter = self.audit_filter();
                let records = read_audit_log(&pool, &filter).await?;
                println!("{}", render_audit_log(&records, &filter, format)?);
            }
        }
        Ok(())
    }

    fn get_database_url(&self, config: &Config) -> Result<String> {
        let EncryptionCommand::Audit { database_url, .. } = self;
        database_url
            .as_deref()
            .or(config.get_database_url())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("Database URL is required"))
    }

    /// The filter described by the `audit` flags.
    fn audit_filter(&self) -> AuditFilter {
        let EncryptionCommand::Audit {
            key_id,
            operation,
            since,
            until,
            success,
            failed,
            limit,
            offset,
            ..
        } = self;
        AuditFilter {
            key_id: key_id.clone(),
            operation: *operation,
            since: *since,
            until: *until,
            success: match (success, failed) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            },
            limit: *limit,
            offset: *offset,
        }
    }
}

/// Parse `--operation` (case-insensitive operation name).
fn parse_operation(value: &str) -> std::result::Result<KeyOperation, String> {
    parse_key_operation(value).map_err(|_| {
        format!(
            "unknown operation '{value}'; expected one of: {}",
            KeyOperation::ALL
                .iter()
                .map(|op| op.to_string().to_lowercase())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

/// Parse `--since` / `--until`: an RFC 3339 timestamp, or a time ago such as `30m`,
/// `24h`, `7d` or `2w` (also `s`).
pub fn parse_time(value: &str) -> std::result::Result<DateTime<Utc>, String> {
    parse_time_at(value, Utc::now())
}

fn parse_time_at(value: &str, now: DateTime<Utc>) -> std::result::Result<DateTime<Utc>, String> {
    let value = value.trim();
    if let Ok(time) = DateTime::parse_from_rfc3339(value) {
        return Ok(time.with_timezone(&Utc));
    }
    let invalid = || {
        format!(
            "invalid time '{value}': expected an RFC 3339 timestamp \
             (2026-10-01T00:00:00Z) or a time ago (30m, 24h, 7d)"
        )
    };
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(invalid)?;
    let (amount, unit) = value.split_at(split);
    let amount: i64 = amount.parse().map_err(|_| invalid())?;
    let ago = match unit {
        "s" => chrono::Duration::try_seconds(amount),
        "m" => chrono::Duration::try_minutes(amount),
        "h" => chrono::Duration::try_hours(amount),
        "d" => chrono::Duration::try_days(amount),
        "w" => chrono::Duration::try_weeks(amount),
        _ => None,
    }
    .ok_or_else(invalid)?;
    now.checked_sub_signed(ago).ok_or_else(invalid)
}

/// The `audit` filter criteria. Unset criteria match everything.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AuditFilter {
    pub key_id: Option<String>,
    pub operation: Option<KeyOperation>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub success: Option<bool>,
    pub limit: u32,
    pub offset: u32,
}

/// One audit log record.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditRecord {
    pub id: String,
    pub key_id: String,
    pub operation: String,
    pub success: bool,
    pub error_message: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub user_id: Option<String>,
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
    pub session_id: Option<String>,
}

/// The audit log query for `filter`: every value is a bound parameter.
pub fn build_audit_query(backend: Backend, filter: &AuditFilter) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let columns = match backend {
        Backend::Postgres => {
            "CAST(id AS TEXT) AS id, key_id, operation, success, error_message, timestamp, \
             user_id, host(client_ip) AS client_ip, user_agent, session_id"
        }
        Backend::MySql => {
            "id, key_id, operation, success, error_message, timestamp, user_id, client_ip, \
             user_agent, session_id"
        }
    };
    let mut sql = format!("SELECT {columns} FROM hammerwork_key_audit_log WHERE 1 = 1");
    if let Some(key_id) = &filter.key_id {
        sql.push_str(&format!(" AND key_id = {}", params.text(key_id)));
    }
    if let Some(operation) = filter.operation {
        sql.push_str(&format!(
            " AND operation = {}",
            params.text(&operation.to_string())
        ));
    }
    if let Some(since) = filter.since {
        sql.push_str(&format!(" AND timestamp >= {}", params.time(since)));
    }
    if let Some(until) = filter.until {
        sql.push_str(&format!(" AND timestamp < {}", params.time(until)));
    }
    if let Some(success) = filter.success {
        sql.push_str(&format!(" AND success = {}", params.boolean(success)));
    }
    sql.push_str(" ORDER BY timestamp DESC, id DESC ");
    sql.push_str(&params.limit(filter.limit));
    sql.push_str(&format!(" OFFSET {}", params.int(i64::from(filter.offset))));
    (sql, params.into_binds())
}

/// Read the audit records matching `filter`, newest first.
pub async fn read_audit_log(pool: &DatabasePool, filter: &AuditFilter) -> Result<Vec<AuditRecord>> {
    let (sql, binds) = build_audit_query(pool.backend(), filter);
    match pool {
        DatabasePool::Postgres(p) => bind_pg(sqlx::query(&sql), &binds)
            .fetch_all(p)
            .await?
            .iter()
            .map(|row| {
                Ok(AuditRecord {
                    id: row.try_get("id")?,
                    key_id: row.try_get("key_id")?,
                    operation: row.try_get("operation")?,
                    success: row.try_get("success")?,
                    error_message: row.try_get("error_message")?,
                    timestamp: row.try_get("timestamp")?,
                    user_id: row.try_get("user_id")?,
                    client_ip: row.try_get("client_ip")?,
                    user_agent: row.try_get("user_agent")?,
                    session_id: row.try_get("session_id")?,
                })
            })
            .collect(),
        DatabasePool::MySQL(p) => bind_mysql(sqlx::query(&sql), &binds)
            .fetch_all(p)
            .await?
            .iter()
            .map(|row| {
                Ok(AuditRecord {
                    id: row.try_get("id")?,
                    key_id: row.try_get("key_id")?,
                    operation: row.try_get("operation")?,
                    success: row.try_get("success")?,
                    error_message: row.try_get("error_message")?,
                    timestamp: row.try_get("timestamp")?,
                    user_id: row.try_get("user_id")?,
                    client_ip: row.try_get("client_ip")?,
                    user_agent: row.try_get("user_agent")?,
                    session_id: row.try_get("session_id")?,
                })
            })
            .collect(),
    }
}

/// The audit records as a table (`table`) or a JSON array (`json`).
pub fn render_audit_log(
    records: &[AuditRecord],
    filter: &AuditFilter,
    format: &str,
) -> Result<String> {
    match format {
        "json" => Ok(serde_json::to_string_pretty(records)?),
        "table" => {
            if records.is_empty() {
                return Ok("No key audit records match.".to_string());
            }
            let mut table = comfy_table::Table::new();
            table.set_header(vec![
                "Time (UTC)",
                "Key",
                "Operation",
                "Result",
                "Actor",
                "Error",
            ]);
            for record in records {
                table.add_row(vec![
                    record.timestamp.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
                    record.key_id.clone(),
                    record.operation.clone(),
                    if record.success { "ok" } else { "FAILED" }.to_string(),
                    record.user_id.clone().unwrap_or_default(),
                    record.error_message.clone().unwrap_or_default(),
                ]);
            }
            let mut output = format!("🔑 Key audit log\n{table}");
            if records.len() as u64 == u64::from(filter.limit) {
                output.push_str(&format!(
                    "\nShowing {} records; use --offset {} for older ones.",
                    records.len(),
                    u64::from(filter.offset) + records.len() as u64
                ));
            }
            Ok(output)
        }
        other => Err(anyhow!("Unsupported format: {other}. Use: table, json")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: EncryptionCommand,
    }

    fn parse(args: &[&str]) -> std::result::Result<EncryptionCommand, clap::Error> {
        let mut argv = vec!["test"];
        argv.extend_from_slice(args);
        TestCli::try_parse_from(argv).map(|cli| cli.command)
    }

    #[test]
    fn audit_defaults() {
        let command = parse(&["audit"]).unwrap();
        assert_eq!(
            command.audit_filter(),
            AuditFilter {
                limit: 50,
                ..AuditFilter::default()
            }
        );
        let EncryptionCommand::Audit {
            format,
            database_url,
            ..
        } = command;
        assert_eq!(format, "table");
        assert!(database_url.is_none());
    }

    #[test]
    fn audit_flags_build_the_filter() {
        let command = parse(&[
            "audit",
            "-u",
            "mysql://db/x",
            "--key-id",
            "payment-key",
            "--operation",
            "ROTATE",
            "--since",
            "2026-10-01T00:00:00Z",
            "--until",
            "2026-10-02T12:00:00+02:00",
            "--failed",
            "--limit",
            "10",
            "--offset",
            "20",
            "--format",
            "json",
        ])
        .unwrap();
        let filter = command.audit_filter();
        assert_eq!(filter.key_id.as_deref(), Some("payment-key"));
        assert_eq!(filter.operation, Some(KeyOperation::Rotate));
        assert_eq!(
            filter.since.unwrap().to_rfc3339(),
            "2026-10-01T00:00:00+00:00"
        );
        assert_eq!(
            filter.until.unwrap().to_rfc3339(),
            "2026-10-02T10:00:00+00:00"
        );
        assert_eq!(filter.success, Some(false));
        assert_eq!((filter.limit, filter.offset), (10, 20));
        assert_eq!(
            command.get_database_url(&Config::default()).unwrap(),
            "mysql://db/x"
        );

        assert_eq!(
            parse(&["audit", "--success", "-k", "k", "-o", "create", "-l", "5"])
                .unwrap()
                .audit_filter()
                .success,
            Some(true)
        );
    }

    #[test]
    fn audit_rejects_invalid_flags() {
        for args in [
            &["audit", "--success", "--failed"][..],
            &["audit", "--operation", "explode"],
            &["audit", "--since", "yesterday"],
            &["audit", "--until", "5x"],
            &["audit", "--format", "csv"],
            &["audit", "--limit", "-1"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
        let err = parse(&["audit", "--operation", "explode"])
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("create, access, rotate"), "{err}");
    }

    #[test]
    fn database_url_comes_from_the_flag_then_the_config() {
        let config = config_for("postgres://config/db");
        let command = parse(&["audit"]).unwrap();
        assert_eq!(
            command.get_database_url(&config).unwrap(),
            "postgres://config/db"
        );
        assert!(command.get_database_url(&Config::default()).is_err());
    }

    #[test]
    fn relative_and_absolute_times() {
        let now = DateTime::parse_from_rfc3339("2026-10-09T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        for (value, expected) in [
            ("30s", "2026-10-09T11:59:30+00:00"),
            ("30m", "2026-10-09T11:30:00+00:00"),
            ("24h", "2026-10-08T12:00:00+00:00"),
            ("7d", "2026-10-02T12:00:00+00:00"),
            ("2w", "2026-09-25T12:00:00+00:00"),
            ("2026-01-02T03:04:05Z", "2026-01-02T03:04:05+00:00"),
        ] {
            assert_eq!(
                parse_time_at(value, now).unwrap().to_rfc3339(),
                expected,
                "{value}"
            );
        }
        for value in ["", "h", "10", "10y", "-5m", "9999999999999999999d", "1.5h"] {
            assert!(parse_time_at(value, now).is_err(), "{value}");
        }
    }

    #[test]
    fn audit_query_binds_every_value() {
        let since = Utc::now();
        let filter = AuditFilter {
            key_id: Some(HOSTILE_QUEUE.to_string()),
            operation: Some(KeyOperation::Rotate),
            since: Some(since),
            until: None,
            success: Some(false),
            limit: 25,
            offset: 50,
        };
        let (sql, binds) = build_audit_query(Backend::Postgres, &filter);
        assert!(!sql.contains(HOSTILE_QUEUE));
        assert!(sql.ends_with(
            "WHERE 1 = 1 AND key_id = $1 AND operation = $2 AND timestamp >= $3 \
             AND success = $4 ORDER BY timestamp DESC, id DESC LIMIT $5 OFFSET $6"
        ));
        assert!(sql.contains("host(client_ip) AS client_ip"));
        assert_eq!(
            binds,
            vec![
                Bind::Text(HOSTILE_QUEUE.to_string()),
                Bind::Text("Rotate".to_string()),
                Bind::Time(since),
                Bind::Bool(false),
                Bind::Int(25),
                Bind::Int(50),
            ]
        );

        let (sql, binds) = build_audit_query(
            Backend::MySql,
            &AuditFilter {
                until: Some(since),
                limit: 5,
                ..AuditFilter::default()
            },
        );
        assert!(sql.ends_with(
            "WHERE 1 = 1 AND timestamp < ? ORDER BY timestamp DESC, id DESC LIMIT ? OFFSET ?"
        ));
        assert_eq!(binds, vec![Bind::Time(since), Bind::Int(5), Bind::Int(0)]);
    }

    fn record(key_id: &str, operation: &str, success: bool) -> AuditRecord {
        AuditRecord {
            id: uuid::Uuid::new_v4().to_string(),
            key_id: key_id.to_string(),
            operation: operation.to_string(),
            success,
            error_message: (!success).then(|| "boom".to_string()),
            timestamp: DateTime::parse_from_rfc3339("2026-10-09T12:00:00.250Z")
                .unwrap()
                .with_timezone(&Utc),
            user_id: Some("alice".to_string()),
            client_ip: None,
            user_agent: None,
            session_id: None,
        }
    }

    #[test]
    fn renders_tables_and_json() {
        let filter = AuditFilter {
            limit: 2,
            ..AuditFilter::default()
        };
        let records = vec![
            record("payment-key", "Rotate", false),
            record("payment-key", "Create", true),
        ];
        let table = render_audit_log(&records, &filter, "table").unwrap();
        for expected in [
            "Key audit log",
            "2026-10-09 12:00:00.250",
            "payment-key",
            "Rotate",
            "FAILED",
            "boom",
            "alice",
            "use --offset 2",
        ] {
            assert!(table.contains(expected), "{expected} in\n{table}");
        }
        let one = render_audit_log(&records[..1], &filter, "table").unwrap();
        assert!(
            !one.contains("--offset"),
            "fewer than --limit: no more pages"
        );
        assert_eq!(
            render_audit_log(&[], &filter, "table").unwrap(),
            "No key audit records match."
        );

        let json: serde_json::Value =
            serde_json::from_str(&render_audit_log(&records, &filter, "json").unwrap()).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 2);
        assert_eq!(json[0]["operation"], "Rotate");
        assert_eq!(json[0]["success"], false);
        assert_eq!(json[0]["error_message"], "boom");
        assert_eq!(json[0]["timestamp"], "2026-10-09T12:00:00.250Z");
        assert_eq!(render_audit_log(&[], &filter, "json").unwrap(), "[]");
        assert!(render_audit_log(&records, &filter, "csv").is_err());
    }

    /// Insert an audit record (as `KeyManager` would, plus client details).
    async fn seed_audit(
        pool: &DatabasePool,
        key_id: &str,
        operation: &str,
        success: bool,
        seconds_ago: i64,
    ) {
        let mut params = SqlParams::new(pool.backend());
        let id = match pool.backend() {
            Backend::Postgres => params.uuid(&uuid::Uuid::new_v4().to_string()),
            Backend::MySql => params.text(&uuid::Uuid::new_v4().to_string()),
        };
        let key = params.text(key_id);
        let op = params.text(operation);
        let ok = params.boolean(success);
        let error = if success {
            "NULL".to_string()
        } else {
            params.text("boom")
        };
        let at = params.time(Utc::now() - chrono::Duration::seconds(seconds_ago));
        let ip = params.text("192.0.2.7");
        let ip = match pool.backend() {
            Backend::Postgres => format!("CAST({ip} AS INET)"),
            Backend::MySql => ip,
        };
        let sql = format!(
            "INSERT INTO hammerwork_key_audit_log \
             (id, key_id, operation, success, error_message, timestamp, user_id, client_ip) \
             VALUES ({id}, {key}, {op}, {ok}, {error}, {at}, 'ops', {ip})"
        );
        let n = crate::utils::sql::execute_binds(pool, &sql, params.binds())
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    async fn audit_command(url: String) {
        let pool = DatabasePool::connect(&url, 2).await.unwrap();
        pool.migrate(false).await.unwrap();
        let key = format!("{HOSTILE_QUEUE} {}", uuid::Uuid::new_v4().simple());
        let other = format!("other-{}", uuid::Uuid::new_v4().simple());
        seed_audit(&pool, &key, "Create", true, 300).await;
        seed_audit(&pool, &key, "Access", true, 200).await;
        seed_audit(&pool, &key, "Rotate", false, 100).await;
        seed_audit(&pool, &key, "Rotate", true, 50).await;
        seed_audit(&pool, &other, "Create", true, 10).await;

        let config = config_for(&url);
        let read = |args: Vec<String>| {
            let pool = &pool;
            async move {
                let mut argv = vec!["audit".to_string()];
                argv.extend(args);
                let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
                let filter = parse(&argv).unwrap().audit_filter();
                read_audit_log(pool, &filter).await.unwrap()
            }
        };
        let ops = |records: &[AuditRecord]| -> Vec<(String, bool)> {
            records
                .iter()
                .map(|r| (r.operation.clone(), r.success))
                .collect()
        };
        let args = |extra: &[&str]| -> Vec<String> {
            let mut all = vec!["--key-id".to_string(), key.clone()];
            all.extend(extra.iter().map(|s| s.to_string()));
            all
        };

        let all = read(args(&[])).await;
        assert_eq!(
            ops(&all),
            vec![
                ("Rotate".into(), true),
                ("Rotate".into(), false),
                ("Access".into(), true),
                ("Create".into(), true),
            ],
            "newest first, only this key"
        );
        assert_eq!(all[1].error_message.as_deref(), Some("boom"));
        assert_eq!(all[0].user_id.as_deref(), Some("ops"));
        assert_eq!(all[0].client_ip.as_deref(), Some("192.0.2.7"));
        assert!(all.iter().all(|r| r.key_id == key));

        assert_eq!(
            ops(&read(args(&["--operation", "rotate"])).await),
            vec![("Rotate".into(), true), ("Rotate".into(), false)]
        );
        assert_eq!(
            ops(&read(args(&["--failed"])).await),
            vec![("Rotate".into(), false)]
        );
        assert_eq!(read(args(&["--success"])).await.len(), 3);
        assert_eq!(
            ops(&read(args(&["--since", "150s", "--until", "75s"])).await),
            vec![("Rotate".into(), false)],
            "since inclusive, until exclusive"
        );
        assert_eq!(
            ops(&read(args(&["--limit", "2", "--offset", "1"])).await),
            vec![("Rotate".into(), false), ("Access".into(), true)]
        );
        assert!(read(args(&["--offset", "4"])).await.is_empty());
        assert!(
            read(vec!["--key-id".into(), format!("missing-{key}")])
                .await
                .is_empty()
        );

        // Rendered output
        let filter = parse(&["audit", "--key-id", &key, "--limit", "2"])
            .unwrap()
            .audit_filter();
        let records = read_audit_log(&pool, &filter).await.unwrap();
        let table = render_audit_log(&records, &filter, "table").unwrap();
        assert!(
            table.contains("Rotate") && table.contains("use --offset 2"),
            "{table}"
        );
        let json: serde_json::Value =
            serde_json::from_str(&render_audit_log(&records, &filter, "json").unwrap()).unwrap();
        assert_eq!(json[0]["key_id"], key.as_str());
        assert_eq!(json[1]["success"], false);

        // The command itself, with the URL from the configuration
        for format in ["table", "json"] {
            parse(&["audit", "--key-id", &key, "--format", format])
                .unwrap()
                .execute(&config)
                .await
                .unwrap();
        }

        assert!(table_exists(&pool).await);
    }

    db_tests!(
        audit_command,
        test_audit_command_postgres,
        test_audit_command_mysql
    );
}
