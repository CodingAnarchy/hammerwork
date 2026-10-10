# Hammerwork Web Dashboard

A modern, real-time web-based admin dashboard for monitoring and managing [Hammerwork](../README.md) job queues. Built with Rust, Warp, and WebSockets for high-performance job queue administration.

## Features

- **Real-time Monitoring**: Live updates via WebSockets for queue statistics, job status, and system health
- **Job Management**: View, retry, cancel, and inspect jobs with detailed payload and error information
- **Job Archive Management**: Archive, restore, and purge jobs with configurable retention policies
- **Queue Administration**: Monitor queue performance, clear queues, and manage queue priorities
- **Archive Statistics**: Track storage savings, compression ratios, and archival operations
- **Multi-Database Support**: Works with both PostgreSQL and MySQL backends
- **Security**: Built-in authentication with bcrypt password hashing and rate limiting
- **Modern UI**: Responsive dashboard with charts, tables, and real-time indicators
- **REST API**: Complete RESTful API for programmatic access
- **High Performance**: Async/await throughout with efficient database pooling

## Screenshots

The dashboard provides:
- **Overview Cards**: Total jobs, pending/running counts, error rates, and throughput
- **Queue Table**: Real-time queue statistics with actions (clear, pause, resume)
- **Jobs Table**: Filterable job listing with status, priority, and actions
- **Archive Section**: Archived jobs management with statistics and restoration capabilities
- **Charts**: Throughput over time and job status distribution
- **Real-time Updates**: WebSocket connections for live data updates

## Quick Start

### Installation

```bash
# Install from crates.io
cargo install hammerwork-web --features postgres

# Or build from source
git clone https://github.com/CodingAnarchy/hammerwork.git
cd hammerwork/hammerwork-web
cargo build --release --features postgres
```

### Basic Usage

```bash
# Start the dashboard locally without authentication (PostgreSQL). Without --no-auth,
# the dashboard refuses to start until a password is configured.
hammerwork-web --database-url postgresql://user:pass@localhost/hammerwork --no-auth

# Start with authentication enabled
hammerwork-web \
  --database-url postgresql://user:pass@localhost/hammerwork \
  --auth \
  --username admin \
  --password-file /path/to/password_hash.txt

# Start with custom port, and let pages on https://ops.example.com call the API (CORS)
hammerwork-web \
  --database-url mysql://user:pass@localhost/hammerwork \
  --bind 0.0.0.0 \
  --port 9090 \
  --cors \
  --allowed-origin https://ops.example.com \
  --auth \
  --password-file /path/to/password_hash.txt
```

