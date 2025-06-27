use hammerwork::{
    cron::{CronSchedule, presets},
    job::Job,
};
use serde_json::json;

#[test]
fn test_cron_schedule_creation() {
    let schedule = CronSchedule::new("0 0 9 * * 1-5").unwrap();
    assert_eq!(schedule.expression, "0 0 9 * * 1-5");
    assert_eq!(schedule.timezone, "UTC");
}

#[test]
fn test_cron_schedule_with_timezone() {
    let schedule = CronSchedule::with_timezone("0 0 9 * * 1-5", "America/New_York").unwrap();
    assert_eq!(schedule.expression, "0 0 9 * * 1-5");
    assert_eq!(schedule.timezone, "America/New_York");
}

#[test]
fn test_invalid_cron_expression() {
    let result = CronSchedule::new("invalid cron");
    assert!(result.is_err());
}

#[test]
fn test_cron_presets() {
    let every_minute = presets::every_minute();
    assert_eq!(every_minute.expression, "0 * * * * *");

    let every_hour = presets::every_hour();
    assert_eq!(every_hour.expression, "0 0 * * * *");

    let daily = presets::daily_at_midnight();
    assert_eq!(daily.expression, "0 0 0 * * *");

    let weekdays = presets::weekdays_at_9am();
    assert_eq!(weekdays.expression, "0 0 9 * * 1-5");

    let mondays = presets::mondays_at_noon();
    assert_eq!(mondays.expression, "0 0 12 * * 1");
}

#[test]
fn test_job_with_cron() {
    let schedule = CronSchedule::new("0 0 9 * * 1-5").unwrap();
    let job = Job::new("test".to_string(), json!({"test": "data"}))
        .with_cron(schedule)
        .unwrap();

    assert!(job.is_recurring());
    assert!(job.has_cron_schedule());
    assert_eq!(job.cron_schedule, Some("0 0 9 * * 1-5".to_string()));
    assert_eq!(job.timezone, Some("UTC".to_string()));
}

#[test]
fn test_job_with_cron_schedule_constructor() {
    let schedule = CronSchedule::new("0 0 9 * * 1-5").unwrap();
    let job =
        Job::with_cron_schedule("test".to_string(), json!({"test": "data"}), schedule).unwrap();

    assert!(job.is_recurring());
    assert!(job.has_cron_schedule());
    assert_eq!(job.cron_schedule, Some("0 0 9 * * 1-5".to_string()));
    assert!(job.next_run_at.is_some());
}

#[test]
fn test_job_next_run_calculation() {
    let schedule = CronSchedule::new("0 0 9 * * 1-5").unwrap(); // Weekdays at 9 AM
    let mut job =
        Job::with_cron_schedule("test".to_string(), json!({"test": "data"}), schedule).unwrap();

    // Test that we can calculate next run
    let next_run = job.calculate_next_run();
    assert!(next_run.is_some());

    // Test prepare_for_next_run
    let next_time = job.prepare_for_next_run();
    assert!(next_time.is_some());
    assert_eq!(job.attempts, 0);
    assert!(job.started_at.is_none());
    assert!(job.completed_at.is_none());
}

#[test]
fn test_cron_validation() {
    assert!(CronSchedule::validate("0 0 9 * * 1-5").is_ok());
    assert!(CronSchedule::validate("0 * * * * *").is_ok());
    assert!(CronSchedule::validate("0 0 0 * * *").is_ok());
    assert!(CronSchedule::validate("invalid").is_err());
}

#[test]
fn test_cron_serialization() {
    let schedule = CronSchedule::with_timezone("0 0 9 * * 1-5", "America/New_York").unwrap();
    let json = serde_json::to_string(&schedule).unwrap();
    let mut deserialized: CronSchedule = serde_json::from_str(&json).unwrap();

    // Need to reinitialize after deserialization
    deserialized.reinitialize().unwrap();

    assert_eq!(deserialized.expression, schedule.expression);
    assert_eq!(deserialized.timezone, schedule.timezone);
}

#[test]
fn test_job_cron_fields() {
    // Test a regular job (non-recurring)
    let regular_job = Job::new("test".to_string(), json!({"test": "data"}));
    assert!(!regular_job.is_recurring());
    assert!(!regular_job.has_cron_schedule());
    assert!(regular_job.cron_schedule.is_none());
    assert!(regular_job.next_run_at.is_none());
    assert!(regular_job.timezone.is_none());

    // Test job marked as recurring
    let recurring_job = Job::new("test".to_string(), json!({"test": "data"})).as_recurring();
    assert!(recurring_job.is_recurring());
    assert!(!recurring_job.has_cron_schedule()); // No cron schedule set

    // Test job with timezone
    let tz_job = Job::new("test".to_string(), json!({"test": "data"}))
        .with_timezone("America/New_York".to_string());
    assert_eq!(tz_job.timezone, Some("America/New_York".to_string()));
}
