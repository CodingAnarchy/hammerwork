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
//!     database_response_time_ms: Some(5.2),
//!     average_queue_depth: 15.5,
//!     jobs_per_second: 8.3,
//!     memory_usage_mb: Some(512.0),
//!     cpu_usage_percent: Some(45.2),
//!     active_workers: None, // not tracked: there is no worker registry
//!     worker_utilization: None,
//! };
//!
//! assert_eq!(metrics.database_response_time_ms, Some(5.2));
//! assert_eq!(metrics.active_workers, None);
//! ```

use super::history::{ErrorGroup, JobHistory, MAX_BUCKETS, hour_floor};
use super::{ApiResponse, error_reply, json_reply};
use hammerwork::queue::DatabaseQueue;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use warp::http::StatusCode;
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
    pub priority_distribution: HashMap<String, f32>,
}

/// Hourly trend data
#[derive(Debug, Serialize)]
pub struct HourlyTrend {
    pub hour: chrono::DateTime<chrono::Utc>,
    pub completed: u64,
    pub failed: u64,
    pub throughput: f64,
    /// Mean run time of jobs completed in the hour; `null` when none completed.
    pub avg_processing_time_ms: Option<f64>,
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
    /// Measured time to fetch the queue statistics for this response.
    pub database_response_time_ms: Option<f64>,
    pub average_queue_depth: f64,
    pub jobs_per_second: f64,
    pub memory_usage_mb: Option<f64>,
    pub cpu_usage_percent: Option<f64>,
    /// Always `null`: Hammerwork has no worker registry, so worker counts are unknown.
    pub active_workers: Option<u32>,
    /// Always `null`, see `active_workers`.
    pub worker_utilization: Option<f64>,
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
    T: JobHistory + 'static,
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
        .and(state_filter.clone())
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

            Ok(json_reply(&ApiResponse::success(overview)))
        }
        Err(e) => Ok(error_reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get statistics: {}", e),
        )),
    }
}

/// Handler for detailed statistics
async fn detailed_stats_handler<T>(
    queue: Arc<T>,
    system_state: Arc<tokio::sync::RwLock<crate::api::system::SystemState>>,
    query: StatsQuery,
) -> Result<impl Reply, warp::Rejection>
where
    T: JobHistory,
{
    let now = chrono::Utc::now();
    let (start, end) = match resolve_range(&query, now) {
        Ok(range) => range,
        Err(message) => return Ok(error_reply(StatusCode::BAD_REQUEST, message)),
    };

    let db_started = std::time::Instant::now();
    match queue.get_all_queue_stats().await {
        Ok(all_stats) => {
            let db_ms = db_started.elapsed().as_secs_f64() * 1000.0;
            // Convert hammerwork stats to our API format
            let mut queue_stats: Vec<QueueStats> = Vec::new();
            for stats in all_stats.iter() {
                // Calculate oldest pending age seconds
                let oldest_pending_age_seconds = try_api!(
                    calculate_oldest_pending_age(&queue, &stats.queue_name).await,
                    "Failed to get oldest pending job age"
                );

                // Get priority distribution from priority stats
                let priority_distribution = try_api!(
                    get_priority_distribution(&queue, &stats.queue_name).await,
                    "Failed to get priority distribution"
                );

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

            let hourly_trends = try_api!(
                compute_trends(&*queue, start, end).await,
                "Failed to compute hourly trends"
            );
            let error_patterns = try_api!(
                generate_error_patterns(&*queue, start).await,
                "Failed to compute error patterns"
            );
            let performance_metrics = calculate_performance_metrics(&all_stats, db_ms);

            let uptime_seconds = system_state.read().await.uptime_seconds().max(0) as u64;
            let overview = generate_overview_from_stats(&all_stats, uptime_seconds);

            let detailed = DetailedStats {
                overview,
                queue_stats,
                hourly_trends,
                error_patterns,
                performance_metrics,
            };

            Ok(json_reply(&ApiResponse::success(detailed)))
        }
        Err(e) => Ok(error_reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get detailed statistics: {}", e),
        )),
    }
}

