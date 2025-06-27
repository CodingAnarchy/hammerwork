use hammerwork::{
    Job,
    priority::{JobPriority, PriorityStats, PriorityWeights},
    stats::{InMemoryStatsCollector, JobEvent, JobEventType, StatisticsCollector},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

/// Test basic priority ordering in job creation and processing
#[tokio::test]
async fn test_job_priority_basic_functionality() {
    // Test job creation with different priorities
    let critical_job =
        Job::new("test_queue".to_string(), json!({"data": "critical"})).as_critical();
    assert_eq!(critical_job.priority, JobPriority::Critical);
    assert!(critical_job.is_critical());
    assert_eq!(critical_job.priority_value(), 4);

    let high_job = Job::new("test_queue".to_string(), json!({"data": "high"})).as_high_priority();
    assert_eq!(high_job.priority, JobPriority::High);
    assert!(high_job.is_high_priority());

    let normal_job = Job::new("test_queue".to_string(), json!({"data": "normal"}));
    assert_eq!(normal_job.priority, JobPriority::Normal);
    assert!(normal_job.is_normal_priority());

    let low_job = Job::new("test_queue".to_string(), json!({"data": "low"})).as_low_priority();
    assert_eq!(low_job.priority, JobPriority::Low);
    assert!(low_job.is_low_priority());

    let background_job =
        Job::new("test_queue".to_string(), json!({"data": "background"})).as_background();
    assert_eq!(background_job.priority, JobPriority::Background);
    assert!(background_job.is_background());

    // Test priority ordering
    assert!(critical_job.priority > high_job.priority);
    assert!(high_job.priority > normal_job.priority);
    assert!(normal_job.priority > low_job.priority);
    assert!(low_job.priority > background_job.priority);
}

/// Test priority weights configuration
#[test]
fn test_priority_weights_configuration() {
    // Test default weights
    let default_weights = PriorityWeights::new();
    assert!(!default_weights.is_strict());
    assert_eq!(default_weights.get_weight(JobPriority::Critical), 20);
    assert_eq!(default_weights.get_weight(JobPriority::High), 10);
    assert_eq!(default_weights.get_weight(JobPriority::Normal), 5);
    assert_eq!(default_weights.get_weight(JobPriority::Low), 2);
    assert_eq!(default_weights.get_weight(JobPriority::Background), 1);

    // Test strict weights
    let strict_weights = PriorityWeights::strict();
    assert!(strict_weights.is_strict());
    assert_eq!(strict_weights.get_weight(JobPriority::Critical), 4);
    assert_eq!(strict_weights.get_weight(JobPriority::Background), 0);

    // Test custom weights
    let custom_weights = PriorityWeights::new()
        .with_weight(JobPriority::Critical, 100)
        .with_weight(JobPriority::High, 50)
        .with_fairness_factor(0.3);

    assert_eq!(custom_weights.get_weight(JobPriority::Critical), 100);
    assert_eq!(custom_weights.get_weight(JobPriority::High), 50);
    assert_eq!(custom_weights.fairness_factor(), 0.3);
}

/// Test priority statistics calculation
#[test]
fn test_priority_statistics() {
    let mut stats = PriorityStats::new();

    // Add some job counts
    stats.job_counts.insert(JobPriority::Critical, 5);
    stats.job_counts.insert(JobPriority::High, 15);
    stats.job_counts.insert(JobPriority::Normal, 80);
    stats.job_counts.insert(JobPriority::Low, 10);
    stats.job_counts.insert(JobPriority::Background, 2);

    // Calculate distribution
    stats.calculate_distribution();

    // Check distribution percentages
    assert_eq!(stats.priority_distribution[&JobPriority::Normal], 71.42857); // 80/112 * 100
    assert_eq!(stats.priority_distribution[&JobPriority::High], 13.392857); // 15/112 * 100

    // Test most active priority
    assert_eq!(stats.most_active_priority(), Some(JobPriority::Normal));

    // Test starvation detection
    let starved = stats.check_starvation(5.0); // 5% threshold
    assert!(starved.contains(&JobPriority::Background)); // 2/112 = 1.78% < 5%
    assert!(!starved.contains(&JobPriority::Low)); // 10/112 = 8.92% > 5%
}

/// Test worker priority configuration (basic configuration testing)
#[test]
fn test_worker_priority_configuration() {
    // Test that priority configuration works properly
    let custom_weights = PriorityWeights::new()
        .with_weight(JobPriority::Critical, 50)
        .with_fairness_factor(0.2);

    // Test that worker would be able to use these weights
    assert_eq!(custom_weights.get_weight(JobPriority::Critical), 50);
    assert_eq!(custom_weights.fairness_factor(), 0.2);

    // Test strict mode configuration
    let strict_weights = PriorityWeights::strict();
    assert!(strict_weights.is_strict());
}

/// Test priority serialization and deserialization
#[test]
fn test_priority_serialization() {
    // Test JobPriority serialization
    let priority = JobPriority::High;
    let serialized = serde_json::to_string(&priority).unwrap();
    let deserialized: JobPriority = serde_json::from_str(&serialized).unwrap();
    assert_eq!(priority, deserialized);

    // Test PriorityWeights serialization
    let weights = PriorityWeights::new().with_weight(JobPriority::Critical, 25);
    let serialized = serde_json::to_string(&weights).unwrap();
    let deserialized: PriorityWeights = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.get_weight(JobPriority::Critical), 25);

    // Test PriorityStats serialization
    let mut stats = PriorityStats::new();
    stats.job_counts.insert(JobPriority::Normal, 50);
    let serialized = serde_json::to_string(&stats).unwrap();
    let deserialized: PriorityStats = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.job_counts[&JobPriority::Normal], 50);
}

