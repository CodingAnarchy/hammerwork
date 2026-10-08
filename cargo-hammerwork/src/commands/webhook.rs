use anyhow::Result;
use clap::Subcommand;
use serde_json::json;
use std::collections::HashMap;
use tracing::info;
use uuid::Uuid;

use crate::config::Config;
use crate::utils::display::create_table;
use hammerwork::webhooks::WebhookConfig;

#[derive(Clone, Subcommand)]
pub enum WebhookCommand {
    #[command(about = "List all configured webhooks")]
    List {
        #[arg(short, long, help = "Show detailed webhook information")]
        detailed: bool,
    },
    #[command(about = "Add a new webhook")]
    Add {
        #[arg(short, long, help = "Webhook name")]
        name: String,
        #[arg(short, long, help = "Webhook URL")]
        url: String,
        #[arg(
            short = 'm',
            long,
            help = "HTTP method (POST, PUT, PATCH)",
            default_value = "POST"
        )]
        method: String,
        #[arg(long, help = "Event types to filter (comma-separated)")]
        events: Option<String>,
        #[arg(long, help = "Queue names to filter (comma-separated)")]
        queues: Option<String>,
        #[arg(long, help = "Job priorities to filter (comma-separated)")]
        priorities: Option<String>,
        #[arg(long, help = "Custom headers in key=value format (comma-separated)")]
        headers: Option<String>,
        #[arg(long, help = "Authentication token for Bearer auth")]
        auth_token: Option<String>,
        #[arg(long, help = "Basic auth in username:password format")]
        basic_auth: Option<String>,
        #[arg(long, help = "API key header name")]
        api_key_header: Option<String>,
        #[arg(long, help = "API key value")]
        api_key: Option<String>,
        #[arg(long, help = "Secret for HMAC signatures")]
        secret: Option<String>,
        #[arg(long, help = "Request timeout in seconds", default_value = "30")]
        timeout: u64,
        #[arg(long, help = "Maximum retry attempts", default_value = "3")]
        max_retries: u32,
        #[arg(long, help = "Include job payload in events")]
        include_payload: bool,
    },
    #[command(about = "Remove a webhook")]
    Remove {
        #[arg(short, long, help = "Webhook ID or name")]
        webhook: String,
        #[arg(long, help = "Confirm the operation")]
        confirm: bool,
    },
    #[command(about = "Test a webhook")]
    Test {
        #[arg(short, long, help = "Webhook ID or name")]
        webhook: String,
        #[arg(long, help = "Test event type", default_value = "completed")]
        event_type: String,
        #[arg(long, help = "Test job ID")]
        job_id: Option<String>,
        #[arg(long, help = "Test queue name", default_value = "test")]
        queue: String,
    },
    #[command(about = "Enable or disable a webhook")]
    Toggle {
        #[arg(short, long, help = "Webhook ID or name")]
        webhook: String,
        #[arg(long, help = "Enable the webhook")]
        enable: bool,
    },
    #[command(about = "Update webhook configuration")]
    Update {
        #[arg(short, long, help = "Webhook ID or name")]
        webhook: String,
        #[arg(short, long, help = "New webhook name")]
        name: Option<String>,
        #[arg(short, long, help = "New webhook URL")]
        url: Option<String>,
        #[arg(short = 'm', long, help = "New HTTP method")]
        method: Option<String>,
        #[arg(long, help = "New event types filter")]
        events: Option<String>,
        #[arg(long, help = "New queue names filter")]
        queues: Option<String>,
        #[arg(long, help = "New priorities filter")]
        priorities: Option<String>,
        #[arg(long, help = "New custom headers")]
        headers: Option<String>,
        #[arg(long, help = "New timeout in seconds")]
        timeout: Option<u64>,
        #[arg(long, help = "New max retry attempts")]
        max_retries: Option<u32>,
    },
}

