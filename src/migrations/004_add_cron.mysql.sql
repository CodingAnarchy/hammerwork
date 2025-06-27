-- Migration 004: Add cron scheduling for MySQL
-- Adds recurring job support with cron expressions and timezone awareness

-- Add cron scheduling fields
ALTER TABLE hammerwork_jobs 
ADD COLUMN cron_schedule VARCHAR(100),
ADD COLUMN next_run_at TIMESTAMP(6),
ADD COLUMN recurring BOOLEAN NOT NULL DEFAULT FALSE,
ADD COLUMN timezone VARCHAR(50);

-- Create indexes for cron job queries
CREATE INDEX idx_recurring_next_run ON hammerwork_jobs (recurring, next_run_at);
CREATE INDEX idx_cron_schedule ON hammerwork_jobs (cron_schedule);