/// Test priority string parsing
#[test]
fn test_priority_string_parsing() {
    // Test valid priority strings
    assert_eq!(
        "critical".parse::<JobPriority>().unwrap(),
        JobPriority::Critical
    );
    assert_eq!(
        "crit".parse::<JobPriority>().unwrap(),
        JobPriority::Critical
    );
    assert_eq!("c".parse::<JobPriority>().unwrap(), JobPriority::Critical);

    assert_eq!("high".parse::<JobPriority>().unwrap(), JobPriority::High);
    assert_eq!("h".parse::<JobPriority>().unwrap(), JobPriority::High);

    assert_eq!(
        "normal".parse::<JobPriority>().unwrap(),
        JobPriority::Normal
    );
    assert_eq!(
        "default".parse::<JobPriority>().unwrap(),
        JobPriority::Normal
    );
    assert_eq!("n".parse::<JobPriority>().unwrap(), JobPriority::Normal);

    assert_eq!("low".parse::<JobPriority>().unwrap(), JobPriority::Low);
    assert_eq!("l".parse::<JobPriority>().unwrap(), JobPriority::Low);

    assert_eq!(
        "background".parse::<JobPriority>().unwrap(),
        JobPriority::Background
    );
    assert_eq!(
        "bg".parse::<JobPriority>().unwrap(),
        JobPriority::Background
    );

    // Test invalid priority strings
    assert!("invalid".parse::<JobPriority>().is_err());
    assert!("".parse::<JobPriority>().is_err());
}

/// Test priority integer conversion
#[test]
fn test_priority_integer_conversion() {
    // Test valid conversions
    assert_eq!(JobPriority::from_i32(0).unwrap(), JobPriority::Background);
    assert_eq!(JobPriority::from_i32(1).unwrap(), JobPriority::Low);
    assert_eq!(JobPriority::from_i32(2).unwrap(), JobPriority::Normal);
    assert_eq!(JobPriority::from_i32(3).unwrap(), JobPriority::High);
    assert_eq!(JobPriority::from_i32(4).unwrap(), JobPriority::Critical);

    // Test invalid conversions
    assert!(JobPriority::from_i32(-1).is_err());
    assert!(JobPriority::from_i32(5).is_err());
    assert!(JobPriority::from_i32(100).is_err());

    // Test round-trip conversion
    for priority in JobPriority::all_priorities() {
        let as_i32 = priority.as_i32();
        let back_to_priority = JobPriority::from_i32(as_i32).unwrap();
        assert_eq!(priority, back_to_priority);
    }
}

/// Test job builder pattern with priorities
#[test]
fn test_job_builder_with_priorities() {
    let job = Job::new("email_queue".to_string(), json!({"to": "user@example.com"}))
        .with_max_attempts(5)
        .with_timeout(Duration::from_secs(30))
        .as_high_priority();

    assert_eq!(job.queue_name, "email_queue");
    assert_eq!(job.max_attempts, 5);
    assert_eq!(job.timeout, Some(Duration::from_secs(30)));
    assert_eq!(job.priority, JobPriority::High);
    assert!(job.is_high_priority());

    // Test delayed job with priority
    let delayed_job = Job::with_delay(
        "batch_queue".to_string(),
        json!({"batch_id": 123}),
        chrono::Duration::minutes(5),
    )
    .as_critical();

    assert_eq!(delayed_job.priority, JobPriority::Critical);
    assert!(delayed_job.scheduled_at > delayed_job.created_at);
}

