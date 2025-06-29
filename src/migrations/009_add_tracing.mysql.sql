-- Migration 009: Add job tracing and correlation support for MySQL
-- Adds distributed tracing fields for job correlation and observability

-- Add tracing fields to support distributed tracing and correlation
ALTER TABLE hammerwork_jobs 
ADD COLUMN trace_id VARCHAR(128) NULL,
ADD COLUMN correlation_id VARCHAR(128) NULL,
ADD COLUMN parent_span_id VARCHAR(128) NULL,
ADD COLUMN span_context TEXT NULL;

-- Index for trace ID lookups (finding all jobs in a trace)
CREATE INDEX idx_hammerwork_jobs_trace_id
    ON hammerwork_jobs (trace_id);

-- Index for correlation ID lookups (finding correlated business operations)
CREATE INDEX idx_hammerwork_jobs_correlation_id
    ON hammerwork_jobs (correlation_id);

-- Composite index for trace and correlation queries
CREATE INDEX idx_hammerwork_jobs_trace_correlation
    ON hammerwork_jobs (trace_id, correlation_id);

-- Index for parent span lookups (hierarchical tracing)
CREATE INDEX idx_hammerwork_jobs_parent_span_id
    ON hammerwork_jobs (parent_span_id);