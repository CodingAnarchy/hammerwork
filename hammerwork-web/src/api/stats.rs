//! Statistics and monitoring API endpoints.
//!
//! This module provides comprehensive monitoring and analytics endpoints for tracking
//! system health, performance metrics, and operational insights across all job queues.
//!
//! # API Endpoints
//!
//! - `GET /api/stats/overview` - System overview with key metrics
//! - `GET /api/stats/detailed` - Detailed statistics with historical trends
//! - `GET /api/stats/trends` - Hourly/daily trend analysis
//! - `GET /api/stats/health` - System health check and alerts
//!
//! # Examples
//!
//! ## System Overview
//!
//! ```rust
//! use hammerwork_web::api::stats::{SystemOverview, SystemHealth, SystemAlert};
//! use chrono::Utc;
//!
//! let overview = SystemOverview {
//!     total_queues: 5,
//!     total_jobs: 10000,
//!     pending_jobs: 50,
//!     running_jobs: 10,
//!     completed_jobs: 9800,
//!     failed_jobs: 125,
//!     dead_jobs: 15,
//!     overall_throughput: 150.5,
//!     overall_error_rate: 0.0125,
//!     avg_processing_time_ms: 250.0,
//!     system_health: SystemHealth {
//!         status: "healthy".to_string(),
//!         database_healthy: true,
//!         high_error_rate: false,
//!         queue_backlog: false,
//!         slow_processing: false,
//!         alerts: vec![],
//!     },
//!     uptime_seconds: 86400,
//!     last_updated: Utc::now(),
//! };
//!
//! assert_eq!(overview.total_queues, 5);
//! assert_eq!(overview.overall_error_rate, 0.0125);
//! assert_eq!(overview.system_health.status, "healthy");
//! ```
//!
//! ## Statistics Queries
//!
//! ```rust
//! use hammerwork_web::api::stats::{StatsQuery, TimeRange};
//! use chrono::{Utc, Duration};
//!
//! let time_range = TimeRange {
//!     start: Utc::now() - Duration::hours(24),
//!     end: Utc::now(),
//! };
//!
//! let query = StatsQuery {
//!     time_range: Some(time_range),
//!     queues: Some(vec!["email".to_string(), "notifications".to_string()]),
//!     granularity: Some("hour".to_string()),
//! };
//!
//! assert!(query.time_range.is_some());
//! assert_eq!(query.queues.as_ref().unwrap().len(), 2);
//! assert_eq!(query.granularity, Some("hour".to_string()));
//! ```
//!
//! ## System Alerts
//!
//! ```rust
//! use hammerwork_web::api::stats::SystemAlert;
//! use chrono::Utc;
//!
//! let alert = SystemAlert {
//!     severity: "warning".to_string(),
//!     message: "Queue backlog detected".to_string(),
//!     queue: Some("image_processing".to_string()),
//!     metric: Some("pending_count".to_string()),
//!     value: Some(1500.0),
//!     threshold: Some(1000.0),
//!     timestamp: Utc::now(),
//! };
//!
//! assert_eq!(alert.severity, "warning");
//! assert_eq!(alert.queue, Some("image_processing".to_string()));
//! assert_eq!(alert.value, Some(1500.0));
//! ```
//!
//! ## Performance Metrics
//!
//! ```rust
//! use hammerwork_web::api::stats::PerformanceMetrics;
//!
//! let metrics = PerformanceMetrics {
//!     database_response_time_ms: 5.2,
//!     average_queue_depth: 15.5,
//!     jobs_per_second: 8.3,
//!     memory_usage_mb: Some(512.0),
//!     cpu_usage_percent: Some(45.2),
//!     active_workers: 12,
//!     worker_utilization: 0.75,
//! };
//!
//! assert_eq!(metrics.database_response_time_ms, 5.2);
//! assert_eq!(metrics.active_workers, 12);
//! assert_eq!(metrics.worker_utilization, 0.75);
//! ```

use super::ApiResponse;
use hammerwork::queue::DatabaseQueue;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use warp::{Filter, Reply};

