//! [`WorkerPool`]: running, supervising and autoscaling workers, and the pool's
//! periodic maintenance tasks.

use super::autoscale::{QueueDepthHistory, apply_queue_depth};
use super::*;

/// What a [`WorkerPool`] supervisor needs to apply autoscaling decisions.
struct Scaling<DB: Database> {
    /// Cloned for every worker started by a scale-up. Only workers of its queue are
    /// started or retired.
    template: Worker<DB>,
    /// Desired number of workers on the template's queue, published by the
    /// autoscaling task
    desired: watch::Receiver<usize>,
    /// Workers on the template's queue when the pool started
    initial: usize,
}

/// Wait for the next value on `rx`. Returns `None` once the sender is gone, and never
/// completes when there is no receiver.
async fn wait_for_change(rx: &mut Option<watch::Receiver<usize>>) -> Option<usize> {
    match rx {
        Some(rx) => match rx.changed().await {
            Ok(()) => Some(*rx.borrow_and_update()),
            Err(_) => None,
        },
        None => std::future::pending().await,
    }
}

/// A pool of [`Worker`]s run and supervised together.
///
/// [`WorkerPool::start`] spawns every worker plus a supervisor. A worker that dies
/// unexpectedly (for example because of a bug that panics outside a job handler) is
/// logged and restarted. [`WorkerPool::shutdown`] (or dropping the pool) stops all
/// workers gracefully: each finishes its in-flight job within its shutdown grace period.
///
/// The pool also runs a stale job reaper that periodically calls
/// [`DatabaseQueue::requeue_stale_jobs`] to reclaim jobs left `Running` by workers
/// that crashed. Configure it with [`WorkerPool::with_stale_job_reaper`] or disable it
/// with [`WorkerPool::without_stale_job_reaper`].
///
/// It also clears expired job results every 5 minutes by calling
/// [`DatabaseQueue::cleanup_expired_results`] ([`WorkerPool::with_result_cleanup`],
/// disable it with [`WorkerPool::without_result_cleanup`]).
///
/// Optionally, the pool also enforces the retention of encrypted jobs by calling
/// [`DatabaseQueue::purge_expired_encrypted_jobs`] periodically
/// ([`WorkerPool::with_encrypted_job_purge`]), archives finished jobs
/// ([`WorkerPool::with_archival`]) and, with the `encryption` feature, rotates
/// encryption keys as they fall due (`WorkerPool::with_key_rotation`).
///
/// These maintenance tasks run each pass in its own task, so workers never wait for
/// them and shutting the pool down never cancels a pass part-way through its database
/// transaction. They stop when the pool shuts down.
pub struct WorkerPool<DB: Database> {
    workers: Vec<Worker<DB>>,
    /// Signals shutdown to the supervisor started by `start` (dropping it also does)
    shutdown_signal: Option<watch::Sender<bool>>,
    /// Becomes `true` once every worker has stopped after a shutdown
    supervisor_done: Option<watch::Receiver<bool>>,
    /// How often the stale job reaper runs (`None` disables it)
    reaper_interval: Option<Duration>,
    /// How often encrypted jobs whose retention ended are purged (`None`: never)
    encrypted_job_purge_interval: Option<Duration>,
    /// Automatic archival and purge of archived jobs (`None`: off)
    archival: Option<PoolArchival>,
    /// How often expired job results are cleared (`None`: never)
    result_cleanup_interval: Option<Duration>,
    /// Automatic rotation of encryption keys (`None`: off)
    #[cfg(feature = "encryption")]
    key_rotation: Option<PoolKeyRotation>,
    stats_collector: Option<Arc<dyn StatisticsCollector>>,
    /// Worker template for creating new workers during autoscaling
    worker_template: Option<Worker<DB>>,
    /// Autoscaling configuration
    autoscale_config: AutoscaleConfig,
    /// Autoscaling metrics and state
    autoscale_metrics: Arc<std::sync::RwLock<AutoscaleMetrics>>,
    /// Queue depth history for averaging
    queue_depth_history: QueueDepthHistory,
    /// Autoscaling task handle
    autoscale_task: Option<tokio::task::JoinHandle<()>>,
}

