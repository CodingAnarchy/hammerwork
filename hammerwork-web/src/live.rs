//! Live updates for connected dashboard clients.
//!
//! The dashboard normally runs as its own process (the `hammerwork-web` binary), apart
//! from the workers. Job lifecycle events published to a worker's in-process
//! [`EventManager`](hammerwork::events::EventManager) never reach it, so the dashboard
//! learns about changes from the database they all share: [`LiveUpdates`] polls it on an
//! interval and pushes what changed to the WebSocket clients, honouring each client's
//! subscriptions:
//!
//! - a `JobUpdate` message (event type `job_updates`) for each job that was created,
//!   started, completed, failed or timed out since the previous poll, or that went from
//!   `Running` back to `Pending` (retried, requeued by the stale job reaper, or released
//!   by a batch-mode worker). Those transitions leave no timestamp behind, so the poller
//!   tracks the running jobs (up to the per-poll limit) and reads the state of the ones
//!   that stopped running;
//! - a `QueueUpdate` message (event type `queue_updates`) for each queue whose statistics
//!   changed.
//!
//! The work is bounded: nothing is queried while no client is connected, one poll reads
//! at most [`WebSocketConfig::live_update_max_jobs`](crate::config::WebSocketConfig)
//! changed jobs, and the memory of what was already sent only covers a short window.
//!
//! An application that embeds the dashboard in the same process as its workers can also
//! forward their events directly with [`forward_job_events`], which delivers them without
//! waiting for the next poll.

use crate::api::history::JobHistory;
use crate::websocket::{JobUpdate, QueueStats, ServerMessage, WebSocketState};
use chrono::{DateTime, Utc};
use hammerwork::events::{EventSubscription, JobLifecycleEvent, JobLifecycleEventType};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

/// How far back each poll looks before the previous one, so a change whose transaction
/// committed after that poll ran (with a timestamp from before it) is still seen.
pub const POLL_OVERLAP: chrono::Duration = chrono::Duration::seconds(5);

/// What a job looked like when it was last pushed: its status, attempts and change time.
type SentState = (String, i32, DateTime<Utc>);

/// The queue statistics last pushed for a queue: pending, running, completed, failed,
/// dead and timed out counts.
type QueueCounts = [u64; 6];

/// Polls the database for job and queue changes and pushes them to WebSocket clients.
pub struct LiveUpdates<Q> {
    queue: Arc<Q>,
    websocket: Arc<RwLock<WebSocketState>>,
    max_jobs: u32,
    /// Database time of the previous poll; `None` before the first poll and while no
    /// client is connected.
    last_poll: Option<DateTime<Utc>>,
    /// Jobs pushed within the overlap window, so the overlap does not push them twice.
    sent: HashMap<String, SentState>,
    /// Queue statistics last pushed, per queue.
    queues: HashMap<String, QueueCounts>,
    /// The jobs `Running` at the previous poll, so a job that goes
    /// back to `Pending` (which changes none of the timestamps the change feed reads) is
    /// still reported.
    running: HashSet<String>,
}

/// What one poll pushed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PollOutcome {
    /// `JobUpdate` messages pushed.
    pub jobs: usize,
    /// `QueueUpdate` messages pushed.
    pub queues: usize,
}

