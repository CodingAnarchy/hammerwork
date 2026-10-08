# Webhooks

`WebhookManager` (module `hammerwork::webhooks`, feature `webhooks`, on by default)
delivers job lifecycle events to HTTP endpoints, with retries, authentication and
HMAC signatures. (Alert notifications are a separate system; see
[Monitoring & Alerting](monitoring.md).)

```rust
# async fn example() -> hammerwork::Result<()> {
use hammerwork::events::{EventFilter, EventManager, JobLifecycleEventType};
use hammerwork::webhooks::{WebhookConfig, WebhookManager};
use std::{sync::Arc, time::Duration};

let events = Arc::new(EventManager::new_default());
let webhooks = WebhookManager::new_default(events.clone());

let hook = WebhookConfig::new(
    "failures".to_string(),
    "https://hooks.example.com/hammerwork".to_string(),
)
.with_filter(EventFilter::new().with_event_types(vec![JobLifecycleEventType::Failed]))
.with_secret("signing-secret".to_string());
webhooks.add_webhook(hook).await?;

// ... on shutdown:
webhooks.shutdown(Duration::from_secs(10)).await;
# Ok(())
# }
```

Without a template, the request body is the `JobLifecycleEvent` as JSON. When a
`secret` is set, `X-Hammerwork-Signature: sha256=<hex>` is the HMAC-SHA256 of the body
that was actually sent (the rendered template, if one is configured).

## Payload templates

Set `payload_template` (or call `WebhookConfig::with_payload_template`) to send the JSON
a receiver expects instead of the raw event. A template is a JSON document. JSON
strings in it may contain `{{ path }}` placeholders:

```rust
# use hammerwork::webhooks::WebhookConfig;
# fn example(slack_url: String) -> hammerwork::Result<()> {
let hook = WebhookConfig::new("slack".to_string(), slack_url)
    .with_payload_template(r#"{
        "text": "Job {{event.job_id}} on {{event.queue_name}} {{event.event_type}}: {{event.error.message}}",
        "metadata": {
            "job_id": "{{event.job_id}}",
            "attempt": "{{event.error.retry_attempt}}",
            "payload": "{{event.payload}}"
        }
    }"#);
# Ok(())
# }
```

Rendering rules:

- **A string that is exactly one placeholder** (`"{{event.payload}}"`) is replaced by the
  value itself and keeps its JSON type: object, array, number, boolean or `null`.
- **A placeholder inside other text** is replaced by the value's text. Strings are
  inserted without quotes, `null` becomes an empty string, and other values are
  inserted as compact JSON.
- **Event data cannot inject JSON.** Substitution works on the parsed template, never on
  its source text, and values are escaped when the body is serialized. A queue name
  such as `q", "admin": true` stays inside its string.
- Values that are absent from a particular event (such as `event.error.message` on a
  completed job) render as `null`.
- Placeholders are not allowed in object keys, and there is no escape for a literal `{{`.

### Available paths

| Path | Value |
|------|-------|
| `event` | the whole event (the default body) |
| `event.event_id`, `event.job_id` | UUID strings |
| `event.queue_name` | queue name |
| `event.event_type` | `enqueued`, `started`, `completed`, `failed`, `retried`, `dead`, `timed_out`, `cancelled`, `archived`, `restored` |
| `event.priority` | `Background`, `Low`, `Normal`, `High`, `Critical` |
| `event.timestamp` | RFC 3339 timestamp |
| `event.processing_time_ms` | number or `null` |
| `event.error` | error object or `null` |
| `event.error.message`, `.error_type`, `.details`, `.retry_attempt` | error fields |
| `event.payload` | job payload when events include it, otherwise `null` |
| `event.payload.<key>.<key>...` | nested payload values. A numeric segment indexes an array (`event.payload.items.0`) |
| `event.metadata`, `event.metadata.<key>` | event metadata |

### Validation

Templates are validated by `WebhookConfig::validate()`, which runs in
`WebhookManager::add_webhook`, `update_webhook`, `WebhookManager::from_config` and
`HammerworkConfig::from_file`. Invalid JSON, an unknown path (`{{event.nope}}`,
`{{job.id}}`, `{{event.queue_name.x}}`), an unterminated `{{` or a placeholder in an
object key is a `HammerworkError::Config` error. The webhook is not registered, and a
failed `update_webhook` leaves the previous configuration in place.

`PayloadTemplate::parse` and `PayloadTemplate::render` are public, so you can check
or preview a template yourself.
