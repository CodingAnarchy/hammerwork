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
        "pending" | "running" | "completed" | "failed" | "dead" | "retrying" | "timed_out" => Ok(()),
        _ => Err(anyhow::anyhow!(
            "Invalid status '{}'. Valid options: pending, running, completed, failed, dead, retrying, timed_out",
            status
        )),
    }
}

pub fn validate_json_payload(payload: &str) -> Result<serde_json::Value> {
    serde_json::from_str(payload).map_err(|e| {
        anyhow::anyhow!("Invalid JSON payload: {}", e)
    })
}

pub fn validate_database_url(url: &str) -> Result<()> {
    if url.starts_with("postgres://") || url.starts_with("postgresql://") || url.starts_with("mysql://") {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Invalid database URL. Must start with postgres://, postgresql://, or mysql://"
        ))
    }
}

pub fn validate_cron_expression(cron: &str) -> Result<()> {
    cron::Schedule::from_str(cron)
        .map_err(|e| anyhow::anyhow!("Invalid cron expression '{}': {}", cron, e))?;
    Ok(())
}