impl<Q: JobHistory + 'static> LiveUpdates<Q> {
    /// Live updates from `queue` to the clients of `websocket`, reading at most `max_jobs`
    /// changed jobs per poll.
    pub fn new(queue: Arc<Q>, websocket: Arc<RwLock<WebSocketState>>, max_jobs: u32) -> Self {
        Self {
            queue,
            websocket,
            max_jobs: max_jobs.max(1),
            last_poll: None,
            sent: HashMap::new(),
            queues: HashMap::new(),
            running: HashSet::new(),
        }
    }

    /// Poll every `interval` in a background task. A failed poll is logged and retried at
    /// the next interval.
    pub fn spawn(mut self, interval: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if let Err(e) = self.poll_once().await {
                    warn!(error = %e, "Dashboard live update poll failed");
                }
            }
        })
    }

    /// Push the changes since the previous poll. Does nothing (and forgets what was sent)
    /// while no client is connected.
    pub async fn poll_once(&mut self) -> hammerwork::Result<PollOutcome> {
        if self.websocket.read().await.connection_count() == 0 {
            self.last_poll = None;
            self.sent.clear();
            self.queues.clear();
            self.running.clear();
            return Ok(PollOutcome::default());
        }

        let now = self.queue.database_now().await?;
        let since = self.last_poll.unwrap_or(now) - POLL_OVERLAP;
        let mut changes = self.queue.recent_job_changes(since, self.max_jobs).await?;
        let running = self.queue.running_jobs(self.max_jobs).await?;
        // Read the state of at most `max_jobs` of the jobs that left the running set per
        // poll; the others stay tracked for the next polls.
        let mut left = self.left_running(&changes, &running);
        let unchecked = left.split_off(left.len().min(self.max_jobs as usize));
        let states = if left.is_empty() {
            Vec::new()
        } else {
            self.queue.job_states(&left).await?
        };
        let (still_running, stopped) = Self::stopped_running(&left, states, now);
        self.running = running
            .iter()
            .chain(&changes)
            .filter(|job| job.status == "Running")
            .map(|job| job.id.clone())
            .chain(still_running)
            .chain(unchecked)
            .collect();
        changes.extend(stopped);
        let stats = self.queue.get_all_queue_stats().await?;
        self.last_poll = Some(now);

        let jobs = self.fresh_changes(changes, since);
        let queues = self.changed_queues(stats);
        let outcome = PollOutcome {
            jobs: jobs.len(),
            queues: queues.len(),
        };

        let websocket = self.websocket.read().await;
        for job in jobs {
            websocket
                .broadcast_to_subscribed(ServerMessage::JobUpdate { job }, "job_updates")
                .await
                .map_err(|e| hammerwork::HammerworkError::Processing(e.to_string()))?;
        }
        for (queue_name, stats) in queues {
            websocket
                .broadcast_to_subscribed(
                    ServerMessage::QueueUpdate { queue_name, stats },
                    "queue_updates",
                )
                .await
                .map_err(|e| hammerwork::HammerworkError::Processing(e.to_string()))?;
        }
        if outcome != PollOutcome::default() {
            debug!(?outcome, "Pushed dashboard live updates");
        }
        Ok(outcome)
    }

    /// The jobs tracked as `Running` that `running` (the running jobs now, up to the
    /// per-poll limit) does not show and whose change is not in `changes`, sorted: they
    /// stopped running without a timestamp the change feed reads, which is how a job
    /// goes back to `Pending`, or they only fell out of the `running` window.
    fn left_running(&self, changes: &[JobUpdate], running: &[JobUpdate]) -> Vec<String> {
        let seen: HashSet<&str> = changes
            .iter()
            .chain(running)
            .map(|job| job.id.as_str())
            .collect();
        let mut left: Vec<String> = self
            .running
            .iter()
            .filter(|id| !seen.contains(id.as_str()))
            .cloned()
            .collect();
        left.sort_unstable();
        left
    }

    /// Splits `states`, the current state of the jobs `left` (see
    /// [`left_running`](Self::left_running)), into the ids of those still `Running` (they
    /// only fell out of the window) and the changes of those that stopped, reported at
    /// `now`, when the change was seen: the timestamps they carry predate it. Jobs
    /// missing from `states` no longer exist.
    fn stopped_running(
        left: &[String],
        states: Vec<JobUpdate>,
        now: DateTime<Utc>,
    ) -> (Vec<String>, Vec<JobUpdate>) {
        let left: HashSet<&str> = left.iter().map(String::as_str).collect();
        let (running, stopped): (Vec<JobUpdate>, Vec<JobUpdate>) = states
            .into_iter()
            .filter(|job| left.contains(job.id.as_str()))
            .partition(|job| job.status == "Running");
        let stopped = stopped
            .into_iter()
            .map(|mut job| {
                job.updated_at = now;
                job
            })
            .collect();
        (running.into_iter().map(|job| job.id).collect(), stopped)
    }

    /// The changes not pushed yet, oldest first; remembers them and forgets the jobs
    /// pushed before `since`.
    fn fresh_changes(&mut self, changes: Vec<JobUpdate>, since: DateTime<Utc>) -> Vec<JobUpdate> {
        self.sent
            .retain(|_, (_, _, changed_at)| *changed_at > since);
        let mut fresh: Vec<JobUpdate> = changes
            .into_iter()
            .filter(|job| {
                let state = (job.status.clone(), job.attempts, job.updated_at);
                self.sent.insert(job.id.clone(), state.clone()) != Some(state)
            })
            .collect();
        fresh.sort_by_key(|job| job.updated_at);
        fresh
    }

    /// The queues whose statistics changed since they were last pushed.
    fn changed_queues(
        &mut self,
        stats: Vec<hammerwork::stats::QueueStats>,
    ) -> Vec<(String, QueueStats)> {
        let present: HashSet<&str> = stats.iter().map(|s| s.queue_name.as_str()).collect();
        self.queues
            .retain(|name, _| present.contains(name.as_str()));
        let mut changed = Vec::new();
        for queue in stats {
            let counts = [
                queue.pending_count,
                queue.running_count,
                queue.completed_count,
                queue.statistics.failed,
                queue.dead_count,
                queue.timed_out_count,
            ];
            if self.queues.insert(queue.queue_name.clone(), counts) == Some(counts) {
                continue;
            }
            let failed_count = crate::api::failed_job_count(&queue);
            changed.push((
                queue.queue_name,
                QueueStats {
                    pending_count: queue.pending_count,
                    running_count: queue.running_count,
                    completed_count: queue.completed_count,
                    failed_count,
                    dead_count: queue.dead_count,
                    throughput_per_minute: queue.statistics.throughput_per_minute,
                    avg_processing_time_ms: queue.statistics.avg_processing_time_ms,
                    error_rate: queue.statistics.error_rate,
                    updated_at: Utc::now(),
                },
            ));
        }
        changed
    }
}