pub async fn handle_webhook_command(command: WebhookCommand, config: &Config) -> Result<()> {
    match command {
        WebhookCommand::List { detailed } => list_webhooks(config, detailed).await,
        WebhookCommand::Add {
            name,
            url,
            method,
            events,
            queues,
            priorities,
            headers,
            auth_token,
            basic_auth,
            api_key_header,
            api_key,
            secret,
            timeout,
            max_retries,
            include_payload,
        } => {
            add_webhook(
                config,
                name,
                url,
                method,
                events,
                queues,
                priorities,
                headers,
                auth_token,
                basic_auth,
                api_key_header,
                api_key,
                secret,
                timeout,
                max_retries,
                include_payload,
            )
            .await
        }
        WebhookCommand::Remove { webhook, confirm } => {
            remove_webhook(config, webhook, confirm).await
        }
        WebhookCommand::Test {
            webhook,
            event_type,
            job_id,
            queue,
        } => test_webhook(config, webhook, event_type, job_id, queue).await,
        WebhookCommand::Toggle { webhook, enable } => toggle_webhook(config, webhook, enable).await,
        WebhookCommand::Update {
            webhook,
            name,
            url,
            method,
            events,
            queues,
            priorities,
            headers,
            timeout,
            max_retries,
        } => {
            update_webhook(
                config,
                webhook,
                name,
                url,
                method,
                events,
                queues,
                priorities,
                headers,
                timeout,
                max_retries,
            )
            .await
        }
    }
}

async fn list_webhooks(config: &Config, detailed: bool) -> Result<()> {
    let webhooks = load_webhooks_config(config)?;

    if webhooks.is_empty() {
        info!("No webhooks configured.");
        return Ok(());
    }

    if detailed {
        for webhook in webhooks {
            println!("\n📎 Webhook: {}", webhook.name);
            println!("  ID: {}", webhook.id);
            println!("  URL: {}", webhook.url);
            println!("  Method: {}", webhook.method);
            println!("  Enabled: {}", webhook.enabled);
            println!("  Timeout: {}s", webhook.timeout_secs);
            println!("  Max Retries: {}", webhook.retry_policy.max_attempts);

            if !webhook.headers.is_empty() {
                println!("  Headers:");
                for (key, value) in &webhook.headers {
                    println!("    {}: {}", key, value);
                }
            }

            if webhook.auth.is_some() {
                println!("  Authentication: Configured");
            }

            if webhook.secret.is_some() {
                println!("  HMAC Secret: Configured");
            }
        }
    } else {
        let mut table = create_table();
        table.set_header(vec!["Name", "URL", "Method", "Enabled", "Events"]);

        for webhook in webhooks {
            let events_filter = if webhook.filter.event_types.is_empty() {
                "All".to_string()
            } else {
                webhook
                    .filter
                    .event_types
                    .iter()
                    .map(|e| format!("{:?}", e))
                    .collect::<Vec<_>>()
                    .join(", ")
            };

            table.add_row(vec![
                webhook.name,
                webhook.url,
                webhook.method.to_string(),
                if webhook.enabled { "✓" } else { "✗" }.to_string(),
                events_filter,
            ]);
        }

        println!("{}", table);
    }

    Ok(())
}

// Mirrors the flags of the corresponding clap subcommand one-to-one.
#[allow(clippy::too_many_arguments)]
async fn add_webhook(
    config: &Config,
    name: String,
    url: String,
    method: String,
    events: Option<String>,
    queues: Option<String>,
    priorities: Option<String>,
    headers: Option<String>,
    auth_token: Option<String>,
    basic_auth: Option<String>,
    api_key_header: Option<String>,
    api_key: Option<String>,
    secret: Option<String>,
    timeout: u64,
    max_retries: u32,
    include_payload: bool,
) -> Result<()> {
    // Create webhook configuration
    let webhook_config = create_webhook_config(
        name.clone(),
        url,
        method,
        events,
        queues,
        priorities,
        headers,
        auth_token,
        basic_auth,
        api_key_header,
        api_key,
        secret,
        timeout,
        max_retries,
        include_payload,
    )?;

    // Save to configuration
    save_webhook_config(config, webhook_config)?;

    info!("✅ Webhook '{}' added successfully", name);
    Ok(())
}

