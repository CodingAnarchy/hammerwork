use anyhow::Result;
use hammerwork::JobPriority;
use std::str::FromStr;

pub fn validate_priority(priority: &str) -> Result<JobPriority> {
    match priority.to_lowercase().as_str() {
        "background" => Ok(JobPriority::Background),
        "low" => Ok(JobPriority::Low),
        "normal" => Ok(JobPriority::Normal),
        "high" => Ok(JobPriority::High),
        "critical" => Ok(JobPriority::Critical),
        _ => Err(anyhow::anyhow!(
            "Invalid priority '{}'. Valid options: background, low, normal, high, critical",
            priority
        )),
    }
}

pub fn validate_status(status: &str) -> Result<()> {
    match status.to_lowercase().as_str() {
        "pending" | "running" | "completed" | "failed" | "dead" | "retrying" | "timed_out" => {
            Ok(())
        }
        _ => Err(anyhow::anyhow!(
            "Invalid status '{}'. Valid options: pending, running, completed, failed, dead, retrying, timed_out",
            status
        )),
    }
}

pub fn validate_json_payload(payload: &str) -> Result<serde_json::Value> {
    serde_json::from_str(payload).map_err(|e| anyhow::anyhow!("Invalid JSON payload: {}", e))
}

pub fn validate_database_url(url: &str) -> Result<()> {
    if url.starts_with("postgres://")
        || url.starts_with("postgresql://")
        || url.starts_with("mysql://")
    {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Invalid database URL. Must start with postgres://, postgresql://, or mysql://"
        ))
    }
}

/// `url` with the password replaced by `***`, safe to log. A URL without credentials is
/// returned unchanged (see [`hammerwork::config::redact_url`]).
pub fn redact_url(url: &str) -> String {
    hammerwork::config::redact_url(url)
}

pub fn validate_cron_expression(cron: &str) -> Result<()> {
    cron::Schedule::from_str(cron)
        .map_err(|e| anyhow::anyhow!("Invalid cron expression '{}': {}", cron, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priorities_are_case_insensitive_and_map_to_levels() {
        for (name, expected) in [
            ("background", JobPriority::Background),
            ("LOW", JobPriority::Low),
            ("Normal", JobPriority::Normal),
            ("high", JobPriority::High),
            ("CRITICAL", JobPriority::Critical),
        ] {
            assert_eq!(validate_priority(name).unwrap(), expected, "{name}");
        }
        let err = validate_priority("urgent").unwrap_err().to_string();
        assert!(err.contains("Invalid priority 'urgent'"), "{err}");
        assert!(
            err.contains("background, low, normal, high, critical"),
            "{err}"
        );
        assert!(validate_priority("").is_err());
    }

    #[test]
    fn statuses_accept_every_state_the_cli_filters_on() {
        for status in [
            "pending",
            "RUNNING",
            "Completed",
            "failed",
            "dead",
            "retrying",
            "timed_out",
        ] {
            assert!(validate_status(status).is_ok(), "{status}");
        }
        let err = validate_status("archived").unwrap_err().to_string();
        assert!(err.contains("Invalid status 'archived'"), "{err}");
        assert!(validate_status("timedout").is_err());
    }

    #[test]
    fn json_payload_must_parse() {
        let value = validate_json_payload(r#"{"a": [1, 2], "b": null}"#).unwrap();
        assert_eq!(value["a"][1], 2);
        assert!(validate_json_payload("123").unwrap().is_number());
        let err = validate_json_payload("{not json").unwrap_err().to_string();
        assert!(err.starts_with("Invalid JSON payload"), "{err}");
    }

    #[test]
    fn database_urls_need_a_supported_scheme() {
        for url in [
            "postgres://u:p@h/db",
            "postgresql://h/db",
            "mysql://root@h:3306/db",
        ] {
            assert!(validate_database_url(url).is_ok(), "{url}");
        }
        for url in ["", "sqlite://x.db", "http://h/db", "POSTGRES://h/db"] {
            assert!(validate_database_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn urls_are_redacted_for_logging() {
        assert_eq!(
            redact_url("postgres://admin:s3cret@db.internal:5432/app?sslmode=require"),
            "postgres://admin:***@db.internal:5432/app?sslmode=require"
        );
        assert_eq!(
            redact_url("mysql://root:p@ss:word@127.0.0.1:3306/db"),
            "mysql://root:***@127.0.0.1:3306/db"
        );
        // Nothing to hide.
        assert_eq!(
            redact_url("postgres://user@host/db"),
            "postgres://user@host/db"
        );
        assert_eq!(redact_url("postgres://host/db"), "postgres://host/db");
        assert_eq!(redact_url("postgres://host"), "postgres://host");
        assert_eq!(redact_url("not a url"), "not a url");
        // An '@' in the path is not a credential separator.
        assert_eq!(
            redact_url("postgres://host/db?options=a:b@c"),
            "postgres://host/db?options=a:b@c"
        );
    }

    #[test]
    fn cron_expressions_use_the_library_field_count() {
        assert!(validate_cron_expression("0 0 9 * * MON-FRI").is_ok());
        assert!(validate_cron_expression("*/5 * * * * *").is_ok());
        // Five-field crontab syntax has no seconds field and is rejected by the cron crate.
        assert!(validate_cron_expression("* * * * *").is_err());
        let err = validate_cron_expression("not a cron")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Invalid cron expression 'not a cron'"),
            "{err}"
        );
    }
}
