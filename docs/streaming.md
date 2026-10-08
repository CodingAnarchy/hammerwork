# Event Streaming

`StreamManager` (module `hammerwork::streaming`) forwards job lifecycle events from an
`EventManager` to Kafka, Amazon Kinesis or Google Cloud Pub/Sub. Each backend needs its
cargo feature: `kafka`, `kinesis` or `google-pubsub`. Adding a stream for a backend
whose feature is disabled fails with a configuration error.

```rust
# async fn example() -> hammerwork::Result<()> {
use hammerwork::events::{EventFilter, EventManager, JobLifecycleEventType};
use hammerwork::streaming::{StreamBackend, StreamConfig, StreamManager, StreamRetryPolicy};
use std::{collections::HashMap, sync::Arc, time::Duration};

let events = Arc::new(EventManager::new_default());
let streams = StreamManager::new_default(events.clone());

let stream = StreamConfig::new(
    "job-events".to_string(),
    StreamBackend::Kafka {
        brokers: vec!["localhost:9092".to_string()],
        topic: "hammerwork-events".to_string(),
        config: HashMap::new(),
    },
)
.with_filter(EventFilter::new().with_event_types(vec![JobLifecycleEventType::Failed]))
.with_retry_policy(StreamRetryPolicy {
    max_attempts: 5,
    initial_delay_secs: 1,
    max_delay_secs: 60,
    backoff_multiplier: 2.0,
    use_jitter: true,
});
streams.add_stream(stream).await?;

// ... on shutdown:
streams.shutdown(Duration::from_secs(10)).await;
# Ok(())
# }
```

## Delivery

Each stream has a listener that buffers matching events and flushes a batch when it
holds `buffer_config.batch_size` events, when `max_buffer_time_secs` has passed, or when
it holds `max_events`. A batch is serialized with the stream's `serialization` format
and handed to the backend processor's `send_batch`, which reports a result per event.
The bytes produced by the serialization format are what the backend receives, for
every backend.

### Retries

Events that fail are retried according to the stream's `StreamRetryPolicy`:

- `max_attempts` is the total number of attempts per event, including the first
  (`0` is treated as `1`).
- Only the events that failed are sent again. If `send_batch` returns an error, the
  whole batch failed. An event the processor returns no result for counts as failed.
- The delay before retry *n* is `initial_delay_secs * backoff_multiplier^(n-1)`, capped at
  `max_delay_secs`. With `use_jitter` the delay is drawn from `[delay / 2, delay]`.
  `StreamRetryPolicy::delay_for_retry(n)` returns it.

### Shutdown

`StreamManager::shutdown(grace)` stops the listeners, then waits up to `grace` for
in-flight batches. A send that is already in progress is allowed to finish, but no
retry starts after shutdown begins, and a batch waiting out a retry delay stops waiting
right away. Events still waiting for a retry are counted as failed, and `last_error`
says that the manager shut down. Batches still running after `grace` are aborted.
Events buffered in a listener that has not flushed yet are dropped.

## Statistics

`get_stream_stats(stream_id)`, `get_all_stream_stats()` and `get_stats()` report what
actually happened. They are updated once per batch, after its retries:

| Field | Meaning |
|-------|---------|
| `total_events` | events that finished (delivered or given up on) |
| `successful_deliveries` | events the backend acknowledged |
| `failed_deliveries` | events that could not be serialized, or were not delivered after all attempts or before shutdown |
| `success_rate` | `successful / (successful + failed)` |
| `avg_delivery_time_ms` | average delivery time of acknowledged events |
| `last_success_at`, `last_failure_at` | when the last batch with a success / failure finished |
| `last_error` | the error behind the most recent failure |

## Health checks

`StreamProcessor::health_check` checks the backend without sending events:

- **Kafka** fetches topic metadata within `health.check.timeout.ms` (default 5000).
  `Ok(false)` if the broker is unreachable or the topic is missing.
- **Kinesis** describes the stream.
- **Pub/Sub** fetches the topic within `health.check.timeout.ms` (default 5000).
  `Ok(true)` if it exists. `Ok(false)` if it does not exist, Pub/Sub is unavailable, or
  the check times out. An error if Pub/Sub rejects the request (for example
  `PERMISSION_DENIED` or `UNAUTHENTICATED`), because retrying does not fix that.

## Custom backends

Implement `StreamProcessor` for your own backend. `send_batch` must return one
`StreamDelivery` per event, matched by `event_id`, with `success` set to the real
outcome. Return `Err` only when the whole batch failed.
