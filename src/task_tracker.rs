//! Tracking for detached background tasks.
//!
//! A bare `tokio::spawn` drops its `JoinHandle`, so a panic inside the task vanishes
//! and nothing can wait for the task at shutdown. [`TaskTracker`] keeps every task it
//! spawns in a `JoinSet`, logs panics as soon as they happen, and lets the owner
//! drain or abort the tasks.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::task::{AbortHandle, JoinSet};

/// Aborts the wrapped task when dropped, so aborting or dropping the supervisor
/// (for example on `JoinSet::abort_all`) also stops the task it watches.
struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Default)]
struct State {
    set: JoinSet<()>,
    closed: bool,
}

/// A cloneable set of tracked background tasks. Clones share the same tasks.
#[derive(Clone)]
pub(crate) struct TaskTracker {
    state: Arc<Mutex<State>>,
    panics: Arc<AtomicU64>,
    owner: &'static str,
}

impl TaskTracker {
    /// `owner` names the component in log messages (for example `"webhook"`).
    pub(crate) fn new(owner: &'static str) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            panics: Arc::new(AtomicU64::new(0)),
            owner,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A poisoned lock only means another thread panicked while holding it; the
        // JoinSet inside is still valid.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Spawn `future` as a tracked task. A panic in it is logged immediately.
    ///
    /// Returns `false` (and drops the future) once the tracker is closed.
    pub(crate) fn spawn<F>(&self, task: &'static str, future: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let owner = self.owner;
        let panics = self.panics.clone();
        let mut state = self.lock();
        if state.closed {
            tracing::warn!("{owner}: not starting {task}: shutting down");
            return false;
        }

        // Reap tasks that already finished so the set does not grow without bound.
        while state.set.try_join_next().is_some() {}

        let handle = tokio::spawn(future);
        let guard = AbortOnDrop(handle.abort_handle());
        state.set.spawn(async move {
            let _guard = guard;
            if let Err(err) = handle.await {
                if err.is_panic() {
                    panics.fetch_add(1, Ordering::Relaxed);
                    tracing::error!("{owner}: {task} task panicked: {err}");
                } else {
                    tracing::debug!("{owner}: {task} task cancelled");
                }
            }
        });
        true
    }

    /// How many tracked tasks have panicked so far.
    pub(crate) fn panic_count(&self) -> u64 {
        self.panics.load(Ordering::Relaxed)
    }

    /// Number of tasks that have not been reaped yet (finished tasks may still count
    /// until the next `spawn` or `drain`).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().set.len()
    }

    /// Stop accepting new tasks.
    pub(crate) fn close(&self) {
        self.lock().closed = true;
    }

    /// Abort every tracked task and wait for them to stop.
    pub(crate) async fn abort_all(&self) {
        let mut set = {
            let mut state = self.lock();
            state.set.abort_all();
            std::mem::take(&mut state.set)
        };
        while set.join_next().await.is_some() {}
    }

    /// Close the tracker and wait up to `grace` for running tasks to finish on their
    /// own; tasks still running after that are aborted. Returns the number aborted.
    pub(crate) async fn drain(&self, grace: Duration) -> usize {
        let mut set = {
            let mut state = self.lock();
            state.closed = true;
            std::mem::take(&mut state.set)
        };

        let finished =
            tokio::time::timeout(grace, async { while set.join_next().await.is_some() {} })
                .await
                .is_ok();

        if finished {
            return 0;
        }
        let remaining = set.len();
        tracing::warn!(
            "{}: aborting {remaining} task(s) still running after {grace:?}",
            self.owner
        );
        set.abort_all();
        while set.join_next().await.is_some() {}
        remaining
    }
}

/// The running listener of one webhook or stream.
///
/// Dropping the handle stops the listener right away, even while it is waiting for
/// an event: the listener selects on the receiver returned by [`ListenerHandle::new`],
/// which completes when the handle is dropped. So removing or re-adding a webhook or
/// stream never leaves an old listener running next to the new one.
pub(crate) struct ListenerHandle {
    /// The listener's event manager subscription
    pub(crate) subscription_id: uuid::Uuid,
    _stop: tokio::sync::oneshot::Sender<()>,
}

