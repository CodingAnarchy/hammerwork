-- Migration 004: Add cron scheduling for PostgreSQL
-- Adds recurring job support with cron expressions and timezone awareness

-- Add cron scheduling fields
ALTER TABLE hammerwork_jobs 
ADD COLUMN IF NOT EXISTS cron_schedule VARCHAR(100),
ADD COLUMN IF NOT EXISTS next_run_at TIMESTAMPTZ,
ADD COLUMN IF NOT EXISTS recurring BOOLEAN NOT NULL DEFAULT FALSE,
ADD COLUMN IF NOT EXISTS timezone VARCHAR(50);

-- Create indexes for cron job queries
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_recurring_next_run
    ON hammerwork_jobs (recurring, next_run_at) WHERE recurring = TRUE;

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_cron_schedule
    ON hammerwork_jobs (cron_schedule) WHERE cron_schedule IS NOT NULL;