async fn remove_webhook(config: &Config, webhook_id: String, confirm: bool) -> Result<()> {
    if !confirm {
        return Err(anyhow::anyhow!("Use --confirm to confirm webhook removal"));
    }

    let mut webhooks = load_webhooks_config(config)?;
    let initial_len = webhooks.len();

    webhooks.retain(|w| w.name != webhook_id && w.id.to_string() != webhook_id);

    if webhooks.len() == initial_len {
        return Err(anyhow::anyhow!("Webhook '{}' not found", webhook_id));
    }

    save_webhooks_config(config, webhooks)?;
    info!("✅ Webhook '{}' removed successfully", webhook_id);
    Ok(())
}

async fn test_webhook(
    config: &Config,
    webhook_id: String,
    event_type: String,
    job_id: Option<String>,
    queue: String,
) -> Result<()> {
    let webhooks = load_webhooks_config(config)?;
    let webhook = find_webhook(&webhooks, &webhook_id)?;
    let job_id = match job_id {
        Some(id) => Uuid::parse_str(&id).map_err(|e| anyhow::anyhow!("Invalid job ID: {e}"))?,
        None => Uuid::new_v4(),
    };
    let event = build_test_event(webhook, &event_type, job_id, &queue)?;

    println!(
        "Sending test event to webhook '{}' ({}):",
        webhook.name, webhook.url
    );
    if !webhook.enabled {
        println!("(note: this webhook is disabled; the test is sent anyway)");
    }
    println!("{}", serde_json::to_string_pretty(&event)?);

    let outcome = send_test_event(webhook, &event).await?;
    println!(
        "Response: HTTP {} in {}ms",
        outcome.status, outcome.duration_ms
    );
    if !outcome.body.is_empty() {
        println!("Body: {}", outcome.body);
    }
    if outcome.success {
        println!("Webhook test succeeded.");
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Webhook test failed: endpoint answered HTTP {}",
            outcome.status
        ))
    }
}

/// Result of delivering the test event.
#[derive(Debug)]
pub struct TestOutcome {
    pub status: u16,
    pub success: bool,
    pub duration_ms: u64,
    /// Response body, truncated to a printable length.
    pub body: String,
}

/// Builds a sample lifecycle event of the given type for `webhook`.
fn build_test_event(
    webhook: &WebhookConfig,
    event_type: &str,
    job_id: Uuid,
    queue: &str,
) -> Result<hammerwork::events::JobLifecycleEvent> {
    use hammerwork::events::{JobError, JobLifecycleEvent, JobLifecycleEventType};
    use hammerwork::priority::JobPriority;

    let event_type = parse_event_types(event_type)?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("An event type is required"))?;
    let is_error = matches!(
        event_type,
        JobLifecycleEventType::Failed
            | JobLifecycleEventType::Dead
            | JobLifecycleEventType::TimedOut
    );
    let is_completed = event_type == JobLifecycleEventType::Completed;

    let mut metadata = HashMap::new();
    metadata.insert("test".to_string(), "true".to_string());
    Ok(JobLifecycleEvent {
        event_id: Uuid::new_v4(),
        job_id,
        queue_name: queue.to_string(),
        event_type,
        priority: JobPriority::Normal,
        timestamp: chrono::Utc::now(),
        processing_time_ms: is_completed.then_some(123),
        error: is_error.then(|| JobError {
            message: "Test error from `cargo hammerwork webhook test`".to_string(),
            error_type: Some("test".to_string()),
            details: None,
            retry_attempt: None,
        }),
        payload: webhook
            .filter
            .include_payload
            .then(|| json!({"test": true})),
        metadata,
    })
}