/// System overview statistics
#[derive(Debug, Serialize)]
pub struct SystemOverview {
    pub total_queues: u32,
    pub total_jobs: u64,
    pub pending_jobs: u64,
    pub running_jobs: u64,
    pub completed_jobs: u64,
    pub failed_jobs: u64,
    pub dead_jobs: u64,
    pub overall_throughput: f64,
    pub overall_error_rate: f64,
    pub avg_processing_time_ms: f64,
    pub system_health: SystemHealth,
    pub uptime_seconds: u64,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

/// System health status
#[derive(Debug, Serialize)]
pub struct SystemHealth {
    pub status: String, // "healthy", "degraded", "critical"
    pub database_healthy: bool,
    pub high_error_rate: bool,
    pub queue_backlog: bool,
    pub slow_processing: bool,
    pub alerts: Vec<SystemAlert>,
}

/// System alert
#[derive(Debug, Serialize)]
pub struct SystemAlert {
    pub severity: String, // "info", "warning", "error", "critical"
    pub message: String,
    pub queue: Option<String>,
    pub metric: Option<String>,
    pub value: Option<f64>,
    pub threshold: Option<f64>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Detailed statistics for monitoring
#[derive(Debug, Serialize)]
pub struct DetailedStats {
    pub overview: SystemOverview,
    pub queue_stats: Vec<QueueStats>,
    pub hourly_trends: Vec<HourlyTrend>,
    pub error_patterns: Vec<ErrorPattern>,
    pub performance_metrics: PerformanceMetrics,
}

/// Queue statistics
#[derive(Debug, Serialize)]
pub struct QueueStats {
    pub name: String,
    pub pending: u64,
    pub running: u64,
    pub completed_total: u64,
    pub failed_total: u64,
    pub dead_total: u64,
    pub throughput_per_minute: f64,
    pub avg_processing_time_ms: f64,
    pub error_rate: f64,
    pub oldest_pending_age_seconds: Option<u64>,
    pub priority_distribution: HashMap<String, u64>,
}

/// Hourly trend data
#[derive(Debug, Serialize)]
pub struct HourlyTrend {
    pub hour: chrono::DateTime<chrono::Utc>,
    pub completed: u64,
    pub failed: u64,
    pub throughput: f64,
    pub avg_processing_time_ms: f64,
    pub error_rate: f64,
}

/// Error pattern analysis
#[derive(Debug, Serialize)]
pub struct ErrorPattern {
    pub error_type: String,
    pub count: u64,
    pub percentage: f64,
    pub sample_message: String,
    pub first_seen: chrono::DateTime<chrono::Utc>,
    pub last_seen: chrono::DateTime<chrono::Utc>,
    pub affected_queues: Vec<String>,
}

/// Performance metrics
#[derive(Debug, Serialize)]
pub struct PerformanceMetrics {
    pub database_response_time_ms: f64,
    pub average_queue_depth: f64,
    pub jobs_per_second: f64,
    pub memory_usage_mb: Option<f64>,
    pub cpu_usage_percent: Option<f64>,
    pub active_workers: u32,
    pub worker_utilization: f64,
}

/// Time range for statistics queries
#[derive(Debug, Deserialize)]
pub struct TimeRange {
    pub start: chrono::DateTime<chrono::Utc>,
    pub end: chrono::DateTime<chrono::Utc>,
}

/// Statistics query parameters
#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    pub time_range: Option<TimeRange>,
    pub queues: Option<Vec<String>>,
    pub granularity: Option<String>, // "hour", "day", "week"
}

/// Create statistics routes
pub fn routes<T>(
    queue: Arc<T>,
    system_state: Arc<tokio::sync::RwLock<crate::api::system::SystemState>>,
) -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone
where
    T: DatabaseQueue + Send + Sync + 'static,
{
    let queue_filter = warp::any().map(move || queue.clone());
    let state_filter = warp::any().map(move || system_state.clone());

    let overview = warp::path("stats")
        .and(warp::path("overview"))
        .and(warp::path::end())
        .and(warp::get())
        .and(queue_filter.clone())
        .and(state_filter.clone())
        .and_then(overview_handler);

    let detailed = warp::path("stats")
        .and(warp::path("detailed"))
        .and(warp::path::end())
        .and(warp::get())
        .and(queue_filter.clone())
        .and(warp::query::<StatsQuery>())
        .and_then(detailed_stats_handler);

    let trends = warp::path("stats")
        .and(warp::path("trends"))
        .and(warp::path::end())
        .and(warp::get())
        .and(queue_filter.clone())
        .and(warp::query::<StatsQuery>())
        .and_then(trends_handler);

    let health = warp::path("stats")
        .and(warp::path("health"))
        .and(warp::path::end())
        .and(warp::get())
        .and(queue_filter)
        .and_then(health_handler);

    overview.or(detailed).or(trends).or(health)
}

/// Handler for system overview statistics
async fn overview_handler<T>(
    queue: Arc<T>,
    system_state: Arc<tokio::sync::RwLock<crate::api::system::SystemState>>,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    match queue.get_all_queue_stats().await {
        Ok(all_stats) => {
            let mut total_pending = 0;
            let mut total_running = 0;
            let mut total_completed = 0;
            let mut total_failed = 0;
            let mut total_dead = 0;
            let mut total_throughput = 0.0;
            let mut total_processing_time = 0.0;
            let mut queue_count = 0;

            for stats in &all_stats {
                total_pending += stats.pending_count;
                total_running += stats.running_count;
                total_completed += stats.completed_count;
                total_failed += stats.dead_count + stats.timed_out_count;
                total_dead += stats.dead_count;
                total_throughput += stats.statistics.throughput_per_minute;
                total_processing_time += stats.statistics.avg_processing_time_ms;
                queue_count += 1;
            }

            let avg_processing_time = if queue_count > 0 {
                total_processing_time / queue_count as f64
            } else {
                0.0
            };

            let total_jobs = total_pending + total_running + total_completed + total_failed;
            let overall_error_rate = if total_jobs > 0 {
                total_failed as f64 / total_jobs as f64
            } else {
                0.0
            };

            // Generate system health assessment
            let health = assess_system_health(&all_stats);

            let overview = SystemOverview {
                total_queues: queue_count,
                total_jobs,
                pending_jobs: total_pending,
                running_jobs: total_running,
                completed_jobs: total_completed,
                failed_jobs: total_failed,
                dead_jobs: total_dead,
                overall_throughput: total_throughput,
                overall_error_rate,
                avg_processing_time_ms: avg_processing_time,
                system_health: health,
                uptime_seconds: {
                    let state = system_state.read().await;
                    state.uptime_seconds() as u64
                },
                last_updated: chrono::Utc::now(),
            };

            Ok(warp::reply::json(&ApiResponse::success(overview)))
        }
        Err(e) => {
            let response = ApiResponse::<()>::error(format!("Failed to get statistics: {}", e));
            Ok(warp::reply::json(&response))
        }
    }
}

/// Handler for detailed statistics
async fn detailed_stats_handler<T>(
    queue: Arc<T>,
    query: StatsQuery,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    // For now, return basic stats. In a real implementation, this would
    // use the time_range and other query parameters to fetch historical data
    let _ = query;

    match queue.get_all_queue_stats().await {
        Ok(all_stats) => {
            // Convert hammerwork stats to our API format
            let mut queue_stats: Vec<QueueStats> = Vec::new();
            for stats in all_stats.iter() {
                // Calculate oldest pending age seconds
                let oldest_pending_age_seconds =
                    calculate_oldest_pending_age(&queue, &stats.queue_name).await;

                // Get priority distribution from priority stats
                let priority_distribution =
                    get_priority_distribution(&queue, &stats.queue_name).await;

                queue_stats.push(QueueStats {
                    name: stats.queue_name.clone(),
                    pending: stats.pending_count,
                    running: stats.running_count,
                    completed_total: stats.completed_count,
                    failed_total: stats.dead_count + stats.timed_out_count,
                    dead_total: stats.dead_count,
                    throughput_per_minute: stats.statistics.throughput_per_minute,
                    avg_processing_time_ms: stats.statistics.avg_processing_time_ms,
                    error_rate: stats.statistics.error_rate,
                    oldest_pending_age_seconds,
                    priority_distribution,
                });
            }

            // Generate realistic data based on actual statistics
            let hourly_trends = generate_hourly_trends(&queue, &all_stats).await;
            let error_patterns = generate_error_patterns(&queue, &all_stats).await;
            let performance_metrics = calculate_performance_metrics(&all_stats);

            // Generate overview from the stats
            let overview = generate_overview_from_stats(&all_stats);

            let detailed = DetailedStats {
                overview,
                queue_stats,
                hourly_trends,
                error_patterns,
                performance_metrics,
            };

            Ok(warp::reply::json(&ApiResponse::success(detailed)))
        }
        Err(e) => {
            let response =
                ApiResponse::<()>::error(format!("Failed to get detailed statistics: {}", e));
            Ok(warp::reply::json(&response))
        }
    }
}

/// Handler for trend analysis
async fn trends_handler<T>(queue: Arc<T>, query: StatsQuery) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    // For now, return mock trend data
    // In a real implementation, this would query historical data based on the time range
    let _ = (queue, query);

