-- Migration 021: Store MySQL times as DATETIME(6) instead of TIMESTAMP (PostgreSQL)
-- Nothing to do: PostgreSQL's TIMESTAMPTZ columns have no year-2038 limit. The MySQL
-- migration of the same name converts the MySQL TIMESTAMP columns to DATETIME(6).

SELECT 1;