/// Sends `event` to the webhook the way the library's delivery does: configured
/// method, custom headers, authentication, and an `X-Hammerwork-Signature` HMAC header
/// when a secret is set. A single attempt, no retries. Transport errors (DNS, connect,
/// timeout) are returned as `Err`; any HTTP response is returned as an outcome.
pub async fn send_test_event(
    webhook: &WebhookConfig,
    event: &hammerwork::events::JobLifecycleEvent,
) -> Result<TestOutcome> {
    use hammerwork::HttpMethod;
    use hammerwork::webhooks::WebhookAuth;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(webhook.timeout_secs.max(1)))
        .user_agent(concat!("hammerwork-cli/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let body = serde_json::to_string(&serde_json::to_value(event)?)?;

    let mut request = match webhook.method {
        HttpMethod::Post => client.post(&webhook.url),
        HttpMethod::Put => client.put(&webhook.url),
        HttpMethod::Patch => client.patch(&webhook.url),
    };
    for (key, value) in &webhook.headers {
        request = request.header(key, value);
    }
    match &webhook.auth {
        Some(WebhookAuth::Bearer { token }) => {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        Some(WebhookAuth::Basic { username, password }) => {
            request = request.basic_auth(username, Some(password));
        }
        Some(WebhookAuth::ApiKey {
            header_name,
            api_key,
        }) => {
            request = request.header(header_name, api_key);
        }
        Some(WebhookAuth::Custom { headers }) => {
            for (key, value) in headers {
                request = request.header(key, value);
            }
        }
        None => {}
    }
    if let Some(secret) = &webhook.secret {
        let signature = hammerwork::webhooks::generate_hmac_signature(secret, body.as_bytes());
        request = request.header("X-Hammerwork-Signature", format!("sha256={signature}"));
    }

    let started = std::time::Instant::now();
    let response = request
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Webhook test failed: could not deliver request: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let body: String = text.chars().take(500).collect();
    Ok(TestOutcome {
        status: status.as_u16(),
        success: status.is_success(),
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        body,
    })
}

fn find_webhook<'a>(webhooks: &'a [WebhookConfig], id_or_name: &str) -> Result<&'a WebhookConfig> {
    webhooks
        .iter()
        .find(|w| w.name == id_or_name || w.id.to_string() == id_or_name)
        .ok_or_else(|| anyhow::anyhow!("Webhook '{}' not found", id_or_name))
}

async fn toggle_webhook(config: &Config, webhook_id: String, enable: bool) -> Result<()> {
    let mut webhooks = load_webhooks_config(config)?;

    let webhook = webhooks
        .iter_mut()
        .find(|w| w.name == webhook_id || w.id.to_string() == webhook_id);

    match webhook {
        Some(w) => {
            w.enabled = enable;
            save_webhooks_config(config, webhooks)?;
            let status = if enable { "enabled" } else { "disabled" };
            info!("✅ Webhook '{}' {}", webhook_id, status);
        }
        None => return Err(anyhow::anyhow!("Webhook '{}' not found", webhook_id)),
    }

    Ok(())
}

// Mirrors the flags of the corresponding clap subcommand one-to-one.
#[allow(clippy::too_many_arguments)]
async fn update_webhook(
    config: &Config,
    webhook_id: String,
    name: Option<String>,
    url: Option<String>,
    method: Option<String>,
    events: Option<String>,
    queues: Option<String>,
    priorities: Option<String>,
    headers: Option<String>,
    timeout: Option<u64>,
    max_retries: Option<u32>,
) -> Result<()> {
    let mut webhooks = load_webhooks_config(config)?;

    let webhook = webhooks
        .iter_mut()
        .find(|w| w.name == webhook_id || w.id.to_string() == webhook_id);

    match webhook {
        Some(w) => {
            if let Some(new_name) = name {
                w.name = new_name;
            }
            if let Some(new_url) = url {
                w.url = new_url;
            }
            if let Some(new_method) = method {
                w.method = parse_http_method(&new_method)?;
            }
            if let Some(new_timeout) = timeout {
                w.timeout_secs = new_timeout;
            }
            if let Some(new_max_retries) = max_retries {
                w.retry_policy.max_attempts = new_max_retries;
            }

            // Update filters
            if let Some(events_str) = events {
                w.filter.event_types = parse_event_types(&events_str)?;
            }
            if let Some(queues_str) = queues {
                w.filter.queue_names = parse_comma_separated(&queues_str);
            }
            if let Some(priorities_str) = priorities {
                w.filter.priorities = parse_priorities(&priorities_str)?;
            }
            if let Some(headers_str) = headers {
                w.headers = parse_headers(&headers_str)?;
            }

            save_webhooks_config(config, webhooks)?;
            info!("✅ Webhook '{}' updated successfully", webhook_id);
        }
        None => return Err(anyhow::anyhow!("Webhook '{}' not found", webhook_id)),
    }

    Ok(())
}