impl ListenerHandle {
    /// A handle for the listener of `subscription_id`, and the stop signal the
    /// listener must watch.
    pub(crate) fn new(subscription_id: uuid::Uuid) -> (Self, tokio::sync::oneshot::Receiver<()>) {
        let (stop, stopped) = tokio::sync::oneshot::channel();
        (
            Self {
                subscription_id,
                _stop: stop,
            },
            stopped,
        )
    }
}

/// Clamp a configured limit into `1..=Semaphore::MAX_PERMITS`, warning when it changes.
///
/// `0` permits would make every waiter wait forever, and more than
/// `Semaphore::MAX_PERMITS` panics inside tokio.
pub(crate) fn clamp_permits(configured: usize, name: &str) -> usize {
    let permits = configured.clamp(1, tokio::sync::Semaphore::MAX_PERMITS);
    if permits != configured {
        tracing::warn!(
            configured,
            effective = permits,
            "{name} is out of range, clamping"
        );
    }
    permits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dropping_a_listener_handle_stops_the_listener() {
        let (handle, mut stopped) = ListenerHandle::new(uuid::Uuid::new_v4());
        assert!(stopped.try_recv().is_err());
        drop(handle);
        // The receiver completes (with an error) once the handle is gone.
        assert!(stopped.await.is_err());
    }

    #[test]
    fn clamp_permits_bounds_the_value() {
        assert_eq!(clamp_permits(0, "x"), 1);
        assert_eq!(clamp_permits(5, "x"), 5);
        assert_eq!(
            clamp_permits(usize::MAX, "x"),
            tokio::sync::Semaphore::MAX_PERMITS
        );
    }
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[tokio::test]
    async fn drain_waits_for_running_tasks() {
        let tracker = TaskTracker::new("test");
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        assert!(tracker.spawn("slow", async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            flag.store(true, Ordering::SeqCst);
        }));

        assert_eq!(tracker.drain(Duration::from_secs(5)).await, 0);
        assert!(done.load(Ordering::SeqCst), "drain must wait for the task");
    }

    #[tokio::test]
    async fn drain_aborts_tasks_that_outlive_the_grace_period() {
        let tracker = TaskTracker::new("test");
        let finished = Arc::new(AtomicBool::new(false));
        let flag = finished.clone();
        tracker.spawn("stuck", async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            flag.store(true, Ordering::SeqCst);
        });

        assert_eq!(tracker.drain(Duration::from_millis(50)).await, 1);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!finished.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn closed_tracker_rejects_new_tasks() {
        let tracker = TaskTracker::new("test");
        tracker.close();
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = ran.clone();
        assert!(!tracker.spawn("late", async move {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(ran.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn abort_all_stops_tasks() {
        let tracker = TaskTracker::new("test");
        let finished = Arc::new(AtomicBool::new(false));
        let flag = finished.clone();
        tracker.spawn("loop", async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            flag.store(true, Ordering::SeqCst);
        });
        tracker.abort_all().await;
        assert_eq!(tracker.len(), 0);
        assert!(!finished.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn finished_tasks_are_reaped_on_spawn() {
        let tracker = TaskTracker::new("test");
        for _ in 0..20 {
            tracker.spawn("quick", async {});
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(tracker.len() < 20, "finished tasks should be reaped");
    }

    #[tokio::test]
    async fn panicking_task_does_not_break_the_tracker() {
        let tracker = TaskTracker::new("test");
        tracker.spawn("boom", async {
            panic!("expected test panic");
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(tracker.panic_count(), 1);
        // The panic is logged by the supervisor; the tracker keeps working.
        let ok = Arc::new(AtomicBool::new(false));
        let flag = ok.clone();
        tracker.spawn("after", async move {
            flag.store(true, Ordering::SeqCst);
        });
        assert_eq!(tracker.drain(Duration::from_secs(5)).await, 0);
        assert!(ok.load(Ordering::SeqCst));
    }
}