/// Forward the job lifecycle events of an in-process
/// [`EventManager`](hammerwork::events::EventManager) subscription to the WebSocket
/// clients subscribed to `job_updates`, until the event manager is dropped.
///
/// Only useful when the dashboard runs in the same process as the workers that publish
/// the events; the standalone dashboard relies on [`LiveUpdates`]. Events the
/// subscription's filter rejects are skipped, and a subscriber that falls behind skips the
/// events it missed.
pub fn forward_job_events(
    websocket: Arc<RwLock<WebSocketState>>,
    mut subscription: EventSubscription,
) -> JoinHandle<()> {
    use tokio::sync::broadcast::error::RecvError;
    tokio::spawn(async move {
        loop {
            match subscription.receiver.recv().await {
                Ok(event) if subscription.filter.matches(&event) => {
                    let message = ServerMessage::JobUpdate {
                        job: job_update_from_event(&event),
                    };
                    let state = websocket.read().await;
                    if let Err(e) = state.broadcast_to_subscribed(message, "job_updates").await {
                        warn!(error = %e, "Failed to forward a job event to dashboard clients");
                    }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(missed)) => {
                    debug!(missed, "Dashboard event forwarding fell behind");
                }
                Err(RecvError::Closed) => break,
            }
        }
    })
}

