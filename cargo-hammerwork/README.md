# cargo-hammerwork

A comprehensive cargo subcommand for managing Hammerwork job queues with advanced tooling and monitoring capabilities.

## Installation

Install from the workspace:

```bash
# Build and install locally
cargo install --path ./cargo-hammerwork

# Or build for development
cargo build -p cargo-hammerwork
```

## Overview

The `cargo-hammerwork` crate provides a modular CLI for managing Hammerwork-based applications with support for:

- 🗄️  **Database Migration Management** - Setup and maintain database schemas
- ⚙️  **Configuration Management** - Centralized config with file and environment support  
- 🧩 **Modular Architecture** - Clean separation of concerns with dedicated command modules
- 🐘 **Multi-Database Support** - PostgreSQL and MySQL compatibility
- 📊 **Advanced Monitoring** - Real-time dashboards and health checks
- 👷 **Worker Status** - Inspect running jobs and worker leases
- 🎯 **Queue Operations** - Comprehensive queue management and statistics
- 📋 **Job Management** - Full job lifecycle control and management
- 📦 **Batch Operations** - Bulk job processing and management
- ⏰ **Cron Management** - Recurring job scheduling and management
- 🔧 **Database Maintenance** - Cleanup, optimization, and integrity checks
- 🔑 **Encryption Key Audit** - Read the key audit log (creation, access, rotation)
- 🔄 **Workflow Management** - Job dependencies and complex pipelines
- 🚀 **Spawn Operations** - Dynamic job spawning, parent-child relationships, and tree visualization
- 💾 **Backup & Restore** - Export and restore job data
- 🗃️ **Archival** - Archive, restore and purge old jobs
- 🔔 **Webhooks** - Manage the webhook registry

## Quick Start

### 1. Database Setup

```bash
# Run migrations to set up the database schema
cargo hammerwork migration run --database-url postgres://localhost/mydb

# Check migration status
cargo hammerwork migration status --database-url postgres://localhost/mydb
```

### 2. Configuration

```bash
# Set your default database URL
cargo hammerwork config set database_url postgres://localhost/mydb

# View current configuration
cargo hammerwork config show

# Set other defaults
cargo hammerwork config set default_queue emails
cargo hammerwork config set log_level debug
```

### 3. Look Around

```bash
cargo hammerwork queue list
cargo hammerwork job list --queue emails --limit 20
cargo hammerwork job enqueue --queue emails --payload '{"to": "a@example.com"}'
```

## Command Reference

Every command group and subcommand below exists in the CLI; run
`cargo hammerwork <group> <subcommand> --help` for the authoritative flag list.

### Conventions