// Helper functions for webhook configuration

// Mirrors the flags of the corresponding clap subcommand one-to-one.
#[allow(clippy::too_many_arguments)]
fn create_webhook_config(
    name: String,
    url: String,
    method: String,
    events: Option<String>,
    queues: Option<String>,
    priorities: Option<String>,
    headers: Option<String>,
    auth_token: Option<String>,
    basic_auth: Option<String>,
    api_key_header: Option<String>,
    api_key: Option<String>,
    secret: Option<String>,
    timeout: u64,
    max_retries: u32,
    include_payload: bool,
) -> Result<WebhookConfig> {
    use hammerwork::events::EventFilter;
    use hammerwork::webhooks::{RetryPolicy, WebhookAuth};

    let http_method = parse_http_method(&method)?;
    let parsed_headers = headers
        .map(|h| parse_headers(&h))
        .transpose()?
        .unwrap_or_default();

    // Parse authentication
    let auth = if let Some(token) = auth_token {
        Some(WebhookAuth::Bearer { token })
    } else if let Some(basic) = basic_auth {
        let parts: Vec<&str> = basic.split(':').collect();
        if parts.len() != 2 {
            return Err(anyhow::anyhow!(
                "Basic auth must be in format username:password"
            ));
        }
        Some(WebhookAuth::Basic {
            username: parts[0].to_string(),
            password: parts[1].to_string(),
        })
    } else if let (Some(header), Some(key)) = (api_key_header, api_key) {
        Some(WebhookAuth::ApiKey {
            header_name: header,
            api_key: key,
        })
    } else {
        None
    };

    // Create event filter
    let mut filter = EventFilter::new();
    if let Some(events_str) = events {
        filter.event_types = parse_event_types(&events_str)?;
    }
    if let Some(queues_str) = queues {
        filter.queue_names = parse_comma_separated(&queues_str);
    }
    if let Some(priorities_str) = priorities {
        filter.priorities = parse_priorities(&priorities_str)?;
    }
    filter.include_payload = include_payload;

    let retry_policy = RetryPolicy {
        max_attempts: max_retries,
        initial_delay_secs: 1,
        max_delay_secs: 300,
        backoff_multiplier: 2.0,
        retry_on_status_codes: vec![408, 429, 500, 502, 503, 504],
    };

    Ok(WebhookConfig {
        id: Uuid::new_v4(),
        name,
        url,
        method: http_method,
        headers: parsed_headers,
        filter,
        retry_policy,
        auth,
        timeout_secs: timeout,
        enabled: true,
        secret,
        payload_template: None,
    })
}

fn parse_http_method(method: &str) -> Result<hammerwork::HttpMethod> {
    use hammerwork::HttpMethod;

    match method.to_uppercase().as_str() {
        "POST" => Ok(HttpMethod::Post),
        "PUT" => Ok(HttpMethod::Put),
        "PATCH" => Ok(HttpMethod::Patch),
        _ => Err(anyhow::anyhow!("Unsupported HTTP method: {}", method)),
    }
}

fn parse_event_types(events_str: &str) -> Result<Vec<hammerwork::events::JobLifecycleEventType>> {
    use hammerwork::events::JobLifecycleEventType;
    let mut result = Vec::new();

    for event in events_str.split(',') {
        let event = event.trim();
        let event_type = match event {
            "enqueued" => JobLifecycleEventType::Enqueued,
            "started" => JobLifecycleEventType::Started,
            "completed" => JobLifecycleEventType::Completed,
            "failed" => JobLifecycleEventType::Failed,
            "retried" => JobLifecycleEventType::Retried,
            "dead" => JobLifecycleEventType::Dead,
            "timed_out" => JobLifecycleEventType::TimedOut,
            "cancelled" => JobLifecycleEventType::Cancelled,
            "archived" => JobLifecycleEventType::Archived,
            "restored" => JobLifecycleEventType::Restored,
            _ => return Err(anyhow::anyhow!("Invalid event type: {}", event)),
        };
        result.push(event_type);
    }

    Ok(result)
}

