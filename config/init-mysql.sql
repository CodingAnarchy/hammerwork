-- MySQL initialization script for Hammerwork job queue
-- This script sets up the database with proper permissions and indexes

-- Use the hammerwork_test database
USE hammerwork_test;

-- Create the hammerwork_jobs table if it doesn't exist
-- This matches the schema from src/queue.rs mysql implementation
CREATE TABLE IF NOT EXISTS hammerwork_jobs (
    id CHAR(36) PRIMARY KEY,
    queue_name VARCHAR(255) NOT NULL,
    payload JSON NOT NULL,
    status VARCHAR(50) NOT NULL,
    attempts INT NOT NULL DEFAULT 0,
    max_attempts INT NOT NULL DEFAULT 3,
    created_at TIMESTAMP(6) NOT NULL,
    scheduled_at TIMESTAMP(6) NOT NULL,
    started_at TIMESTAMP(6) NULL,
    completed_at TIMESTAMP(6) NULL,
    error_message TEXT NULL,
    INDEX idx_queue_status_scheduled (queue_name, status, scheduled_at),
    INDEX idx_created_at (created_at),
    INDEX idx_status (status)
);

-- Grant necessary permissions to the hammerwork user
GRANT ALL PRIVILEGES ON hammerwork_test.* TO 'hammerwork'@'%';
GRANT ALL PRIVILEGES ON hammerwork_test.* TO 'hammerwork'@'localhost';
FLUSH PRIVILEGES;

-- Insert some initial test data for validation
INSERT IGNORE INTO hammerwork_jobs (
    id, 
    queue_name, 
    payload, 
    status, 
    attempts, 
    max_attempts, 
    created_at, 
    scheduled_at
) VALUES (
    UUID(),
    'test_queue',
    JSON_OBJECT('message', 'Initial test job', 'type', 'validation'),
    'Pending',
    0,
    3,
    NOW(6),
    NOW(6)
);

-- Log successful initialization
SELECT 'Hammerwork MySQL database initialized successfully' AS message;