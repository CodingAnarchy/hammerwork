# Upgrading from 2.x to 3.0

Hammerwork 3.0 is a major release. This guide lists each breaking change, what it
affects and what you need to change in your code, configuration or deployment. Read
it together with the `3.0.0` entry in the [CHANGELOG](../CHANGELOG.md).

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
2. **`azure-kv` is now `azure-kms`**, like `aws-kms`, `gcp-kms` and `vault-kms`. There is no alias.
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
`Worker::with_config` when set. If you build a `WorkerConfig` with a struct literal, add
the two fields or end it with `..WorkerConfig::default()`.

### `with_max_retries` is now `with_attempt_cap`

`Worker::with_max_retries(n)` is renamed `Worker::with_attempt_cap(n)`. Its behaviour
is unchanged: it caps the number of attempts this worker gives a job at
`min(job.max_attempts, n)`, and can only lower a job's own limit. There is no
deprecated alias.

```rust,no_run
# async fn example(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler) {
use hammerwork::Worker;

// Before (2.x):
// let worker = Worker::new(queue, "emails".into(), handler)
//     .with_batch_processing_enabled(true)
//     .with_max_retries(3);

// After (3.0):
let worker = Worker::new(queue, "emails".into(), handler)
    .with_batch_size(1)
    .with_attempt_cap(3);
# let _ = worker;
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
microseconds. The 2038 checks
are gone, and a new install gets the same schema by running all migrations.

What the migration keeps:

- **Values.** `TIMESTAMP` stores UTC and converts to the session `time_zone`; `DATETIME`
  stores the value as written. The migration sets the session `time_zone` to `+00:00`
  for the conversion (and restores it afterwards), so every existing value keeps its
  UTC reading whatever your connection's time zone is. Hammerwork has always written
  UTC (`UTC_TIMESTAMP(6)`, UTC values bound by sqlx).
- **`NULL` / `NOT NULL` and defaults.** `CURRENT_TIMESTAMP` defaults become
  `CURRENT_TIMESTAMP(6)`, as `DATETIME(6)` requires. The one `ON UPDATE` clause,
  `hammerwork_queue_pause.updated_at`, is kept.
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
   column, so completed tables are skipped.

### 2.x workers and the converted schema

2.x code works against the converted columns, so a rolling upgrade is possible once the
migration has finished. 2.x only ever binds UTC `DateTime<Utc>` values and writes
`UTC_TIMESTAMP(6)` under a `+00:00` session, which `DATETIME` stores as given, and sqlx
0.8 decodes `DATETIME` and `TIMESTAMP` into `DateTime<Utc>` alike. A 2.x worker also
ignores the migration record it does not know. What 2.x does not gain is the longer
range: it still rejects times after 2038 itself.

The migration blocks writes while it runs, so 2.x workers will stall (and may time out
waiting on locks) during the conversion, so stop them or let them drain first.
