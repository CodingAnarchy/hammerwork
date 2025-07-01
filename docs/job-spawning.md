# Dynamic Job Spawning

Dynamic job spawning allows jobs to create child jobs during execution, enabling powerful fan-out processing patterns and complex job hierarchies. This feature is particularly useful for parallel processing, batch operations, and decomposing large tasks into smaller, manageable units.

## Overview

The spawning system provides:
- **Parent-child relationships**: Jobs can spawn multiple child jobs with dependency tracking
- **Trait-based architecture**: Extensible spawn handlers for custom spawning logic
- **Configuration management**: Configurable spawn limits, inheritance rules, and batch processing
- **Monitoring & visualization**: CLI tools and web API for tracking spawn trees and operations
- **Database compatibility**: Full support for both PostgreSQL and MySQL

## Key Concepts

### SpawnHandler Trait

The `SpawnHandler` trait defines the interface for implementing custom spawning logic:

```rust
use hammerwork::spawn::{SpawnHandler, SpawnContext, SpawnConfig};
use hammerwork::{Job, Result};

#[derive(Clone)]
pub struct DataProcessingSpawner;

#[async_trait]
impl SpawnHandler<sqlx::Postgres> for DataProcessingSpawner {
    async fn spawn_jobs(&self, context: SpawnContext<sqlx::Postgres>) -> Result<Vec<Job>> {
        let parent_job = &context.parent_job;
        let config = &context.config;
        
        // Extract data from parent job
        let file_path = parent_job.payload["file_path"].as_str().unwrap();
        let batch_size = parent_job.payload["batch_size"].as_u64().unwrap_or(1000);
        
        // Create child jobs for parallel processing
        let mut child_jobs = Vec::new();
        for i in 0..config.max_spawn_count.unwrap_or(10) {
            let child_job = Job::new(
                "process_batch",
                serde_json::json!({
                    "file_path": file_path,
                    "start_offset": i * batch_size,
                    "end_offset": (i + 1) * batch_size,
                    "batch_id": format!("batch-{}", i)
                })
            )
            .with_priority(parent_job.priority) // Inherit priority if configured
            .depends_on_jobs(vec![parent_job.id]); // Set parent dependency
            
            child_jobs.push(child_job);
        }
        
        Ok(child_jobs)
    }
    
    async fn validate_spawn(&self, parent_job: &Job, config: &SpawnConfig) -> Result<()> {
        // Validate that spawning is appropriate for this job
        if !parent_job.payload.get("file_path").is_some() {
            return Err(anyhow::anyhow!("Parent job missing required file_path"));
        }
        Ok(())
    }
}
```

### SpawnManager

The `SpawnManager` registers and manages spawn handlers:

```rust
use hammerwork::spawn::{SpawnManager, SpawnConfig};
use hammerwork::Worker;

// Create spawn manager and register handlers
let mut spawn_manager = SpawnManager::new();
spawn_manager.register_handler("data_processor", DataProcessingSpawner);

// Configure spawning behavior
let spawn_config = SpawnConfig::new()
    .with_max_spawn_count(20)
    .with_inherit_priority(true)
    .with_spawn_delay(Duration::from_millis(100));

// Attach to worker for automatic spawn execution
let worker = Worker::new(queue.clone(), job_handler)
    .with_spawn_manager(spawn_manager)
    .build();
```

### Job Configuration

Add spawn configuration to job payloads:

```rust
use hammerwork::{Job, JobSpawnExt};
use serde_json::json;

// Method 1: Using JobSpawnExt trait
let job = Job::new("large_dataset", json!({"dataset_id": "ds-123"}))
    .with_spawn_handler("data_processor")
    .with_spawn_config(spawn_config);

// Method 2: Manual payload configuration
let job = Job::new(
    "large_dataset",
    json!({
        "dataset_id": "ds-123",
        "records_count": 50000,
        "_spawn_config": {
            "handler": "data_processor",
            "max_spawn_count": 15,
            "operation_id": "op-dataset-123",
            "inherit_priority": true,
            "spawn_delay_ms": 200
        }
    })
);
```

## Configuration Options

### SpawnConfig Parameters

```rust
use hammerwork::spawn::SpawnConfig;
use std::time::Duration;

let config = SpawnConfig::new()
    .with_handler("batch_processor")           // Handler name
    .with_max_spawn_count(50)                  // Maximum children to spawn
    .with_inherit_priority(true)               // Inherit parent's priority
    .with_inherit_queue(false)                 // Use different queue for children
    .with_spawn_delay(Duration::from_millis(100))  // Delay between spawning children
    .with_operation_id("custom-op-123")        // Custom operation identifier
    .with_batch_size(10);                      // Process children in batches
```

