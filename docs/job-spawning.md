# Dynamic Job Spawning

Dynamic job spawning lets a job create child jobs once it has completed, enabling fan-out processing and job hierarchies. It is useful for parallel processing, batch operations and decomposing large tasks into smaller units.

## Overview

The spawning system provides:
- **Parent-child relationships**: spawned children depend on the parent job and share its workflow
- **Trait-based handlers**: implement `SpawnHandler` to decide which children to create
- **Configuration**: a `SpawnConfig` stored in the parent's payload controls limits and what children inherit
- **CLI tools**: `cargo hammerwork spawn` inspects spawn operations and trees
- **Database compatibility**: works with PostgreSQL and MySQL

## How It Works

1. A worker is configured with a `SpawnManager` (`Worker::with_spawn_manager`).
2. A job whose payload contains a `_spawn_config` entry runs and **succeeds**.
3. The worker looks up the spawn handler registered for the job's **queue name**. If there is none, nothing is spawned.
4. The handler's `validate_spawn` is called, then `spawn_jobs` returns the child jobs.
5. If the number of children exceeds `max_spawn_count`, the operation fails with `SpawnError::SpawnLimitExceeded` and nothing is enqueued.
6. The manager applies the inheritance settings, sets each child's `depends_on` to the parent, copies the parent's workflow, enqueues the children and calls `on_spawn_complete`.

A failure to spawn is logged by the worker; it does not fail the parent job, which has already completed.

## Key Concepts

### SpawnHandler Trait

`SpawnHandler<DB>` is an `async_trait` with one required method, `spawn_jobs`, and two optional hooks, `validate_spawn` and `on_spawn_complete`:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use async_trait::async_trait;
use hammerwork::spawn::{SpawnConfig, SpawnContext, SpawnError, SpawnHandler};
use hammerwork::{HammerworkError, Job, Result};

#[derive(Clone)]
pub struct DataProcessingSpawner;

#[async_trait]
impl SpawnHandler<sqlx::Postgres> for DataProcessingSpawner {
    async fn spawn_jobs(&self, context: SpawnContext<sqlx::Postgres>) -> Result<Vec<Job>> {
        let parent_job = &context.parent_job;

        let file_path = parent_job.payload["file_path"].as_str().unwrap_or_default();
        let batch_size = parent_job.payload["batch_size"].as_u64().unwrap_or(1000);
        let count = context.config.max_spawn_count.unwrap_or(10) as u64;

        // Create one child job per batch
        let mut child_jobs = Vec::new();
        for i in 0..count {
            child_jobs.push(Job::new(
                "process_batch".to_string(),
                serde_json::json!({
                    "file_path": file_path,
                    "start_offset": i * batch_size,
                    "end_offset": (i + 1) * batch_size,
                    "batch_id": format!("batch-{}", i)
                }),
            ));
        }

        // The SpawnManager sets each child's dependency on the parent and applies
        // the inheritance settings from SpawnConfig, so there is no need to do it here.
        Ok(child_jobs)
    }

    async fn validate_spawn(&self, parent_job: &Job, _config: &SpawnConfig) -> Result<()> {
        if parent_job.payload.get("file_path").is_none() {
            return Err(HammerworkError::SpawnError(SpawnError::InvalidConfig {
                message: "Parent job missing required file_path".to_string(),
            }));
        }
        Ok(())
    }
}
# Ok(())
# }
```

`SpawnContext` gives the handler `parent_job`, the resolved `config` and a `queue` handle. For simple cases that need no `.await`, wrap a closure in `ClosureSpawnHandler`:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::spawn::{ClosureSpawnHandler, SpawnContext, SpawnManager};
use hammerwork::Job;

let handler = ClosureSpawnHandler::new(|context: SpawnContext<sqlx::Postgres>| {
    let user_ids = context.parent_job.payload["user_ids"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(user_ids
        .into_iter()
        .map(|id| Job::new("send_notification".to_string(), serde_json::json!({"user_id": id})))
        .collect())
});

let mut spawn_manager: SpawnManager<sqlx::Postgres> = SpawnManager::new();
spawn_manager.register_handler("send_notifications", handler);
# Ok(())
# }
```

### SpawnManager

A `SpawnManager` maps a **queue name** to the handler that spawns children for jobs on that queue. Attach it to a worker with `Worker::with_spawn_manager`:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::spawn::{ClosureSpawnHandler, SpawnContext, SpawnManager};
use hammerwork::{Job, Worker};
use std::sync::Arc;