fn parse_priorities(priorities_str: &str) -> Result<Vec<hammerwork::priority::JobPriority>> {
    use hammerwork::priority::JobPriority;
    let mut result = Vec::new();

    for priority in priorities_str.split(',') {
        let priority = priority.trim();
        let job_priority = match priority {
            "Background" => JobPriority::Background,
            "Low" => JobPriority::Low,
            "Normal" => JobPriority::Normal,
            "High" => JobPriority::High,
            "Critical" => JobPriority::Critical,
            _ => return Err(anyhow::anyhow!("Invalid priority: {}", priority)),
        };
        result.push(job_priority);
    }

    Ok(result)
}

fn parse_comma_separated(input: &str) -> Vec<String> {
    input.split(',').map(|s| s.trim().to_string()).collect()
}

fn parse_headers(headers_str: &str) -> Result<HashMap<String, String>> {
    let mut headers = HashMap::new();

    for header in headers_str.split(',') {
        let parts: Vec<&str> = header.split('=').collect();
        if parts.len() != 2 {
            return Err(anyhow::anyhow!(
                "Header must be in format key=value: {}",
                header
            ));
        }
        headers.insert(parts[0].trim().to_string(), parts[1].trim().to_string());
    }

    Ok(headers)
}

// Configuration persistence functions

// Configuration persistence
//
// Webhooks are stored as JSON next to the CLI's `config.toml` (the TOML file only holds
// flat scalar settings). The file may contain secrets, so it is written owner-only.

fn load_webhooks_config(config: &Config) -> Result<Vec<WebhookConfig>> {
    read_webhooks(&config.webhooks_file_path()?)
}

fn save_webhook_config(config: &Config, webhook: WebhookConfig) -> Result<()> {
    let path = config.webhooks_file_path()?;
    let mut webhooks = read_webhooks(&path)?;
    if webhooks.iter().any(|w| w.name == webhook.name) {
        return Err(anyhow::anyhow!(
            "A webhook named '{}' already exists",
            webhook.name
        ));
    }
    webhooks.push(webhook);
    write_webhooks(&path, &webhooks)
}

fn save_webhooks_config(config: &Config, webhooks: Vec<WebhookConfig>) -> Result<()> {
    write_webhooks(&config.webhooks_file_path()?, &webhooks)
}

