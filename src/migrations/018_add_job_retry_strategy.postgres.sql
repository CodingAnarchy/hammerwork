-- Migration 018: Persist per-job retry strategies (PostgreSQL)
-- Job::with_retry_strategy was previously lost on enqueue because hammerwork_jobs
-- had no column for it. The strategy is stored in its serde JSON form.

ALTER TABLE hammerwork_jobs
ADD COLUMN IF NOT EXISTS retry_strategy JSONB;

-- Workers check whether a batch still has unfinished jobs every time one of its
-- jobs reaches a terminal status; this index keeps that check cheap for big batches.
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_batch_status
    ON hammerwork_jobs (batch_id, status)
    WHERE batch_id IS NOT NULL;