    let trends: Vec<HourlyTrend> = (0..24)
        .map(|hour| HourlyTrend {
            hour: chrono::Utc::now() - chrono::Duration::hours(23 - hour),
            completed: (hour * 10 + 50) as u64,
            failed: (hour / 4) as u64,
            throughput: 5.0 + (hour as f64 * 0.5),
            avg_processing_time_ms: 100.0 + (hour as f64 * 2.0),
            error_rate: 0.01 + (hour as f64 * 0.001),
        })
        .collect();

    Ok(warp::reply::json(&ApiResponse::success(trends)))
}

/// Handler for system health check
async fn health_handler<T>(queue: Arc<T>) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    match queue.get_all_queue_stats().await {
        Ok(all_stats) => {
            let health = assess_system_health(&all_stats);
            Ok(warp::reply::json(&ApiResponse::success(health)))
        }
        Err(e) => {
            let health = SystemHealth {
                status: "critical".to_string(),
                database_healthy: false,
                high_error_rate: false,
                queue_backlog: false,
                slow_processing: false,
                alerts: vec![SystemAlert {
                    severity: "critical".to_string(),
                    message: format!("Database connection failed: {}", e),
                    queue: None,
                    metric: Some("database_connectivity".to_string()),
                    value: None,
                    threshold: None,
                    timestamp: chrono::Utc::now(),
                }],
            };
            Ok(warp::reply::json(&ApiResponse::success(health)))
        }
    }
}

