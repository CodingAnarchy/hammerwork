use chrono::{DateTime, Timelike, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use thiserror::Error;

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum CronError {
    #[error("Invalid cron expression: {0}")]
    InvalidExpression(String),
    #[error("Invalid timezone: {0}")]
    InvalidTimezone(String),
    #[error("Cron parsing error: {0}")]
    ParseError(#[from] cron::error::Error),
}

/// The serialized form of a [`CronSchedule`]: deserializing rebuilds the schedule from it.
#[derive(Deserialize)]
struct CronScheduleData {
    expression: String,
    timezone: String,
}

impl TryFrom<CronScheduleData> for CronSchedule {
    type Error = CronError;

    fn try_from(data: CronScheduleData) -> Result<Self, Self::Error> {
        CronSchedule::with_timezone(&data.expression, &data.timezone)
    }
}

/// Represents a cron schedule with timezone support.
///
/// Deserializing parses the expression and timezone again, so a deserialized schedule
/// fires like one built with [`CronSchedule::with_timezone`]; an invalid expression or
/// timezone is a deserialization error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "CronScheduleData")]
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

    /// Check whether this schedule fires at any time during the minute of `datetime`
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

        // The schedule matches if it fires at any second of the datetime's minute: take
        // the first fire strictly after the second before that minute starts.
        let into_minute = chrono::Duration::seconds(i64::from(datetime_tz.second()))
            + chrono::Duration::nanoseconds(i64::from(datetime_tz.nanosecond()));
        let minute_start = datetime_tz - into_minute;
        let check_time = minute_start - chrono::Duration::seconds(1);
        match schedule.after(&check_time).next() {
            Some(next_time) => next_time < minute_start + chrono::Duration::minutes(1),
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
        Self::new("0 0 9 * * Mon-Fri")
    }

    pub fn every_monday_at_noon() -> Result<Self, CronError> {
        Self::new("0 0 12 * * Mon")
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

    /// #114: a deserialized schedule is rebuilt, so it fires; invalid input errors.
    #[test]
    fn test_deserialized_schedule_fires() {
        let original = CronSchedule::with_timezone("0 0 9 * * *", "America/New_York").unwrap();
        let json = serde_json::to_string(&original).unwrap();
        let restored: CronSchedule = serde_json::from_str(&json).unwrap();
        let after = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        assert!(restored.next_execution(after).is_some());
        assert_eq!(
            restored.next_execution(after),
            original.next_execution(after)
        );
        assert!(restored.matches(Utc.with_ymd_and_hms(2024, 1, 1, 14, 0, 30).unwrap()));

        assert!(
            serde_json::from_str::<CronSchedule>(r#"{"expression":"nonsense","timezone":"UTC"}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<CronSchedule>(
                r#"{"expression":"0 0 9 * * *","timezone":"Not/AZone"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn test_cron_schedule_creation() {
        let schedule = CronSchedule::new("0 0 9 * * Mon-Fri").unwrap();
        assert_eq!(schedule.expression, "0 0 9 * * Mon-Fri");
        assert_eq!(schedule.timezone, "UTC");
    }

    #[test]
    fn test_cron_schedule_with_timezone() {
        let schedule =
            CronSchedule::with_timezone("0 0 9 * * Mon-Fri", "America/New_York").unwrap();
        assert_eq!(schedule.expression, "0 0 9 * * Mon-Fri");
        assert_eq!(schedule.timezone, "America/New_York");
    }

    #[test]
    fn test_invalid_cron_expression() {
        let result = CronSchedule::new("invalid cron");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_timezone() {
        let result = CronSchedule::with_timezone("0 9 * * Mon-Fri", "Invalid/Timezone");
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
        assert_eq!(weekdays.expression, "0 0 9 * * Mon-Fri");

        let mondays = presets::mondays_at_noon();
        assert_eq!(mondays.expression, "0 0 12 * * Mon");
    }

    #[test]
    fn test_weekday_presets_fire_on_the_right_days() {
        use chrono::Weekday;
        // Sunday 2023-01-01 00:00 UTC; walk two weeks of executions.
        let start = Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();

        let weekdays = presets::weekdays_at_9am();
        let mut after = start;
        let mut days = Vec::new();
        for _ in 0..10 {
            let next = weekdays.next_execution(after).unwrap();
            assert_eq!(next.hour(), 9);
            days.push(next.weekday());
            after = next;
        }
        assert!(
            days.iter()
                .all(|d| !matches!(d, Weekday::Sat | Weekday::Sun)),
            "weekday preset fired on {days:?}"
        );
        assert_eq!(days[0], Weekday::Mon);

        let mondays = presets::mondays_at_noon();
        let next = mondays.next_execution(start).unwrap();
        assert_eq!(next.weekday(), Weekday::Mon);
        assert_eq!(next.hour(), 12);
    }

    #[test]
    fn test_matches_any_fire_time_in_the_minute() {
        let noon = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        // Fires every second, so every minute matches.
        assert!(CronSchedule::new("* * * * * *").unwrap().matches(noon));
        // Fires at :00 and :30; 11:59:30 must not hide the 12:00:00 fire.
        assert!(CronSchedule::new("0,30 * * * * *").unwrap().matches(noon));
        // Fires at :30 of every minute, later in the same minute as 12:00:10.
        let ten_past = noon + chrono::Duration::seconds(10);
        assert!(CronSchedule::new("30 * * * * *").unwrap().matches(ten_past));
        // Once a minute at :00, checked mid-minute.
        assert!(CronSchedule::new("0 * * * * *").unwrap().matches(ten_past));
        // Not in this minute.
        assert!(!CronSchedule::new("0 5 * * * *").unwrap().matches(noon));
        assert!(!CronSchedule::new("0 0 13 * * *").unwrap().matches(noon));
    }

    #[test]
    fn test_cron_validation() {
        assert!(CronSchedule::validate("0 0 9 * * Mon-Fri").is_ok());
        assert!(CronSchedule::validate("0 * * * * *").is_ok());
        assert!(CronSchedule::validate("0 0 0 * * *").is_ok());
        assert!(CronSchedule::validate("invalid").is_err());
    }

    #[test]
    fn test_serialization() {
        let schedule =
            CronSchedule::with_timezone("0 0 9 * * Mon-Fri", "America/New_York").unwrap();
        let json = serde_json::to_string(&schedule).unwrap();
        let mut deserialized: CronSchedule = serde_json::from_str(&json).unwrap();

        // Need to reinitialize after deserialization
        deserialized.reinitialize().unwrap();

        assert_eq!(deserialized.expression, schedule.expression);
        assert_eq!(deserialized.timezone, schedule.timezone);
    }

    #[test]
    fn test_from_str() {
        let schedule: CronSchedule = "0 0 9 * * Mon-Fri".parse().unwrap();
        assert_eq!(schedule.expression, "0 0 9 * * Mon-Fri");
        assert_eq!(schedule.timezone, "UTC");
    }
}
