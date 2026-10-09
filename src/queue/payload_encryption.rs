//! Payload encryption shared by [`JobQueue`](super::JobQueue) and the in-memory
//! `TestQueue`: sealing jobs before they are stored and opening them for a handler.

use crate::{
    HammerworkError, Result,
    job::{Job, JobStatus},
};
#[cfg(feature = "encryption")]
use std::sync::Arc;

/// The encryption engine of a queue and the queues whose jobs it encrypts by default.
#[derive(Clone, Default)]
pub(crate) struct PayloadEncryption {
    #[cfg(feature = "encryption")]
    pub(crate) engine: Option<Arc<crate::encryption::EncryptionEngine>>,
    /// Queues whose jobs are encrypted even without an encryption config (`"*"`: all)
    #[cfg(feature = "encryption")]
    pub(crate) encrypted_queues: Vec<String>,
    /// Refuse to store plaintext jobs on queues that hold encrypted jobs (see
    /// `JobQueue::with_plaintext_guard`)
    pub(crate) plaintext_guard: bool,
}

impl std::fmt::Debug for PayloadEncryption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("PayloadEncryption");
        #[cfg(feature = "encryption")]
        s.field("engine", &self.engine)
            .field("encrypted_queues", &self.encrypted_queues);
        s.field("plaintext_guard", &self.plaintext_guard);
        s.finish()
    }
}

impl PayloadEncryption {
    /// The queues the plaintext guard must check before `jobs` (already sealed) are
    /// stored: the distinct queues of the jobs that stay unencrypted. Empty when the
    /// guard is off.
    #[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
    pub(crate) fn guarded_queues(&self, jobs: &[Job]) -> Vec<String> {
        if !self.plaintext_guard {
            return Vec::new();
        }
        let mut queues: Vec<String> = jobs
            .iter()
            .filter(|job| !job.is_encrypted)
            .map(|job| job.queue_name.clone())
            .collect();
        queues.sort();
        queues.dedup();
        queues
    }

    /// The error for a plaintext job refused by the plaintext guard.
    #[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
    pub(crate) fn plaintext_refused(queue_name: &str) -> HammerworkError {
        HammerworkError::Encryption {
            message: format!(
                "Queue '{}' holds encrypted jobs, but this queue handle would store the new \
                 job in plaintext; refusing. Give it the application's encryption settings \
                 (the [encryption] section and its key), or encrypt the job explicitly",
                queue_name
            ),
        }
    }

    /// Whether jobs on `queue_name` are encrypted without an encryption config.
    #[cfg(feature = "encryption")]
    fn encrypts_queue(&self, queue_name: &str) -> bool {
        self.encrypted_queues
            .iter()
            .any(|queue| queue == "*" || queue == queue_name)
    }

    /// Encrypts the payloads of `jobs` that need it, before they are written. Fails (and
    /// nothing should be written) if a job needs encryption that cannot be provided.
    ///
    /// A job needs encryption when it has an encryption config, or when its queue is one
    /// of `encrypted_queues` (it then gets the engine's algorithm and key id).
    pub(crate) async fn seal(&self, jobs: &mut [Job]) -> Result<()> {
        for job in jobs.iter_mut() {
            if job.is_encrypted {
                continue;
            }
            #[cfg(feature = "encryption")]
            if job.encryption_config.is_none()
                && let Some(engine) = &self.engine
                && self.encrypts_queue(&job.queue_name)
            {
                let config = engine.config();
                let mut job_config =
                    crate::encryption::EncryptionConfig::new(config.algorithm.clone())
                        .with_key_id(engine.key_id());
                job_config.compression_enabled = config.compression_enabled;
                job.encryption_config = Some(job_config);
            }
            if !job.has_encryption() {
                continue;
            }
            #[cfg(feature = "encryption")]
            {
                let engine = self
                    .engine
                    .as_ref()
                    .ok_or_else(|| HammerworkError::Encryption {
                        message: format!(
                            "Job {} has an encryption config but the queue has no encryption \
                             engine; configure one with JobQueue::with_encryption",
                            job.id
                        ),
                    })?;
                crate::encryption::job_payload::seal_job(engine, job).await?;
            }
        }
        Ok(())
    }

    /// Returns `job` with its payload decrypted (see `JobQueue::decrypt_job`).
    pub(crate) async fn open(&self, job: Job) -> Result<Job> {
        if !job.is_encrypted {
            return Ok(job);
        }
        if job.status == JobStatus::Archived && !has_ciphertext(&job) {
            return Err(HammerworkError::Encryption {
                message: format!(
                    "Job {} is archived and its ciphertext stays in the archive; restore it \
                     with restore_archived_job and decrypt the restored job",
                    job.id
                ),
            });
        }
        #[cfg(feature = "encryption")]
        {
            let engine = self
                .engine
                .as_ref()
                .ok_or_else(|| HammerworkError::Encryption {
                    message: format!(
                        "Job {} has an encrypted payload but the queue has no encryption \
                         engine; configure one with JobQueue::with_encryption",
                        job.id
                    ),
                })?;
            let job_id = job.id;
            crate::encryption::job_payload::open_job(engine, job)
                .await
                .map_err(|e| HammerworkError::Encryption {
                    message: format!("Cannot decrypt the payload of job {}: {}", job_id, e),
                })
        }
        #[cfg(not(feature = "encryption"))]
        {
            Err(HammerworkError::Encryption {
                message: format!(
                    "Job {} has an encrypted payload; decrypting it needs the `encryption` \
                     feature and an encryption engine (JobQueue::with_encryption)",
                    job.id
                ),
            })
        }
    }
}

/// Whether `job` carries its ciphertext.
fn has_ciphertext(#[allow(unused_variables)] job: &Job) -> bool {
    #[cfg(feature = "encryption")]
    {
        job.encrypted_payload.is_some()
    }
    #[cfg(not(feature = "encryption"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plaintext_guard_checks_each_unencrypted_queue_once() {
        let mut sealed = Job::new("payments".into(), json!({}));
        sealed.is_encrypted = true;
        let jobs = vec![
            Job::new("emails".into(), json!({})),
            Job::new("emails".into(), json!({})),
            Job::new("audit".into(), json!({})),
            sealed,
        ];
        assert!(
            PayloadEncryption::default()
                .guarded_queues(&jobs)
                .is_empty()
        );
        // Without the `encryption` feature `plaintext_guard` is the only field.
        #[allow(clippy::needless_update)]
        let guard = PayloadEncryption {
            plaintext_guard: true,
            ..Default::default()
        };
        assert_eq!(guard.guarded_queues(&jobs), vec!["audit", "emails"]);
        let message = PayloadEncryption::plaintext_refused("payments").to_string();
        assert!(
            message.contains("'payments' holds encrypted jobs"),
            "{message}"
        );
        assert!(format!("{guard:?}").contains("plaintext_guard: true"));
    }
}