/// Assess overall system health based on queue statistics
fn assess_system_health(stats: &[hammerwork::stats::QueueStats]) -> SystemHealth {
    let mut alerts = Vec::new();
    let mut high_error_rate = false;
    let mut queue_backlog = false;
    let mut slow_processing = false;

    for stat in stats {
        // Check error rate
        if stat.statistics.error_rate > 0.1 {
            // > 10% error rate
            high_error_rate = true;
            alerts.push(SystemAlert {
                severity: "warning".to_string(),
                message: format!("High error rate in queue '{}'", stat.queue_name),
                queue: Some(stat.queue_name.clone()),
                metric: Some("error_rate".to_string()),
                value: Some(stat.statistics.error_rate),
                threshold: Some(0.1),
                timestamp: chrono::Utc::now(),
            });
        }

        // Check queue backlog
        if stat.pending_count > 1000 {
            queue_backlog = true;
            alerts.push(SystemAlert {
                severity: "warning".to_string(),
                message: format!("Large backlog in queue '{}'", stat.queue_name),
                queue: Some(stat.queue_name.clone()),
                metric: Some("pending_count".to_string()),
                value: Some(stat.pending_count as f64),
                threshold: Some(1000.0),
                timestamp: chrono::Utc::now(),
            });
        }

        // Check processing time
        if stat.statistics.avg_processing_time_ms > 30000.0 {
            // > 30 seconds
            slow_processing = true;
            alerts.push(SystemAlert {
                severity: "info".to_string(),
                message: format!("Slow processing in queue '{}'", stat.queue_name),
                queue: Some(stat.queue_name.clone()),
                metric: Some("avg_processing_time_ms".to_string()),
                value: Some(stat.statistics.avg_processing_time_ms),
                threshold: Some(30000.0),
                timestamp: chrono::Utc::now(),
            });
        }
    }

    let status = if alerts.iter().any(|a| a.severity == "critical") {
        "critical"
    } else if alerts.iter().any(|a| a.severity == "warning") {
        "degraded"
    } else {
        "healthy"
    };

    SystemHealth {
        status: status.to_string(),
        database_healthy: true, // If we got here, DB is accessible
        high_error_rate,
        queue_backlog,
        slow_processing,
        alerts,
    }
}

