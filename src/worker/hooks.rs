//! Job lifecycle event hooks ([`JobEventHooks`]).

use super::*;

/// Event data for job lifecycle event hooks.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct JobHookEvent {
    /// The job that triggered the event
    pub job: Job,
    /// When the event occurred
    pub timestamp: DateTime<Utc>,
    /// Processing duration (for completion events)
    pub duration: Option<Duration>,
    /// Error message (for failure events)
    pub error: Option<String>,
}

/// Type alias for job event hook handler functions.
pub type JobHookHandler = Arc<dyn Fn(JobHookEvent) + Send + Sync>;

/// Job lifecycle event hooks that can be registered with workers.
///
/// Hooks run on the worker's task. A hook that panics is contained: the panic is logged
/// at error level, the job's outcome is recorded as usual and the worker keeps polling.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct JobEventHooks {
    /// Called when a job starts processing
    pub on_job_start: Option<JobHookHandler>,
    /// Called when a job completes successfully
    pub on_job_complete: Option<JobHookHandler>,
    /// Called when a job fails (before retry logic)
    pub on_job_fail: Option<JobHookHandler>,
    /// Called when a job times out
    pub on_job_timeout: Option<JobHookHandler>,
    /// Called when a job is retried
    pub on_job_retry: Option<JobHookHandler>,
}

impl JobEventHooks {
    /// Create a new set of empty job event hooks.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the job start event handler.
    pub fn on_start<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.on_job_start = Some(Arc::new(handler));
        self
    }

    /// Set the job completion event handler.
    pub fn on_complete<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.on_job_complete = Some(Arc::new(handler));
        self
    }

    /// Set the job failure event handler.
    pub fn on_fail<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.on_job_fail = Some(Arc::new(handler));
        self
    }

    /// Set the job timeout event handler.
    pub fn on_timeout<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.on_job_timeout = Some(Arc::new(handler));
        self
    }

    /// Set the job retry event handler.
    pub fn on_retry<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.on_job_retry = Some(Arc::new(handler));
        self
    }

    /// Fire the job start event if a handler is registered.
    pub(crate) fn fire_job_start(&self, job: Job) {
        if let Some(handler) = &self.on_job_start {
            let event = JobHookEvent {
                job,
                timestamp: Utc::now(),
                duration: None,
                error: None,
            };
            run_hook("on_job_start", handler, event);
        }
    }

    /// Fire the job completion event if a handler is registered.
    pub(crate) fn fire_job_complete(&self, job: Job, duration: Duration) {
        if let Some(handler) = &self.on_job_complete {
            let event = JobHookEvent {
                job,
                timestamp: Utc::now(),
                duration: Some(duration),
                error: None,
            };
            run_hook("on_job_complete", handler, event);
        }
    }

    /// Fire the job failure event if a handler is registered.
    pub(crate) fn fire_job_fail(&self, job: Job, error: String) {
        if let Some(handler) = &self.on_job_fail {
            let event = JobHookEvent {
                job,
                timestamp: Utc::now(),
                duration: None,
                error: Some(error),
            };
            run_hook("on_job_fail", handler, event);
        }
    }

    /// Fire the job timeout event if a handler is registered.
    pub(crate) fn fire_job_timeout(&self, job: Job, duration: Duration) {
        if let Some(handler) = &self.on_job_timeout {
            let event = JobHookEvent {
                job,
                timestamp: Utc::now(),
                duration: Some(duration),
                error: Some("Job timed out".to_string()),
            };
            run_hook("on_job_timeout", handler, event);
        }
    }

    /// Fire the job retry event if a handler is registered.
    pub(crate) fn fire_job_retry(&self, job: Job, error: String) {
        if let Some(handler) = &self.on_job_retry {
            let event = JobHookEvent {
                job,
                timestamp: Utc::now(),
                duration: None,
                error: Some(error),
            };
            run_hook("on_job_retry", handler, event);
        }
    }
}

