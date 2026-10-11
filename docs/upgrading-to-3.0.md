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

- No database migration is needed. Hammerwork's migrations, and the way they are run
  (one statement at a time, over the simple text protocol on MySQL), are the same.
- Timestamps decode as before: PostgreSQL `TIMESTAMPTZ` and MySQL `TIMESTAMP(6)`
  columns map to `chrono::DateTime<Utc>`, and sqlx still sets the MySQL session
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