/// Generate system overview from queue statistics
fn generate_overview_from_stats(stats: &[hammerwork::stats::QueueStats]) -> SystemOverview {
    let mut total_pending = 0;
    let mut total_running = 0;
    let mut total_completed = 0;
    let mut total_failed = 0;
    let mut total_dead = 0;
    let mut total_throughput = 0.0;
    let mut total_processing_time = 0.0;
    let queue_count = stats.len();

    for stat in stats {
        total_pending += stat.pending_count;
        total_running += stat.running_count;
        total_completed += stat.completed_count;
        total_failed += stat.dead_count + stat.timed_out_count;
        total_dead += stat.dead_count;
        total_throughput += stat.statistics.throughput_per_minute;
        total_processing_time += stat.statistics.avg_processing_time_ms;
    }

    let avg_processing_time = if queue_count > 0 {
        total_processing_time / queue_count as f64
    } else {
        0.0
    };

    let total_jobs = total_pending + total_running + total_completed + total_failed;
    let overall_error_rate = if total_jobs > 0 {
        total_failed as f64 / total_jobs as f64
    } else {
        0.0
    };

    let health = assess_system_health(stats);

    SystemOverview {
        total_queues: queue_count as u32,
        total_jobs,
        pending_jobs: total_pending,
        running_jobs: total_running,
        completed_jobs: total_completed,
        failed_jobs: total_failed,
        dead_jobs: total_dead,
        overall_throughput: total_throughput,
        overall_error_rate,
        avg_processing_time_ms: avg_processing_time,
        system_health: health,
        uptime_seconds: 0,
        last_updated: chrono::Utc::now(),
    }
}

/// Calculate the oldest pending job age in seconds for a queue
async fn calculate_oldest_pending_age<T>(queue: &Arc<T>, queue_name: &str) -> Option<u64>
where
    T: DatabaseQueue + Send + Sync,
{
    // Get ready jobs (pending jobs) and find the oldest
    match queue.get_ready_jobs(queue_name, 100).await {
        Ok(jobs) => {
            let now = chrono::Utc::now();
            jobs.iter()
                .filter(|job| matches!(job.status, hammerwork::job::JobStatus::Pending))
                .map(|job| {
                    let age = now - job.created_at;
                    age.num_seconds() as u64
                })
                .max()
        }
        Err(_) => None,
    }
}

/// Get priority distribution from priority stats for a queue
async fn get_priority_distribution<T>(queue: &Arc<T>, queue_name: &str) -> HashMap<String, f32>
where
    T: DatabaseQueue + Send + Sync,
{
    match queue.get_priority_stats(queue_name).await {
        Ok(priority_stats) => priority_stats
            .priority_distribution
            .into_iter()
            .map(|(priority, percentage)| {
                let priority_name = match priority {
                    hammerwork::priority::JobPriority::Background => "background",
                    hammerwork::priority::JobPriority::Low => "low",
                    hammerwork::priority::JobPriority::Normal => "normal",
                    hammerwork::priority::JobPriority::High => "high",
                    hammerwork::priority::JobPriority::Critical => "critical",
                };
                (priority_name.to_string(), percentage)
            })
            .collect(),
        Err(_) => HashMap::new(),
    }
}