The password file holds a **bcrypt hash** of the password, never the password itself
(see [Generate Password Hash](#example-generate-password-hash)). `--password` hashes a
password given on the command line for you.

### Configuration File

Create a `dashboard.toml` configuration file:

```toml
bind_address = "0.0.0.0"
port = 8080
database_url = "postgresql://localhost/hammerwork"
pool_size = 10
static_dir = "./assets"
enable_cors = false
# Other origins whose pages may change data / open WebSockets (and, with enable_cors, read the API)
allowed_origins = []

[auth]
enabled = true
username = "admin"
password_hash = "$2b$12$..." # bcrypt hash, never the password itself
session_timeout = "8h"        # caps how long a verified login is remembered (at most 60 s)
max_failed_attempts = 5       # per client address
lockout_duration = "15m"

[websocket]
ping_interval = "30s"
max_connections = 100
message_buffer_size = 1024    # outgoing messages queued per connection
max_message_size = 65536      # largest message a client may send
# Live updates: poll the database for job/queue changes while clients are connected
live_update_interval = "2s"     # zero disables live updates
live_update_max_jobs = 100    # changed jobs read and pushed per poll
```

Then start with:

```bash
hammerwork-web --config dashboard.toml
```

### Encrypted Queues

Jobs created from the dashboard (`POST /api/jobs`) are encrypted like your application's.
Set `HAMMERWORK_ENCRYPTION_CONFIG` to the application's `hammerwork.toml` (only its
`[encryption]` section is read; the `HAMMERWORK_ENCRYPTION_*` variables apply on top) and
make its key available, e.g. `HAMMERWORK_ENCRYPTION_KEY`. Jobs on its `encrypted_queues` are
then encrypted with its key. If encryption is enabled but the key cannot be loaded, the
dashboard does not start. Without these settings it still refuses to create a plaintext job
on a queue that already holds encrypted jobs. Viewing jobs never needs the key: encrypted
payloads are shown redacted.

## Library Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
hammerwork-web = { version = "2.0", features = ["postgres"] }
# or for MySQL:
# hammerwork-web = { version = "2.0", features = ["mysql"] }
```

### Programmatic Usage

```rust
use hammerwork_web::{WebDashboard, DashboardConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = DashboardConfig {
        bind_address: "127.0.0.1".to_string(),
        port: 8080,
        database_url: "postgresql://localhost/hammerwork".to_string(),
        pool_size: 5,
        static_dir: "./assets".into(),
        enable_cors: true,
        // CORS is only granted to listed origins; enabling it without any is an error.
        allowed_origins: vec!["https://ops.example.com".to_string()],
        ..Default::default()
    };

    let dashboard = WebDashboard::new(config).await?;
    dashboard.start().await?;

    Ok(())
}
```

### With Authentication

```rust
use hammerwork_web::{WebDashboard, DashboardConfig, AuthConfig};

let config = DashboardConfig::new()
    .with_database_url("postgresql://localhost/hammerwork")
    .with_bind_address("0.0.0.0", 8080)
    .with_auth("admin", "$2b$12$hash...") // bcrypt hash
    .with_cors(true)
    .with_allowed_origin("https://ops.example.com");

let dashboard = WebDashboard::new(config).await?;
dashboard.start().await?;
```

## API Reference

The dashboard exposes a complete REST API:

### Authentication

All API endpoints (except `/health`) require authentication when enabled:

```bash
curl -u admin:password http://localhost:8080/api/stats/overview
```

### Endpoints

#### System
- `GET /health` - Health check (no auth required)
- `GET /api/stats/overview` - System overview statistics
- `GET /api/stats/detailed` - Per-queue statistics, hourly trends, error patterns and performance metrics
- `GET /api/stats/trends` - Completed/failed jobs per hour from the database (last 24 hours, or a `time_range` of up to 31 days)
- `GET /api/stats/health` - Health assessment

Values the dashboard cannot measure are reported as `null` (for example memory usage on macOS, worker counts, CPU, and the metrics count), never as made-up numbers. "Failed" in trends and error patterns means jobs currently in `Failed`, `Dead` or `TimedOut`, counted at the hour they failed.

#### Queues
- `GET /api/queues` - List all queues with statistics
- `GET /api/queues/{name}` - Queue details with hourly throughput and recent errors
- `GET /api/queues/{name}/jobs` - Jobs in the queue (same filters as `/api/jobs`)
- `POST /api/queues/{name}/actions` - `{"action": "pause" | "resume" | "clear_completed" | "clear_dead"}`; `clear_completed` deletes the queue's completed jobs, `clear_dead` its dead jobs older than 7 days

#### Jobs
- `GET /api/jobs?status=failed&limit=50` - List jobs with filters
- `GET /api/jobs/{id}` - Get job details
- `POST /api/jobs` - Create new job
- `POST /api/jobs/{id}/retry` - Retry failed job
- `DELETE /api/jobs/{id}` - Delete job

#### Archive Management
- `GET /api/archive/jobs?queue=email&limit=50` - List archived jobs with filters
- `POST /api/archive/jobs` - Archive jobs based on configurable policy
- `POST /api/archive/jobs/{id}/restore` - Restore an archived job to pending status
- `DELETE /api/archive/purge` - Permanently purge old archived jobs
- `GET /api/archive/stats?queue=email` - Get archive statistics and metrics

#### System
- `GET /api/system/info`, `GET /api/system/config`, `GET /api/system/metrics`, `GET /api/version`
- `POST /api/system/maintenance` - `{"operation": ..., "target": ..., "dry_run": ...}`:
  - `cleanup` deletes dead jobs older than 7 days
  - `vacuum`: `VACUUM (ANALYZE)` on PostgreSQL, `OPTIMIZE TABLE` on MySQL
  - `reindex`: `REINDEX TABLE` on PostgreSQL, `OPTIMIZE TABLE` on MySQL (InnoDB rebuilds the indexes)
  - `optimize`: `ANALYZE` on PostgreSQL, `OPTIMIZE TABLE` on MySQL

  Table operations run on every Hammerwork table that exists (`hammerwork_jobs`,
  `hammerwork_jobs_archive`, ...), or only on `target` when it names one of them; other
  names are rejected. The response lists each statement that ran, or with `dry_run` would run.

### WebSocket API

Connect to `ws://localhost:8080/ws` for real-time updates:

```javascript
const ws = new WebSocket('ws://localhost:8080/ws');

// Subscribe to events
ws.send(JSON.stringify({
    type: 'Subscribe',
    event_types: ['queue_updates', 'job_updates', 'system_alerts']
}));

// Handle updates
ws.onmessage = (event) => {
    const message = JSON.parse(event.data);
    console.log('Received:', message);
};
```

A new connection receives every event type until it sends its first `Subscribe` or
`Unsubscribe`. Each message has a `type`:

| `type` | Event type | Contents |
|---|---|---|
| `JobUpdate` | `job_updates` | `job`: `id`, `queue_name`, `status`, `priority`, `attempts`, `updated_at` |
| `QueueUpdate` | `queue_updates` | `queue_name`, `stats` (pending, running, completed, failed and dead counts, throughput, error rate) |
| `SystemAlert` | `system_alerts` | `message`, `severity` |
| `JobArchived`, `JobRestored`, `BulkArchive*`, `JobsPurged` | `archive_events` | archive operation details |
| `Pong` | | the answer to a client `{"type": "Ping"}` |

#### Live updates

The dashboard usually runs in its own process, apart from the workers, so it cannot see
the job events a worker publishes in its own process. Instead, while at least one client
is connected, it polls the database every `websocket.live_update_interval` (default 2
seconds) and pushes:

- a `JobUpdate` for each job created, started, completed, failed or timed out since the
  previous poll (at most `websocket.live_update_max_jobs` per poll, newest first);
- a `QueueUpdate` for each queue whose counts changed.

Each poll runs two small queries plus the queue statistics query. Nothing is polled
while no client is connected; set `live_update_interval` to zero to turn polling off.
Changes that leave no timestamp (a retry back to `Pending`, a deleted job) show up only
in the queue statistics.

An application that embeds the dashboard in the same process as its workers can also
forward their events as they happen:

```rust
// `events` is the Arc<EventManager> the workers were given with `with_event_manager`.
let dashboard = WebDashboard::new(config)
    .await?
    .with_event_manager(events.clone());
dashboard.start().await?;
```

## Development

### Prerequisites

- Rust 1.70+
- PostgreSQL or MySQL database
- Node.js (for frontend development)

### Database Setup

Use the provided scripts to set up test databases:

```bash
# Set up both PostgreSQL and MySQL test databases
./scripts/setup-test-databases.sh both

# PostgreSQL only
./scripts/setup-test-databases.sh postgres

# MySQL only  
./scripts/setup-test-databases.sh mysql
```

Default test database connections:
- **PostgreSQL**: `postgresql://postgres:hammerwork@localhost:5433/hammerwork`
- **MySQL**: `mysql://root:hammerwork@localhost:3307/hammerwork`

### Building

```bash
# Build with PostgreSQL support
cargo build --features postgres

# Build with MySQL support
cargo build --features mysql

# Authentication (bcrypt) is a default feature. A build without it
# (--no-default-features) refuses to start with authentication enabled.
cargo build --no-default-features --features postgres

# Build everything
cargo build --all-features
```

### Testing

```bash
# Unit tests
cargo test

# Integration tests (requires database)
cargo test --features postgres -- --ignored

# Test with both databases
./scripts/setup-test-databases.sh test
```

### Development Commands

```bash
# Format code
cargo fmt

# Lint code
cargo clippy --all-features -- -D warnings

# Run with hot reload (requires cargo-watch)
cargo watch -x "run --features postgres -- --database-url postgresql://postgres:hammerwork@localhost:5433/hammerwork"

# Generate documentation
cargo doc --all-features --open
```

## Frontend Development

The dashboard frontend is built with vanilla JavaScript, HTML, and CSS for minimal dependencies and fast loading.

### Asset Structure

```
assets/
├── index.html          # Main dashboard page
├── dashboard.css       # Styles and responsive design
├── dashboard.js        # WebSocket client and UI logic
└── chart.min.js        # Chart.js for data visualization
```

### Adding Features

1. **New API Endpoint**: Add to appropriate module in `src/api/`
2. **WebSocket Message**: Update `ServerMessage` enum in `src/websocket.rs`
3. **Frontend Component**: Add to `assets/dashboard.js` and update UI in `index.html`
4. **Tests**: Add unit tests in module and integration tests in `tests/`

### Archive API Examples

```bash
# List archived jobs
curl -u admin:password "http://localhost:8080/api/archive/jobs?queue=email&limit=10"

# Archive completed jobs older than 7 days
curl -u admin:password -X POST http://localhost:8080/api/archive/jobs \
  -H "Content-Type: application/json" \
  -d '{
    "queue_name": "email",
    "reason": "automatic",
    "archived_by": "scheduler",
    "dry_run": false,
    "policy": {
      "archive_completed_after": 604800000000000,
      "compress_payloads": true,
      "enabled": true
    }
  }'

# Restore an archived job
curl -u admin:password -X POST http://localhost:8080/api/archive/jobs/12345/restore \
  -H "Content-Type: application/json" \
  -d '{"reason": "data recovery", "restored_by": "admin"}'

# Get archive statistics
curl -u admin:password http://localhost:8080/api/archive/stats?queue=email
```

## Security

### Authentication

- **Basic Authentication**: RFC 7617 compliant with base64 encoding
- **Password Hashing**: the configured `password_hash` is a bcrypt hash and is only ever
  verified with bcrypt, never compared to the password as text. bcrypt comes with the
  `auth` feature, which is on by default; a build without it refuses to start with
  authentication enabled.
- **No blocking**: bcrypt runs on Tokio's blocking pool, one verification per CPU at a
  time. A successful login is remembered for 60 seconds (or `session_timeout`, if shorter;
  `0` turns this off), so a dashboard polling several endpoints does not pay for bcrypt on
  every request.
- **Constant-time checks**: usernames are compared in constant time, and bcrypt runs
  whether or not the username matched, so response times do not reveal the username.
- **Lockout**: failures are counted per client address (the TCP peer address, never a
  header) and username. After `max_failed_attempts` failures that client is refused with
  `429` for `lockout_duration`; afterwards the count starts over, and a successful login
  resets it. An attacker therefore cannot lock the administrator out from other addresses.
  Behind a reverse proxy every client shares the proxy's address. At most 10,000 clients are
  tracked, and made-up usernames share one record per address.

### Cross-site requests (CSRF)

A page the operator visits in the same browser could make it send requests to the dashboard,
with any cached Basic credentials attached. The dashboard therefore:

- requires `Content-Type: application/json` on every request with a body (`415` otherwise),
  which a page on another origin cannot send without a CORS preflight;
- refuses state-changing requests (`POST`, `PUT`, `DELETE`, ...) and WebSocket handshakes
  that a browser sent from another origin (`403`), judged by `Sec-Fetch-Site` or, without
  it, by comparing `Origin` with `Host`. Requests without either header (curl, scripts) are
  not from a browser page and pass. Origins in `allowed_origins` (`--allowed-origin`) pass too.
- grants CORS (`enable_cors` / `--cors`) only to the origins in `allowed_origins`, never to
  every origin; enabling CORS without any is a startup error.

### Request limits

- JSON request bodies are limited to 1 MiB and need a `Content-Length` header (`413` / `411`).
- A bulk job action takes at most 1,000 job IDs.
- Pages hold at most 1,000 items (`limit` is clamped before the offset is computed).
  Archive listings are filtered, counted and paged in the database.
- WebSocket messages from clients are limited to `websocket.max_message_size`; each
  connection queues at most `websocket.message_buffer_size` outgoing messages, and messages
  for a client that does not read are dropped. Subscriptions accept only the known event
  types.

### Best Practices

- Change default credentials immediately
- Use strong passwords (12+ characters)
- Enable authentication in production
- Use HTTPS in production
- Configure firewall rules appropriately
- Store password hashes securely
- Regular security updates

### Example: Generate Password Hash

```bash
# Using the htpasswd tool (Apache utils); strip the leading "user:"
htpasswd -bnBC 12 "" "your-password" | tr -d ':\n'

# Or in Rust
use bcrypt::{hash, DEFAULT_COST};
let hash = hash("your-password", DEFAULT_COST)?;
```

## Deployment

### Docker

```dockerfile
FROM rust:1.70 as builder
WORKDIR /app
COPY . .
RUN cargo build --release --features postgres

FROM debian:booksworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/hammerwork-web /usr/local/bin/
COPY --from=builder /app/hammerwork-web/assets /app/assets
WORKDIR /app
EXPOSE 8080
CMD ["hammerwork-web", "--bind", "0.0.0.0", "--port", "8080", "--auth", "--password-file", "/run/secrets/dashboard_password_hash"]
```

### systemd Service

```ini
[Unit]
Description=Hammerwork Web Dashboard
After=network.target postgresql.service

[Service]
Type=simple
User=hammerwork
WorkingDirectory=/opt/hammerwork
ExecStart=/opt/hammerwork/hammerwork-web --config /etc/hammerwork/dashboard.toml
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

### Nginx Reverse Proxy

```nginx
server {
    listen 80;
    server_name hammerwork.example.com;
    
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection 'upgrade';
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_cache_bypass $http_upgrade;
    }
}
```

## Performance

### Optimization Tips

- **Connection Pooling**: Tune `pool_size` based on concurrent users
- **WebSocket Limits**: Configure `max_connections` for your use case  
- **Request Timeouts**: The dashboard applies none of its own (`request_timeout` was removed in 2.0; files that still set it load); set them on a reverse proxy in front of it
- **Database Indexes**: Ensure proper indexes on hammerwork_jobs table
- **Static Assets**: Use a CDN for production deployments
- **Monitoring**: Enable structured logging and metrics collection

### Monitoring

```bash
# Enable debug logging
RUST_LOG=hammerwork_web=debug hammerwork-web --config dashboard.toml

