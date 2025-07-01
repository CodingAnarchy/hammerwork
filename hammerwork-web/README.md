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
# Start the dashboard (PostgreSQL)
hammerwork-web --database-url postgresql://user:pass@localhost/hammerwork

# Start with authentication enabled
hammerwork-web \
  --database-url postgresql://user:pass@localhost/hammerwork \
  --auth \
  --username admin \
  --password-file /path/to/password_hash.txt

# Start with custom port and CORS enabled
hammerwork-web \
  --database-url mysql://user:pass@localhost/hammerwork \
  --bind 0.0.0.0 \
  --port 9090 \
  --cors
```

### Configuration File

Create a `dashboard.toml` configuration file:

```toml
bind_address = "0.0.0.0"
port = 8080
database_url = "postgresql://localhost/hammerwork"
pool_size = 10
static_dir = "./assets"
enable_cors = false
request_timeout = "30s"

[auth]
enabled = true
username = "admin"
password_hash = "$2b$12$..." # bcrypt hash
session_timeout = "8h"
max_failed_attempts = 5
lockout_duration = "15m"

[websocket]
ping_interval = "30s"
max_connections = 100
message_buffer_size = 1024
max_message_size = 65536
```

Then start with:

```bash
hammerwork-web --config dashboard.toml
```

## Library Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
hammerwork-web = { version = "1.3", features = ["postgres"] }
# or for MySQL:
# hammerwork-web = { version = "1.3", features = ["mysql"] }
```

### Programmatic Usage

```rust
use hammerwork_web::{WebDashboard, DashboardConfig};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = DashboardConfig {
        bind_address: "127.0.0.1".to_string(),
        port: 8080,
        database_url: "postgresql://localhost/hammerwork".to_string(),
        pool_size: 5,
        static_dir: "./assets".into(),
        enable_cors: true,
        request_timeout: Duration::from_secs(30),
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
    .with_cors(true);

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
- `GET /api/stats/throughput?period=24h` - Throughput data

#### Queues
- `GET /api/queues` - List all queues with statistics
- `POST /api/queues/{name}/clear` - Clear all jobs from queue
- `POST /api/queues/{name}/pause` - Pause queue processing
- `POST /api/queues/{name}/resume` - Resume queue processing

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

# Build with authentication support
cargo build --features "postgres,auth"

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
- **Password Hashing**: bcrypt with configurable cost (default: 12)
- **Rate Limiting**: Failed login attempt tracking with account lockout
- **Session Management**: Configurable session timeouts

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
# Using bcrypt command-line tool
echo -n "your-password" | bcrypt-tool hash

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
CMD ["hammerwork-web", "--bind", "0.0.0.0", "--port", "8080"]
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
- **Request Timeouts**: Set appropriate `request_timeout` for your network
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