/// Generate hourly trends from queue statistics
async fn generate_hourly_trends<T>(
    queue: &Arc<T>,
    all_stats: &[hammerwork::queue::QueueStats],
) -> Vec<HourlyTrend>
where
    T: DatabaseQueue + Send + Sync,
{
    let now = chrono::Utc::now();
    let mut trends = Vec::new();

    // Generate trends for the last 24 hours using actual database queries
    for i in 0..24 {
        let hour_start = now - chrono::Duration::hours(23 - i);
        let hour_end = hour_start + chrono::Duration::hours(1);

        let mut hour_completed = 0u64;
        let mut hour_failed = 0u64;
        let mut hour_processing_times = Vec::new();

        // Get completed jobs for this specific hour across all queues
        if let Ok(completed_jobs) = queue
            .get_jobs_completed_in_range(None, hour_start, hour_end, Some(1000))
            .await
        {
            hour_completed = completed_jobs.len() as u64;

            // Collect processing times for completed jobs
            for job in completed_jobs {
                if let (Some(started_at), Some(completed_at)) = (job.started_at, job.completed_at) {
                    let processing_time = (completed_at - started_at).num_milliseconds() as f64;
                    hour_processing_times.push(processing_time);
                }
            }
        }

        // Get failed jobs for this hour using error frequencies
        // Since we don't have a direct method for failed jobs in time range,
        // we'll estimate based on error frequencies for this hour
        if let Ok(error_frequencies) = queue.get_error_frequencies(None, hour_start).await {
            // This gives us errors since hour_start, so we need to estimate for just this hour
            let total_errors_since_start = error_frequencies.values().sum::<u64>();

            // For recent hours, use a more accurate estimate
            if i < 3 {
                // For the last 3 hours, assume more recent distribution
                hour_failed = total_errors_since_start / ((i + 1) as u64).max(1);
            } else {
                // For older hours, use a smaller fraction
                hour_failed = total_errors_since_start / 24; // Rough hourly average
            }
        }

        // Calculate throughput (jobs per second for this hour)
        let hour_throughput = (hour_completed + hour_failed) as f64 / 3600.0;

        // Calculate average processing time for this hour
        let avg_processing_time_ms = if !hour_processing_times.is_empty() {
            hour_processing_times.iter().sum::<f64>() / hour_processing_times.len() as f64
        } else {
            // If no processing times available, use overall average from stats
            if !all_stats.is_empty() {
                all_stats
                    .iter()
                    .map(|s| s.statistics.avg_processing_time_ms)
                    .sum::<f64>()
                    / all_stats.len() as f64
            } else {
                0.0
            }
        };

        let error_rate = if (hour_completed + hour_failed) > 0 {
            hour_failed as f64 / (hour_completed + hour_failed) as f64
        } else {
            0.0
        };

        trends.push(HourlyTrend {
            hour: hour_start,
            completed: hour_completed,
            failed: hour_failed,
            throughput: hour_throughput,
            avg_processing_time_ms,
            error_rate,
        });
    }

    trends
}

/// Generate error patterns from queue statistics
async fn generate_error_patterns<T>(
    queue: &Arc<T>,
    all_stats: &[hammerwork::queue::QueueStats],
) -> Vec<ErrorPattern>
where
    T: DatabaseQueue + Send + Sync,
{
    let mut error_patterns = Vec::new();
    let total_errors = all_stats.iter().map(|s| s.dead_count).sum::<u64>();

    if total_errors == 0 {
        return error_patterns;
    }

    // Collect error messages from dead jobs across all queues
    let mut error_messages = Vec::new();
    for stats in all_stats {
        if let Ok(dead_jobs) = queue
            .get_dead_jobs_by_queue(&stats.queue_name, Some(20), Some(0))
            .await
        {
            for job in dead_jobs {
                if let Some(error_msg) = job.error_message {
                    error_messages.push((error_msg, job.failed_at.unwrap_or(job.created_at)));
                }
            }
        }
    }

    // Group similar error messages
    let mut error_counts = std::collections::HashMap::new();
    let mut error_first_seen = std::collections::HashMap::new();

    for (error_msg, failed_at) in error_messages {
        let error_type = extract_error_type(&error_msg);
        let count = error_counts.entry(error_type.clone()).or_insert(0);
        *count += 1;

        error_first_seen
            .entry(error_type.clone())
            .or_insert_with(|| (error_msg, failed_at));
    }

    // Convert to error patterns
    for (error_type, count) in error_counts {
        let percentage = (count as f64 / total_errors as f64) * 100.0;
        let (sample_message, first_seen) = error_first_seen.get(&error_type).unwrap();

        error_patterns.push(ErrorPattern {
            error_type,
            count,
            percentage,
            sample_message: sample_message.clone(),
            first_seen: *first_seen,
        });
    }

    // Sort by count descending
    error_patterns.sort_by(|a, b| b.count.cmp(&a.count));

    error_patterns
}