- **Global flags**: `-v/--verbose` and `-q/--quiet` are accepted by every command.
- **Database URL**: every command that touches the database accepts `--database-url`
  (short form `-u`; `queue` commands also still accept the old `-d`). When the flag is omitted the
  URL comes from the `DATABASE_URL` environment variable, then from `database_url` in the
  config file (see [Configuration System](#configuration-system)). If none is set the
  command fails with `Database URL is required`. The `config`, `webhook` and
  `backup list` commands do not use a database. The examples below omit the flag and
  assume a URL is configured.
- **Destructive operations** (`job purge`, `queue clear`, `batch retry`, `batch cancel`,
  `maintenance vacuum`, `maintenance dead-jobs`, `maintenance reindex`,
  `maintenance purge-encrypted`, `archive purge`, `backup restore`, `cron delete`,
  `webhook remove`, `config reset`) refuse to run without `--confirm`. Where a command has
  `--dry-run`, that previews the change without `--confirm`.
- **Queue filter**: `-n/--queue` on every command that takes one (`-Q` and
  `archive --queue-name` remain as aliases).
- **Connect timeout**: connecting to an unreachable database fails after 10 seconds. Change
  it with `connect_timeout_secs` in the config file, `config set connect_timeout_secs N`
  (1-600) or the `HAMMERWORK_CONNECT_TIMEOUT` environment variable.
- **Job IDs** are positional arguments on `job show`, `job retry`, `job cancel`, `cron enable|disable|update|delete`,
  `workflow dependencies`, `archive restore` and `spawn tree|lineage`.
- **Statuses** are `pending`, `running`, `completed`, `failed`, `dead`, `retrying`,
  `timed_out`. **Priorities** are `background`, `low`, `normal`, `high`, `critical`.

### Migration Commands

Create and inspect the Hammerwork schema.

```bash
cargo hammerwork migration run --database-url postgres://localhost/mydb
cargo hammerwork migration status --database-url postgres://localhost/mydb

# Drop the existing tables first, then migrate (destroys all data)
cargo hammerwork migration run --drop
```

### Configuration Commands

Read and write the CLI's config file. These commands never touch the database.

```bash
cargo hammerwork config show                    # All settings, and where each value comes from
cargo hammerwork config set default_queue emails
cargo hammerwork config get default_queue
cargo hammerwork config path                    # Location of the config file
cargo hammerwork config reset --confirm         # Back to defaults
```

Valid keys for `set` and `get` are `database_url`, `default_queue`, `default_limit`,
`log_level`, `connection_pool_size`, `connect_timeout_secs` and `encryption_config`. `set` validates the value (the database URL must
start with `postgres://`, `postgresql://` or `mysql://`, `log_level` must be one of
`trace`, `debug`, `info`, `warn`, `error`, and `connection_pool_size` must be 1-100, `connect_timeout_secs` 1-600,
and `encryption_config` must be a readable `hammerwork.toml`). `config show` masks the
password of the database URL, and summarises the payload encryption settings.

#### Encrypted queues

Commands that write jobs (`job enqueue`, `batch enqueue`, `cron create`, `workflow create`)
encrypt them like your application. Point the CLI at the application's `hammerwork.toml`
and make its key available:

```bash
cargo hammerwork config set encryption_config /etc/myapp/hammerwork.toml
export HAMMERWORK_ENCRYPTION_KEY=...   # the key its [encryption] key_source names
```

Only the `[encryption]` section is read; `HAMMERWORK_ENCRYPTION_CONFIG` overrides the path,
and the `HAMMERWORK_ENCRYPTION_*` variables override the settings, as for the application.
Jobs on its `encrypted_queues` are then encrypted; `job enqueue --encrypt` (whole payload)
or `--pii-field <field>` (repeatable) encrypt a job on any queue. The CLI never writes
plaintext by mistake: with encryption enabled but the key unavailable, these commands fail,
and without any settings they still refuse to write a plaintext job to a queue that already
holds encrypted jobs. KMS key sources need the matching cargo feature of `cargo-hammerwork`
(`aws-kms`, `gcp-kms`, `vault-kms`, `azure-kv`).

### Job Management Commands

Inspect and manage individual jobs.

```bash
cargo hammerwork job list
cargo hammerwork job list --queue emails --status failed --limit 20
cargo hammerwork job list --priority high --last-hours 24
cargo hammerwork job list --failed                # or --completed
cargo hammerwork job show <job-id>                # Detailed job information

cargo hammerwork job enqueue --queue emails --payload '{"to": "a@example.com"}'
cargo hammerwork job enqueue --queue emails --payload '{"n": 1}' --priority high --delay 60 --max-attempts 5 --timeout 30
cargo hammerwork job enqueue --queue payments --payload '{"card": "4111..."}' --pii-field card   # encrypted

cargo hammerwork job retry <job-id>                 # Retry one job (--job-id also works)
cargo hammerwork job retry --queue emails         # Retry every failed job in a queue
cargo hammerwork job retry --all                  # Retry every failed job

cargo hammerwork job cancel <job-id>                # Cancel one pending job
cargo hammerwork job cancel --queue emails        # Cancel the pending jobs of a queue
cargo hammerwork job cancel --all-pending

cargo hammerwork job purge --completed --older-than-days 30 --confirm
cargo hammerwork job purge --queue emails --dead --failed --confirm

cargo hammerwork job requeue-stale --older-than-secs 3600
```

- `job enqueue` requires `--queue` and `--payload`; `--payload` must be valid JSON,
  `--delay` and `--timeout` are in seconds.
- `job retry` and `job cancel` need one of a job ID (positional or `--job-id`), `--queue` or the `--all` /
  `--all-pending` flag. Only failed, dead and timed-out jobs are retried and only pending jobs are
  cancelled. There is no `job delete`.
- `job purge` needs at least one of `--completed`, `--dead`, `--failed`, optionally
  narrowed with `--queue` and `--older-than-days`, plus `--confirm`.
- `job requeue-stale` moves `Running` jobs whose lease has expired (their worker crashed
  or was killed) back to `Pending`, or to `Dead` when they have no attempts left. Workers
  take the lease when they claim a job, so a job whose worker is alive is never
  reclaimed, whatever `--older-than-secs` is. Only jobs without a lease (claimed by an
  older Hammerwork version) are reclaimed by age, once they started more than
  `--older-than-secs` ago (default 3600). It is safe to run alongside workers, and needs
  migration `015_add_job_leases`.
- `job show` prints the redacted payload of an encrypted job (the CLI never decrypts)
  together with its key id, algorithm and retention.

### Worker Management Commands

Report on running jobs and their worker leases.

```bash
cargo hammerwork worker status
cargo hammerwork worker status --queue emails --jobs
```

Hammerwork has no worker registry: workers live inside your applications, so the CLI
cannot list, start or stop them. `worker status` reports what the database records: the
`Running` jobs of each queue with their lease (`last_heartbeat_at` / `lease_expires_at`)
and flags expired leases, which mean a worker stopped heartbeating. Reclaim those jobs
with `job requeue-stale`. `--jobs` also lists every running job. Needs migration
`015_add_job_leases`.

### Queue Management Commands

Inspect and control queues.

```bash
cargo hammerwork queue list
cargo hammerwork queue stats
cargo hammerwork queue stats --queue emails --detailed   # Breakdown by priority
cargo hammerwork queue health
cargo hammerwork queue health --queue emails

cargo hammerwork queue pause --queue emails
cargo hammerwork queue resume --queue emails
cargo hammerwork queue paused                            # List paused queues

cargo hammerwork queue clear --queue emails --confirm
cargo hammerwork queue clear --queue emails --pending-only --confirm
```

`queue pause`, `resume` and `clear` require `--queue`. `queue clear` deletes jobs
permanently; `--pending-only` keeps jobs in any other status.

### Monitoring Commands

Dashboards, health checks and metrics.

```bash
cargo hammerwork monitor dashboard
cargo hammerwork monitor dashboard --refresh 10 --queue emails
cargo hammerwork monitor health
cargo hammerwork monitor health --format json
cargo hammerwork monitor metrics --period 24h --queue emails
```

`--refresh` is in seconds (default 5). `--format` is `table` or `json`; `--period` is
`1h`, `24h` or `7d`.

### Batch Operation Commands

Operate on many jobs at once.

```bash
# Enqueue from a JSON-lines file, or from stdin when --file is omitted
cargo hammerwork batch enqueue --file jobs.jsonl --queue emails
cargo hammerwork batch enqueue --queue emails --priority low --continue-on-error --progress-every 500

# Retry or cancel by criteria (preview first with --dry-run)
cargo hammerwork batch retry --queue emails --status failed --dry-run
cargo hammerwork batch retry --queue emails --status dead --failed-since-hours 24 --confirm
cargo hammerwork batch cancel --queue emails --status pending --older-than-hours 48 --confirm

# Export job data
cargo hammerwork batch export --output jobs.json --queue emails --status failed
cargo hammerwork batch export --output jobs.csv --format csv --include-payload --limit 1000
```

- `batch enqueue` requires `--queue`, the default for lines without their own. Each line
  is a JSON object: `{"queue": "emails", "payload": {"to": "a@example.com"}, "priority": "high"}`.
  `payload` is required, `queue` and `priority` are optional. A bad line stops the run
  unless `--continue-on-error` is given; `--progress-every` (alias `--batch-size`) only controls how
  often progress is printed.
- `batch retry` takes `--status failed` or `--status dead`; `batch cancel` takes
  `--status pending` or `--status running`. Both refuse to run without `--confirm` or
  `--dry-run`. There is no `batch create` or `batch status`, and jobs are selected by
  criteria, not by a batch ID.
- `batch export` requires `--output`; `--format` is `csv`, `json` or `jsonl`.

### Cron Management Commands

Manage recurring jobs.

```bash
cargo hammerwork cron list
cargo hammerwork cron list --queue emails --active-only --detailed

cargo hammerwork cron create --queue reports --payload '{"report": "daily"}' --schedule "0 0 9 * * MON-FRI" --timezone America/New_York --priority high

cargo hammerwork cron next --count 5
cargo hammerwork cron next --queue reports --hours 48

cargo hammerwork cron update <job-id> --schedule "0 30 8 * * *" --timezone UTC
cargo hammerwork cron disable <job-id>
cargo hammerwork cron enable <job-id>
cargo hammerwork cron delete <job-id> --confirm
```

`cron create` requires `--queue`, `--payload` and `--schedule`. Cron expressions have a
leading seconds field (six fields, e.g. `0 0 9 * * MON-FRI`). `cron next` shows 10
executions unless `--count` is given. There is no `cron add` or `cron remove`; use
`create` and `delete`. `cron disable` holds the job's pending run until `cron enable`,
which resumes it at the next occurrence of its schedule (also when its last run finished
while it was disabled).

### Maintenance Commands

Cleanup and database upkeep.

```bash
cargo hammerwork maintenance vacuum --dry-run
cargo hammerwork maintenance vacuum --keep-completed-days 30 --keep-failed-days 7 --confirm

cargo hammerwork maintenance dead-jobs --stale-hours 24 --dry-run
cargo hammerwork maintenance dead-jobs --stale-hours 24 --confirm

cargo hammerwork maintenance reindex --confirm
cargo hammerwork maintenance analyze
cargo hammerwork maintenance check
cargo hammerwork maintenance check --fix

# Enforce retention policies of encrypted jobs: delete finished encrypted jobs
# (and archived ones) whose retention period has ended. Needs no encryption key.
cargo hammerwork maintenance purge-encrypted --dry-run
cargo hammerwork maintenance purge-encrypted --confirm
```

- `vacuum` deletes old completed and failed jobs (defaults: 30 and 7 days).
- `dead-jobs` marks stale jobs (no update for `--stale-hours`, default 24) as dead. Compare `job requeue-stale`, which gives them another attempt.
- `check` inspects integrity and job consistency; `--fix` repairs minor issues.
- There is no `maintenance cleanup` or `maintenance health`; use `vacuum` and `check`
  (or `monitor health`).

### Encryption Commands

Read the key audit log (`hammerwork_key_audit_log`), where the library's `KeyManager`
records key creation, access and rotation. Needs no encryption key.

```bash
# The newest 50 records
cargo hammerwork encryption audit

# One key's records from the last 7 days
cargo hammerwork encryption audit --key-id payment-key --since 7d

# Failed rotations between two times, as JSON
cargo hammerwork encryption audit --operation rotate --failed \
  --since 2026-10-01T00:00:00Z --until 2026-10-08T00:00:00Z --format json

# Page through older records
cargo hammerwork encryption audit --limit 20 --offset 20
```

- Records are listed newest first. `--key-id`, `--operation` (`create`, `access`,
  `rotate`, `retire`, `revoke`, `delete`, `update`), `--success` / `--failed`,
  `--since` (inclusive) and `--until` (exclusive) combine.
- `--since` and `--until` take an RFC 3339 timestamp or a time ago (`30m`, `24h`, `7d`,
  `2w`).
- `--limit` (default 50) and `--offset` page through the records; the table says when
  more may follow. `--format json` prints every column, including the optional user,
  client IP, user agent and session id.
- The same records are available in code with `KeyManager::audit_log`.

### Workflow Commands

Job dependencies and pipelines.

```bash
cargo hammerwork workflow create --name etl --jobs-file workflow.json
cargo hammerwork workflow create --name etl --jobs-file workflow.json --failure-policy continue_on_failure --metadata '{"owner": "data"}'

cargo hammerwork workflow list
cargo hammerwork workflow list --running --limit 20
cargo hammerwork workflow show <workflow-id>
cargo hammerwork workflow show <workflow-id> --dependencies
cargo hammerwork workflow graph <workflow-id> --format mermaid
cargo hammerwork workflow dependencies <job-id> --tree
cargo hammerwork workflow dependencies <job-id> --dependents
cargo hammerwork workflow cancel <workflow-id>
cargo hammerwork workflow cancel <workflow-id> --force
```

`workflow create` requires `--name` and `--jobs-file`, and creates the workflow and
enqueues its jobs in one transaction. The jobs file is a JSON array; each element has a
`queue` and a `payload`, and optionally a `priority` and `depends_on`, a list of indexes
of earlier jobs in the array that must complete first:

```json
[
  {"queue": "etl", "payload": {"step": "extract"}},
  {"queue": "etl", "payload": {"step": "load"}, "depends_on": [0]}
]
```

`--failure-policy` is `fail_fast`, `continue_on_failure` or `manual`. `workflow graph`
formats are `text`, `dot`, `mermaid` and `json`. `workflow list` filters with `--running`,
`--completed` and `--failed`. There is no `workflow status`; use `workflow show`.
`workflow cancel` refuses to cancel while jobs are running unless `--force` is given.

### Spawn Operation Commands

Dynamic job spawning and parent-child relationships.

```bash
cargo hammerwork spawn list
cargo hammerwork spawn list --queue emails --recent --limit 20
cargo hammerwork spawn tree <job-id>
cargo hammerwork spawn tree <job-id> --format mermaid --full
cargo hammerwork spawn tree <job-id> --children-only --format json
cargo hammerwork spawn stats --queue emails --hours 24 --detailed
cargo hammerwork spawn lineage <job-id> --ancestors --descendants --depth 5
cargo hammerwork spawn pending --queue emails --show-config
cargo hammerwork spawn monitor --queue emails --interval 5
```

`spawn tree --format` is `text`, `json` or `mermaid`; `--full` shows the tree both up and
down from the job. `spawn list`, `stats`, `pending` and `monitor` filter by `-n/--queue`.

### Archive Commands

Archival and retention of finished jobs.

```bash
cargo hammerwork archive run --dry-run
cargo hammerwork archive run --queue emails --completed-after-days 7 --failed-after-days 30 --reason "monthly cleanup"
cargo hammerwork archive run --no-compress --batch-size 500

cargo hammerwork archive list --queue emails --limit 50 --offset 0 --format json
cargo hammerwork archive stats --format json
cargo hammerwork archive restore <job-id>
cargo hammerwork archive purge --older-than-days 365 --dry-run
cargo hammerwork archive purge --older-than-days 365 --confirm
```

`archive run` archives completed, failed, dead and timed-out jobs older than their
`--*-after-days` thresholds (defaults 7, 30, 30 and 30), compressing payloads unless
`--compress false` is given. `archive purge` requires `--older-than-days` and either
`--confirm` or `--dry-run`.

### Webhook Commands

Manage the webhook registry.

```bash
cargo hammerwork webhook add --name ci --url https://example.com/hook
cargo hammerwork webhook add --name alerts --url https://example.com/alerts --events failed,dead --queues emails --auth-token T --secret S
cargo hammerwork webhook list
cargo hammerwork webhook list --detailed
cargo hammerwork webhook update --webhook ci --url https://example.com/new-hook --timeout 10
cargo hammerwork webhook toggle --webhook ci --enable
cargo hammerwork webhook test --webhook ci --event-type failed   # Sends a real request
cargo hammerwork webhook remove --webhook ci --confirm
```

Webhooks are stored as JSON in `webhooks.json` next to `config.toml` (override with
`HAMMERWORK_WEBHOOKS_FILE`; written owner-only because it can hold tokens and secrets).
`--webhook` takes a webhook's ID or name. `webhook test` sends one sample event to the URL
with the configured method, headers, authentication and `X-Hammerwork-Signature` HMAC,
prints the response, and exits non-zero if the endpoint is unreachable or answers with an
error status. The file is a registry for the CLI only: delivery from your application is
configured in code with `WebhookManager`. There is no `webhook stats` (delivery statistics
only exist in the memory of the process running the manager) and no `streaming` command.

### Backup Commands

Export and restore job data as files.

```bash
cargo hammerwork backup create --output backup.json
cargo hammerwork backup create --output backup.json --queue emails --include-completed --include-failed
cargo hammerwork backup create --output backup.csv --format csv

cargo hammerwork backup list
cargo hammerwork backup list --path ./backups

cargo hammerwork backup restore --input backup.json --confirm
cargo hammerwork backup restore --input backup.json --skip-existing --confirm
```

`backup create` requires `--output` and `--format` is `json` or `csv`. A JSON backup
(format version 2.0) holds every column of every selected job: encrypted payloads stay
encrypted, with their nonce, tag, key id and metadata, and cron schedules, timeouts, retry
strategies, dependencies, workflows, batches, results, tracing ids and retention are all
kept. A CSV backup is an export of the main columns and cannot be restored.

`backup restore` requires `--input` and refuses to run without `--confirm`. It recreates
the jobs exactly as they were backed up (into PostgreSQL or MySQL, whichever the backup
came from), and restored encrypted jobs decrypt with the same key. Jobs whose id already
exists are never overwritten or duplicated: they are skipped and counted, with or without
`--skip-existing`. The whole backup is checked before anything is written and the jobs are
inserted in one transaction, so a bad backup restores nothing; a backup from a newer schema
must be restored into a database migrated at least as far. Backups from earlier versions
(format 1.0) still restore. `backup list` only reads the `.json` and `.csv` files in a local
directory (default `./backups`). There is no `backup verify`.

## Architecture & Design

### Modular Structure

```
cargo-hammerwork/
├── src/
│   ├── commands/           # One module per command group
│   │   ├── migration.rs, config.rs, job.rs, queue.rs, worker.rs, monitor.rs
│   │   ├── batch.rs, cron.rs, encryption.rs, maintenance.rs, workflow.rs, spawn.rs
│   │   └── archive.rs, backup.rs, webhook.rs
│   ├── config/            # Config loading and management
│   │   └── mod.rs
│   ├── utils/             # Shared utilities
│   │   ├── database.rs    # Database connection handling
│   │   ├── display.rs     # Table formatting and display
│   │   └── validation.rs  # Input validation
│   └── main.rs           # CLI entry point
```

### Configuration System

Settings come from these sources, highest priority first:

1. **Command-line flags**: for the database URL, `--database-url` / `-u` (`-d` on `queue` commands)
2. **Environment variables**: `DATABASE_URL`, `HAMMERWORK_DEFAULT_QUEUE`,
   `HAMMERWORK_DEFAULT_LIMIT`, `HAMMERWORK_LOG_LEVEL`, `HAMMERWORK_POOL_SIZE`,
   `HAMMERWORK_ENCRYPTION_CONFIG` (and the `HAMMERWORK_ENCRYPTION_*` payload encryption settings)
3. **Configuration file**
4. **Default values** (`default_limit = 50`, `log_level = "info"`, `connection_pool_size = 5`)

The config file is `hammerwork/config.toml` in the platform config directory
(`~/.config/hammerwork/config.toml` on Linux, `~/Library/Application Support/hammerwork/config.toml`
on macOS, `%APPDATA%\hammerwork\config.toml` on Windows). Set `HAMMERWORK_CONFIG` to use a
different file; `cargo hammerwork config path` prints the one in use. A missing file means
defaults, but a file that exists and cannot be parsed is an error. `HAMMERWORK_WEBHOOKS_FILE`
overrides the location of the webhook registry.

Example configuration file:

```toml
database_url = "postgres://localhost/hammerwork"
default_queue = "emails"
default_limit = 50
log_level = "info"
connection_pool_size = 5
encryption_config = "/etc/myapp/hammerwork.toml"   # optional: the application's encryption settings
```

### Database Support

- **PostgreSQL** (`postgres://` or `postgresql://`) and **MySQL** (`mysql://`) URLs are supported.
- **Connection Pooling**: pool size is set with `connection_pool_size` or `HAMMERWORK_POOL_SIZE`.

## Advanced Usage

### Environment Integration

```bash
# Set environment variables
export DATABASE_URL=postgres://localhost/hammerwork
export HAMMERWORK_DEFAULT_QUEUE=processing
export HAMMERWORK_LOG_LEVEL=debug

# Commands will automatically use environment settings
cargo hammerwork migration run
```

### Cargo Subcommand Usage

```bash
# Works as a standard cargo subcommand
cargo hammerwork migration run --database-url postgres://localhost/mydb

# Or direct invocation
./target/debug/cargo-hammerwork migration run --database-url postgres://localhost/mydb
```

### Global Options

```bash
# Enable verbose logging
cargo hammerwork -v migration run

# Suppress output (errors only)
cargo hammerwork -q config show
```

## Development & Extension

The modular architecture makes it easy to extend functionality:

1. **Add New Commands**: Create modules in `src/commands/`
2. **Extend Utilities**: Add shared functionality in `src/utils/`
3. **Database Support**: Extend `DatabasePool` for new database types
4. **Configuration**: Add new config keys in `Config` struct

### Testing

```bash
# Run unit tests
cargo test -p cargo-hammerwork

# Set up test databases (requires Docker)
../scripts/setup-test-databases.sh both

# Run integration tests with databases
../scripts/setup-test-databases.sh test

# Check CLI structure
cargo run -p cargo-hammerwork -- --help

# Test specific commands
cargo run -p cargo-hammerwork -- migration status --database-url postgres://postgres:hammerwork@localhost:5433/hammerwork
cargo run -p cargo-hammerwork -- migration status --database-url mysql://root:hammerwork@localhost:3307/hammerwork
```

### Test Database Management

The project includes convenient scripts for managing test databases:

```bash
# From the project root directory:

# Set up test databases
./scripts/setup-test-databases.sh both      # Both PostgreSQL and MySQL
./scripts/setup-test-databases.sh postgres  # PostgreSQL only
./scripts/setup-test-databases.sh mysql     # MySQL only

# Check database status
./scripts/setup-test-databases.sh status

# Run integration tests
./scripts/setup-test-databases.sh test

# Stop databases
./scripts/setup-test-databases.sh stop

# Remove databases
./scripts/setup-test-databases.sh remove
```

Test database connection strings:
- PostgreSQL: `postgres://postgres:hammerwork@localhost:5433/hammerwork`
- MySQL: `mysql://root:hammerwork@localhost:3307/hammerwork`

### Development Workflow

A development helper script is available for common tasks:

```bash
# From the project root directory:

# Run full check (format + lint + test)
./scripts/dev.sh check

# Run tests with database integration
./scripts/dev.sh test-db

# CLI development workflow
./scripts/dev.sh cli

# Build everything
./scripts/dev.sh build

# Format code
./scripts/dev.sh fmt

# Run clippy
./scripts/dev.sh lint

# Generate docs
./scripts/dev.sh docs

# See all available commands
./scripts/dev.sh help
```

### Code Quality

The codebase follows Rust best practices:

- **Error Handling**: Comprehensive error types with context
- **Documentation**: Inline docs and examples
- **Modularity**: Clean separation of concerns
- **Type Safety**: Leverages Rust's type system for reliability
- **Async/Await**: Modern async patterns throughout

## Integration with Hammerwork

This CLI is designed to work seamlessly with Hammerwork applications:

- **Database Schema**: Creates and maintains compatible table structures
- **Job Format**: Handles Hammerwork job formats and priorities
- **Worker Compatibility**: Designed to work with Hammerwork workers
- **Migration Safety**: Respects existing Hammerwork installations

## Troubleshooting

### Common Issues

1. **Database Connection Errors**: Verify your DATABASE_URL and database accessibility
2. **Permission Errors**: Ensure database user has necessary privileges
3. **Configuration Issues**: Check config file location with `config path`

### Debugging

```bash
# Enable debug logging
cargo hammerwork -v migration run

# Check configuration
cargo hammerwork config show

# Verify database connectivity
cargo hammerwork migration status
```

## Future Roadmap

Planned enhancements include:

- **Web Dashboard**: Browser-based monitoring interface
- **Cluster Management**: Multi-node coordination features
- **Plugin System**: Extensible plugin architecture
- **Advanced Analytics**: Historical performance analysis and trending
- **External Integrations**: Webhook notifications and third-party service integration

## License

Same as the parent Hammerwork project: MIT OR Apache-2.0