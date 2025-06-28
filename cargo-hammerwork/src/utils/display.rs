use comfy_table::Table;
use std::fmt;

pub struct JobTable {
    table: Table,
}

impl Default for JobTable {
    fn default() -> Self {
        Self::new()
    }
}

impl JobTable {
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

pub fn format_duration(seconds: Option<i64>) -> String {
    match seconds {
        Some(secs) if secs < 60 => format!("{}s", secs),
        Some(secs) if secs < 3600 => format!("{}m {}s", secs / 60, secs % 60),
        Some(secs) => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
        None => "N/A".to_string(),
    }
}

pub fn format_size(bytes: Option<i64>) -> String {
    match bytes {
        Some(b) if b < 1024 => format!("{}B", b),
        Some(b) if b < 1024 * 1024 => format!("{:.1}KB", b as f64 / 1024.0),
        Some(b) if b < 1024 * 1024 * 1024 => format!("{:.1}MB", b as f64 / (1024.0 * 1024.0)),
        Some(b) => format!("{:.1}GB", b as f64 / (1024.0 * 1024.0 * 1024.0)),
        None => "N/A".to_string(),
    }
}