/// Calculate performance metrics from queue statistics
fn calculate_performance_metrics(
    all_stats: &[hammerwork::queue::QueueStats],
) -> PerformanceMetrics {
    let total_jobs = all_stats
        .iter()
        .map(|s| s.pending_count + s.running_count + s.completed_count + s.dead_count)
        .sum::<u64>();
    let total_throughput = all_stats
        .iter()
        .map(|s| s.statistics.throughput_per_minute)
        .sum::<f64>();
    let avg_processing_time = if !all_stats.is_empty() {
        all_stats
            .iter()
            .map(|s| s.statistics.avg_processing_time_ms)
            .sum::<f64>()
            / all_stats.len() as f64
    } else {
        0.0
    };

    let average_queue_depth = if !all_stats.is_empty() {
        all_stats
            .iter()
            .map(|s| s.pending_count as f64)
            .sum::<f64>()
            / all_stats.len() as f64
    } else {
        0.0
    };

    // Estimate database response time based on processing time
    let database_response_time_ms = if avg_processing_time > 0.0 {
        (avg_processing_time * 0.1).max(1.0).min(100.0) // Assume DB is 10% of processing time
    } else {
        2.0
    };

    PerformanceMetrics {
        database_response_time_ms,
        average_queue_depth,
        jobs_per_second: total_throughput / 60.0, // Convert from per minute to per second
        memory_usage_mb: None,                    // Would need system monitoring
        cpu_usage_percent: None,                  // Would need system monitoring
        active_workers: all_stats.iter().map(|s| s.running_count as u32).sum(),
        worker_utilization: if total_jobs > 0 {
            all_stats.iter().map(|s| s.running_count).sum::<u64>() as f64 / total_jobs as f64
        } else {
            0.0
        },
    }
}

/// Extract error type from error message for grouping
fn extract_error_type(error_msg: &str) -> String {
    // Simple error classification logic
    if error_msg.contains("timeout") || error_msg.contains("Timeout") {
        "Timeout Error".to_string()
    } else if error_msg.contains("connection") || error_msg.contains("Connection") {
        "Connection Error".to_string()
    } else if error_msg.contains("parse")
        || error_msg.contains("Parse")
        || error_msg.contains("invalid")
    {
        "Parse Error".to_string()
    } else if error_msg.contains("permission")
        || error_msg.contains("Permission")
        || error_msg.contains("forbidden")
    {
        "Permission Error".to_string()
    } else if error_msg.contains("not found") || error_msg.contains("Not Found") {
        "Not Found Error".to_string()
    } else {
        // Use first word of error message as type
        error_msg
            .split_whitespace()
            .next()
            .map(|s| format!("{} Error", s))
            .unwrap_or_else(|| "Unknown Error".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stats_query_deserialization() {
        let json = r#"{
            "time_range": {
                "start": "2024-01-01T00:00:00Z",
                "end": "2024-01-02T00:00:00Z"
            },
            "queues": ["email", "data-processing"],
            "granularity": "hour"
        }"#;

        let query: StatsQuery = serde_json::from_str(json).unwrap();
        assert!(query.time_range.is_some());
        assert_eq!(query.queues.as_ref().unwrap().len(), 2);
        assert_eq!(query.granularity, Some("hour".to_string()));
    }

    #[test]
    fn test_system_alert_serialization() {
        let alert = SystemAlert {
            severity: "warning".to_string(),
            message: "High error rate detected".to_string(),
            queue: Some("email".to_string()),
            metric: Some("error_rate".to_string()),
            value: Some(0.15),
            threshold: Some(0.1),
            timestamp: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&alert).unwrap();
        assert!(json.contains("warning"));
        assert!(json.contains("High error rate"));
    }
}