/// The `JobUpdate` sent for a job lifecycle event.
pub fn job_update_from_event(event: &JobLifecycleEvent) -> JobUpdate {
    use JobLifecycleEventType as E;
    let status = match event.event_type {
        E::Enqueued | E::Retried | E::Restored => "Pending",
        E::Started => "Running",
        E::Completed => "Completed",
        E::Failed => "Failed",
        E::Dead => "Dead",
        E::TimedOut => "TimedOut",
        E::Cancelled => "Cancelled",
        E::Archived => "Archived",
    };
    let attempts = event
        .error
        .as_ref()
        .and_then(|error| error.retry_attempt)
        .and_then(|attempt| i32::try_from(attempt).ok())
        .unwrap_or_default();
    JobUpdate {
        id: event.job_id.to_string(),
        queue_name: event.queue_name.clone(),
        status: status.to_string(),
        priority: event.priority.to_string(),
        attempts,
        updated_at: event.timestamp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WebSocketConfig;
    use hammerwork::events::{EventFilter, EventManager, JobLifecycleEventBuilder};
    use hammerwork::priority::JobPriority;
    use uuid::Uuid;
    use warp::Filter;

    type Shared = Arc<RwLock<WebSocketState>>;

    fn new_state() -> Shared {
        Arc::new(RwLock::new(WebSocketState::new(WebSocketConfig::default())))
    }

    async fn connect(state: &Shared) -> warp::test::WsClient {
        let state = state.clone();
        let route = warp::path("ws")
            .and(warp::ws())
            .map(move |ws: warp::ws::Ws| {
                let state = state.clone();
                ws.on_upgrade(move |socket| async move {
                    let _ = WebSocketState::serve_connection(state, socket).await;
                })
            });
        warp::test::ws()
            .path("/ws")
            .handshake(route)
            .await
            .expect("handshake")
    }

    async fn wait_for_connections(state: &Shared, expected: usize) {
        for _ in 0..200 {
            if state.read().await.connection_count() == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("expected {expected} connections");
    }

    /// The next text message as JSON, or `None` if nothing arrives soon.
    async fn next_json(client: &mut warp::test::WsClient) -> Option<serde_json::Value> {
        match tokio::time::timeout(Duration::from_millis(500), client.recv()).await {
            Ok(Ok(message)) if message.is_text() => {
                Some(serde_json::from_str(message.to_str().unwrap()).unwrap())
            }
            _ => None,
        }
    }

    /// Subscribes `client` to `event_types` and waits for the server to apply it.
    async fn subscribe(client: &mut warp::test::WsClient, event_types: &[&str]) {
        let message = serde_json::json!({"type": "Subscribe", "event_types": event_types});
        client.send_text(message.to_string()).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    #[test]
    fn job_events_map_to_dashboard_statuses() {
        let id = Uuid::new_v4();
        let event = JobLifecycleEvent::failed(
            id,
            "emails".to_string(),
            JobPriority::High,
            hammerwork::events::JobError {
                message: "boom".to_string(),
                error_type: None,
                details: None,
                retry_attempt: Some(2),
            },
        );
        let update = job_update_from_event(&event);
        assert_eq!(update.id, id.to_string());
        assert_eq!(update.queue_name, "emails");
        assert_eq!(update.status, "Failed");
        assert_eq!(update.priority, "high");
        assert_eq!(update.attempts, 2);
        assert_eq!(update.updated_at, event.timestamp);

        let completed =
            JobLifecycleEvent::completed(id, "emails".to_string(), JobPriority::Normal, 12);
        assert_eq!(job_update_from_event(&completed).status, "Completed");
        let started = JobLifecycleEvent::started(id, "emails".to_string(), JobPriority::Normal);
        assert_eq!(job_update_from_event(&started).status, "Running");
    }

    /// #71: in-process job events reach the clients subscribed to `job_updates`, and only
    /// those.
    #[tokio::test]
    async fn in_process_job_events_reach_subscribed_clients() {
        let state = new_state();
        let events = EventManager::new_default();
        let subscription = events.subscribe(EventFilter::new()).await.unwrap();
        let forwarder = forward_job_events(state.clone(), subscription);

        let mut jobs = connect(&state).await;
        let mut archive_only = connect(&state).await;
        wait_for_connections(&state, 2).await;
        subscribe(&mut jobs, &["job_updates"]).await;
        subscribe(&mut archive_only, &["archive_events"]).await;

        let id = Uuid::new_v4();
        events
            .publish_event(JobLifecycleEvent::completed(
                id,
                "emails".to_string(),
                JobPriority::Normal,
                5,
            ))
            .await
            .unwrap();

        let message = next_json(&mut jobs).await.expect("job update delivered");
        assert_eq!(message["type"], "JobUpdate");
        assert_eq!(message["job"]["id"], id.to_string());
        assert_eq!(message["job"]["status"], "Completed");
        assert!(next_json(&mut archive_only).await.is_none());

        // The forwarder ends with the event manager.
        drop(events);
        tokio::time::timeout(Duration::from_secs(2), forwarder)
            .await
            .expect("forwarder stopped")
            .unwrap();
    }

    fn change(id: &str, status: &str, at: DateTime<Utc>) -> JobUpdate {
        JobUpdate {
            id: id.to_string(),
            queue_name: "q".to_string(),
            status: status.to_string(),
            priority: "normal".to_string(),
            attempts: 0,
            updated_at: at,
        }
    }

    /// The overlap between polls does not push a change twice, a new state of the same
    /// job is pushed, and the memory of sent changes only covers the overlap. (Async only
    /// because the unreachable queue's lazy pool needs a runtime.)
    #[tokio::test]
    async fn overlapping_polls_push_each_change_once() {
        let mut live = LiveUpdates::new(
            crate::api::test_support::unreachable_queue(),
            new_state(),
            10,
        );
        let t0 = Utc::now();
        let first = live.fresh_changes(
            vec![
                change("b", "Running", t0),
                change("a", "Pending", t0 - POLL_OVERLAP / 2),
            ],
            t0 - POLL_OVERLAP,
        );
        let ids: Vec<&str> = first.iter().map(|j| j.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"], "oldest first");

        let t1 = t0 + chrono::Duration::seconds(1);
        let second = live.fresh_changes(
            vec![
                change("a", "Running", t1),
                change("b", "Running", t0),
                change("c", "Pending", t1),
            ],
            t1 - POLL_OVERLAP,
        );
        let pushed: Vec<(&str, &str)> = second
            .iter()
            .map(|j| (j.id.as_str(), j.status.as_str()))
            .collect();
        assert_eq!(pushed, [("a", "Running"), ("c", "Pending")]);

        // Changes older than the overlap are forgotten.
        live.fresh_changes(Vec::new(), t1 + chrono::Duration::hours(1));
        assert!(live.sent.is_empty());
    }

    #[tokio::test]
    async fn queue_statistics_are_pushed_when_they_change() {
        let mut live = LiveUpdates::new(
            crate::api::test_support::unreachable_queue(),
            new_state(),
            10,
        );
        let stats = |name: &str, pending: u64| hammerwork::stats::QueueStats {
            queue_name: name.to_string(),
            pending_count: pending,
            running_count: 0,
            dead_count: 0,
            timed_out_count: 0,
            completed_count: 0,
            statistics: Default::default(),
        };
        let first = live.changed_queues(vec![stats("a", 1), stats("b", 0)]);
        assert_eq!(first.len(), 2);
        let second = live.changed_queues(vec![stats("a", 1), stats("b", 3)]);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].0, "b");
        assert_eq!(second[0].1.pending_count, 3);
        // A queue that disappears is forgotten.
        live.changed_queues(vec![stats("a", 1)]);
        assert_eq!(live.queues.len(), 1);

        // A change in the `Failed` jobs alone is pushed, and `failed_count` counts every
        // failed job (`Failed`, `Dead` and `TimedOut`), as the REST API does.
        let mut failing = stats("a", 1);
        failing.statistics.failed = 2;
        failing.dead_count = 3;
        failing.timed_out_count = 4;
        live.changed_queues(vec![failing.clone()]);
        let mut more_failed = failing.clone();
        more_failed.statistics.failed = 5;
        let changed = live.changed_queues(vec![more_failed]);
        assert_eq!(changed.len(), 1, "a change in Failed jobs is pushed");
        assert_eq!(changed[0].1.failed_count, 5 + 3 + 4);
        assert_eq!(changed[0].1.dead_count, 3);
    }

    /// Without connected clients the poller does not touch the database (the queue here
    /// is unreachable, so a query would fail).
    #[tokio::test]
    async fn without_clients_nothing_is_polled() {
        let mut live = LiveUpdates::new(
            crate::api::test_support::unreachable_queue(),
            new_state(),
            10,
        );
        assert_eq!(live.poll_once().await.unwrap(), PollOutcome::default());
    }

    /// #71: a client subscribed to job updates is told when a job changes state in the
    /// database, whoever changed it; a client subscribed to other events is not.
    async fn clients_receive_job_state_changes<Q: JobHistory + 'static>(queue: Arc<Q>) {
        use hammerwork::Job;
        let state = new_state();
        let mut live = LiveUpdates::new(queue.clone(), state.clone(), 100);
        let mut jobs = connect(&state).await;
        let mut alerts_only = connect(&state).await;
        wait_for_connections(&state, 2).await;
        subscribe(&mut jobs, &["job_updates"]).await;
        subscribe(&mut alerts_only, &["system_alerts"]).await;
        live.poll_once().await.unwrap();
        // Drain what the first poll pushed (other tests' recent jobs, queue statistics).
        while next_json(&mut jobs).await.is_some() {}

        let queue_name = format!("live_{}", Uuid::new_v4().simple());
        let id = queue
            .enqueue(Job::new(queue_name.clone(), serde_json::json!({})))
            .await
            .unwrap();
        let pending = wait_for(&mut live, &mut jobs, id, "Pending").await;
        assert_eq!(pending["job"]["queue_name"], queue_name.as_str());

        let run = queue.dequeue_one(&queue_name).await.unwrap().expect("job");
        queue
            .finish_job_run(&run, hammerwork::queue::JobOutcome::Completed)
            .await
            .unwrap();
        wait_for(&mut live, &mut jobs, id, "Completed").await;
        assert!(
            next_json(&mut alerts_only).await.is_none(),
            "not subscribed to job updates"
        );

        queue.delete_job(id).await.unwrap();
    }

    /// #94: a job that goes from `Running` back to `Pending` changes none of the
    /// timestamps the change feed reads; the poller still reports it. The job is created
    /// well before the poll window, so only the tracking of running jobs can see it.
    async fn clients_see_running_jobs_go_back_to_pending<Q: JobHistory + 'static>(queue: Arc<Q>) {
        use hammerwork::Job;
        let state = new_state();
        let mut live = LiveUpdates::new(queue.clone(), state.clone(), 100);
        let mut jobs = connect(&state).await;
        wait_for_connections(&state, 1).await;
        subscribe(&mut jobs, &["job_updates"]).await;
        live.poll_once().await.unwrap();
        while next_json(&mut jobs).await.is_some() {}

        let queue_name = format!("live_back_{}", Uuid::new_v4().simple());
        let mut job = Job::new(queue_name.clone(), serde_json::json!({}));
        job.created_at = Utc::now() - chrono::Duration::hours(1);
        job.scheduled_at = job.created_at;
        let id = queue.enqueue(job).await.unwrap();
        let cleanup = CleanupJob {
            queue: queue.clone(),
            id,
        };

        let run = queue.dequeue_one(&queue_name).await.unwrap().expect("job");
        wait_for(&mut live, &mut jobs, id, "Running").await;
        assert!(queue.release_job_run(&run).await.unwrap());
        let released = wait_for(&mut live, &mut jobs, id, "Pending").await;
        assert_eq!(released["job"]["attempts"], 0);

        // A retry of a run goes back to Pending too.
        let run = queue.dequeue_one(&queue_name).await.unwrap().expect("job");
        wait_for(&mut live, &mut jobs, id, "Running").await;
        queue
            .finish_job_run(
                &run,
                hammerwork::queue::JobOutcome::Retry {
                    retry_at: Utc::now() + chrono::Duration::hours(1),
                    error: "again".to_string(),
                    timed_out: false,
                },
            )
            .await
            .unwrap();
        let retried = wait_for(&mut live, &mut jobs, id, "Pending").await;
        assert_eq!(retried["job"]["attempts"], 1);
        drop(cleanup);
    }

    /// Deletes a test job when dropped, so it is removed even if the test fails.
    struct CleanupJob<Q: JobHistory + 'static> {
        queue: Arc<Q>,
        id: Uuid,
    }

    impl<Q: JobHistory + 'static> Drop for CleanupJob<Q> {
        fn drop(&mut self) {
            let (queue, id) = (self.queue.clone(), self.id);
            // A test's runtime may be shutting down; deleting is best effort.
            let _ = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(queue.delete_job(id))
            });
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires DATABASE_URL (PostgreSQL)"]
    async fn postgres_clients_see_running_jobs_go_back_to_pending() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        clients_see_running_jobs_go_back_to_pending(Arc::new(hammerwork::JobQueue::new(pool)))
            .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires MYSQL_DATABASE_URL"]
    async fn mysql_clients_see_running_jobs_go_back_to_pending() {
        let url = std::env::var("MYSQL_DATABASE_URL").expect("MYSQL_DATABASE_URL");
        let pool = sqlx::MySqlPool::connect(&url).await.unwrap();
        clients_see_running_jobs_go_back_to_pending(Arc::new(hammerwork::JobQueue::new(pool)))
            .await;
    }

    /// A tracked running job that the change feed and the running set no longer show is
    /// looked up; one that went back to Pending is reported at the poll time, one still
    /// running (out of the window) stays tracked, and a deleted one is forgotten.
    #[tokio::test]
    async fn jobs_that_stop_running_without_a_timestamp_are_reported() {
        let mut live = LiveUpdates::new(
            crate::api::test_support::unreachable_queue(),
            new_state(),
            10,
        );
        let t0 = Utc::now();
        live.running = ["a", "b", "c", "d"].map(String::from).into_iter().collect();
        let left = live.left_running(
            &[change("d", "Completed", t0)],
            &[change("c", "Running", t0)],
        );
        assert_eq!(left, ["a", "b"]);

        let now = t0 + chrono::Duration::seconds(5);
        let (still_running, stopped) =
            LiveUpdates::<hammerwork::JobQueue<sqlx::Postgres>>::stopped_running(
                &left,
                vec![
                    change("a", "Pending", t0 - chrono::Duration::hours(1)),
                    change("b", "Running", t0),
                    change("x", "Pending", t0),
                ],
                now,
            );
        assert_eq!(still_running, ["b"]);
        assert_eq!(stopped.len(), 1);
        assert_eq!(stopped[0].id, "a");
        assert_eq!(stopped[0].status, "Pending");
        assert_eq!(
            stopped[0].updated_at, now,
            "reported when the change was seen"
        );
    }

    /// Polls until `client` is told that job `id` is now `status` (the database clock may
    /// be a moment behind).
    async fn wait_for<Q: JobHistory + 'static>(
        live: &mut LiveUpdates<Q>,
        client: &mut warp::test::WsClient,
        id: Uuid,
        status: &str,
    ) -> serde_json::Value {
        for _ in 0..20 {
            live.poll_once().await.unwrap();
            while let Some(message) = next_json(client).await {
                assert_eq!(message["type"], "JobUpdate", "{message}");
                if message["job"]["id"] == id.to_string().as_str()
                    && message["job"]["status"] == status
                {
                    return message;
                }
            }
        }
        panic!("no {status} update for job {id}");
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL (PostgreSQL)"]
    async fn postgres_clients_receive_job_state_changes() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        clients_receive_job_state_changes(Arc::new(hammerwork::JobQueue::new(pool))).await;
    }

    #[tokio::test]
    #[ignore = "requires MYSQL_DATABASE_URL"]
    async fn mysql_clients_receive_job_state_changes() {
        let url = std::env::var("MYSQL_DATABASE_URL").expect("MYSQL_DATABASE_URL");
        let pool = sqlx::MySqlPool::connect(&url).await.unwrap();
        clients_receive_job_state_changes(Arc::new(hammerwork::JobQueue::new(pool))).await;
    }
}