let mut spawn_manager: SpawnManager<sqlx::Postgres> = SpawnManager::new();
spawn_manager.register_handler(
    "data_processing",
    ClosureSpawnHandler::new(|_context: SpawnContext<sqlx::Postgres>| {
        Ok(vec![Job::new("process_batch".to_string(), serde_json::json!({}))])
    }),
);

assert!(spawn_manager.has_handler("data_processing"));
assert_eq!(spawn_manager.registered_types(), vec!["data_processing".to_string()]);

let worker = Worker::new(queue.clone(), "data_processing".to_string(), handler)
    .with_spawn_manager(Arc::new(spawn_manager));
# Ok(())
# }
```

### Job Configuration

The spawn configuration travels in the job payload under the `_spawn_config` key (`hammerwork::spawn::SPAWN_CONFIG_KEY`). The easiest way to set it is `JobSpawnExt`:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::spawn::{JobSpawnExt, SpawnConfig};
use hammerwork::Job;
use serde_json::json;

let spawn_config = SpawnConfig {
    max_spawn_count: Some(15),
    operation_id: Some("op-dataset-123".to_string()),
    ..SpawnConfig::default()
};

// The payload must be a JSON object; anything else is rejected with
// `HammerworkError::InvalidJobPayload` instead of silently losing the configuration.
let job = Job::new("data_processing".to_string(), json!({"dataset_id": "ds-123"}))
    .with_spawn_config(spawn_config)?;

// With the default configuration
let job = Job::new("data_processing".to_string(), json!({"dataset_id": "ds-456"}))
    .with_spawning()?;

queue.enqueue(job).await?;
# Ok(())
# }
```

You can also write the configuration into the payload yourself. Every field of `SpawnConfig` except the optional ones must be present:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::Job;
use hammerwork::spawn::spawn_config_from_payload;
use serde_json::json;

let job = Job::new(
    "data_processing".to_string(),
    json!({
        "dataset_id": "ds-123",
        "records_count": 50000,
        "_spawn_config": {
            "max_spawn_count": 15,
            "inherit_priority": true,
            "inherit_retry_strategy": true,
            "inherit_timeout": false,
            "inherit_trace_context": true,
            "operation_id": "op-dataset-123"
        }
    }),
);

let config = spawn_config_from_payload(&job.payload)?.expect("config present");
assert_eq!(config.max_spawn_count, Some(15));
# Ok(())
# }
```

`spawn_config_from_payload` returns `Ok(None)` when the key is absent and an error when it is present but malformed.

Spawn configuration of encrypted jobs: the worker reads `_spawn_config` from the job's
*decrypted* payload, so spawning works for encrypted jobs, and the spawn handler receives the
decrypted parent. The stored row of a whole-payload-encrypted job holds only ciphertext, so
`cargo hammerwork spawn list/stats/pending` (which query the payload in SQL) do not see
encrypted jobs.

## Configuration Options

### SpawnConfig Fields

`SpawnConfig` is a plain struct with public fields. `SpawnConfig::default()` allows 100 children, inherits priority, retry strategy and trace context, and does not inherit the timeout.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::spawn::SpawnConfig;

let config = SpawnConfig {
    max_spawn_count: Some(50),           // Maximum children one parent may spawn (None = unlimited)
    inherit_priority: true,              // Children get the parent's priority
    inherit_retry_strategy: true,        // Children get the parent's retry strategy
    inherit_timeout: false,              // Children keep their own timeout
    inherit_trace_context: true,         // Children get trace_id, correlation_id, parent_span_id, span_context
    operation_id: Some("custom-op-123".to_string()), // Identifier reported in SpawnResult
};

let defaults = SpawnConfig::default();
assert_eq!(defaults.max_spawn_count, Some(100));
# Ok(())
# }
```

The manager runs the handler first and then applies the inheritance flags, so inherited values overwrite whatever the handler set on the child. If you want children with their own priority, set `inherit_priority: false`.

### Children Use Their Own Queue

Children are enqueued on whatever queue the handler gives them. Spawn handlers are keyed by the parent's queue, so a child on a different queue does not spawn further children unless a handler is registered for that queue too and the child carries its own `_spawn_config`.

## Common Patterns

### Fan-Out Processing