# Monitor WebSocket connections
curl http://localhost:8080/api/stats/overview | jq '.websocket_connections'

# Database connection health
curl http://localhost:8080/health
```

## Troubleshooting

### Common Issues

**Database Connection Failed**
```
Error: Database connection failed
```
- Verify database URL format and credentials
- Check database server is running and accessible
- Ensure database exists and migrations are applied

**Static Assets Not Found**
```
Error: Static file not found
```
- Check `static_dir` path in configuration
- Ensure assets directory contains `index.html`
- Verify file permissions

**Authentication Issues**
```
401 Unauthorized
```
- Verify username and password
- Check password hash format (bcrypt)
- Review rate limiting settings

**WebSocket Connection Failed**
```
WebSocket connection closed unexpectedly
```
- Check firewall settings for WebSocket traffic
- Verify proxy configuration for Upgrade headers
- Review browser console for detailed errors

### Debug Mode

```bash
# Enable verbose logging
RUST_LOG=debug hammerwork-web --config dashboard.toml

# Enable specific module logging
RUST_LOG=hammerwork_web::websocket=trace hammerwork-web --config dashboard.toml
```

## Contributing

We welcome contributions! Please see the main [CONTRIBUTING.md](../CONTRIBUTING.md) for guidelines.

### Areas for Contribution

- Additional database backends (SQLite, CockroachDB)
- Enhanced security features (OAuth, JWT)
- More visualization options (metrics dashboards)
- Archive management enhancements (scheduled policies, bulk operations)
- Real-time WebSocket events for archive operations
- Mobile-responsive improvements
- Internationalization (i18n)
- Plugin system for custom integrations

## License

This project is licensed under the MIT License - see the [LICENSE](../LICENSE) file for details.

## Changelog

See [CHANGELOG.md](../CHANGELOG.md) for version history and changes.