/// Test priority display and descriptions
#[test]
fn test_priority_display_and_descriptions() {
    assert_eq!(JobPriority::Critical.to_string(), "critical");
    assert_eq!(JobPriority::High.to_string(), "high");
    assert_eq!(JobPriority::Normal.to_string(), "normal");
    assert_eq!(JobPriority::Low.to_string(), "low");
    assert_eq!(JobPriority::Background.to_string(), "background");

    // Test descriptions contain meaningful text
    assert!(JobPriority::Critical.description().contains("highest"));
    assert!(JobPriority::Normal.description().contains("default"));
    assert!(
        JobPriority::Background
            .description()
            .contains("no other jobs")
    );
}

/// Test priority weight edge cases
#[test]
fn test_priority_weight_edge_cases() {
    let weights = PriorityWeights::new();

    // Test total weight calculation
    let total = weights.total_weight();
    assert!(total > 0);

    // Test that all priority levels have weights
    for priority in JobPriority::all_priorities() {
        let weight = weights.get_weight(priority);
        assert!(
            weight > 0,
            "Priority {:?} should have a positive weight",
            priority
        );
    }

    // Test fairness factor bounds
    let clamped_weights = PriorityWeights::new().with_fairness_factor(2.0); // Should clamp to 1.0
    assert_eq!(clamped_weights.fairness_factor(), 1.0);

    let negative_weights = PriorityWeights::new().with_fairness_factor(-0.5); // Should clamp to 0.0
    assert_eq!(negative_weights.fairness_factor(), 0.0);
}

/// Test that all priority levels are covered
#[test]
fn test_priority_completeness() {
    let all_priorities = JobPriority::all_priorities();
    assert_eq!(all_priorities.len(), 5);

    // Ensure we have all expected priorities
    assert!(all_priorities.contains(&JobPriority::Background));
    assert!(all_priorities.contains(&JobPriority::Low));
    assert!(all_priorities.contains(&JobPriority::Normal));
    assert!(all_priorities.contains(&JobPriority::High));
    assert!(all_priorities.contains(&JobPriority::Critical));

    // Ensure they're in the correct order (lowest to highest)
    assert_eq!(all_priorities[0], JobPriority::Background);
    assert_eq!(all_priorities[4], JobPriority::Critical);
}

/// Test priority with job statistics integration
#[tokio::test]
async fn test_priority_statistics_integration() {
    let stats_collector = Arc::new(InMemoryStatsCollector::new_default());

    // Create events with different priorities
    let events = vec![
        JobEvent {
            job_id: uuid::Uuid::new_v4(),
            queue_name: "test_queue".to_string(),
            event_type: JobEventType::Completed,
            priority: JobPriority::Critical,
            processing_time_ms: Some(1000),
            error_message: None,
            timestamp: chrono::Utc::now(),
        },
        JobEvent {
            job_id: uuid::Uuid::new_v4(),
            queue_name: "test_queue".to_string(),
            event_type: JobEventType::Completed,
            priority: JobPriority::High,
            processing_time_ms: Some(2000),
            error_message: None,
            timestamp: chrono::Utc::now(),
        },
        JobEvent {
            job_id: uuid::Uuid::new_v4(),
            queue_name: "test_queue".to_string(),
            event_type: JobEventType::Completed,
            priority: JobPriority::Normal,
            processing_time_ms: Some(1500),
            error_message: None,
            timestamp: chrono::Utc::now(),
        },
    ];

    // Record events
    for event in events {
        stats_collector.record_event(event).await.unwrap();
    }

    // Get statistics
    let stats = stats_collector
        .get_queue_statistics("test_queue", Duration::from_secs(60))
        .await
        .unwrap();

    assert_eq!(stats.total_processed, 3);
    assert_eq!(stats.completed, 3);

    // Verify priority statistics are included
    assert!(stats.priority_stats.is_some());
    let priority_stats = stats.priority_stats.unwrap();

    // Check that we have job counts for each priority we used
    assert_eq!(
        priority_stats.job_counts.get(&JobPriority::Critical),
        Some(&1)
    );
    assert_eq!(priority_stats.job_counts.get(&JobPriority::High), Some(&1));
    assert_eq!(
        priority_stats.job_counts.get(&JobPriority::Normal),
        Some(&1)
    );

    // Check processing times are tracked by priority
    assert!(
        priority_stats
            .avg_processing_times
            .contains_key(&JobPriority::Critical)
    );
    assert!(
        priority_stats
            .avg_processing_times
            .contains_key(&JobPriority::High)
    );
    assert!(
        priority_stats
            .avg_processing_times
            .contains_key(&JobPriority::Normal)
    );
}