Process a large dataset by splitting it into parallel chunks:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use async_trait::async_trait;
use hammerwork::spawn::{SpawnContext, SpawnHandler};
use hammerwork::{HammerworkError, Job, Result};

#[derive(Clone)]
pub struct DatasetProcessor;

#[async_trait]
impl SpawnHandler<sqlx::Postgres> for DatasetProcessor {
    async fn spawn_jobs(&self, context: SpawnContext<sqlx::Postgres>) -> Result<Vec<Job>> {
        let dataset_size = context.parent_job.payload["size"].as_u64().ok_or_else(|| {
            HammerworkError::InvalidJobPayload {
                message: "payload.size must be a number".to_string(),
            }
        })?;
        let chunk_size = 1000;
        let num_chunks = dataset_size.div_ceil(chunk_size);

        let mut jobs = Vec::new();
        for i in 0..num_chunks {
            let start = i * chunk_size;
            let end = std::cmp::min(start + chunk_size, dataset_size);

            jobs.push(Job::new(
                "process_chunk".to_string(),
                serde_json::json!({
                    "chunk_id": i,
                    "start_index": start,
                    "end_index": end,
                    "dataset_id": context.parent_job.payload["dataset_id"]
                }),
            ));
        }

        Ok(jobs)
    }
}
# Ok(())
# }
```

Remember that `max_spawn_count` applies: a dataset that needs more chunks than the limit makes the whole spawn fail, so raise the limit in the parent's `SpawnConfig` accordingly.

### Image Processing Pipeline

Process images with multiple size variants:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use async_trait::async_trait;
use hammerwork::spawn::{SpawnContext, SpawnHandler};
use hammerwork::{Job, Result};

#[derive(Clone)]
pub struct ImageProcessor;

#[async_trait]
impl SpawnHandler<sqlx::Postgres> for ImageProcessor {
    async fn spawn_jobs(&self, context: SpawnContext<sqlx::Postgres>) -> Result<Vec<Job>> {
        let image_ids: Vec<&str> = context.parent_job.payload["image_ids"]
            .as_array()
            .map(|ids| ids.iter().filter_map(|id| id.as_str()).collect())
            .unwrap_or_default();

        let sizes = ["thumbnail", "medium", "large"];
        let mut jobs = Vec::new();

        for image_id in image_ids {
            for size in sizes {
                jobs.push(Job::new(
                    "resize_image".to_string(),
                    serde_json::json!({
                        "image_id": image_id,
                        "target_size": size,
                        "source_bucket": context.parent_job.payload["source_bucket"],
                        "output_bucket": context.parent_job.payload["output_bucket"]
                    }),
                ));
            }
        }

        Ok(jobs)
    }
}
# Ok(())
# }
```

### Workflow Decomposition

Break a workflow into steps. The manager makes every child depend on the parent and replaces any `depends_on` the handler set, so spawned children are siblings that become runnable together once the parent has completed. If you need the steps to run in a strict order, enqueue them as a workflow instead (see the [workflows guide](workflows.md)).

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use async_trait::async_trait;
use hammerwork::spawn::{SpawnContext, SpawnError, SpawnHandler};
use hammerwork::{HammerworkError, Job, Result};
use serde_json::json;

#[derive(Clone)]
pub struct WorkflowDecomposer;

#[async_trait]
impl SpawnHandler<sqlx::Postgres> for WorkflowDecomposer {
    async fn spawn_jobs(&self, context: SpawnContext<sqlx::Postgres>) -> Result<Vec<Job>> {
        let workflow_type = context.parent_job.payload["workflow_type"]
            .as_str()
            .unwrap_or_default();

        let steps = match workflow_type {
            "etl_pipeline" => vec![
                ("extract", json!({"source": "database"})),
                ("transform", json!({"rules": "business_rules.json"})),
                ("load", json!({"target": "data_warehouse"})),
            ],
            "ml_training" => vec![
                ("preprocess", json!({"features": ["feature_1", "feature_2"]})),
                ("train", json!({"algorithm": "random_forest"})),
                ("validate", json!({"test_split": 0.2})),
                ("deploy", json!({"environment": "staging"})),
            ],
            other => {
                return Err(HammerworkError::SpawnError(SpawnError::InvalidConfig {
                    message: format!("Unknown workflow type: {}", other),
                }));
            }
        };

        Ok(steps
            .into_iter()
            .map(|(step_name, step_config)| Job::new(step_name.to_string(), step_config))
            .collect())
    }
}
# Ok(())
# }
```

## CLI Management

### List Spawn Operations

```bash
# List recent spawn operations
cargo hammerwork spawn list --recent --limit 20

