# Upgrading from 2.x to 3.0

Hammerwork 3.0 is a major release. This guide lists each breaking change, what it
affects and what you need to change in your code, configuration or deployment. Read
it together with the `3.0.0` entry in the [CHANGELOG](../CHANGELOG.md).

## Upgrade order

Do the upgrade in this order. Each step links to the section with the details.

1. **Stop every Hammerwork 1.x worker**, and anything else built on 1.x that claims
   jobs. 3.0 never reclaims a `Running` job that has no lease, and 1.x workers claimed
   jobs without one ([Stop every 1.x worker first](#stop-every-1x-worker-first)).
2. **Update your code and build.** Use Rust 1.94 and sqlx 0.9
   ([sqlx 0.9 and Rust 1.94](#sqlx-09-and-rust-194)), name the Cargo features you use
   ([Feature flags](#feature-flags)), and fix the breaking API changes this guide lists.
3. **Drain the 2.x workers, back up the database and run `cargo hammerwork migration
   run` off-peak.** On MySQL, migration 021 rewrites every table and blocks writes to it
   while it does ([MySQL timestamps](#mysql-timestamps-migration-021)). Confirm the
   result with `cargo hammerwork migration status`, which lists the applied and
   pending migrations.
4. **Run `cargo hammerwork maintenance check --fix`** to give the `Running` jobs a 1.x
   worker left behind a lease. Preview what the reaper would reclaim with
   `cargo hammerwork job requeue-stale --dry-run` (add `--queue <name>` to look at one
   queue).
5. **Start the 3.0 workers.**

## sqlx 0.9 and Rust 1.94

Hammerwork 3.0 depends on [sqlx](https://crates.io/crates/sqlx) 0.9 instead of 0.8,
and its minimum supported Rust version (MSRV) is now **1.94.0** (it was 1.91.1).
sqlx 0.9 requires Rust 1.94.

sqlx is part of Hammerwork's public API. You create the `sqlx::Pool` that
`JobQueue::new` takes, `JobQueue<DB>` and `DatabaseQueue` are generic over
`sqlx::Database`, the `MigrationRunner`s take sqlx pools, and `JobStatus` and
`BatchStatus` implement sqlx's `Type`, `Encode` and `Decode`. Your application must
therefore use the same sqlx major version as Hammerwork. If it still depends on sqlx
0.8, the build fails with errors such as "expected `Pool<Postgres>`, found
`Pool<Postgres>`", because two different versions of the `sqlx` crate are in the
dependency graph.

### What to change

1. **Use Rust 1.94.0 or newer.**

2. **Upgrade your own sqlx dependency to 0.9.** sqlx 0.9 removed the combined
   runtime and TLS features, so name them separately:

   ```toml
   # Before (2.x)
   sqlx = { version = "0.8", features = ["runtime-tokio-rustls", "postgres"] }

   # After (3.0)
   sqlx = { version = "0.9", features = ["runtime-tokio", "tls-rustls-ring-webpki", "postgres"] }
   ```

   `runtime-tokio-native-tls` becomes `runtime-tokio` plus `tls-native-tls`.
   Hammerwork itself enables `runtime-tokio` and `tls-rustls-ring-webpki`.

3. **Wrap dynamic SQL strings in your own queries.** sqlx 0.9's `query`,
   `query_as`, `query_scalar`, `raw_sql` and `Executor::execute` accept only string
   literals (`&'static str`) unless you mark the string as reviewed:

   ```rust,no_run
   # async fn example(pool: sqlx::PgPool, table: &str) -> Result<(), sqlx::Error> {
   // Before (no longer compiles):
   // sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE status = $1"))
   let pending: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
       "SELECT COUNT(*) FROM {table} WHERE status = $1"
   )))
   .bind("Pending")
   .fetch_one(&pool)
   .await?;
   # Ok(())
   # }
   ```

   This affects only SQL your application writes itself, for example reporting
   queries against `hammerwork_jobs`. No Hammerwork API takes an SQL string.

4. **MySQL without TLS: enable `mysql-rsa` if you need it.** sqlx 0.9 moved the RSA
   password encryption used by MySQL's `caching_sha2_password` and `sha256_password`
   authentication over unencrypted connections behind a new `mysql-rsa` feature.
   Without it, connecting with `ssl-mode=disabled`, or to a server without TLS,
   fails with "RSA auth backend disabled" whenever the server needs a full
   authentication (for example the first login after a server restart). The
   library does not enable `mysql-rsa`. If you connect without TLS, either enable
   TLS or add the feature to your own sqlx dependency; Cargo's feature unification
   turns it on for Hammerwork too:

   ```toml
   sqlx = { version = "0.9", features = ["runtime-tokio", "tls-rustls-ring-webpki", "mysql", "mysql-rsa"] }
   ```

   The `cargo-hammerwork` CLI and the `hammerwork-web` dashboard enable `mysql-rsa`,
   so they connect as before.

5. **MySQL connection collation.** sqlx 0.8 ran `SET NAMES utf8mb4 COLLATE
   utf8mb4_unicode_ci` on every new MySQL connection. sqlx 0.9 runs `SET NAMES
   utf8mb4` and lets the server pick its default collation (`utf8mb4_0900_ai_ci` on
   MySQL 8.0). Hammerwork's queries compare columns with parameters, where the column
   collation applies, so Hammerwork behaves the same. Queries of your own that
   compare string literals or parameters with each other may sort or match
   differently. To keep the old behaviour, set it on the connect options:
   `MySqlConnectOptions::from_str(url)?.collation("utf8mb4_unicode_ci")`.

For other sqlx 0.9 changes that may affect your own sqlx code (`RawSql`,
`IntoArguments` and `Arguments` lost their lifetime parameter, the `Migrate` trait,
`sqlx.toml`), see the
[sqlx 0.9.0 changelog](https://github.com/launchbadge/sqlx/blob/main/CHANGELOG.md#090---2026-05-06).

### What does not change

- The sqlx upgrade itself needs no database migration. Hammerwork's migrations, and the
  way they are run (one statement at a time, over the simple text protocol on MySQL), are
  the same. (3.0 has its own MySQL migration, 021; see
  [MySQL timestamps](#mysql-timestamps-migration-021).)
- Timestamps decode as before: PostgreSQL `TIMESTAMPTZ` and MySQL `TIMESTAMP(6)` or
  `DATETIME(6)` columns map to `chrono::DateTime<Utc>`, and sqlx still sets the MySQL session
  `time_zone` to `+00:00`.
- Hammerwork's own types and function signatures are unchanged by this upgrade;
  only the sqlx version behind them moves.

## Feature flags

Three changes to the Cargo features of `hammerwork` (#92):

1. **No default features.** In 2.x, `default = ["metrics", "alerting", "webhooks"]`. In 3.0, `default = []`, so a plain `hammerwork` dependency builds only the queue, worker and job APIs, without `warp`, `reqwest`, `lettre` or `prometheus`.
2. **`azure-kv` is now `azure-kms`**, like `aws-kms`, `gcp-kms` and `vault-kms`. There is no alias. The `cargo-hammerwork` crate renames its own `azure-kv` feature the same way, so `cargo install cargo-hammerwork --features azure-kv` becomes `--features azure-kms`.
3. **`test` is now `test-queue`.** There is no alias.

New: a `full` feature that enables `metrics`, `alerting`, `webhooks`, `streaming`, `tracing` and `encryption`. It does not include a database backend (`postgres`, `mysql`), a KMS key source, an event stream sink (`kafka`, `google-pubsub`, `kinesis`) or `test-queue`.

### What to change

If your 2.x `Cargo.toml` relied on the defaults, name the features you use:

```toml
# 2.x (implicitly metrics + alerting + webhooks)
hammerwork = { version = "2", features = ["postgres"] }

# 3.0, same behaviour
hammerwork = { version = "3", features = ["postgres", "metrics", "alerting", "webhooks"] }

# 3.0, or everything that is not a backend, KMS source or event sink
hammerwork = { version = "3", features = ["postgres", "full"] }
```

If you used `default-features = false`, you can drop it.

Rename the other features:

```toml
# 2.x
hammerwork = { version = "2", features = ["postgres", "encryption", "azure-kv"] }
[dev-dependencies]
hammerwork = { version = "2", features = ["test"] }

# 3.0
hammerwork = { version = "3", features = ["postgres", "encryption", "azure-kms"] }
[dev-dependencies]
hammerwork = { version = "3", features = ["test-queue"] }
```

A compile error such as `unresolved import hammerwork::metrics`, `hammerwork::alerting` or `hammerwork::webhooks`, or a missing `TestQueue`, means a feature is not enabled. `cargo-hammerwork` and `hammerwork-web` already enable the features they need.

If you gate your own code with `cfg(feature = "...")` on features that you forward to hammerwork, rename them in the same way.

### Configuration sections and worker events

The `[webhooks]`, `[streaming]`, `[alerting]` and `[metrics]` sections of a
`HammerworkConfig` file follow the features (#130):

- With the feature enabled, the section is optional (it was required): a file saved by
  a build without the feature, which writes no such section, loads.
- Without the feature, `HammerworkConfig::from_file` now fails with a
  `HammerworkError::Config` such as "[alerting] is set but hammerwork was built without
  the \`alerting\` feature" instead of silently ignoring the section (`[streaming]` is
  accepted with any of `streaming`, `kafka`, `google-pubsub` or `kinesis`). Enable the
  feature, or delete the section.

`Worker::with_event_manager` no longer needs the `webhooks` feature: workers publish
lifecycle events to an `EventManager` in every build, so streaming-only builds (and
plain `EventManager` subscribers) get them too. In 2.x this worked only because
`webhooks` was a default feature.

`config::SimpleEventFilter`, which existed only in builds without `webhooks` and was
never used, is removed. Use `events::EventFilter`.

## cargo-hammerwork and hammerwork-web as libraries

In 2.x both companion crates exported their internals as library API. In 3.0 they do not,
so their modules can change without a semver break.

### cargo-hammerwork

`cargo-hammerwork` is a binary only. The `cargo_hammerwork::commands`,
`cargo_hammerwork::config` and `cargo_hammerwork::utils` modules (and the glob re-exports
of them at the crate root) no longer exist, and the crate has no library target to depend
on. Use the CLI through the `cargo hammerwork` executable. If you need job, queue or
migration operations from Rust code, use the `hammerwork` crate (`DatabaseQueue`,
`JobQueue`) directly.

#### CLI behavior changes

- `job requeue-stale` takes `--queue <name>` and `--dry-run`. The dry run counts the
  jobs the lease rule would reclaim and changes nothing.
- `monitor health` and `queue health` no longer call a `Running` job stuck because it
  started more than an hour ago. A job is stuck when its lease expired, the rule the
  reaper uses; a long-running job that keeps heartbeating is healthy. `Running` jobs
  without a lease (left by a 1.x worker) are reported on their own row
  (`Lease-less Running Jobs`) and are not counted as stuck. The JSON output of
  `monitor health` gains that check.
- `migration status` lists the migrations this version registers as applied or pending,
  from the `hammerwork_migrations` table, instead of the columns of `hammerwork_jobs`.
  Applied migrations this version does not know are listed separately.

### hammerwork-web

The public API is `WebDashboard`, `DashboardConfig`, `AuthConfig` and `WebSocketConfig`,
all re-exported at the crate root. The `api`, `auth`, `config`, `live`, `security`,
`server` and `websocket` modules are private, including `server::app_routes`, which needed
other private types (`AuthState`, `SystemState`, `WebSocketState`, `AllowedOrigins`) to call.

To embed the dashboard in your own process, build a `DashboardConfig` and run a
`WebDashboard` (optionally `with_event_manager` to forward worker events):

```rust,ignore
use hammerwork_web::{DashboardConfig, WebDashboard};

let config = DashboardConfig::new()
    .with_database_url("postgresql://localhost/hammerwork");
WebDashboard::new(config).await?.start().await?;
```

Migrate imports as follows:

| 2.x path | 3.0 |
| --- | --- |
| `hammerwork_web::config::DashboardConfig` | `hammerwork_web::DashboardConfig` |
| `hammerwork_web::config::AuthConfig` | `hammerwork_web::AuthConfig` |
| `hammerwork_web::config::WebSocketConfig` | `hammerwork_web::WebSocketConfig` |
| `hammerwork_web::server::WebDashboard` | `hammerwork_web::WebDashboard` |
| `hammerwork_web::server::app_routes`, `api::*`, `auth::*`, `security::*`, `websocket::*`, `live::*` | removed; mount the dashboard with `WebDashboard` |

### Dashboard metrics endpoint

`GET /api/system/metrics` no longer returns `metrics_endpoint`, `custom_metrics_count`
or `last_scrape`. The dashboard serves no `/metrics` route and keeps no metrics registry,
so they were always `"/metrics"`, `null` and `null`. Only `prometheus_enabled` (always
`false`) remains.

## DatabaseQueue is sealed

`DatabaseQueue` is now a sealed trait (#87): only Hammerwork can implement it. Its
implementations are `JobQueue<Postgres>` (feature `postgres`), `JobQueue<MySql>`
(feature `mysql`) and the in-memory `TestQueue` (feature `test-queue`). Because no
other implementations exist, new methods can be added in minor releases without
default bodies, and the defaults that existed only for implementations outside the
crate are gone. The built-in queues behave as before.

Code that only *uses* a queue is unaffected: `DatabaseQueue` is still the bound for
generic code and the trait to import for its methods.

```rust
use hammerwork::queue::DatabaseQueue;

// Still works: generic over any built-in queue.
async fn pending_count(queue: &impl DatabaseQueue, name: &str) -> hammerwork::Result<u64> {
    Ok(queue.get_queue_stats(name).await?.pending_count)
}
```

### What to change

If you implemented `DatabaseQueue` yourself, usually as a mock for tests, that no
longer compiles (`the trait bound ...: Sealed is not satisfied`). Use `TestQueue`
instead. It implements the whole trait in memory, with the same claim, lease, retry,
workflow and archive behaviour as the database backends, and a `MockClock` for
delayed and cron jobs (see [Testing](testing.md)):

```toml
[dev-dependencies]
hammerwork = { version = "3", features = ["test-queue"] }
```

```rust
// Before (2.x): a hand-written mock
// struct MockQueue { /* ... */ }
// #[async_trait] impl DatabaseQueue for MockQueue { /* ... */ }

// After (3.0)
use hammerwork::queue::{DatabaseQueue, test::TestQueue};

# async fn example() -> hammerwork::Result<()> {
let queue = TestQueue::new();
let id = queue
    .enqueue(hammerwork::Job::new("emails".into(), serde_json::json!({})))
    .await?;
assert!(queue.get_job(id).await?.is_some());
# Ok(())
# }
```

`hammerwork::queue::RELEASED_JOB_MESSAGE` is removed: it was the error message of the
`release_job_run` fallback for queues outside the crate. The built-in queues release
a job without recording an error.

## Claiming jobs and worker builders

The claim API and two worker builders were consolidated (#89).

### One claim method

`DatabaseQueue::dequeue` now takes a [`DequeueOptions`](https://docs.rs/hammerwork/3/hammerwork/queue/struct.DequeueOptions.html)
and returns the claimed jobs as a `Vec<Job>`. It replaces `dequeue`,
`dequeue_with_priority_weights`, `dequeue_leased`, `dequeue_batch` and
`dequeue_batch_leased`. For the most common call, claiming one job with the defaults,
use `dequeue_one`, which is the 2.x `dequeue`:

| 2.x | 3.0 |
| --- | --- |
| `dequeue(q)` | `dequeue_one(q)` |
| `dequeue_with_priority_weights(q, &w)` | `dequeue(q, &DequeueOptions::new().with_priority_weights(w.clone()))` |
| `dequeue_leased(q, None, lease)` | `dequeue(q, &DequeueOptions::new().with_lease_duration(lease))` |
| `dequeue_batch(q, n)` | `dequeue(q, &DequeueOptions::batch(n))` |
| `dequeue_batch_leased(q, Some(&w), lease, n)` | `dequeue(q, &DequeueOptions::batch(n).with_priority_weights(w.clone()).with_lease_duration(lease))` |

The single-job forms returned `Option<Job>`; `dequeue` returns a `Vec<Job>` with at
most one job when `max_jobs` is 1 (the default). Claims behave exactly as before: the
same statements, priority selection, leases and pause checks.

```rust
use hammerwork::queue::{DatabaseQueue, DequeueOptions};
use hammerwork::PriorityWeights;
use std::time::Duration;

# async fn example(queue: &impl DatabaseQueue) -> hammerwork::Result<()> {
// Before (2.x):
// let job = queue.dequeue("emails").await?;
// let job = queue.dequeue_with_priority_weights("emails", &weights).await?;
// let jobs = queue.dequeue_batch_leased("emails", None, Duration::from_secs(60), 20).await?;

// After (3.0):
let job = queue.dequeue_one("emails").await?;

let weights = PriorityWeights::new();
let options = DequeueOptions::new().with_priority_weights(weights);
let job = queue.dequeue("emails", &options).await?.into_iter().next();

let options = DequeueOptions::batch(20).with_lease_duration(Duration::from_secs(60));
let jobs = queue.dequeue("emails", &options).await?;
# let _ = (job, jobs);
# Ok(())
# }
```

`DequeueOptions` is `#[non_exhaustive]`: build it with `DequeueOptions::new()` (or
`default()`) or `DequeueOptions::batch(n)` and the `with_*` methods. Its fields are
public, so you can still read them or assign them, for example
`options.priority_weights = maybe_weights;`.

### Batch mode is set by the batch size

`Worker::with_batch_processing_enabled` is removed. `with_batch_size(n)` already enabled
batch mode and is now the only switch. To keep `with_batch_processing_enabled(true)`'s
behaviour (batch statistics, one job claimed per poll), use `with_batch_size(1)`. To
turn batch mode off, do not call `with_batch_size`.

Batch settings can now also come from configuration: the optional `[worker]` keys
`batch_size` and `batch_concurrency` (`WorkerConfig::batch_size` and
`WorkerConfig::batch_concurrency`, both `Option<usize>`) are applied by
`Worker::with_config` when set. `WorkerConfig` is now `#[non_exhaustive]` (see
[below](#non-exhaustive-structs-and-enums)): build it with
`WorkerConfig::default().with_batch_size(n).with_batch_concurrency(m)`.
`WorkerConfig::validate` (run by `HammerworkConfig::from_file`/`from_env`,
`WorkerPool::from_hammerwork_config` and now `Worker::with_hammerwork_config`) rejects
`batch_size = 0` (which would have turned batch mode on), a `batch_size` above
`queue::MAX_DEQUEUE_BATCH_SIZE` (1000) and `batch_concurrency = 0` (#130).

### `with_max_retries` is now `with_attempt_cap`

`Worker::with_max_retries(n)` is renamed `Worker::with_attempt_cap(n)`. Its behaviour
is unchanged: it caps the number of attempts this worker gives a job at
`min(job.max_attempts, n)`, and can only lower a job's own limit. There is no
deprecated alias. It now returns `hammerwork::Result<Worker>` and rejects a cap below 1
with `HammerworkError::Config`, instead of silently treating it as 1 (a claimed job
always runs once).

```rust,no_run
# async fn example(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler) -> hammerwork::Result<()> {
use hammerwork::Worker;

// Before (2.x):
// let worker = Worker::new(queue, "emails".into(), handler)
//     .with_batch_processing_enabled(true)
//     .with_max_retries(3);

// After (3.0):
let worker = Worker::new(queue, "emails".into(), handler)
    .with_batch_size(1)
    .with_attempt_cap(3)?;
# let _ = worker;
# Ok(())
# }
```

## MySQL timestamps (migration 021)

Applies to MySQL only; on PostgreSQL migration `021_mysql_datetime` does nothing.

MySQL's `TIMESTAMP` type ends at 2038-01-19 03:14:07 UTC, so 2.x rejected jobs scheduled
after that. Migration `021_mysql_datetime` converts every `TIMESTAMP` and `TIMESTAMP(n)`
column of every Hammerwork table (39 columns in `hammerwork_jobs`,
`hammerwork_jobs_archive`, `hammerwork_batches`, `hammerwork_encryption_keys`,
`hammerwork_key_audit_log`, `hammerwork_kms_data_keys`, `hammerwork_queue_pause` and
`hammerwork_migrations`) to `DATETIME(6)`, which holds years 1000 to 9999. The three
second-precision `DATETIME` columns of `hammerwork_workflows` (`created_at`,
`completed_at`, `failed_at`) also become `DATETIME(6)`, so every MySQL time keeps
microseconds. The 2038 checks are gone, and a new install gets the same schema by running all migrations.

What the migration keeps:

- **Values.** `TIMESTAMP` stores UTC and converts to the session `time_zone`; `DATETIME`
  stores the value as written. The migration sets the session `time_zone` to `+00:00`
  for the conversion (and restores it afterwards), so every existing value keeps its
  UTC reading whatever your connection's time zone is. Hammerwork has always written
  UTC (`UTC_TIMESTAMP(6)`, UTC values bound by sqlx).
- **`NULL` / `NOT NULL`.** Nullability is unchanged.
- **Defaults are UTC.** On a `DATETIME` column `CURRENT_TIMESTAMP` is the session's
  local time, not UTC, so the `CURRENT_TIMESTAMP` defaults become the expression
  default `(UTC_TIMESTAMP(6))`, which needs **MySQL 8.0.13 or later**. The one
  `ON UPDATE CURRENT_TIMESTAMP` clause, on `hammerwork_queue_pause.updated_at`, is
  dropped for the same reason; Hammerwork (2.x included) writes `updated_at` itself.
  If your own SQL inserts into these tables without the time columns, it now gets UTC;
  if it updates `hammerwork_queue_pause` and relies on `updated_at` changing by itself,
  set it explicitly (`updated_at = UTC_TIMESTAMP(6)`).
- Second-precision columns (the queue pause table, `hammerwork_workflows`, and `hammerwork_jobs.archived_at`) now
  carry microseconds.

### Cost: it rewrites every table

MySQL cannot change `TIMESTAMP` to `DATETIME` in place. It copies each table
(`ALGORITHM=COPY`) and blocks writes to it while it does, so `enqueue`, dequeues and job
completions on `hammerwork_jobs` wait (readers are not blocked). There is one `ALTER` per
table, so the lock on each table lasts as long as that table's copy.

Measured on a local Docker MySQL 8 with an `hammerwork_jobs` table of 200,000 rows
(about 300 bytes each, with its indexes) and an equally large `hammerwork_jobs_archive`:
the whole migration took about 4.6 seconds, roughly 85,000 rows per second. Expect
something like 10 to 20 seconds per million rows on comparable hardware, and much more on
a slow disk or a large buffer-pool-bound table; measure on a copy of your own data.

Before upgrading:

1. **Take a backup.** DDL on MySQL cannot be rolled back.
2. **Run it off-peak**, ideally with the workers stopped or idle (for example run
   `cargo hammerwork migration run` before starting the 3.0 workers), and make sure there
   is free disk space for a copy of your largest table. Archive or purge old jobs first
   if the table is much bigger than it needs to be.
3. If a statement fails part-way (for example the disk fills up), fix the cause and run
   the migrations again. Each `ALTER` only runs while its table still has a `TIMESTAMP`
   column (or a `CURRENT_TIMESTAMP` default or `ON UPDATE` clause), so completed tables
   are skipped.

### 2.x workers and the converted schema

2.x code works against the converted columns, so a rolling upgrade is possible once the
migration has finished. 2.x only ever binds UTC `DateTime<Utc>` values and writes
`UTC_TIMESTAMP(6)` under a `+00:00` session, which `DATETIME` stores as given, and sqlx
0.8 decodes `DATETIME` and `TIMESTAMP` into `DateTime<Utc>` alike. A 2.x worker also
ignores the migration record it does not know. What 2.x does not gain is the longer
range: it still rejects times after 2038 itself.

The migration blocks writes while it runs, so 2.x workers stall (and may time out
waiting on locks) during the conversion. Stop them or let them drain first.

## Non-exhaustive structs and enums

Configuration, statistics and record types that gain fields from release to release are
`#[non_exhaustive]` in 3.0 ([#81](https://github.com/CodingAnarchy/hammerwork/issues/81)),
so a new field is no longer a breaking change. Outside Hammerwork you can still read and
assign their public fields and match them with `..`, but you cannot build them with a
struct literal, not even one that ends in `..Default::default()`. Start from `default()` or
a constructor and chain the `with_*` methods instead:

```rust
use hammerwork::config::{HammerworkConfig, WorkerConfig};
use std::time::Duration;

// Before (2.x):
// let worker = WorkerConfig {
//     pool_size: 8,
//     job_timeout: Duration::from_secs(60),
//     batch_size: Some(25),
//     ..WorkerConfig::default()
// };

// After (3.0):
let worker = WorkerConfig::default()
    .with_pool_size(8)
    .with_job_timeout(Duration::from_secs(60))
    .with_batch_size(25); // `Option` fields take the value and store `Some(value)`
let config = HammerworkConfig::new().with_worker(worker);

// Assigning fields of a default value works too:
let mut worker = WorkerConfig::default();
worker.pool_size = 8;
# let _ = (config, worker);
```

### Configuration types

Every public field of these types has a `with_<field>` method unless noted otherwise.

| Module | Types | How to build them |
| --- | --- | --- |
| `config` | `HammerworkConfig` | `new()` / `default()`, one setter per section (`with_database`, `with_worker`, `with_events`, `with_webhooks`, `with_streaming`, `with_alerting`, `with_metrics`, `with_archive`, `with_rate_limiting`, `with_logging`, `with_encryption`) and the existing shortcuts |
| `config` | `DatabaseConfig`, `WorkerConfig`, `WebhookConfigs`, `WebhookGlobalSettings`, `StreamingConfigs`, `StreamingGlobalSettings`, `ArchiveConfig`, `RateLimitingConfig`, `LoggingConfig`, `PayloadEncryptionConfig`, `KeyRotationConfig` | `default()` and the new `with_*` setters; also `RateLimitingConfig::with_queue_throttle(queue, throttle)` and `PayloadEncryptionConfig::with_decryption_key(key_id, source)` |
| `events` | `EventConfig` | `default()` and the new setters |
| `events` | `EventFilter` | `new()` and its existing builders |
| `events` | `JobLifecycleEvent` | the `JobLifecycleEventBuilder` constructors (`enqueued`, `failed`, ...) or the new `JobLifecycleEvent::new(job_id, queue, event_type, priority)`, then `with_timestamp`, `with_processing_time_ms`, `with_error`, `with_payload`, `with_metadata` |
| `events` | `JobError` | new `JobError::new(message)`, then `with_error_type`, `with_details`, `with_retry_attempt` |
| `webhooks` | `WebhookConfig` | `new(name, url)` and its existing builders |
| `webhooks` | `RetryPolicy`, `WebhookManagerConfig` | `default()` and the new setters |
| `streaming` | `StreamConfig` | `new(name, backend)` and its existing builders |
| `streaming` | `StreamRetryPolicy`, `BufferConfig`, `StreamManagerConfig` | `default()` and the new setters |
| `alerting` | `AlertingConfig`, `SmtpConfig` | `new()` / `SmtpConfig::new(host, from)` and their existing builders |
| `archive` | `ArchivalPolicy`, `ArchivalConfig` | `new()` and their existing builders |
| `encryption` | `EncryptionConfig`, `KeyManagerConfig`, `KeyAuditFilter` | `new()` and their existing builders; new `KeyManagerConfig::with_key_derivation_config` |
| `encryption` | `ExternalKmsConfig` | new `ExternalKmsConfig::new(service_type, endpoint)`, then `with_auth(key, value)`, `with_auth_config`, `with_region`, `with_namespace` |
| `encryption` | `KeyDerivationConfig` | `default()` and the new setters |
| `metrics`, `tracing`, `rate_limit`, `worker` | `MetricsConfig`, `TracingConfig`, `ThrottleConfig`, `AutoscaleConfig`, `JobEventHooks` | `new()` and their existing builders |
| `spawn`, `stats` | `SpawnConfig`, `StatsConfig` | `default()` and the new setters |
| `stats` | `JobEvent` | new `JobEvent::new(job_id, queue, event_type, priority)`, then `with_processing_time_ms`, `with_error_message`, `with_timestamp` |
| `job`, `batch`, `workflow` | `Job`, `ResultConfig`, `JobBatch`, `JobGroup` | `Job::new` / `Job::with_delay` / `Job::with_cron_schedule`, `ResultConfig::new`, `JobBatch::new`, `JobGroup::new` and their existing builders |
| `migrations` | `Migration` | new `Migration::new(id, description, version)` (and `with_created_at`) |
| `archive` | `ArchivedJob` | new `ArchivedJob::new(id, queue, status, created_at, archived_at, reason)`, then `with_original_payload_size`, `with_payload_compressed`, `with_archived_by`, `with_workflow_id`, `with_workflow_name` |
| `spawn` | `SpawnContext` | new `SpawnContext::new(parent_job, config, queue)`, for testing a `SpawnHandler` |
| `hammerwork_web` | `DashboardConfig` | `new()` and its builders, plus the new `with_pool_size`, `with_auth_config`, `with_websocket`, `with_allowed_origins` |
| `hammerwork_web` | `AuthConfig`, `WebSocketConfig` | `default()` and the new setters; `AuthConfig::disabled()` |

### Statistics and records

These are returned by Hammerwork and have no builders. Read their fields as before. In
tests, start from `default()` where the type has one and assign the fields you need
(`QueueStats` now implements `Default` for this).

- Statistics: `stats::JobStatistics`, `stats::QueueStats`, `stats::DeadJobSummary`,
  `priority::PriorityStats`, `worker::BatchProcessingStats`, `worker::AutoscaleMetrics`,
  `archive::ArchivalStats`, `archive::ScheduledArchivalPass`,
  `encryption::EncryptionStats`, `encryption::KeyManagerStats`,
  `events::EventManagerStats`, `webhooks::WebhookStats`, `webhooks::WebhookManagerStats`,
  `streaming::StreamStats`, `streaming::StreamManagerGlobalStats`.
- Records: `webhooks::WebhookDelivery`, `streaming::StreamDelivery`,
  `streaming::StreamedEvent`, `encryption::EncryptionKey`, `encryption::KeyAuditRecord`,
  `encryption::KmsWrappedKey`, `encryption::EncryptionMetadata`,
  `encryption::EncryptedPayload`, `migrations::MigrationRecord`,
  `queue::QueuePauseInfo`, `queue::RecordedOutcome`, `spawn::SpawnResult`,
  `worker::JobHookEvent`, `worker::JobResult` (still built with `JobResult::success()` and
  `JobResult::with_data`), `batch::BatchResult`, `alerting::Alert`.

Small types that are complete by definition stay exhaustive: `queue::StaleJobRecovery`
(`requeued`, `dead`), `queue::EncryptedJobPurge` (`jobs`, `archived_jobs`),
`rate_limit::RateLimit` (`rate`, `per`, `burst_limit`) and the newtypes
`tracing::TraceId`, `tracing::CorrelationId` and `config::KeySourceRef`.

### Enums

Enums that gain variants over time are `#[non_exhaustive]` too
([#86](https://github.com/CodingAnarchy/hammerwork/issues/86)). Constructing their
variants and matching specific ones is unchanged, but a `match` outside Hammerwork must
now end with a wildcard arm, so that a variant added in a minor release does not break
your build:

```rust
use hammerwork::events::JobLifecycleEventType;

fn describe(event_type: &JobLifecycleEventType) -> &'static str {
    match event_type {
        JobLifecycleEventType::Completed => "done",
        JobLifecycleEventType::Failed | JobLifecycleEventType::Dead => "failed",
        // Before (2.x) the remaining variants could be listed exhaustively.
        // After (3.0) a wildcard arm is required:
        _ => "other",
    }
}
# assert_eq!(describe(&JobLifecycleEventType::Completed), "done");
```

The non-exhaustive enums are:

- Errors: `HammerworkError`, `encryption::EncryptionError`, `cron::CronError`,
  `spawn::SpawnError`, `priority::PriorityError`.
- Events: `events::JobLifecycleEventType`, `stats::JobEventType`, `archive::ArchiveEvent`,
  `alerting::AlertType`, `encryption::KeyOperation`.
- Backends and options: `streaming::StreamBackend`, `streaming::SerializationFormat`,
  `streaming::PartitioningStrategy`, `streaming::PartitionField`, `alerting::AlertTarget`,
  `alerting::SecretSource`, `webhooks::WebhookAuth`, `webhooks::HttpMethod`,
  `encryption::KeySource`, `encryption::RetentionPolicy`,
  `encryption::EncryptionAlgorithm`, `config::PayloadEncryptionAlgorithm`,
  `encryption::KeyPurpose`, `archive::ArchivalReason`, `batch::PartialFailureMode`,
  `job::ResultStorage`, `priority::PrioritySelectionStrategy`, `retry::RetryStrategy`,
  `retry::JitterType`, `workflow::FailurePolicy`, `worker::JobHandlerType`,
  `queue::JobTransition`, `queue::JobOutcome`.

These stay exhaustive, so matching every variant keeps working: `JobStatus` and
`JobPriority`, the other state enums (`batch::BatchStatus`, `workflow::WorkflowStatus`,
`workflow::DependencyStatus`, `encryption::KeyStatus`), `alerting::AlertSeverity` (a
fixed scale, like `JobPriority`) and `alerting::SmtpTls` (the three ways to connect to an
SMTP server).

## `ArchivedJob` lists the workflow

`ArchivedJob` has two new fields, `workflow_id` and `workflow_name`, filled in by
`list_archived_jobs` on PostgreSQL, MySQL and `TestQueue` (the archive has stored them
since migration 019). Both are `None` for a job outside a workflow.

## Event subscriptions unregister when dropped

Dropping an `EventSubscription` now unregisters it from its `EventManager`: its filter
stops counting toward `subscription_count` and `payload_wanted` without a call to
`EventManager::unsubscribe` (which still works, for example to unregister by id). The
subscription keeps a reference to the manager's registry, so its fields can be borrowed
but no longer moved out: keep the whole subscription alive while you read from it.

```rust
# async fn example(events: hammerwork::events::EventManager) -> hammerwork::Result<()> {
use hammerwork::events::EventFilter;

// Before (2.x):
// let receiver = events.subscribe(EventFilter::new()).await?.receiver;
// ... later: events.unsubscribe(id).await?;

// After (3.0):
let mut subscription = events.subscribe(EventFilter::new()).await?;
while let Ok(event) = subscription.receiver.recv().await {
    println!("{} {}", event.job_id, event.event_type);
#   break;
}
drop(subscription); // unregisters
# Ok(())
# }
```

## Webhook statistics count failed attempts

`WebhookStats` and `WebhookManagerStats` have a new `failed_attempts` counter: HTTP
delivery attempts that failed, retries included, next to `total_attempts`. The
per-delivery counters (`successful_deliveries`, `failed_deliveries`, `success_rate`) are
unchanged. Serialized statistics without the field read it as 0.

## Removed legacy compatibility

3.0 drops code that only existed for Hammerwork 1.x and for APIs deprecated in 2.0
([#90](https://github.com/CodingAnarchy/hammerwork/issues/90)).

### Stop every 1.x worker first

The stale job reaper now relies on leases alone. Every 2.x and 3.x claim writes a lease
in the statement that claims the job, and the reaper reclaims a `Running` job only once
that lease has expired. Hammerwork 1.x claimed jobs without a lease; 2.x reclaimed such
jobs by age (the `older_than` window), 3.0 never reclaims them.

So before you start any 3.0 worker:

1. Stop every Hammerwork 1.x worker (and anything else built on 1.x that claims jobs).
2. Fix the jobs those workers left `Running` without a lease, once. Either run

   ```bash
   cargo hammerwork maintenance check --fix --database-url <url>
   ```

   or run this SQL (it works on PostgreSQL and MySQL):

   ```sql
   UPDATE hammerwork_jobs
   SET started_at = COALESCE(started_at, created_at),
       last_heartbeat_at = COALESCE(started_at, created_at),
       lease_expires_at = COALESCE(started_at, created_at)
   WHERE status = 'Running'
     AND (started_at IS NULL OR last_heartbeat_at IS NULL OR lease_expires_at IS NULL
          OR last_heartbeat_at < started_at);
   ```

   Both give each such job a lease that expired when it started; they leave jobs with a
   lease of their own alone. (`check --fix` retries `Running` jobs without a start time
   instead of giving them a lease.)
3. Let the reaper reclaim them: a `WorkerPool` does within a minute of starting, or run
   `cargo hammerwork job requeue-stale` (`--dry-run` counts what it would reclaim
   without changing anything, `--queue <name>` limits it to one queue). Jobs with attempts left go back to `Pending`,
   the others to `Dead`, with the usual side effects (recurring jobs are rescheduled,
   dependents, workflows and batches see the failure).

Do step 2 only after step 1: a 1.x worker still running one of these jobs would see it
run a second time. If you skip step 2, the jobs stay `Running` until you retry or delete
them by hand; `cargo hammerwork maintenance check` reports how many there are.

### Stale job API

`DatabaseQueue::requeue_stale_jobs` and `WorkerPool::with_stale_job_reaper` lose their
`older_than` argument, which only applied to lease-less jobs. `requeue_stale_jobs` takes
an optional queue name instead: `None` reclaims every queue, as before, and
`Some("emails")` reclaims that queue only, inside the same atomic statement as the lease
check:

```rust,no_run
# async fn example(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>) -> hammerwork::Result<()> {
use hammerwork::{WorkerPool, queue::DatabaseQueue};
use std::time::Duration;

// Before (2.x):
// let recovery = queue.requeue_stale_jobs(Duration::from_secs(3600)).await?;
// let pool = WorkerPool::new().with_stale_job_reaper(Duration::from_secs(30), Duration::from_secs(600));

// After (3.0):
let recovery = queue.requeue_stale_jobs(None).await?;
let emails_only = queue.requeue_stale_jobs(Some("emails")).await?;
let pool: WorkerPool<sqlx::Postgres> =
    WorkerPool::new().with_stale_job_reaper(Duration::from_secs(30));
# let _ = (recovery, emails_only, pool);
# Ok(())
# }
```

In the CLI, `job requeue-stale --older-than-secs` and `maintenance dead-jobs
--stale-hours` are removed; drop the flags from scripts. Both commands apply the lease
rule, and neither touches lease-less jobs.

### `AlertingConfig::email` is removed

`AlertingConfig::email(recipient)`, deprecated since 2.0.0, created an email target
without SMTP settings, which could not send. Use `email_via_smtp`. `AlertTarget::Email`
now holds a required `smtp: SmtpConfig` (it was an `Option`), so a configuration file
whose email target has no `[alerting.targets.Email.smtp]` table fails to load.

```rust
use hammerwork::alerting::{AlertingConfig, SmtpConfig};

// Before (2.x): AlertingConfig::new().email("oncall@example.com")
let config = AlertingConfig::new().email_via_smtp(
    "oncall@example.com",
    SmtpConfig::new("smtp.example.com", "alerts@example.com"),
);
# let _ = config;
```

### `JobArchiver::new` takes no pool

`JobArchiver` kept the pool it was created with but never used it: every operation takes
the queue it works on. `JobArchiver::new()` takes no argument, and `JobArchiver` is no
longer generic (`JobArchiver<sqlx::Postgres>` is now `JobArchiver`).

```rust
use hammerwork::archive::JobArchiver;

// Before (2.x): let archiver = JobArchiver::new(queue.pool.clone());
let archiver = JobArchiver::new();
# let _ = archiver;
```

### Zero rates are rejected

A rate limit of zero never refilled, so a worker using one waited forever.
`RateLimit::per_second`, `per_minute` and `per_hour` now return
`hammerwork::Result<RateLimit>` and fail with `HammerworkError::Config` for zero, and
`RateLimit`'s fields are private (`rate()`, `per()`, `burst_limit()`). An enabled
`ThrottleConfig` with `rate_per_minute(0)` is rejected by `ThrottleConfig::validate`,
`Worker::with_throttle_config` and `Worker::with_hammerwork_config` (both now return a
`Result`), `JobQueue::set_throttle`, `HammerworkConfig::from_file` and
`WorkerPool::from_hammerwork_config`. `ThrottleConfig::to_rate_limit` returns
`Result<Option<RateLimit>>`. `TestQueue::set_throttle_config` applies the same check as
the database backends (#130).

A burst limit of zero is rejected the same way (#130): `RateLimit::with_burst_limit`
returns `hammerwork::Result<RateLimit>` and fails for 0 (2.x treated it as 1), so add a
`?`: `RateLimit::per_second(10)?.with_burst_limit(20)?`.

```rust,no_run
# async fn example(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler) -> hammerwork::Result<()> {
use hammerwork::{RateLimit, ThrottleConfig, Worker};

// Before (2.x):
// let worker = Worker::new(queue, "emails".into(), handler)
//     .with_rate_limit(RateLimit::per_second(10))
//     .with_throttle_config(ThrottleConfig::new().max_concurrent(5));

// After (3.0):
let worker = Worker::new(queue, "emails".into(), handler)
    .with_rate_limit(RateLimit::per_second(10)?)
    .with_throttle_config(ThrottleConfig::new().max_concurrent(5))?;
# let _ = worker;
# Ok(())
# }
```

### `WebhookManager::new` returns a `Result`

`WebhookManager::new` panicked when the HTTP client could not be built (an invalid
`user_agent`, a TLS backend failure); `try_new` was the fallible variant. In 3.0
`WebhookManager::new` and `WebhookManager::new_default` return
`hammerwork::Result<WebhookManager>` and `try_new` is removed (#130): add a `?` to
`new`, and rename `try_new` to `new`.

### `MigrationRunner` is sealed

Hammerwork's migrations are written for PostgreSQL and MySQL only, so `MigrationRunner`
can no longer be implemented outside the crate; `PostgresMigrationRunner` and
`MySqlMigrationRunner` implement it. `MigrationManager::new` still takes a
`Box<dyn MigrationRunner<DB> + Send + Sync>`. The no-op default bodies of
`acquire_migration_lock` and `release_migration_lock` are gone.

### What stays

Some code that reads data older versions wrote is kept, because that data can still be
in your database after the upgrade and removing the code would need a data migration:

- Status columns stored JSON-quoted (`"\"Pending\""`) by early versions are still
  decoded, for jobs, batches and archive reasons, and the `Retrying` status is still read.
- A job that an older version archived without deleting its `hammerwork_jobs` row is
  still handled when restoring and counted once in workflow statistics.
- Archived jobs without workflow fields, and encrypted payloads written before payload
  binding or without a key version, still load.
- Configuration files still load with keys of settings removed in 2.0 (they are ignored),
  and archive durations still accept a bare number of days (`"7"`).
- CLI aliases from the 2.0 command-line cleanup (`-Q` for `-n/--queue`,
  `archive --queue-name`, `batch enqueue --progress-every`, `archive run --compress false`)
  still work.
