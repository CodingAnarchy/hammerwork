use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CronError {
    #[error("Invalid cron expression: {0}")]
    InvalidExpression(String),
    #[error("Invalid timezone: {0}")]
    InvalidTimezone(String),
    #[error("Cron parsing error: {0}")]
    ParseError(#[from] cron::error::Error),
}

/// Represents a cron schedule with timezone support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronSchedule {
    pub expression: String,
    pub timezone: String,
    #[serde(skip)]
    schedule: Option<Schedule>,
    #[serde(skip)]
    tz: Option<Tz>,
}

impl CronSchedule {
    /// Create a new CronSchedule with UTC timezone
    pub fn new(expression: &str) -> Result<Self, CronError> {
        Self::with_timezone(expression, "UTC")
    }

    /// Create a new CronSchedule with a specific timezone
    pub fn with_timezone(expression: &str, timezone: &str) -> Result<Self, CronError> {
        let schedule = Schedule::from_str(expression)
            .map_err(|e| CronError::InvalidExpression(format!("{}: {}", expression, e)))?;

        let tz = timezone
            .parse::<Tz>()
            .map_err(|_| CronError::InvalidTimezone(timezone.to_string()))?;

        Ok(CronSchedule {
            expression: expression.to_string(),
            timezone: timezone.to_string(),
            schedule: Some(schedule),
            tz: Some(tz),
        })
    }

    /// Get the next execution time after the given datetime
    pub fn next_execution(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let schedule = self.schedule.as_ref()?;
        let tz = self.tz.as_ref()?;

        // Convert UTC to the cron's timezone
        let after_tz = after.with_timezone(tz);

        // Get the next execution in the cron's timezone
        let next_tz = schedule.after(&after_tz).next()?;

        // Convert back to UTC
        Some(next_tz.with_timezone(&Utc))
    }

    /// Get the next execution time from now
    pub fn next_execution_from_now(&self) -> Option<DateTime<Utc>> {
        self.next_execution(Utc::now())
    }

    /// Check if the given datetime matches this cron schedule
    pub fn matches(&self, datetime: DateTime<Utc>) -> bool {
        let schedule = match self.schedule.as_ref() {
            Some(s) => s,
            None => return false,
        };

        let tz = match self.tz.as_ref() {
            Some(t) => t,
            None => return false,
        };

        // Convert to cron's timezone
        let datetime_tz = datetime.with_timezone(tz);

        // Check if this time matches the cron schedule
        // We get the next execution from a minute before to see if it matches our time
        let check_time = datetime_tz - chrono::Duration::minutes(1);
        let next_exec = schedule.after(&check_time).next();
        match next_exec {
            Some(next_time) => {
                // Check if the next execution time is within the same minute
                next_time.timestamp() / 60 == datetime_tz.timestamp() / 60
            }
            None => false,
        }
    }

    /// Validate that the cron expression is valid
    pub fn validate(expression: &str) -> Result<(), CronError> {
        Schedule::from_str(expression)
            .map_err(|e| CronError::InvalidExpression(format!("{}: {}", expression, e)))?;
        Ok(())
    }

    /// Get common cron expressions
    pub fn every_minute() -> Result<Self, CronError> {
        Self::new("0 * * * * *")
    }

    pub fn every_hour() -> Result<Self, CronError> {
        Self::new("0 0 * * * *")
    }

    pub fn every_day_at_midnight() -> Result<Self, CronError> {
        Self::new("0 0 0 * * *")
    }

    pub fn every_weekday_at_9am() -> Result<Self, CronError> {
        Self::new("0 0 9 * * 1-5")
    }

    pub fn every_monday_at_noon() -> Result<Self, CronError> {
        Self::new("0 0 12 * * 1")
    }

    /// Reinitialize the schedule and timezone after deserialization
    pub fn reinitialize(&mut self) -> Result<(), CronError> {
        self.schedule =
            Some(Schedule::from_str(&self.expression).map_err(|e| {
                CronError::InvalidExpression(format!("{}: {}", self.expression, e))
            })?);

        self.tz = Some(
            self.timezone
                .parse::<Tz>()
                .map_err(|_| CronError::InvalidTimezone(self.timezone.clone()))?,
        );

        Ok(())
    }
}

impl FromStr for CronSchedule {
    type Err = CronError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

/// Helper function to create common cron schedules
pub mod presets {
    use super::CronSchedule;

    pub fn every_minute() -> CronSchedule {
        CronSchedule::every_minute().unwrap()
    }

    pub fn every_hour() -> CronSchedule {
        CronSchedule::every_hour().unwrap()
    }

    pub fn daily_at_midnight() -> CronSchedule {
        CronSchedule::every_day_at_midnight().unwrap()
    }

    pub fn weekdays_at_9am() -> CronSchedule {
        CronSchedule::every_weekday_at_9am().unwrap()
    }

    pub fn mondays_at_noon() -> CronSchedule {
        CronSchedule::every_monday_at_noon().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone, Timelike};

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
    fn test_invalid_timezone() {
        let result = CronSchedule::with_timezone("0 9 * * 1-5", "Invalid/Timezone");
        assert!(result.is_err());
    }

    #[test]
    fn test_next_execution() {
        let schedule = CronSchedule::new("0 0 9 * * *").unwrap(); // Every day at 9 AM
        let now = Utc.with_ymd_and_hms(2023, 1, 1, 8, 0, 0).unwrap();
        let next = schedule.next_execution(now).unwrap();

        // Should be 9 AM on the same day
        assert_eq!(next.hour(), 9);
        assert_eq!(next.minute(), 0);
        assert_eq!(next.day(), 1);
    }

    #[test]
    fn test_next_execution_with_timezone() {
        let schedule = CronSchedule::with_timezone("0 0 9 * * *", "America/New_York").unwrap();
        let now = Utc.with_ymd_and_hms(2023, 1, 1, 12, 0, 0).unwrap(); // 12 PM UTC
        let next = schedule.next_execution(now);
        assert!(next.is_some());
    }

    #[test]
    fn test_presets() {
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
    fn test_cron_validation() {
        assert!(CronSchedule::validate("0 0 9 * * 1-5").is_ok());
        assert!(CronSchedule::validate("0 * * * * *").is_ok());
        assert!(CronSchedule::validate("0 0 0 * * *").is_ok());
        assert!(CronSchedule::validate("invalid").is_err());
    }

    #[test]
    fn test_serialization() {
        let schedule = CronSchedule::with_timezone("0 0 9 * * 1-5", "America/New_York").unwrap();
        let json = serde_json::to_string(&schedule).unwrap();
        let mut deserialized: CronSchedule = serde_json::from_str(&json).unwrap();

        // Need to reinitialize after deserialization
        deserialized.reinitialize().unwrap();

        assert_eq!(deserialized.expression, schedule.expression);
        assert_eq!(deserialized.timezone, schedule.timezone);
    }

    #[test]
    fn test_from_str() {
        let schedule: CronSchedule = "0 0 9 * * 1-5".parse().unwrap();
        assert_eq!(schedule.expression, "0 0 9 * * 1-5");
        assert_eq!(schedule.timezone, "UTC");
    }
}