impl<DB: Database + Send + Sync + 'static> WorkerPool<DB>
where
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    /// Create an empty pool. Autoscaling is off; enable it with
    /// [`with_autoscaling`](Self::with_autoscaling). The stale job reaper and the
    /// expired result cleanup are on.
    pub fn new() -> Self {
        Self {
            workers: Vec::new(),
            shutdown_signal: None,
            supervisor_done: None,
            reaper_interval: Some(DEFAULT_STALE_JOB_REAPER_INTERVAL),
            encrypted_job_purge_interval: None,
            archival: None,
            result_cleanup_interval: Some(DEFAULT_RESULT_CLEANUP_INTERVAL),
            #[cfg(feature = "encryption")]
            key_rotation: None,
            stats_collector: None,
            worker_template: None,
            autoscale_config: AutoscaleConfig::disabled(),
            autoscale_metrics: Arc::new(std::sync::RwLock::new(AutoscaleMetrics::default())),
            queue_depth_history: Arc::new(std::sync::RwLock::new(Vec::new())),
            autoscale_task: None,
        }
    }

    /// Build a pool from a [`WorkerConfig`](crate::config::WorkerConfig).
    ///
    /// Adds `pool_size` copies of `worker` (at least one), uses it as the autoscaling
    /// template, configures autoscaling from `autoscaling_enabled`, `min_workers`
    /// and `max_workers`, and the expired result cleanup from `result_cleanup_enabled`
    /// and `result_cleanup_interval`. Apply the per-worker settings to `worker` first
    /// with [`Worker::with_config`] or [`Worker::with_hammerwork_config`].
    pub fn from_config(worker: Worker<DB>, config: &crate::config::WorkerConfig) -> Self {
        let mut pool = Self::new()
            .with_autoscaling(config.autoscale_config())
            .with_worker_template(worker.clone());
        pool.result_cleanup_interval = config
            .result_cleanup_enabled
            .then_some(config.result_cleanup_interval);
        for _ in 1..config.pool_size {
            pool.add_worker(worker.clone());
        }
        pool.add_worker(worker);
        pool
    }

    /// Build a pool from a [`HammerworkConfig`](crate::config::HammerworkConfig).
    ///
    /// Like [`WorkerPool::from_config`] with `config.worker` (including the expired
    /// result cleanup every `worker.result_cleanup_interval`, on by default), and also
    /// schedules the encrypted job retention purge every
    /// `encryption.purge_interval_secs`, when set
    /// ([`WorkerPool::with_encrypted_job_purge`]), automatic archival every
    /// `archive.check_interval` when `archive.enabled` ([`WorkerPool::with_archival`]
    /// with [`ArchiveConfig::archival_policy`](crate::config::ArchiveConfig::archival_policy)
    /// and [`ArchiveConfig::archival_config`](crate::config::ArchiveConfig::archival_config)),
    /// and key rotation every `encryption.key_rotation.check_interval_secs` when
    /// `encryption.key_rotation.enabled` (see `WorkerPool::with_key_rotation`; the
    /// `KeyManager` is created on the worker's database, with
    /// [`KeyRotationConfig::key_manager_config`](crate::config::KeyRotationConfig::key_manager_config),
    /// when the pool starts, and [`WorkerPool::start`] fails if its master key cannot be
    /// loaded). Validates the `encryption`, `worker` and `archive` sections; enabling
    /// key rotation without the `encryption` feature is an error.
    pub fn from_hammerwork_config(
        worker: Worker<DB>,
        config: &crate::config::HammerworkConfig,
    ) -> Result<Self> {
        config.encryption.validate()?;
        config.worker.validate()?;
        config.archive.validate()?;
        config.rate_limiting.validate()?;
        let rotation = &config.encryption.key_rotation;
        #[cfg(feature = "encryption")]
        let key_rotation = if rotation.enabled {
            Some(PoolKeyRotation {
                interval: rotation.check_interval(),
                start: key_rotation_from_config(&worker.queue.pool, rotation.key_manager_config())?,
            })
        } else {
            None
        };
        #[cfg(not(feature = "encryption"))]
        if rotation.enabled {
            return Err(HammerworkError::Config(
                "encryption.key_rotation.enabled is set but hammerwork was built without the \
                 `encryption` feature"
                    .to_string(),
            ));
        }
        let mut pool = Self::from_config(worker, &config.worker);
        #[cfg(feature = "encryption")]
        {
            pool.key_rotation = key_rotation;
        }
        pool.encrypted_job_purge_interval = config.encryption.purge_interval();
        if config.archive.enabled {
            pool = pool.with_archival(
                config.archive.archival_policy(),
                config.archive.archival_config(),
                config.archive.check_interval,
            );
        }
        Ok(pool)
    }

    /// Record the statistics of every worker of the pool (those already added, those
    /// added later and those started by autoscaling) in `stats_collector`.
    pub fn with_stats_collector(mut self, stats_collector: Arc<dyn StatisticsCollector>) -> Self {
        for worker in &mut self.workers {
            worker.stats_collector = Some(Arc::clone(&stats_collector));
        }
        self.stats_collector = Some(stats_collector);
        self
    }

    /// Configure autoscaling for the worker pool.
    ///
    /// Autoscaling is off unless enabled here (or by
    /// [`from_config`](Self::from_config) with `autoscaling_enabled`). It scales one
    /// queue: that of the [worker template](Self::with_worker_template), or of the first
    /// worker added when no template is set. It measures that queue's depth, starts
    /// clones of the template while it is deep and retires workers of that queue (never
    /// below `min_workers`) once it drains. Workers of other queues are never started
    /// or retired. A retired worker stops like on shutdown: it finishes its in-flight
    /// job within its [shutdown grace period](Worker::with_shutdown_grace_period).
    pub fn with_autoscaling(mut self, config: AutoscaleConfig) -> Self {
        self.autoscale_config = config;
        self
    }

    /// Disable autoscaling for the worker pool
    pub fn without_autoscaling(mut self) -> Self {
        self.autoscale_config = AutoscaleConfig::disabled();
        self
    }

    /// Configure the stale job reaper.
    ///
    /// Every `interval` the pool calls [`DatabaseQueue::requeue_stale_jobs`], which
    /// moves `Running` jobs whose lease expired back to `Pending` (or to `Dead` when
    /// they have no attempts left). Workers take their lease when they claim a job, so
    /// a job whose worker is alive is never reclaimed, whatever this pool's settings or
    /// those of other pools. How soon a crashed worker's job is reclaimed depends on the
    /// worker's [lease duration](Worker::with_lease_duration) plus this interval. The
    /// reaper is enabled by default with a 60 second interval
    /// ([`DEFAULT_STALE_JOB_REAPER_INTERVAL`](crate::worker::DEFAULT_STALE_JOB_REAPER_INTERVAL)).
    ///
    /// Running reapers in several pools or processes at once, with different
    /// intervals, is safe.
    pub fn with_stale_job_reaper(mut self, interval: Duration) -> Self {
        self.reaper_interval = Some(interval);
        self
    }

    /// Disable the stale job reaper (for example when it runs elsewhere, such as a
    /// scheduled `cargo hammerwork job requeue-stale`).
    pub fn without_stale_job_reaper(mut self) -> Self {
        self.reaper_interval = None;
        self
    }

    /// Purge encrypted jobs whose retention period ended every `interval`.
    ///
    /// The pool calls [`DatabaseQueue::purge_expired_encrypted_jobs`], which deletes
    /// finished encrypted jobs (and archived ones) past their `retention_delete_at`; it
    /// needs no encryption key. Off by default: without it, run
    /// `cargo hammerwork maintenance purge-encrypted` on a schedule. Running purges in
    /// several pools or processes at once is safe.
    ///
    /// The first purge runs when the pool starts. Each purge runs in its own task, so
    /// shutting the pool down never cancels one part-way through its transaction.
    pub fn with_encrypted_job_purge(mut self, interval: Duration) -> Self {
        self.encrypted_job_purge_interval = Some(interval);
        self
    }

    /// Disable the encrypted job retention purge (the default).
    pub fn without_encrypted_job_purge(mut self) -> Self {
        self.encrypted_job_purge_interval = None;
        self
    }

    /// Archive finished jobs, and purge old archived jobs, every `interval`.
    ///
    /// Each pass runs [`JobArchiver::run_scheduled_pass`](crate::archive::JobArchiver::run_scheduled_pass)
    /// with `policy` as the policy for every queue and `config` for compression: it
    /// moves completed, failed, dead and timed out jobs older than the policy's
    /// thresholds to `hammerwork_jobs_archive` (reason `Automatic`, archived by
    /// `"worker_pool"`), then deletes archived jobs older than `purge_archived_after`,
    /// when set. Results are logged. Off by default; a disabled `policy` does nothing.
    ///
    /// The first pass runs when the pool starts. Passes run in their own task, so
    /// workers are never blocked by them. On shutdown no new pass starts, and a running
    /// pass stops before its next batch; a batch that has started commits or rolls back
    /// as a whole. Running archival in several pools or processes at once is safe: rows
    /// are claimed with `FOR UPDATE SKIP LOCKED`, so each job is archived once.
    pub fn with_archival(
        mut self,
        policy: crate::archive::ArchivalPolicy,
        config: crate::archive::ArchivalConfig,
        interval: Duration,
    ) -> Self {
        self.archival = Some(PoolArchival {
            policy,
            config,
            interval,
        });
        self
    }

    /// Disable automatic archival (the default).
    pub fn without_archival(mut self) -> Self {
        self.archival = None;
        self
    }

    /// Clear expired job results every `interval`.
    ///
    /// The pool calls [`DatabaseQueue::cleanup_expired_results`], which removes the
    /// stored results (`result_data`) of jobs whose result TTL has passed. Expired
    /// results are never returned by
    /// [`DatabaseQueue::get_job_result`], so this only frees the space they take (and
    /// stops [`DatabaseQueue::get_job`] from still showing them in `Job::result_data`).
    /// On by default every 5 minutes ([`DEFAULT_RESULT_CLEANUP_INTERVAL`]); a pool
    /// built from configuration uses `worker.result_cleanup_enabled` and
    /// `worker.result_cleanup_interval`. Running it in several pools or processes at
    /// once is safe.
    ///
    /// The first cleanup runs when the pool starts. Each cleanup runs in its own task,
    /// so workers never wait for it, and shutting the pool down stops the schedule
    /// without cancelling a cleanup part-way. Each cleanup that clears results logs how
    /// many at `info` level (failures at `warn`).
    pub fn with_result_cleanup(mut self, interval: Duration) -> Self {
        self.result_cleanup_interval = Some(interval);
        self
    }

    /// Disable the expired result cleanup (for example when it runs elsewhere).
    pub fn without_result_cleanup(mut self) -> Self {
        self.result_cleanup_interval = None;
        self
    }

    /// Rotate the encryption keys of `key_manager` as they fall due, checking every
    /// `check_interval`.
    ///
    /// Each pass runs [`KeyManager::perform_automatic_rotation`](crate::encryption::KeyManager::perform_automatic_rotation),
    /// which rotates the keys whose `next_rotation_at` has passed, and logs the rotated
    /// keys (`info`) or the failure (`error`). The key manager must have automatic
    /// rotation enabled ([`KeyManagerConfig::with_auto_rotation_enabled`](crate::encryption::KeyManagerConfig::with_auto_rotation_enabled));
    /// otherwise the pool logs a warning and does not rotate. Off by default; a pool
    /// built with [`WorkerPool::from_hammerwork_config`] turns it on from
    /// `[encryption.key_rotation]`.
    ///
    /// The first pass runs when the pool starts. Passes run in their own task, so
    /// workers never wait for them, and shutting the pool down stops the schedule
    /// without interrupting a rotation part-way. Several pools or processes can rotate
    /// at once: each due key gets exactly one new version (see
    /// [`KeyManager::perform_automatic_rotation`](crate::encryption::KeyManager::perform_automatic_rotation)).
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "encryption", feature = "postgres"))]
    /// # {
    /// use hammerwork::encryption::{KeyManager, KeyManagerConfig};
    /// use hammerwork::{JobQueue, Worker, WorkerPool, worker::JobHandler};
    /// use std::{sync::Arc, time::Duration};
    ///
    /// # async fn example(pool: sqlx::PgPool, handler: JobHandler) -> Result<(), Box<dyn std::error::Error>> {
    /// let key_manager = KeyManager::new(
    ///     KeyManagerConfig::new()
    ///         .with_master_key_env("HAMMERWORK_MASTER_KEY")
    ///         .with_auto_rotation_enabled(true),
    ///     pool.clone(),
    /// )
    /// .await?;
    /// let queue = Arc::new(JobQueue::new(pool));
    /// let mut worker_pool = WorkerPool::new()
    ///     .with_key_rotation(key_manager, Duration::from_secs(3600));
    /// worker_pool.add_worker(Worker::new(queue, "default".to_string(), handler));
    /// worker_pool.start().await?;
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    #[cfg(feature = "encryption")]
    pub fn with_key_rotation<K: crate::encryption::KeyManagerBackend>(
        mut self,
        key_manager: crate::encryption::KeyManager<K>,
        check_interval: Duration,
    ) -> Self {
        let pass = rotation_pass(key_manager);
        self.key_rotation = Some(PoolKeyRotation {
            interval: check_interval,
            start: Arc::new(move || {
                let pass = pass.clone();
                Box::pin(async move { Ok(pass) })
            }),
        });
        self
    }

    /// Disable automatic key rotation (the default).
    #[cfg(feature = "encryption")]
    pub fn without_key_rotation(mut self) -> Self {
        self.key_rotation = None;
        self
    }

    /// Set a worker template for autoscaling
    /// This worker will be cloned when creating new workers
    pub fn with_worker_template(mut self, worker: Worker<DB>) -> Self {
        self.worker_template = Some(worker);
        self
    }

    pub fn add_worker(&mut self, mut worker: Worker<DB>) {
        // Apply the pool's stats collector to the worker if available
        if let Some(stats_collector) = &self.stats_collector {
            worker.stats_collector = Some(Arc::clone(stats_collector));
        }

        // If no worker template is set and autoscaling is enabled, use the first worker
        // as template (`start` also falls back to the first worker).
        if self.worker_template.is_none()
            && self.autoscale_config.enabled
            && self.workers.is_empty()
        {
            self.worker_template = Some(worker.clone());
        }

        self.workers.push(worker);
    }

    /// Start all workers and supervise them until the pool shuts down.
    ///
    /// Returns once every worker has stopped after [`WorkerPool::shutdown`] (or after
    /// the pool is dropped). Workers run in their own tasks, so dropping the returned
    /// future (for example in a `tokio::select!`) does not stop them; call
    /// [`WorkerPool::shutdown`] afterwards to stop them gracefully.
    pub async fn start(&mut self) -> Result<()> {
        info!("Starting worker pool with {} workers", self.workers.len());

        // Created first, so a key manager that cannot start fails the pool before any
        // worker runs.
        #[cfg(feature = "encryption")]
        let key_rotation = match &self.key_rotation {
            Some(rotation) => (rotation.start)()
                .await?
                .map(|pass| (pass, rotation.interval)),
            None => None,
        };

        let (signal_tx, signal_rx) = watch::channel(false);
        let (done_tx, done_rx) = watch::channel(false);
        self.shutdown_signal = Some(signal_tx);
        self.supervisor_done = Some(done_rx);

        let workers = std::mem::take(&mut self.workers);
        #[cfg_attr(not(feature = "encryption"), allow(unused_mut))]
        let mut maintenance: Vec<tokio::task::JoinHandle<()>> = [
            self.start_stale_job_reaper(&workers, signal_rx.clone()),
            self.start_encrypted_job_purge(&workers, signal_rx.clone()),
            self.start_archival(&workers, signal_rx.clone()),
            self.start_result_cleanup(&workers, signal_rx.clone()),
        ]
        .into_iter()
        .flatten()
        .collect();
        #[cfg(feature = "encryption")]
        if let Some((pass, interval)) = key_rotation {
            info!("Rotating due encryption keys every {:?}", interval);
            maintenance.push(spawn_periodic(interval, signal_rx.clone(), move || pass()));
        }

        // Start autoscaling task if enabled. It publishes the desired number of workers
        // on the template's queue, which the supervisor applies by starting workers from
        // the template or retiring workers of that queue.
        let scaling = if self.autoscale_config.enabled {
            self.start_autoscaling_task(&workers)
        } else {
            None
        };
        if let Ok(mut metrics) = self.autoscale_metrics.write() {
            metrics.active_workers = match &scaling {
                Some(scaling) => scaling.initial,
                None => workers.len(),
            };
        }

        let supervisor = tokio::spawn(Self::supervise(
            workers,
            signal_rx,
            done_tx,
            maintenance,
            scaling,
        ));

        // Dropping a JoinHandle detaches the task, so the supervisor keeps running
        // even if this future is dropped.
        supervisor.await.map_err(|e| HammerworkError::Worker {
            message: format!("Worker pool supervisor failed: {}", e),
        })?
    }

    /// Spawn one worker task with its own shutdown channel.
    fn spawn_worker(
        set: &mut JoinSet<(usize, std::thread::Result<Result<()>>)>,
        index: usize,
        worker: Worker<DB>,
        delay: Option<Duration>,
    ) -> mpsc::Sender<()> {
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        set.spawn(async move {
            if let Some(delay) = delay {
                sleep(delay).await;
            }
            // Catch panics so the supervisor always knows which worker stopped.
            let run = CatchUnwind {
                inner: Box::pin(async move { worker.run(shutdown_rx).await }),
            };
            (index, run.await)
        });
        shutdown_tx
    }

    /// Supervise worker tasks: restart workers that stop unexpectedly, and on
    /// shutdown signal every worker and wait for all of them to stop.
    async fn supervise(
        workers: Vec<Worker<DB>>,
        mut signal_rx: watch::Receiver<bool>,
        done_tx: watch::Sender<bool>,
        maintenance: Vec<tokio::task::JoinHandle<()>>,
        scaling: Option<Scaling<DB>>,
    ) -> Result<()> {
        let mut templates: Vec<Worker<DB>> = workers.to_vec();
        let mut set = JoinSet::new();
        let mut senders: Vec<mpsc::Sender<()>> = workers
            .into_iter()
            .enumerate()
            .map(|(index, worker)| Self::spawn_worker(&mut set, index, worker, None))
            .collect();
        // Workers stopped by a scale-down: not restarted, and their slot is reused by
        // the next scale-up once their task has finished.
        let mut retired: Vec<bool> = vec![false; senders.len()];
        let mut finished: Vec<bool> = vec![false; senders.len()];
        let (template, mut desired_rx) = match scaling {
            Some(scaling) => (Some(scaling.template), Some(scaling.desired)),
            None => (None, None),
        };

        let mut shutting_down = *signal_rx.borrow();
        if shutting_down {
            for tx in &senders {
                let _ = tx.try_send(());
            }
        }

        loop {
            tokio::select! {
                // An error means the pool (and its signal sender) was dropped: treat it
                // as a shutdown request too.
                _ = signal_rx.changed(), if !shutting_down => {
                    info!("Worker pool shutting down; waiting for in-flight jobs");
                    shutting_down = true;
                    for tx in &senders {
                        let _ = tx.try_send(());
                    }
                }
                changed = wait_for_change(&mut desired_rx), if !shutting_down => {
                    let (Some(desired), Some(template)) = (changed, template.as_ref()) else {
                        // The autoscaler stopped: keep the current workers.
                        desired_rx = None;
                        continue;
                    };
                    // Only the template's queue is scaled: workers of other queues are
                    // never retired, and new workers are clones of the template.
                    let active: Vec<usize> = (0..senders.len())
                        .filter(|i| !retired[*i] && templates[*i].queue_name == template.queue_name)
                        .collect();
                    if desired > active.len() {
                        for _ in active.len()..desired {
                            let worker = template.clone();
                            let reusable = (0..senders.len()).find(|i| retired[*i] && finished[*i]);
                            let index = reusable.unwrap_or(senders.len());
                            let tx = Self::spawn_worker(&mut set, index, worker.clone(), None);
                            if index == senders.len() {
                                templates.push(worker);
                                senders.push(tx);
                                retired.push(false);
                                finished.push(false);
                            } else {
                                templates[index] = worker;
                                senders[index] = tx;
                                retired[index] = false;
                                finished[index] = false;
                            }
                        }
                        info!(
                            "Autoscaling: started workers for queue {}, now {}",
                            template.queue_name, desired
                        );
                    } else {
                        for index in active.iter().rev().take(active.len() - desired) {
                            retired[*index] = true;
                            let _ = senders[*index].try_send(());
                        }
                        if desired < active.len() {
                            info!(
                                "Autoscaling: retiring workers of queue {}, now {}",
                                template.queue_name, desired
                            );
                        }
                    }
                }
                joined = set.join_next() => {
                    let Some(joined) = joined else { break };
                    let index = match joined {
                        Ok((index, Ok(Ok(())))) => {
                            if shutting_down {
                                continue;
                            }
                            if retired[index] {
                                finished[index] = true;
                                continue;
                            }
                            warn!("Worker {} stopped unexpectedly", index);
                            index
                        }
                        Ok((index, Ok(Err(e)))) => {
                            error!("Worker {} failed: {}", index, e);
                            index
                        }
                        Ok((index, Err(payload))) => {
                            error!(
                                "Worker {} panicked: {}",
                                index,
                                panic_message(payload.as_ref())
                            );
                            index
                        }
                        Err(e) => {
                            // Only reachable if a worker task is aborted externally.
                            error!("Worker task ended abnormally: {}", e);
                            continue;
                        }
                    };
                    if retired[index] {
                        finished[index] = true;
                    } else if !shutting_down {
                        info!("Restarting worker {} in {:?}", index, WORKER_RESTART_DELAY);
                        senders[index] = Self::spawn_worker(
                            &mut set,
                            index,
                            templates[index].clone(),
                            Some(WORKER_RESTART_DELAY),
                        );
                    }
                }
            }
        }

        // Stops the maintenance loops; a pass already running finishes in its own task
        for task in maintenance {
            task.abort();
        }
        info!("All workers stopped");
        done_tx.send_replace(true);
        Ok(())
    }

    /// Spawn the periodic stale job reaper, if enabled.
    fn start_stale_job_reaper(
        &self,
        workers: &[Worker<DB>],
        signal_rx: watch::Receiver<bool>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let interval = self.reaper_interval?;
        spawn_queue_task(workers, interval, signal_rx, |queue| async move {
            match queue.requeue_stale_jobs().await {
                Ok(recovery) if !recovery.is_empty() => info!(
                    "Stale job reaper requeued {} and marked {} dead",
                    recovery.requeued.len(),
                    recovery.dead.len()
                ),
                Ok(_) => {}
                Err(e) => warn!("Stale job reaper failed: {}", e),
            }
        })
    }

    /// Spawn the periodic encrypted job retention purge, if enabled.
    fn start_encrypted_job_purge(
        &self,
        workers: &[Worker<DB>],
        signal_rx: watch::Receiver<bool>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let interval = self.encrypted_job_purge_interval?;
        spawn_queue_task(workers, interval, signal_rx, |queue| async move {
            match queue.purge_expired_encrypted_jobs().await {
                Ok(purge) if purge.total() > 0 => info!(
                    "Encrypted job purge deleted {} jobs and {} archived jobs",
                    purge.jobs, purge.archived_jobs
                ),
                Ok(_) => {}
                Err(e) => warn!("Encrypted job purge failed: {}", e),
            }
        })
    }

    /// Spawn the periodic archival pass, if enabled.
    fn start_archival(
        &self,
        workers: &[Worker<DB>],
        signal_rx: watch::Receiver<bool>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let archival = self.archival.clone()?;
        let queue = Arc::clone(&workers.first()?.queue);
        if !archival.policy.enabled {
            info!("Automatic archival is configured with a disabled policy; not starting it");
            return None;
        }
        let archiver = Arc::new(
            crate::archive::JobArchiver::new()
                .with_default_policy(archival.policy)
                .with_config(archival.config),
        );
        let stop_rx = signal_rx.clone();
        Some(spawn_periodic(archival.interval, signal_rx, move || {
            let (queue, archiver, stop_rx) =
                (Arc::clone(&queue), Arc::clone(&archiver), stop_rx.clone());
            async move {
                let pass = archiver
                    .run_scheduled_pass(queue.as_ref(), Some("worker_pool"), || *stop_rx.borrow())
                    .await;
                match pass {
                    Ok(pass) if pass.jobs_archived > 0 || pass.jobs_purged > 0 => info!(
                        "Archival archived {} jobs and purged {} archived jobs{}",
                        pass.jobs_archived,
                        pass.jobs_purged,
                        if pass.stopped_early {
                            " (stopped early for shutdown)"
                        } else {
                            ""
                        }
                    ),
                    Ok(_) => debug!("Archival found nothing to archive or purge"),
                    Err(e) => warn!("Archival failed: {}", e),
                }
            }
        }))
    }

    /// Spawn the periodic expired result cleanup, if enabled.
    fn start_result_cleanup(
        &self,
        workers: &[Worker<DB>],
        signal_rx: watch::Receiver<bool>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let interval = self.result_cleanup_interval?;
        if !workers.is_empty() {
            debug!("Clearing expired job results every {:?}", interval);
        }
        spawn_queue_task(workers, interval, signal_rx, |queue| async move {
            match queue.cleanup_expired_results().await {
                Ok(cleared) if cleared > 0 => {
                    info!("Result cleanup cleared {} expired job results", cleared)
                }
                Ok(_) => debug!("Result cleanup found no expired job results"),
                Err(e) => warn!("Result cleanup failed: {}", e),
            }
        })
    }

    /// Start the autoscaling background task.
    ///
    /// Returns the worker template and a channel carrying the desired worker count,
    /// which the supervisor applies, or `None` when there is no worker template.
    fn start_autoscaling_task(&mut self, workers: &[Worker<DB>]) -> Option<Scaling<DB>> {
        // Without an explicit template, scale the first worker's queue.
        let Some(worker_template) = self.worker_template.as_ref().or(workers.first()) else {
            warn!("Cannot start autoscaling: no worker template available");
            return None;
        };
        let mut template = worker_template.clone();
        let initial_workers = workers
            .iter()
            .filter(|worker| worker.queue_name == template.queue_name)
            .count();
        if let Some(stats_collector) = &self.stats_collector {
            template.stats_collector = Some(Arc::clone(stats_collector));
        }
        let queue = Arc::clone(&template.queue);
        let queue_name = template.queue_name.clone();
        let config = self.autoscale_config.clone();
        let metrics = Arc::clone(&self.autoscale_metrics);
        let history = Arc::clone(&self.queue_depth_history);
        let (desired_tx, desired_rx) = watch::channel(initial_workers);

        let task = tokio::spawn(async move {
            Self::autoscaling_loop(queue, queue_name, config, metrics, history, desired_tx).await;
        });

        if let Some(previous) = self.autoscale_task.replace(task) {
            previous.abort();
        }
        info!(
            "Autoscaling task started for queue: {}",
            template.queue_name
        );
        Some(Scaling {
            template,
            desired: desired_rx,
            initial: initial_workers,
        })
    }

    /// Main autoscaling evaluation loop
    async fn autoscaling_loop(
        queue: Arc<JobQueue<DB>>,
        queue_name: String,
        config: AutoscaleConfig,
        metrics: Arc<std::sync::RwLock<AutoscaleMetrics>>,
        history: QueueDepthHistory,
        desired: watch::Sender<usize>,
    ) {
        // `interval` panics on a zero period, so never pass one through.
        let mut interval =
            tokio::time::interval((config.evaluation_window / 2).max(Duration::from_millis(1)));

        loop {
            interval.tick().await;

            match Self::evaluate_scaling_decision(&queue, &queue_name, &config, &metrics, &history)
                .await
            {
                Ok(Some(count)) => {
                    // An error means the supervisor stopped: nothing left to scale.
                    if desired.send(count).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(e) => warn!("Autoscaling evaluation error: {}", e),
            }
        }
    }

    /// Evaluate whether scaling up or down is needed.
    ///
    /// Returns the new worker count when the pool should scale.
    async fn evaluate_scaling_decision(
        queue: &Arc<JobQueue<DB>>,
        queue_name: &str,
        config: &AutoscaleConfig,
        metrics: &Arc<std::sync::RwLock<AutoscaleMetrics>>,
        history: &QueueDepthHistory,
    ) -> Result<Option<usize>> {
        // Get current queue depth
        let current_depth = queue.get_queue_depth(queue_name).await?;
        Ok(apply_queue_depth(
            current_depth,
            Utc::now(),
            config,
            metrics,
            history,
        ))
    }

    /// Get current autoscaling metrics
    pub fn get_autoscale_metrics(&self) -> AutoscaleMetrics {
        self.autoscale_metrics
            .read()
            .map(|metrics| metrics.clone())
            .unwrap_or_default()
    }

    /// Shut the pool down gracefully.
    ///
    /// Signals every worker to stop polling, then waits until all of them have
    /// stopped. Each worker lets its in-flight job finish within its
    /// [shutdown grace period](Worker::with_shutdown_grace_period).
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down worker pool");

        // Stop autoscaling task
        if let Some(task) = &self.autoscale_task {
            task.abort();
            info!("Autoscaling task stopped");
        }

        if let Some(signal) = &self.shutdown_signal {
            signal.send_replace(true);
        }

        if let Some(done) = &self.supervisor_done {
            let mut done = done.clone();
            // An error means the supervisor is gone, so there is nothing to wait for.
            let _ = done.wait_for(|finished| *finished).await;
        }

        Ok(())
    }

    /// Get the statistics collector for the worker pool
    pub fn stats_collector(&self) -> Option<Arc<dyn StatisticsCollector>> {
        self.stats_collector.clone()
    }
}

