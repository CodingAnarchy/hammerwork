//! Display formatting utilities for the Hammerwork CLI.
//!
//! This module provides table formatting and display utilities for presenting
//! job information, statistics, and other data in a user-friendly format.
//!
//! # Examples
//!
//! ## Creating a Job Table
//!
//! ```rust
//! use cargo_hammerwork::utils::display::JobTable;
//!
//! let mut table = JobTable::new();
//! table.add_job_row(
//!     "550e8400-e29b-41d4-a716-446655440000",
//!     "email",
//!     "pending",
//!     "high",
//!     0,
//!     "2024-01-01 10:00:00",
//!     "2024-01-01 10:05:00"
//! );
//!
//! // Display the table
//! println!("{}", table);
//! ```
//!
//! ## Creating a Statistics Table
//!
//! ```rust
//! use cargo_hammerwork::utils::display::StatsTable;
//!
//! let mut stats = StatsTable::new();
//! stats.add_stats_row("pending", "normal", 42);
//! stats.add_stats_row("running", "high", 5);
//! stats.add_stats_row("completed", "normal", 1337);
//!
//! println!("{}", stats);
//! ```
//!
//! ## Formatting Helpers
//!
//! ```rust
//! use cargo_hammerwork::utils::display::{format_duration, format_size};
//!
//! // Format durations
//! assert_eq!(format_duration(Some(45)), "45s");
//! assert_eq!(format_duration(Some(125)), "2m 5s");
//! assert_eq!(format_duration(Some(3661)), "1h 1m");
//! assert_eq!(format_duration(None), "N/A");
//!
//! // Format sizes
//! assert_eq!(format_size(Some(512)), "512B");
//! assert_eq!(format_size(Some(2048)), "2.0KB");
//! assert_eq!(format_size(Some(1_048_576)), "1.0MB");
//! assert_eq!(format_size(None), "N/A");
//! ```

use comfy_table::Table;
use std::fmt;

/// Table formatter for displaying job information.
///
/// This struct creates formatted tables with job details including
/// status icons, priority indicators, and truncated IDs for readability.
///
/// # Examples
///
/// ```rust
/// use cargo_hammerwork::utils::display::JobTable;
///
/// let mut table = JobTable::new();
///
/// // Add multiple jobs
/// table.add_job_row(
///     "job-id-1", "email", "pending", "normal", 0,
///     "2024-01-01 10:00:00", "2024-01-01 10:00:00"
/// );
/// table.add_job_row(
///     "job-id-2", "data-processing", "running", "high", 1,
///     "2024-01-01 09:55:00", "2024-01-01 10:00:00"
/// );
///
/// // The table will display with color-coded status and priority
/// ```
pub struct JobTable {
    table: Table,
}

impl Default for JobTable {
    fn default() -> Self {
        Self::new()
    }
}

impl JobTable {
    /// Create a new job table with predefined headers.
    ///
    /// Headers include: ID, Queue, Status, Priority, Attempts, Created At, Scheduled At
    pub fn new() -> Self {
        let mut table = Table::new();
        table.set_header(vec![
            "ID",
            "Queue",
            "Status",
            "Priority",
            "Attempts",
            "Created At",
            "Scheduled At",
        ]);
        Self { table }
    }

    /// Add a job row to the table.
    ///
    /// The method automatically:
    /// - Truncates job IDs to 8 characters for readability
    /// - Adds status icons and colors (🟡 pending, 🔵 running, 🟢 completed, etc.)
    /// - Adds priority icons (🚨 critical, ⚡ high, 📝 normal, etc.)
    ///
    /// # Arguments
    ///
    /// * `id` - Job UUID
    /// * `queue_name` - Name of the queue
    /// * `status` - Job status (pending, running, completed, failed, dead, retrying)
    /// * `priority` - Job priority (critical, high, normal, low, background)
    /// * `attempts` - Number of execution attempts
    /// * `created_at` - Creation timestamp
    /// * `scheduled_at` - Scheduled execution timestamp
    #[allow(clippy::too_many_arguments)]
    pub fn add_job_row(
        &mut self,
        id: &str,
        queue_name: &str,
        status: &str,
        priority: &str,
        attempts: i32,
        created_at: &str,
        scheduled_at: &str,
    ) {
        let status_colored = match status {
            "pending" => format!("🟡 {}", status),
            "running" => format!("🔵 {}", status),
            "completed" => format!("🟢 {}", status),
            "failed" => format!("🔴 {}", status),
            "dead" => format!("💀 {}", status),
            "retrying" => format!("🟠 {}", status),
            _ => status.to_string(),
        };

        let priority_colored = match priority {
            "critical" => format!("🚨 {}", priority),
            "high" => format!("⚡ {}", priority),
            "normal" => format!("📝 {}", priority),
            "low" => format!("🐌 {}", priority),
            "background" => format!("💤 {}", priority),
            _ => priority.to_string(),
        };

        self.table.add_row(vec![
            &id[..8.min(id.len())],
            queue_name,
            &status_colored,
            &priority_colored,
            &attempts.to_string(),
            created_at,
            scheduled_at,
        ]);
    }
}