/// Run a user-supplied hook, containing a panic: it is logged at error level and the
/// worker carries on, so the job's outcome is still recorded and the worker keeps polling.
fn run_hook(name: &str, handler: &JobHookHandler, event: JobHookEvent) {
    let job_id = event.job.id;
    if let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(|| handler(event))) {
        error!(
            "Job event hook {} panicked for job {}: {}",
            name,
            job_id,
            panic_message(payload.as_ref())
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_event_hooks_default() {
        let hooks = JobEventHooks::default();
        assert!(hooks.on_job_start.is_none());
        assert!(hooks.on_job_complete.is_none());
        assert!(hooks.on_job_fail.is_none());
        assert!(hooks.on_job_timeout.is_none());
        assert!(hooks.on_job_retry.is_none());
    }

    #[test]
    fn test_job_event_hooks_new() {
        let hooks = JobEventHooks::new();
        assert!(hooks.on_job_start.is_none());
        assert!(hooks.on_job_complete.is_none());
        assert!(hooks.on_job_fail.is_none());
        assert!(hooks.on_job_timeout.is_none());
        assert!(hooks.on_job_retry.is_none());
    }

    #[test]
    fn test_job_event_hooks_builder() {
        use std::sync::{Arc, Mutex};

        let events = Arc::new(Mutex::new(Vec::new()));

        let events_start = Arc::clone(&events);
        let events_complete = Arc::clone(&events);
        let events_fail = Arc::clone(&events);
        let events_timeout = Arc::clone(&events);
        let events_retry = Arc::clone(&events);

        let hooks = JobEventHooks::new()
            .on_start(move |event: JobHookEvent| {
                events_start
                    .lock()
                    .unwrap()
                    .push(format!("start:{}", event.job.id));
            })
            .on_complete(move |event: JobHookEvent| {
                events_complete
                    .lock()
                    .unwrap()
                    .push(format!("complete:{}", event.job.id));
            })
            .on_fail(move |event: JobHookEvent| {
                events_fail
                    .lock()
                    .unwrap()
                    .push(format!("fail:{}", event.job.id));
            })
            .on_timeout(move |event: JobHookEvent| {
                events_timeout
                    .lock()
                    .unwrap()
                    .push(format!("timeout:{}", event.job.id));
            })
            .on_retry(move |event: JobHookEvent| {
                events_retry
                    .lock()
                    .unwrap()
                    .push(format!("retry:{}", event.job.id));
            });

        // Verify all hooks are set
        assert!(hooks.on_job_start.is_some());
        assert!(hooks.on_job_complete.is_some());
        assert!(hooks.on_job_fail.is_some());
        assert!(hooks.on_job_timeout.is_some());
        assert!(hooks.on_job_retry.is_some());
    }

    #[test]
    fn test_job_hook_event_creation() {
        use crate::Job;
        use serde_json::json;
        use std::time::Duration;

        let job = Job::new("test_queue".to_string(), json!({"test": "data"}))
            .with_trace_id("trace-123")
            .with_correlation_id("corr-456");

        let event = JobHookEvent {
            job: job.clone(),
            timestamp: Utc::now(),
            duration: Some(Duration::from_millis(500)),
            error: Some("Test error".to_string()),
        };

        assert_eq!(event.job.id, job.id);
        assert_eq!(event.job.queue_name, "test_queue");
        assert_eq!(event.job.trace_id, Some("trace-123".to_string()));
        assert_eq!(event.job.correlation_id, Some("corr-456".to_string()));
        assert_eq!(event.duration, Some(Duration::from_millis(500)));
        assert_eq!(event.error, Some("Test error".to_string()));
    }

    #[test]
    fn test_job_event_hooks_fire_methods() {
        use crate::Job;
        use serde_json::json;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let events = Arc::new(Mutex::new(Vec::new()));
        let job = Job::new("test_queue".to_string(), json!({"test": "data"}));

        // Test fire_job_start
        {
            let events_clone = Arc::clone(&events);
            let hooks = JobEventHooks::new().on_start(move |event: JobHookEvent| {
                events_clone
                    .lock()
                    .unwrap()
                    .push(format!("start:{}", event.job.queue_name));
            });

            hooks.fire_job_start(job.clone());
            let captured_events = events.lock().unwrap();
            assert_eq!(captured_events.len(), 1);
            assert_eq!(captured_events[0], "start:test_queue");
        }

        // Clear events for next test
        events.lock().unwrap().clear();

        // Test fire_job_complete
        {
            let events_clone = Arc::clone(&events);
            let hooks = JobEventHooks::new().on_complete(move |event: JobHookEvent| {
                events_clone.lock().unwrap().push(format!(
                    "complete:{}:{}ms",
                    event.job.queue_name,
                    event.duration.unwrap_or_default().as_millis()
                ));
            });

            hooks.fire_job_complete(job.clone(), Duration::from_millis(150));
            let captured_events = events.lock().unwrap();
            assert_eq!(captured_events.len(), 1);
            assert_eq!(captured_events[0], "complete:test_queue:150ms");
        }

        // Clear events for next test
        events.lock().unwrap().clear();

        // Test fire_job_fail
        {
            let events_clone = Arc::clone(&events);
            let hooks = JobEventHooks::new().on_fail(move |event: JobHookEvent| {
                events_clone.lock().unwrap().push(format!(
                    "fail:{}:{}",
                    event.job.queue_name,
                    event.error.unwrap_or_default()
                ));
            });

            hooks.fire_job_fail(job.clone(), "Connection timeout".to_string());
            let captured_events = events.lock().unwrap();
            assert_eq!(captured_events.len(), 1);
            assert_eq!(captured_events[0], "fail:test_queue:Connection timeout");
        }

        // Clear events for next test
        events.lock().unwrap().clear();

        // Test fire_job_timeout
        {
            let events_clone = Arc::clone(&events);
            let hooks = JobEventHooks::new().on_timeout(move |event: JobHookEvent| {
                events_clone.lock().unwrap().push(format!(
                    "timeout:{}:{}ms",
                    event.job.queue_name,
                    event.duration.unwrap_or_default().as_millis()
                ));
            });

            hooks.fire_job_timeout(job.clone(), Duration::from_secs(30));
            let captured_events = events.lock().unwrap();
            assert_eq!(captured_events.len(), 1);
            assert_eq!(captured_events[0], "timeout:test_queue:30000ms");
        }

        // Clear events for next test
        events.lock().unwrap().clear();

        // Test fire_job_retry
        {
            let events_clone = Arc::clone(&events);
            let hooks = JobEventHooks::new().on_retry(move |event: JobHookEvent| {
                events_clone.lock().unwrap().push(format!(
                    "retry:{}:{}",
                    event.job.queue_name,
                    event.error.unwrap_or_default()
                ));
            });

            hooks.fire_job_retry(job.clone(), "API rate limit exceeded".to_string());
            let captured_events = events.lock().unwrap();
            assert_eq!(captured_events.len(), 1);
            assert_eq!(
                captured_events[0],
                "retry:test_queue:API rate limit exceeded"
            );
        }
    }

    /// A panicking hook is contained: firing it neither panics nor stops the other hooks
    /// from running.
    #[test]
    fn panicking_hooks_are_contained() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let calls = Arc::new(AtomicUsize::new(0));
        let counted = |calls: &Arc<AtomicUsize>| {
            let calls = Arc::clone(calls);
            move |_event: JobHookEvent| {
                calls.fetch_add(1, Ordering::SeqCst);
                panic!("hook exploded");
            }
        };
        let hooks = JobEventHooks::new()
            .on_start(counted(&calls))
            .on_complete(counted(&calls))
            .on_fail(counted(&calls))
            .on_timeout(counted(&calls))
            .on_retry(counted(&calls));

        let job = crate::Job::new("test".to_string(), serde_json::json!({}));
        hooks.fire_job_start(job.clone());
        hooks.fire_job_complete(job.clone(), Duration::from_millis(1));
        hooks.fire_job_fail(job.clone(), "boom".to_string());
        hooks.fire_job_timeout(job.clone(), Duration::from_secs(1));
        hooks.fire_job_retry(job, "boom".to_string());

        assert_eq!(calls.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn test_job_event_hooks_clone() {
        use std::sync::{Arc, Mutex};

        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);

        let hooks = JobEventHooks::new().on_start(move |event: JobHookEvent| {
            events_clone
                .lock()
                .unwrap()
                .push(format!("cloned:{}", event.job.id));
        });

        // Clone the hooks
        let hooks_clone = hooks.clone();

        // Both original and clone should work
        let job = crate::Job::new("test".to_string(), serde_json::json!({}));
        hooks.fire_job_start(job.clone());
        hooks_clone.fire_job_start(job);

        let captured_events = events.lock().unwrap();
        assert_eq!(captured_events.len(), 2);
        assert!(captured_events[0].starts_with("cloned:"));
        assert!(captured_events[1].starts_with("cloned:"));
    }
}