impl<DB: Database + Send + Sync + 'static> Default for WorkerPool<DB>
where
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    fn default() -> Self {
        Self::new()
    }
}

/// Automatic archival settings of a [`WorkerPool`] ([`WorkerPool::with_archival`]).
#[derive(Debug, Clone)]
struct PoolArchival {
    policy: crate::archive::ArchivalPolicy,
    config: crate::archive::ArchivalConfig,
    interval: Duration,
}

/// Default interval of the expired result cleanup of a [`WorkerPool`]
/// ([`WorkerPool::with_result_cleanup`]): five minutes.
pub const DEFAULT_RESULT_CLEANUP_INTERVAL: Duration =
    crate::config::DEFAULT_RESULT_CLEANUP_INTERVAL;

/// One key rotation pass, whatever the key manager's database.
#[cfg(feature = "encryption")]
type RotationPass = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Creates the key manager when the pool starts and returns its rotation pass (`None`
/// when its automatic rotation is disabled).
#[cfg(feature = "encryption")]
type RotationStarter = Arc<
    dyn Fn() -> Pin<
            Box<
                dyn Future<
                        Output = std::result::Result<
                            Option<RotationPass>,
                            crate::encryption::EncryptionError,
                        >,
                    > + Send,
            >,
        > + Send
        + Sync,
>;

/// Key rotation settings of a [`WorkerPool`] (`WorkerPool::with_key_rotation`).
#[cfg(feature = "encryption")]
#[derive(Clone)]
struct PoolKeyRotation {
    interval: Duration,
    start: RotationStarter,
}

/// The rotation pass of `manager`, or `None` (logged) when its automatic rotation is
/// disabled.
#[cfg(feature = "encryption")]
fn rotation_pass<K: crate::encryption::KeyManagerBackend>(
    manager: crate::encryption::KeyManager<K>,
) -> Option<RotationPass> {
    if !manager.auto_rotation_enabled() {
        warn!(
            "Key rotation is configured with a KeyManager whose automatic rotation is \
             disabled; not starting it"
        );
        return None;
    }
    Some(Arc::new(move || Box::pin(manager.clone().rotation_pass())))
}

/// A starter that creates a `KeyManager` with `config` on `pool`.
#[cfg(feature = "encryption")]
#[allow(dead_code)] // unused without a database backend
fn key_manager_starter<K: crate::encryption::KeyManagerBackend>(
    pool: sqlx::Pool<K>,
    config: crate::encryption::KeyManagerConfig,
) -> RotationStarter {
    Arc::new(move || {
        let (pool, config) = (pool.clone(), config.clone());
        Box::pin(async move {
            let manager = crate::encryption::KeyManager::new(config, pool).await?;
            Ok(rotation_pass(manager))
        })
    })
}

/// The starter for `[encryption.key_rotation]`: a `KeyManager` on the pool's own
/// database, which must be PostgreSQL or MySQL.
#[cfg(feature = "encryption")]
#[allow(unused_variables)] // without a database backend nothing matches
fn key_rotation_from_config<DB: Database>(
    pool: &sqlx::Pool<DB>,
    config: crate::encryption::KeyManagerConfig,
) -> Result<RotationStarter> {
    let pool: &dyn std::any::Any = pool;
    #[cfg(feature = "postgres")]
    if let Some(pool) = pool.downcast_ref::<sqlx::PgPool>() {
        return Ok(key_manager_starter(pool.clone(), config));
    }
    #[cfg(feature = "mysql")]
    if let Some(pool) = pool.downcast_ref::<sqlx::MySqlPool>() {
        return Ok(key_manager_starter(pool.clone(), config));
    }
    Err(HammerworkError::Config(
        "encryption.key_rotation needs a PostgreSQL or MySQL database".to_string(),
    ))
}

/// Spawn a task that calls `run` every `interval` (first right away) until shutdown is
/// signalled on `signal_rx` (or its sender is dropped).
///
/// Each run is spawned as its own task and awaited, so neither the shutdown signal nor
/// aborting the returned handle cancels a run part-way: database work always reaches
/// commit or rollback instead of leaving a pooled connection inside a transaction.
fn spawn_periodic<F, Fut>(
    interval: Duration,
    mut signal_rx: watch::Receiver<bool>,
    run: F,
) -> tokio::task::JoinHandle<()>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval.max(Duration::from_millis(1)));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = signal_rx.changed() => break,
                _ = ticker.tick() => {}
            }
            if *signal_rx.borrow() {
                break;
            }
            if let Err(e) = tokio::spawn(run()).await
                && e.is_panic()
            {
                error!("Periodic maintenance task panicked: {}", e);
            }
        }
    })
}