### Advanced Configuration

```rust
// Complex spawn configuration with metadata
let advanced_config = SpawnConfig::new()
    .with_handler("image_processor")
    .with_max_spawn_count(100)
    .with_inherit_priority(true)
    .with_metadata(json!({
        "image_format": "jpeg",
        "quality_settings": {
            "thumbnail": "low",
            "preview": "medium", 
            "full": "high"
        },
        "output_bucket": "processed-images"
    }));
```

## Common Patterns

### Fan-Out Processing

Process large datasets by splitting into parallel chunks:

```rust
#[derive(Clone)]
pub struct DatasetProcessor;

#[async_trait]
impl SpawnHandler<sqlx::Postgres> for DatasetProcessor {
    async fn spawn_jobs(&self, context: SpawnContext<sqlx::Postgres>) -> Result<Vec<Job>> {
        let dataset_size = context.parent_job.payload["size"].as_u64().unwrap();
        let chunk_size = 1000;
        let num_chunks = (dataset_size + chunk_size - 1) / chunk_size; // Ceiling division
        
        let mut jobs = Vec::new();
        for i in 0..num_chunks {
            let start = i * chunk_size;
            let end = std::cmp::min(start + chunk_size, dataset_size);
            
            let job = Job::new(
                "process_chunk",
                serde_json::json!({
                    "chunk_id": i,
                    "start_index": start,
                    "end_index": end,
                    "dataset_id": context.parent_job.payload["dataset_id"]
                })
            ).with_priority(context.parent_job.priority);
            
            jobs.push(job);
        }
        
        Ok(jobs)
    }
}
```

### Image Processing Pipeline

Process images with multiple size variants:

```rust
#[derive(Clone)]
pub struct ImageProcessor;

#[async_trait]
impl SpawnHandler<sqlx::Postgres> for ImageProcessor {
    async fn spawn_jobs(&self, context: SpawnContext<sqlx::Postgres>) -> Result<Vec<Job>> {
        let image_ids = context.parent_job.payload["image_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap())
            .collect::<Vec<_>>();
        
        let sizes = ["thumbnail", "medium", "large"];
        let mut jobs = Vec::new();
        
        for image_id in image_ids {
            for &size in &sizes {
                let job = Job::new(
                    "resize_image",
                    serde_json::json!({
                        "image_id": image_id,
                        "target_size": size,
                        "source_bucket": context.parent_job.payload["source_bucket"],
                        "output_bucket": context.parent_job.payload["output_bucket"]
                    })
                );
                jobs.push(job);
            }
        }
        
        Ok(jobs)
    }
}
```

### Workflow Decomposition

Break complex workflows into manageable steps:

```rust
#[derive(Clone)]
pub struct WorkflowDecomposer;

#[async_trait]
impl SpawnHandler<sqlx::Postgres> for WorkflowDecomposer {
    async fn spawn_jobs(&self, context: SpawnContext<sqlx::Postgres>) -> Result<Vec<Job>> {
        let workflow_type = context.parent_job.payload["workflow_type"].as_str().unwrap();
        
        let steps = match workflow_type {
            "etl_pipeline" => vec![
                ("extract", json!({"source": "database"})),
                ("transform", json!({"rules": "business_rules.json"})),
                ("load", json!({"target": "data_warehouse"}))
            ],
            "ml_training" => vec![
                ("preprocess", json!({"features": ["feature_1", "feature_2"]})),
                ("train", json!({"algorithm": "random_forest"})),
                ("validate", json!({"test_split": 0.2})),
                ("deploy", json!({"environment": "staging"}))
            ],
            _ => return Err(anyhow::anyhow!("Unknown workflow type: {}", workflow_type))
        };
        
        let mut jobs = Vec::new();
        let mut previous_job_id = Some(context.parent_job.id);
        
        for (step_name, step_config) in steps {
            let mut job = Job::new(step_name, step_config);
            
            if let Some(prev_id) = previous_job_id {
                job = job.depends_on_jobs(vec![prev_id]);
            }
            
            previous_job_id = Some(job.id);
            jobs.push(job);
        }
        
        Ok(jobs)
    }
}
```

## CLI Management

### List Spawn Operations

```bash
# List all recent spawn operations
cargo hammerwork spawn list --recent --limit 20

# Filter by queue
cargo hammerwork spawn list --queue data_processing

# Show detailed information
cargo hammerwork spawn list --detailed
```