/// The `[start, end)` window of a stats request: `time_range` when given (validated),
/// otherwise the last 24 hours (23 whole hours plus the current one).
fn resolve_range(
    query: &StatsQuery,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>), String> {
    match &query.time_range {
        Some(range) => {
            if range.start >= range.end {
                Err("time_range.start must be before time_range.end".to_string())
            } else if range.end - range.start > chrono::Duration::hours(MAX_BUCKETS) {
                Err(format!("time_range may span at most {} hours", MAX_BUCKETS))
            } else {
                Ok((range.start, range.end))
            }
        }
        None => Ok((hour_floor(now) - chrono::Duration::hours(23), now)),
    }
}

/// Handler for trend analysis: hourly completed/failed counts from the database.
async fn trends_handler<T>(queue: Arc<T>, query: StatsQuery) -> Result<impl Reply, warp::Rejection>
where
    T: JobHistory,
{
    let (start, end) = match resolve_range(&query, chrono::Utc::now()) {
        Ok(range) => range,
        Err(message) => return Ok(error_reply(StatusCode::BAD_REQUEST, message)),
    };
    let trends = try_api!(
        compute_trends(&*queue, start, end).await,
        "Failed to compute hourly trends"
    );
    Ok(json_reply(&ApiResponse::success(trends)))
}

/// Handler for system health check
async fn health_handler<T>(queue: Arc<T>) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    match queue.get_all_queue_stats().await {
        Ok(all_stats) => {
            let health = assess_system_health(&all_stats);
            Ok(json_reply(&ApiResponse::success(health)))
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
            Ok(json_reply(&ApiResponse::success(health)))
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
fn generate_overview_from_stats(
    stats: &[hammerwork::stats::QueueStats],
    uptime_seconds: u64,
) -> SystemOverview {
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
        uptime_seconds,
        last_updated: chrono::Utc::now(),
    }
}

/// Calculate the oldest pending job age in seconds for a queue
async fn calculate_oldest_pending_age<T>(
    queue: &Arc<T>,
    queue_name: &str,
) -> hammerwork::Result<Option<u64>>
where
    T: DatabaseQueue + Send + Sync,
{
    // Get ready jobs (pending jobs) and find the oldest
    let jobs = queue.get_ready_jobs(queue_name, 100).await?;
    let now = chrono::Utc::now();
    Ok(jobs
        .iter()
        .filter(|job| matches!(job.status, hammerwork::job::JobStatus::Pending))
        .map(|job| age_seconds(now, job.created_at))
        .max())
}

/// Age in whole seconds between `created_at` and `now`, clamped to zero when
/// `created_at` is in the future (clock skew) instead of wrapping to ~1.8e19.
fn age_seconds(
    now: chrono::DateTime<chrono::Utc>,
    created_at: chrono::DateTime<chrono::Utc>,
) -> u64 {
    u64::try_from((now - created_at).num_seconds()).unwrap_or(0)
}

/// Get priority distribution from priority stats for a queue
async fn get_priority_distribution<T>(
    queue: &Arc<T>,
    queue_name: &str,
) -> hammerwork::Result<HashMap<String, f32>>
where
    T: DatabaseQueue + Send + Sync,
{
    let priority_stats = queue.get_priority_stats(queue_name).await?;
    Ok(priority_stats
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
        .collect())
}

/// Hourly trends for `[start, end)` computed from the database (zero-filled).
async fn compute_trends<T>(
    queue: &T,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
) -> hammerwork::Result<Vec<HourlyTrend>>
where
    T: JobHistory,
{
    let buckets = queue.hourly_activity(None, start, end).await?;
    Ok(buckets.into_iter().map(trend_from_bucket).collect())
}

fn trend_from_bucket(bucket: super::history::HourBucket) -> HourlyTrend {
    let total = bucket.completed + bucket.failed;
    HourlyTrend {
        hour: bucket.hour,
        completed: bucket.completed,
        failed: bucket.failed,
        throughput: total as f64 / 3600.0,
        avg_processing_time_ms: bucket.avg_processing_time_ms,
        error_rate: if total > 0 {
            bucket.failed as f64 / total as f64
        } else {
            0.0
        },
    }
}

/// Error patterns of failures since `since`, grouped by error type.
///
/// Built from the 500 most frequent distinct error messages, so counts and percentages
/// are exact unless the failure set has more distinct messages than that.
async fn generate_error_patterns<T>(
    queue: &T,
    since: chrono::DateTime<chrono::Utc>,
) -> hammerwork::Result<Vec<ErrorPattern>>
where
    T: JobHistory,
{
    Ok(group_error_patterns(queue.error_groups(since, 500).await?))
}

/// Folds per-(queue, message) groups into one pattern per error type with real
/// first/last-seen times and the set of affected queues.
fn group_error_patterns(groups: Vec<ErrorGroup>) -> Vec<ErrorPattern> {
    use std::collections::{BTreeMap, BTreeSet};

    struct Agg {
        count: u64,
        sample: (u64, String),
        first_seen: chrono::DateTime<chrono::Utc>,
        last_seen: chrono::DateTime<chrono::Utc>,
        queues: BTreeSet<String>,
    }

    let total: u64 = groups.iter().map(|g| g.count).sum();
    let mut by_type: BTreeMap<String, Agg> = BTreeMap::new();
    for group in groups {
        let error_type = extract_error_type(&group.message);
        let agg = by_type.entry(error_type).or_insert_with(|| Agg {
            count: 0,
            sample: (0, group.message.clone()),
            first_seen: group.first_seen,
            last_seen: group.last_seen,
            queues: BTreeSet::new(),
        });
        agg.count += group.count;
        if group.count > agg.sample.0 {
            agg.sample = (group.count, group.message.clone());
        }
        agg.first_seen = agg.first_seen.min(group.first_seen);
        agg.last_seen = agg.last_seen.max(group.last_seen);
        agg.queues.insert(group.queue_name);
    }

    let mut patterns: Vec<ErrorPattern> = by_type
        .into_iter()
        .map(|(error_type, agg)| ErrorPattern {
            error_type,
            count: agg.count,
            percentage: if total > 0 {
                agg.count as f64 / total as f64 * 100.0
            } else {
                0.0
            },
            sample_message: agg.sample.1,
            first_seen: agg.first_seen,
            last_seen: agg.last_seen,
            affected_queues: agg.queues.into_iter().collect(),
        })
        .collect();
    patterns.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.error_type.cmp(&b.error_type))
    });
    patterns
}