impl fmt::Display for JobTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.table)
    }
}

/// Table formatter for displaying job statistics.
///
/// This struct creates formatted tables showing job counts by status and priority,
/// with visual indicators for easy scanning.
///
/// # Examples
///
/// ```rust
/// use cargo_hammerwork::utils::display::StatsTable;
///
/// let mut stats = StatsTable::new();
/// stats.add_stats_row("pending", "normal", 100);
/// stats.add_stats_row("running", "high", 10);
/// stats.add_stats_row("failed", "critical", 2);
///
/// // Display shows icons: 🟡 pending, 🔵 running, 🔴 failed
/// ```
pub struct StatsTable {
    table: Table,
}

impl Default for StatsTable {
    fn default() -> Self {
        Self::new()
    }
}

impl StatsTable {
    pub fn new() -> Self {
        let mut table = Table::new();
        table.set_header(vec!["Status", "Priority", "Count"]);
        Self { table }
    }

    pub fn add_stats_row(&mut self, status: &str, priority: &str, count: i64) {
        let status_icon = match status {
            "pending" => "🟡",
            "running" => "🔵",
            "completed" => "🟢",
            "failed" => "🔴",
            "dead" => "💀",
            "retrying" => "🟠",
            _ => "❓",
        };

        let priority_icon = match priority {
            "critical" => "🚨",
            "high" => "⚡",
            "normal" => "📝",
            "low" => "🐌",
            "background" => "💤",
            _ => "❓",
        };

        self.table.add_row(vec![
            &format!("{} {}", status_icon, status),
            &format!("{} {}", priority_icon, priority),
            &count.to_string(),
        ]);
    }
}

impl fmt::Display for StatsTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.table)
    }
}

/// Format a duration in seconds to a human-readable string.
///
/// # Examples
///
/// ```rust
/// use cargo_hammerwork::utils::display::format_duration;
///
/// assert_eq!(format_duration(Some(30)), "30s");
/// assert_eq!(format_duration(Some(90)), "1m 30s");
/// assert_eq!(format_duration(Some(3665)), "1h 1m");
/// assert_eq!(format_duration(Some(7200)), "2h 0m");
/// assert_eq!(format_duration(None), "N/A");
/// ```
pub fn format_duration(seconds: Option<i64>) -> String {
    match seconds {
        Some(secs) if secs < 60 => format!("{}s", secs),
        Some(secs) if secs < 3600 => format!("{}m {}s", secs / 60, secs % 60),
        Some(secs) => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
        None => "N/A".to_string(),
    }
}

/// Format a byte size to a human-readable string.
///
/// # Examples
///
/// ```rust
/// use cargo_hammerwork::utils::display::format_size;
///
/// assert_eq!(format_size(Some(100)), "100B");
/// assert_eq!(format_size(Some(1024)), "1.0KB");
/// assert_eq!(format_size(Some(1536)), "1.5KB");
/// assert_eq!(format_size(Some(1048576)), "1.0MB");
/// assert_eq!(format_size(Some(1073741824)), "1.0GB");
/// assert_eq!(format_size(None), "N/A");
/// ```
pub fn format_size(bytes: Option<i64>) -> String {
    match bytes {
        Some(b) if b < 1024 => format!("{}B", b),
        Some(b) if b < 1024 * 1024 => format!("{:.1}KB", b as f64 / 1024.0),
        Some(b) if b < 1024 * 1024 * 1024 => format!("{:.1}MB", b as f64 / (1024.0 * 1024.0)),
        Some(b) => format!("{:.1}GB", b as f64 / (1024.0 * 1024.0 * 1024.0)),
        None => "N/A".to_string(),
    }
}
