-- PostgreSQL initialization script for Hammerwork job queue
-- This script sets up the database with proper permissions and indexes

-- Create the hammerwork_jobs table if it doesn't exist
-- This matches the schema from src/queue.rs postgres implementation
CREATE TABLE IF NOT EXISTS hammerwork_jobs (
    id UUID PRIMARY KEY,
    queue_name VARCHAR NOT NULL,
    payload JSONB NOT NULL,
    status VARCHAR NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    created_at TIMESTAMPTZ NOT NULL,
    scheduled_at TIMESTAMPTZ NOT NULL,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    error_message TEXT
);

-- Create optimized indexes for job queue polling
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_queue_status_scheduled 
ON hammerwork_jobs (queue_name, status, scheduled_at);

-- Additional indexes for monitoring and cleanup
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_created_at 
ON hammerwork_jobs (created_at);

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_status 
ON hammerwork_jobs (status);

-- Grant necessary permissions
GRANT ALL PRIVILEGES ON TABLE hammerwork_jobs TO postgres;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO postgres;

-- Insert some initial test data for validation
INSERT INTO hammerwork_jobs (
    id, 
    queue_name, 
    payload, 
    status, 
    attempts, 
    max_attempts, 
    created_at, 
    scheduled_at
) VALUES (
    gen_random_uuid(),
    'test_queue',
    '{"message": "Initial test job", "type": "validation"}',
    'Pending',
    0,
    3,
    NOW(),
    NOW()
) ON CONFLICT DO NOTHING;

-- Log successful initialization
DO $$
BEGIN
    RAISE NOTICE 'Hammerwork PostgreSQL database initialized successfully';
END $$;