### Visualize Spawn Trees

```bash
# Show spawn tree in text format
cargo hammerwork spawn tree 550e8400-e29b-41d4-a716-446655440000

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

## Web API Integration

### REST Endpoints

```bash
# Get spawn operations
curl "http://localhost:8080/api/spawn/operations?limit=50"

# Get spawn tree for job
curl "http://localhost:8080/api/jobs/550e8400-e29b-41d4-a716-446655440000/spawn-tree?format=json"

# Get spawn statistics
curl "http://localhost:8080/api/spawn/stats?queue=data_processing"

# List child jobs
curl "http://localhost:8080/api/jobs/550e8400-e29b-41d4-a716-446655440000/children"
```

### JavaScript Client Example

```javascript
class SpawnManager {
    constructor(baseUrl) {
        this.baseUrl = baseUrl;
    }
    
    async getSpawnTree(jobId, format = 'json', maxDepth = 10) {
        const response = await fetch(
            `${this.baseUrl}/api/jobs/${jobId}/spawn-tree?format=${format}&max_depth=${maxDepth}`
        );
        return response.json();
    }
    
    async getSpawnStats(queue = null, hours = 24) {
        const params = new URLSearchParams();
        if (queue) params.append('queue', queue);
        params.append('hours', hours);
        
        const response = await fetch(`${this.baseUrl}/api/spawn/stats?${params}`);
        return response.json();
    }
    
    async getSpawnOperations(limit = 50) {
        const response = await fetch(`${this.baseUrl}/api/spawn/operations?limit=${limit}`);
        return response.json();
    }
}

// Usage
const spawnManager = new SpawnManager('http://localhost:8080');
const spawnTree = await spawnManager.getSpawnTree('550e8400-e29b-41d4-a716-446655440000');
console.log('Spawn tree:', spawnTree);
```

## Performance Considerations

### Database Optimization

- **PostgreSQL**: Uses `@>` operator and JSONB queries for efficient dependency tracking
- **MySQL**: Uses `JSON_CONTAINS()` and `JSON_EXTRACT()` functions for compatibility
- **Indexing**: Ensure proper indexes on `depends_on` and payload JSON fields

### Spawn Limits

Configure appropriate limits to prevent resource exhaustion:

```rust
let config = SpawnConfig::new()
    .with_max_spawn_count(100)        // Limit total children
    .with_batch_size(10)              // Process in batches
    .with_spawn_delay(Duration::from_millis(50)); // Rate limiting
```

### Memory Management

- Use streaming for large spawn operations
- Consider database connection pool limits
- Monitor worker memory usage with large spawn trees

## Best Practices

1. **Validate Spawn Conditions**: Always validate in `validate_spawn()` before creating children
2. **Use Operation IDs**: Include operation IDs for tracking and debugging
3. **Monitor Performance**: Track spawn success rates and processing times
4. **Limit Depth**: Prevent infinite recursion with depth limits
5. **Graceful Degradation**: Handle spawn failures without breaking parent job
6. **Resource Limits**: Set appropriate limits for spawn count and frequency
7. **Database Compatibility**: Test with both PostgreSQL and MySQL if using both

## Error Handling

```rust
impl SpawnHandler<sqlx::Postgres> for SafeSpawner {
    async fn spawn_jobs(&self, context: SpawnContext<sqlx::Postgres>) -> Result<Vec<Job>> {
        // Validate before spawning
        self.validate_spawn(&context.parent_job, &context.config).await?;
        
        // Check resource limits
        if context.config.max_spawn_count.unwrap_or(0) > 1000 {
            return Err(anyhow::anyhow!("Spawn count exceeds safety limit"));
        }
        
        // Handle failures gracefully
        let mut successful_jobs = Vec::new();
        let mut failed_count = 0;
        
        for i in 0..context.config.max_spawn_count.unwrap_or(10) {
            match self.create_child_job(i, &context).await {
                Ok(job) => successful_jobs.push(job),
                Err(e) => {
                    failed_count += 1;
                    tracing::warn!("Failed to create child job {}: {}", i, e);
                    
                    // Fail if too many errors
                    if failed_count > 3 {
                        return Err(anyhow::anyhow!("Too many spawn failures"));
                    }
                }
            }
        }
        
        Ok(successful_jobs)
    }
}
```

This spawning system provides a robust foundation for implementing complex job processing patterns while maintaining performance and reliability across different database backends.