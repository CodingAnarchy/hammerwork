use anyhow::Result;
use clap::Subcommand;
use serde_json::json;
use std::collections::HashMap;
use tracing::{error, info};
use uuid::Uuid;

use crate::config::Config;
use crate::utils::display::create_table;

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
    #[command(about = "Show webhook statistics")]
    Stats {
        #[arg(short, long, help = "Webhook ID or name")]
        webhook: Option<String>,
        #[arg(long, help = "Time window in hours", default_value = "24")]
        hours: u64,
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
        WebhookCommand::Stats { webhook, hours } => {
            show_webhook_stats(config, webhook, hours).await
        }
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
            println!("  Timeout: {}s", webhook.timeout);
            println!("  Max Retries: {}", webhook.max_retries);

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
        error!("❌ Use --confirm to confirm webhook removal");
        return Ok(());
    }

    let mut webhooks = load_webhooks_config(config)?;
    let initial_len = webhooks.len();

    webhooks.retain(|w| w.name != webhook_id && w.id.to_string() != webhook_id);

    if webhooks.len() == initial_len {
        error!("❌ Webhook '{}' not found", webhook_id);
        return Ok(());
    }

    save_webhooks_config(config, webhooks)?;
    info!("✅ Webhook '{}' removed successfully", webhook_id);
    Ok(())
}

async fn test_webhook(
    _config: &Config,
    webhook_id: String,
    event_type: String,
    job_id: Option<String>,
    queue: String,
) -> Result<()> {
    // Create a test event
    let test_event = json!({
        "event_type": event_type,
        "job_id": job_id.unwrap_or_else(|| Uuid::new_v4().to_string()),
        "queue_name": queue,
        "priority": "Normal",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "test": true
    });

    info!("🧪 Testing webhook '{}' with event:", webhook_id);
    println!("{}", serde_json::to_string_pretty(&test_event)?);

    // TODO: Implement actual webhook testing by loading configuration and sending request
    info!("✅ Test event would be sent to webhook (implementation pending)");
    Ok(())
}

async fn show_webhook_stats(_config: &Config, webhook: Option<String>, hours: u64) -> Result<()> {
    // TODO: Implement webhook statistics display
    if let Some(webhook_id) = webhook {
        info!(
            "📊 Statistics for webhook '{}' (last {} hours):",
            webhook_id, hours
        );
    } else {
        info!("📊 Statistics for all webhooks (last {} hours):", hours);
    }

    // Placeholder for webhook statistics
    println!("Total deliveries: 0");
    println!("Successful: 0");
    println!("Failed: 0");
    println!("Success rate: 0%");
    println!("Average response time: 0ms");

    Ok(())
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
        None => {
            error!("❌ Webhook '{}' not found", webhook_id);
        }
    }

    Ok(())
}

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
                w.timeout = new_timeout;
            }
            if let Some(new_max_retries) = max_retries {
                w.max_retries = new_max_retries;
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
        None => {
            error!("❌ Webhook '{}' not found", webhook_id);
        }
    }

    Ok(())
}

// Helper functions for webhook configuration

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
) -> Result<WebhookConfigEntry> {
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

    Ok(WebhookConfigEntry {
        id: Uuid::new_v4(),
        name,
        url,
        method: http_method,
        headers: parsed_headers,
        filter,
        retry_policy,
        auth,
        timeout,
        enabled: true,
        secret,
        max_retries,
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

#[derive(serde::Serialize, serde::Deserialize)]
struct WebhookConfigEntry {
    id: Uuid,
    name: String,
    url: String,
    method: hammerwork::HttpMethod,
    headers: HashMap<String, String>,
    filter: hammerwork::events::EventFilter,
    retry_policy: hammerwork::webhooks::RetryPolicy,
    auth: Option<hammerwork::webhooks::WebhookAuth>,
    timeout: u64,
    enabled: bool,
    secret: Option<String>,
    max_retries: u32,
}

fn load_webhooks_config(_config: &Config) -> Result<Vec<WebhookConfigEntry>> {
    // TODO: Implement loading from configuration file
    Ok(Vec::new())
}

fn save_webhook_config(_config: &Config, _webhook: WebhookConfigEntry) -> Result<()> {
    // TODO: Implement saving to configuration file
    info!("Webhook configuration would be saved (implementation pending)");
    Ok(())
}

fn save_webhooks_config(_config: &Config, _webhooks: Vec<WebhookConfigEntry>) -> Result<()> {
    // TODO: Implement saving to configuration file
    info!("Webhooks configuration would be saved (implementation pending)");
    Ok(())
}