/// Reads the webhook list; a missing file is an empty list, a corrupt one is an error.
fn read_webhooks(path: &std::path::Path) -> Result<Vec<WebhookConfig>> {
    match std::fs::read_to_string(path) {
        Ok(content) if content.trim().is_empty() => Ok(Vec::new()),
        Ok(content) => serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Invalid webhook file {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(anyhow::anyhow!("Cannot read {}: {e}", path.display())),
    }
}

/// Writes the list atomically (temp file + rename), owner-read/write only on Unix.
fn write_webhooks(path: &std::path::Path, webhooks: &[WebhookConfig]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(webhooks)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: WebhookCommand,
    }

    fn sample(url: &str) -> WebhookConfig {
        create_webhook_config(
            "ci".into(),
            url.into(),
            "POST".into(),
            None,
            None,
            None,
            Some("X-Custom=yes".into()),
            Some("tok123".into()),
            None,
            None,
            None,
            Some("s3cret".into()),
            5,
            2,
            true,
        )
        .unwrap()
    }

    /// Serves one request, answers with `status`, and returns the raw request text.
    async fn one_shot_server(status: u16) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = sock.read(&mut buf).await.unwrap();
                data.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&data).to_string();
                if let Some(idx) = text.find("\r\n\r\n") {
                    let len = text[..idx]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if data.len() >= idx + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let reply =
                format!("HTTP/1.1 {status} X\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            sock.write_all(reply.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&data).to_string()
        });
        (url, handle)
    }

    #[test]
    fn test_stats_subcommand_was_removed() {
        assert!(TestCli::try_parse_from(["t", "stats"]).is_err());
        assert!(TestCli::try_parse_from(["t", "test", "-w", "x"]).is_ok());
    }

    #[test]
    fn test_webhook_file_roundtrip_preserves_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("webhooks.json");
        assert!(read_webhooks(&path).unwrap().is_empty());

        let hook = sample("http://localhost/x");
        write_webhooks(&path, std::slice::from_ref(&hook)).unwrap();
        let loaded = read_webhooks(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, hook.id);
        assert_eq!(loaded[0].headers.get("X-Custom").unwrap(), "yes");
        assert_eq!(loaded[0].timeout_secs, 5);
        assert_eq!(loaded[0].retry_policy.max_attempts, 2);
        assert_eq!(loaded[0].secret.as_deref(), Some("s3cret"));
        assert!(loaded[0].auth.is_some());
        assert!(loaded[0].filter.include_payload);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn test_corrupt_webhook_file_is_an_error_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("webhooks.json");
        std::fs::write(&path, "{not json").unwrap();
        assert!(read_webhooks(&path).is_err());
    }

    #[test]
    fn test_find_webhook_by_name_or_id() {
        let hook = sample("http://localhost/x");
        let id = hook.id.to_string();
        let list = vec![hook];
        assert!(find_webhook(&list, "ci").is_ok());
        assert!(find_webhook(&list, &id).is_ok());
        assert!(find_webhook(&list, "nope").is_err());
    }

    #[test]
    fn test_build_test_event_marks_test_and_rejects_unknown_type() {
        let hook = sample("http://localhost/x");
        let event = build_test_event(&hook, "failed", Uuid::new_v4(), "q").unwrap();
        assert_eq!(event.queue_name, "q");
        assert!(event.error.is_some());
        assert_eq!(event.metadata.get("test").unwrap(), "true");
        assert!(event.payload.is_some(), "include_payload was set");
        assert!(build_test_event(&hook, "bogus", Uuid::new_v4(), "q").is_err());
    }

    #[tokio::test]
    async fn test_send_test_event_delivers_with_auth_headers_and_valid_signature() {
        let (url, server) = one_shot_server(200).await;
        let hook = sample(&url);
        let event = build_test_event(&hook, "completed", Uuid::new_v4(), "q").unwrap();
        let outcome = send_test_event(&hook, &event).await.unwrap();
        assert!(outcome.success);
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "ok");

        let raw = server.await.unwrap();
        let lower = raw.to_ascii_lowercase();
        assert!(raw.starts_with("POST /hook"));
        assert!(lower.contains("authorization: bearer tok123"));
        assert!(lower.contains("x-custom: yes"));
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        let sig = head
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .starts_with("x-hammerwork-signature:")
                    .then(|| l.split_once(':').unwrap().1.trim().to_string())
            })
            .expect("signature header");
        let hex = sig.strip_prefix("sha256=").unwrap();
        assert!(hammerwork::webhooks::verify_hmac_signature(
            "s3cret",
            body.as_bytes(),
            hex
        ));
        let parsed: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(parsed["event_type"], "completed");
        assert_eq!(parsed["metadata"]["test"], "true");
    }

    #[tokio::test]
    async fn test_send_test_event_reports_http_failure() {
        let (url, _server) = one_shot_server(500).await;
        let hook = sample(&url);
        let event = build_test_event(&hook, "completed", Uuid::new_v4(), "q").unwrap();
        let outcome = send_test_event(&hook, &event).await.unwrap();
        assert!(!outcome.success);
        assert_eq!(outcome.status, 500);
    }

    #[tokio::test]
    async fn test_send_test_event_connection_refused_is_an_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        drop(listener);
        let hook = sample(&url);
        let event = build_test_event(&hook, "completed", Uuid::new_v4(), "q").unwrap();
        assert!(send_test_event(&hook, &event).await.is_err());
    }
}