# Filter by queue
cargo hammerwork spawn list --queue data_processing

# Jobs configured to spawn that have not spawned yet
cargo hammerwork spawn pending --queue data_processing --show-config
```

### Visualize Spawn Trees

```bash
# Show spawn tree in text format
cargo hammerwork spawn tree 550e8400-e29b-41d4-a716-446655440000

# Only the children of the job
cargo hammerwork spawn tree 550e8400-e29b-41d4-a716-446655440000 --children-only

# Export as JSON for processing
cargo hammerwork spawn tree 550e8400-e29b-41d4-a716-446655440000 --format json > spawn_tree.json

# Generate Mermaid diagram
cargo hammerwork spawn tree 550e8400-e29b-41d4-a716-446655440000 --format mermaid > diagram.mmd
```

### Monitor Spawn Statistics

```bash
# Get spawn statistics for last 24 hours
cargo hammerwork spawn stats --hours 24 --detailed

# Monitor specific queue
cargo hammerwork spawn stats --queue image_processing --hours 12

# Real-time monitoring
cargo hammerwork spawn monitor --interval 5
```

### Track Job Lineage

```bash
# Show ancestors and descendants
cargo hammerwork spawn lineage 550e8400-e29b-41d4-a716-446655440000 --depth 5

# Show only descendants (children tree)
cargo hammerwork spawn lineage 550e8400-e29b-41d4-a716-446655440000 --descendants

# Show only ancestors (parent chain)
cargo hammerwork spawn lineage 550e8400-e29b-41d4-a716-446655440000 --ancestors
```

The web dashboard has no spawn endpoints; spawn trees are available from the CLI only.

## Performance Considerations

- **PostgreSQL** uses JSONB operators and **MySQL** uses `JSON_CONTAINS()` / `JSON_EXTRACT()` for dependency and payload lookups; index `depends_on` and the payload fields you query.
- Use `max_spawn_count` to bound how many children a single parent can create.
- Each child is enqueued individually, so very large fan-outs are better split across levels (a parent that spawns coordinators, which spawn workers).

## Best Practices

1. **Validate spawn conditions** in `validate_spawn` before creating children.
2. **Set operation IDs** so spawn operations can be traced in `SpawnResult`.
3. **Size `max_spawn_count`** to the largest fan-out you expect.
4. **Bound recursion yourself**: children that carry their own `_spawn_config` can spawn further, so track depth in the payload if you need a limit.
5. **Handle spawn failures**: they are logged, not retried, and do not fail the parent.
6. **Test with both databases** if you run both.

## Error Handling

Return a `HammerworkError` (for example a `SpawnError` wrapped in `HammerworkError::SpawnError`) to abort a spawn:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use async_trait::async_trait;
use hammerwork::spawn::{SpawnContext, SpawnError, SpawnHandler};
use hammerwork::{HammerworkError, Job, Result};

pub struct SafeSpawner;

#[async_trait]
impl SpawnHandler<sqlx::Postgres> for SafeSpawner {
    async fn spawn_jobs(&self, context: SpawnContext<sqlx::Postgres>) -> Result<Vec<Job>> {
        let requested = context.parent_job.payload["count"].as_u64().unwrap_or(0) as usize;

        // Enforce a safety limit of our own
        if requested > 1000 {
            return Err(HammerworkError::SpawnError(SpawnError::SpawnLimitExceeded {
                attempted: requested,
                limit: 1000,
            }));
        }

        let mut jobs = Vec::new();
        for i in 0..requested {
            match build_child(i, &context) {
                Ok(job) => jobs.push(job),
                Err(e) => ::tracing::warn!("Skipping child job {}: {}", i, e),
            }
        }
        Ok(jobs)
    }
}

fn build_child(index: usize, context: &SpawnContext<sqlx::Postgres>) -> Result<Job> {
    let source = context.parent_job.payload["source"].as_str().ok_or_else(|| {
        HammerworkError::InvalidJobPayload {
            message: "payload.source must be a string".to_string(),
        }
    })?;
    Ok(Job::new(
        "child".to_string(),
        serde_json::json!({"index": index, "source": source}),
    ))
}
# Ok(())
# }
```