/// Calculate performance metrics from queue statistics.
///
/// `database_response_time_ms` is the measured time of the statistics query. Worker
/// counts and utilization are `None`: running jobs are not workers, and there is no
/// worker registry to count them from.
fn calculate_performance_metrics(
    all_stats: &[hammerwork::stats::QueueStats],
    database_response_time_ms: f64,
) -> PerformanceMetrics {
    let total_throughput = all_stats
        .iter()
        .map(|s| s.statistics.throughput_per_minute)
        .sum::<f64>();

    let average_queue_depth = if !all_stats.is_empty() {
        all_stats
            .iter()
            .map(|s| s.pending_count as f64)
            .sum::<f64>()
            / all_stats.len() as f64
    } else {
        0.0
    };

    PerformanceMetrics {
        database_response_time_ms: Some(database_response_time_ms),
        average_queue_depth,
        jobs_per_second: total_throughput / 60.0, // Convert from per minute to per second
        memory_usage_mb: None,
        cpu_usage_percent: None,
        active_workers: None,
        worker_utilization: None,
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
    use crate::api::test_support::{body_json, unreachable_queue};

    fn state() -> Arc<tokio::sync::RwLock<crate::api::system::SystemState>> {
        Arc::new(tokio::sync::RwLock::new(
            crate::api::system::SystemState::new(
                crate::DashboardConfig::default(),
                "PostgreSQL".to_string(),
                1,
            ),
        ))
    }

    fn empty_query() -> StatsQuery {
        serde_json::from_value(serde_json::json!({})).unwrap()
    }

    #[tokio::test]
    async fn test_detailed_stats_returns_500_when_database_is_down() {
        let response = detailed_stats_handler(unreachable_queue(), state(), empty_query())
            .await
            .unwrap()
            .into_response();
        let (status, body) = body_json(response).await;
        assert_eq!(status, 500);
        assert_eq!(body["success"], false);
    }

    #[tokio::test]
    async fn test_trends_returns_500_when_database_is_down() {
        let response = trends_handler(unreachable_queue(), empty_query())
            .await
            .unwrap()
            .into_response();
        let (status, body) = body_json(response).await;
        assert_eq!(status, 500);
        assert_eq!(body["success"], false);
    }

    #[tokio::test]
    async fn test_trends_rejects_invalid_time_range() {
        let inverted: StatsQuery = serde_json::from_value(serde_json::json!({
            "time_range": {"start": "2024-01-02T00:00:00Z", "end": "2024-01-01T00:00:00Z"}
        }))
        .unwrap();
        let response = trends_handler(unreachable_queue(), inverted)
            .await
            .unwrap()
            .into_response();
        assert_eq!(body_json(response).await.0, 400);

        let too_long: StatsQuery = serde_json::from_value(serde_json::json!({
            "time_range": {"start": "2020-01-01T00:00:00Z", "end": "2024-01-01T00:00:00Z"}
        }))
        .unwrap();
        let response = trends_handler(unreachable_queue(), too_long)
            .await
            .unwrap()
            .into_response();
        assert_eq!(body_json(response).await.0, 400);
    }

    #[test]
    fn test_default_range_is_24_hourly_buckets() {
        let now = chrono::Utc::now();
        let (start, end) = resolve_range(&empty_query(), now).unwrap();
        assert_eq!(end, now);
        assert_eq!(crate::api::history::hour_range(start, end).len(), 24);
    }

    #[test]
    fn test_trend_from_bucket_computes_rates_without_inventing_values() {
        let hour = hour_floor(chrono::Utc::now());
        let busy = trend_from_bucket(crate::api::history::HourBucket {
            hour,
            completed: 3,
            failed: 1,
            avg_processing_time_ms: Some(40.0),
        });
        assert_eq!(busy.error_rate, 0.25);
        assert_eq!(busy.throughput, 4.0 / 3600.0);
        let idle = trend_from_bucket(crate::api::history::HourBucket {
            hour,
            completed: 0,
            failed: 0,
            avg_processing_time_ms: None,
        });
        assert_eq!(idle.error_rate, 0.0);
        assert_eq!(idle.avg_processing_time_ms, None);
    }

    #[test]
    fn test_group_error_patterns_uses_real_times_and_queues() {
        let now = chrono::Utc::now();
        let t = |h| now - chrono::Duration::hours(h);
        let group = |queue: &str, msg: &str, count, first, last| ErrorGroup {
            queue_name: queue.to_string(),
            message: msg.to_string(),
            count,
            first_seen: t(first),
            last_seen: t(last),
        };
        let patterns = group_error_patterns(vec![
            group("emails", "connection refused by host a", 6, 10, 2),
            group("reports", "connection reset", 2, 8, 1),
            group("emails", "request timeout", 2, 5, 3),
        ]);
        assert_eq!(patterns.len(), 2);
        let conn = &patterns[0];
        assert_eq!(conn.error_type, "Connection Error");
        assert_eq!(conn.count, 8);
        assert_eq!(conn.percentage, 80.0);
        assert_eq!(conn.sample_message, "connection refused by host a");
        assert_eq!(conn.affected_queues, vec!["emails", "reports"]);
        assert_eq!(conn.first_seen, t(10));
        assert_eq!(conn.last_seen, t(1));
        assert_eq!(patterns[1].affected_queues, vec!["emails"]);
        assert!(group_error_patterns(Vec::new()).is_empty());
    }

    #[test]
    fn test_performance_metrics_report_unknowns_as_none() {
        let metrics = calculate_performance_metrics(&[], 3.5);
        assert_eq!(metrics.database_response_time_ms, Some(3.5));
        assert_eq!(metrics.active_workers, None);
        assert_eq!(metrics.worker_utilization, None);
        assert_eq!(metrics.memory_usage_mb, None);
    }

    #[test]
    fn test_age_seconds_clamps_future_timestamps_to_zero() {
        let now = chrono::Utc::now();
        assert_eq!(age_seconds(now, now + chrono::Duration::seconds(30)), 0);
        assert_eq!(age_seconds(now, now - chrono::Duration::seconds(30)), 30);
    }

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
