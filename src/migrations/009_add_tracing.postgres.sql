-- Migration 009: Add job tracing and correlation support for PostgreSQL
-- Adds distributed tracing fields for job correlation and observability

-- Add tracing fields to support distributed tracing and correlation
ALTER TABLE hammerwork_jobs 
ADD COLUMN IF NOT EXISTS trace_id VARCHAR(128),
ADD COLUMN IF NOT EXISTS correlation_id VARCHAR(128),
ADD COLUMN IF NOT EXISTS parent_span_id VARCHAR(128),
ADD COLUMN IF NOT EXISTS span_context TEXT;

-- Index for trace ID lookups (finding all jobs in a trace)
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_trace_id
    ON hammerwork_jobs (trace_id) WHERE trace_id IS NOT NULL;

-- Index for correlation ID lookups (finding correlated business operations)
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_correlation_id
    ON hammerwork_jobs (correlation_id) WHERE correlation_id IS NOT NULL;

-- Composite index for trace and correlation queries
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_trace_correlation
    ON hammerwork_jobs (trace_id, correlation_id) 
    WHERE trace_id IS NOT NULL OR correlation_id IS NOT NULL;

-- Index for parent span lookups (hierarchical tracing)
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_parent_span_id
    ON hammerwork_jobs (parent_span_id) WHERE parent_span_id IS NOT NULL;