/// [`spawn_periodic`] for a maintenance pass on the queue of the pool's first worker
/// (`None` when the pool has no workers).
fn spawn_queue_task<DB, F, Fut>(
    workers: &[Worker<DB>],
    interval: Duration,
    signal_rx: watch::Receiver<bool>,
    pass: F,
) -> Option<tokio::task::JoinHandle<()>>
where
    DB: Database + Send + Sync + 'static,
    F: Fn(Arc<JobQueue<DB>>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let queue = Arc::clone(&workers.first()?.queue);
    Some(spawn_periodic(interval, signal_rx, move || {
        pass(Arc::clone(&queue))
    }))
}

impl<DB: Database> Drop for WorkerPool<DB> {
    fn drop(&mut self) {
        // Stop autoscaling task when dropping the pool
        if let Some(task) = &self.autoscale_task {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "postgres")]
    use crate::worker::tests::lazy_worker;

    #[tokio::test]
    async fn spawn_periodic_runs_until_shutdown() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (signal_tx, signal_rx) = watch::channel(false);
        let counter = Arc::clone(&runs);
        let task = spawn_periodic(Duration::from_millis(10), signal_rx, move || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        signal_tx.send_replace(true);
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the loop stops on shutdown")
            .unwrap();
        let seen = runs.load(std::sync::atomic::Ordering::SeqCst);
        assert!(seen >= 2, "ran {seen} times");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), seen);
    }

    /// Shutting down (or aborting the loop, as the pool's supervisor does) while a run is
    /// in flight must not cancel the run: it would drop a database transaction mid-way.
    #[tokio::test]
    async fn spawn_periodic_never_cancels_a_run() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let started = Arc::new(tokio::sync::Notify::new());
        let finished = Arc::new(AtomicBool::new(false));
        let (signal_tx, signal_rx) = watch::channel(false);
        let (run_started, run_finished) = (Arc::clone(&started), Arc::clone(&finished));
        let task = spawn_periodic(Duration::from_secs(3600), signal_rx, move || {
            let (started, finished) = (Arc::clone(&run_started), Arc::clone(&run_finished));
            async move {
                started.notify_one();
                tokio::time::sleep(Duration::from_millis(200)).await;
                finished.store(true, Ordering::SeqCst);
            }
        });
        started.notified().await;
        signal_tx.send_replace(true);
        task.abort();
        assert!(!finished.load(Ordering::SeqCst));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            finished.load(Ordering::SeqCst),
            "the in-flight run completed after the loop was stopped"
        );
    }

    #[cfg(any(feature = "postgres", feature = "mysql"))]
    #[test]
    fn encrypted_job_purge_is_opt_in_and_configurable() {
        #[cfg(feature = "postgres")]
        type Db = sqlx::Postgres;
        #[cfg(all(feature = "mysql", not(feature = "postgres")))]
        type Db = sqlx::MySql;

        let pool = WorkerPool::<Db>::new();
        assert_eq!(pool.encrypted_job_purge_interval, None);
        let pool = pool.with_encrypted_job_purge(Duration::from_secs(30));
        assert_eq!(
            pool.encrypted_job_purge_interval,
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            pool.without_encrypted_job_purge()
                .encrypted_job_purge_interval,
            None
        );
    }

    #[cfg(any(feature = "postgres", feature = "mysql"))]
    #[test]
    fn result_cleanup_is_on_by_default_and_configurable() {
        #[cfg(feature = "postgres")]
        type Db = sqlx::Postgres;
        #[cfg(all(feature = "mysql", not(feature = "postgres")))]
        type Db = sqlx::MySql;

        let pool = WorkerPool::<Db>::new();
        assert_eq!(
            pool.result_cleanup_interval,
            Some(DEFAULT_RESULT_CLEANUP_INTERVAL)
        );
        assert_eq!(DEFAULT_RESULT_CLEANUP_INTERVAL, Duration::from_secs(300));
        let pool = pool.with_result_cleanup(Duration::from_secs(42));
        assert_eq!(pool.result_cleanup_interval, Some(Duration::from_secs(42)));
        assert_eq!(pool.without_result_cleanup().result_cleanup_interval, None);
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn pool_from_config_applies_the_result_cleanup_settings() {
        let mut config = crate::HammerworkConfig::new();
        let pool = WorkerPool::from_hammerwork_config(lazy_worker("r"), &config).unwrap();
        assert_eq!(
            pool.result_cleanup_interval,
            Some(DEFAULT_RESULT_CLEANUP_INTERVAL),
            "on by default"
        );

        config.worker.result_cleanup_interval = Duration::from_secs(60);
        let pool = WorkerPool::from_hammerwork_config(lazy_worker("r"), &config).unwrap();
        assert_eq!(pool.result_cleanup_interval, Some(Duration::from_secs(60)));

        config.worker.result_cleanup_enabled = false;
        let pool = WorkerPool::from_config(lazy_worker("r"), &config.worker);
        assert_eq!(pool.result_cleanup_interval, None);

        config.worker.result_cleanup_enabled = true;
        config.worker.result_cleanup_interval = Duration::ZERO;
        assert!(
            WorkerPool::from_hammerwork_config(lazy_worker("r"), &config).is_err(),
            "a zero interval is rejected"
        );
    }

    #[cfg(all(feature = "postgres", feature = "encryption"))]
    #[tokio::test]
    async fn pool_from_config_schedules_key_rotation_when_enabled() {
        let mut config = crate::HammerworkConfig::new();
        let pool = WorkerPool::from_hammerwork_config(lazy_worker("k"), &config).unwrap();
        assert!(pool.key_rotation.is_none(), "off by default");

        config.encryption.key_rotation.enabled = true;
        config.encryption.key_rotation.check_interval_secs = 90;
        let pool = WorkerPool::from_hammerwork_config(lazy_worker("k"), &config).unwrap();
        let rotation = pool.key_rotation.as_ref().expect("rotation scheduled");
        assert_eq!(rotation.interval, Duration::from_secs(90));
        assert!(pool.without_key_rotation().key_rotation.is_none());

        config.encryption.key_rotation.check_interval_secs = 0;
        assert!(WorkerPool::from_hammerwork_config(lazy_worker("k"), &config).is_err());
    }

    /// A pool whose key manager cannot load its master key fails to start, before any
    /// worker runs.
    #[cfg(all(feature = "postgres", feature = "encryption"))]
    #[tokio::test]
    async fn pool_with_unloadable_master_key_fails_to_start() {
        let mut config = crate::HammerworkConfig::new();
        config.encryption.key_rotation.enabled = true;
        config.encryption.key_rotation.master_key_source =
            crate::config::KeySourceRef::parse("env://HAMMERWORK_TEST_UNSET_MASTER_KEY_7F3A")
                .unwrap();
        let mut pool = WorkerPool::from_hammerwork_config(lazy_worker("k"), &config).unwrap();
        let err = pool.start().await.unwrap_err();
        assert!(
            matches!(err, HammerworkError::Encryption { .. }),
            "unexpected error: {err}"
        );
        assert!(pool.shutdown_signal.is_none(), "no worker was started");
    }

    #[cfg(all(feature = "postgres", not(feature = "encryption")))]
    #[tokio::test]
    async fn key_rotation_without_the_encryption_feature_is_rejected() {
        let mut config = crate::HammerworkConfig::new();
        config.encryption.key_rotation.enabled = true;
        let err = WorkerPool::from_hammerwork_config(lazy_worker("k"), &config)
            .err()
            .expect("rejected");
        assert!(err.to_string().contains("`encryption` feature"), "{err}");
    }

    #[cfg(any(feature = "postgres", feature = "mysql"))]
    #[test]
    fn archival_is_opt_in() {
        #[cfg(feature = "postgres")]
        type Db = sqlx::Postgres;
        #[cfg(all(feature = "mysql", not(feature = "postgres")))]
        type Db = sqlx::MySql;

        let pool = WorkerPool::<Db>::new();
        assert!(pool.archival.is_none());
        let policy = crate::archive::ArchivalPolicy::new().with_batch_size(7);
        let pool = pool.with_archival(
            policy,
            crate::archive::ArchivalConfig::new().with_compression_level(2),
            Duration::from_secs(90),
        );
        let archival = pool.archival.as_ref().unwrap();
        assert_eq!(archival.interval, Duration::from_secs(90));
        assert_eq!(archival.policy.batch_size, 7);
        assert_eq!(archival.config.compression_level, 2);
        assert!(pool.without_archival().archival.is_none());
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn pool_from_config_uses_pool_size_and_autoscaling() {
        let config = crate::config::WorkerConfig {
            pool_size: 3,
            autoscaling_enabled: true,
            min_workers: 2,
            max_workers: 6,
            ..Default::default()
        };
        let pool = WorkerPool::from_config(lazy_worker("p").with_config(&config), &config);
        assert_eq!(pool.workers.len(), 3);
        assert!(pool.worker_template.is_some());
        assert!(pool.autoscale_config.enabled);
        assert_eq!(pool.autoscale_config.min_workers, 2);
        assert_eq!(pool.autoscale_config.max_workers, 6);

        let config = crate::config::WorkerConfig {
            pool_size: 0,
            ..Default::default()
        };
        let pool = WorkerPool::from_config(lazy_worker("p"), &config);
        assert_eq!(pool.workers.len(), 1, "a pool always has a worker");
        assert!(!pool.autoscale_config.enabled);
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn pool_from_hammerwork_config_schedules_purge_and_validates_encryption() {
        let mut config = crate::HammerworkConfig::new();
        config.worker.pool_size = 2;
        config.encryption.purge_interval_secs = Some(90);
        let pool = WorkerPool::from_hammerwork_config(lazy_worker("p"), &config).unwrap();
        assert_eq!(pool.workers.len(), 2);
        assert_eq!(
            pool.encrypted_job_purge_interval,
            Some(Duration::from_secs(90))
        );

        config.encryption.purge_interval_secs = Some(0);
        assert!(
            WorkerPool::from_hammerwork_config(lazy_worker("p"), &config).is_err(),
            "an invalid encryption section is rejected"
        );
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn pool_stats_collector_is_applied_to_workers() {
        use crate::stats::InMemoryStatsCollector;
        let stats: Arc<dyn StatisticsCollector> = Arc::new(InMemoryStatsCollector::new_default());
        let mut pool = WorkerPool::new().with_stats_collector(Arc::clone(&stats));
        pool.add_worker(lazy_worker("s"));
        assert!(Arc::ptr_eq(
            pool.workers[0].stats_collector.as_ref().unwrap(),
            &stats
        ));
        assert!(Arc::ptr_eq(
            pool.stats_collector().as_ref().unwrap(),
            &stats
        ));

        // With autoscaling on, the first worker becomes the template.
        let mut pool = WorkerPool::new().with_autoscaling(AutoscaleConfig::new());
        assert!(pool.worker_template.is_none());
        pool.add_worker(lazy_worker("s"));
        assert!(pool.worker_template.is_some());
        let mut pool = WorkerPool::new().without_autoscaling();
        pool.add_worker(lazy_worker("s"));
        assert!(pool.worker_template.is_none());

        // #64 H9: a plain pool does not autoscale.
        let mut pool = WorkerPool::new();
        assert!(!pool.autoscale_config.enabled);
        pool.add_worker(lazy_worker("s"));
        assert!(pool.worker_template.is_none());
    }

    /// `from_config(..).with_stats_collector(c)` is the natural order: the collector
    /// must reach the workers the pool already has, not only workers added later.
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn stats_collector_set_after_workers_reaches_them() {
        use crate::stats::InMemoryStatsCollector;
        let stats: Arc<dyn StatisticsCollector> = Arc::new(InMemoryStatsCollector::new_default());
        let config = crate::config::WorkerConfig {
            pool_size: 2,
            ..Default::default()
        };
        let pool = WorkerPool::from_config(lazy_worker("late_stats"), &config)
            .with_stats_collector(Arc::clone(&stats));
        assert_eq!(pool.workers.len(), 2);
        for worker in &pool.workers {
            assert!(Arc::ptr_eq(
                worker.stats_collector.as_ref().unwrap(),
                &stats
            ));
        }
    }
}
