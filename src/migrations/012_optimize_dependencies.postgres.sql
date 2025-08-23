-- Migration 012: Optimize job dependencies using native PostgreSQL arrays
-- Converts JSONB dependency arrays to native UUID[] arrays for better performance
-- This migration is wrapped in a transaction for safety

BEGIN;

-- Step 1: Add new UUID array columns
ALTER TABLE hammerwork_jobs 
ADD COLUMN IF NOT EXISTS depends_on_array UUID[] DEFAULT '{}';

ALTER TABLE hammerwork_jobs 
ADD COLUMN IF NOT EXISTS dependents_array UUID[] DEFAULT '{}';

-- Step 2: Migrate existing JSONB data to UUID arrays with validation
-- Handle depends_on column with UUID validation
UPDATE hammerwork_jobs 
SET depends_on_array = CASE 
    WHEN depends_on IS NULL OR depends_on = 'null'::jsonb OR depends_on = '[]'::jsonb THEN '{}'::UUID[]
    WHEN jsonb_typeof(depends_on) = 'array' THEN 
        ARRAY(
            SELECT elem::UUID 
            FROM jsonb_array_elements_text(depends_on) AS elem
            WHERE elem::text ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
        )
    ELSE '{}'::UUID[]
END;

-- Handle dependents column with UUID validation
UPDATE hammerwork_jobs 
SET dependents_array = CASE 
    WHEN dependents IS NULL OR dependents = 'null'::jsonb OR dependents = '[]'::jsonb THEN '{}'::UUID[]
    WHEN jsonb_typeof(dependents) = 'array' THEN 
        ARRAY(
            SELECT elem::UUID 
            FROM jsonb_array_elements_text(dependents) AS elem
            WHERE elem::text ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
        )
    ELSE '{}'::UUID[]
END;

-- Step 3: Verify data migration integrity
DO $$
DECLARE
    unmigrated_depends_on INTEGER;
    unmigrated_dependents INTEGER;
BEGIN
    -- Check for any non-empty JSONB arrays that didn't migrate
    SELECT COUNT(*) INTO unmigrated_depends_on
    FROM hammerwork_jobs 
    WHERE depends_on IS NOT NULL 
        AND depends_on != 'null'::jsonb 
        AND depends_on != '[]'::jsonb
        AND jsonb_typeof(depends_on) = 'array'
        AND jsonb_array_length(depends_on) > 0
        AND array_length(depends_on_array, 1) IS NULL;
    
    SELECT COUNT(*) INTO unmigrated_dependents
    FROM hammerwork_jobs 
    WHERE dependents IS NOT NULL 
        AND dependents != 'null'::jsonb 
        AND dependents != '[]'::jsonb
        AND jsonb_typeof(dependents) = 'array'
        AND jsonb_array_length(dependents) > 0
        AND array_length(dependents_array, 1) IS NULL;
    
    IF unmigrated_depends_on > 0 OR unmigrated_dependents > 0 THEN
        RAISE EXCEPTION 'Data migration failed: % depends_on and % dependents records were not migrated', 
            unmigrated_depends_on, unmigrated_dependents;
    END IF;
END $$;

-- Step 4: Create indexes on new array columns (before dropping old ones)
CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_depends_on_array
    ON hammerwork_jobs USING GIN (depends_on_array);

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_dependents_array
    ON hammerwork_jobs USING GIN (dependents_array);

-- Step 5: Drop old JSONB indexes (will be recreated after column rename)
DROP INDEX IF EXISTS idx_hammerwork_jobs_depends_on;
DROP INDEX IF EXISTS idx_hammerwork_jobs_dependents;

-- Step 6: Drop old JSONB columns and rename array columns
ALTER TABLE hammerwork_jobs DROP COLUMN IF EXISTS depends_on;
ALTER TABLE hammerwork_jobs DROP COLUMN IF EXISTS dependents;

ALTER TABLE hammerwork_jobs RENAME COLUMN depends_on_array TO depends_on;
ALTER TABLE hammerwork_jobs RENAME COLUMN dependents_array TO dependents;

-- Step 7: Recreate indexes with original names
DROP INDEX IF EXISTS idx_hammerwork_jobs_depends_on_array;
DROP INDEX IF EXISTS idx_hammerwork_jobs_dependents_array;

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_depends_on
    ON hammerwork_jobs USING GIN (depends_on);

CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_dependents
    ON hammerwork_jobs USING GIN (dependents);

-- Step 8: Update comments to reflect new column types
COMMENT ON COLUMN hammerwork_jobs.depends_on IS 'Array of job IDs this job depends on (native UUID array)';
COMMENT ON COLUMN hammerwork_jobs.dependents IS 'Cached array of job IDs that depend on this job (native UUID array)';

-- Step 9: Add constraint to ensure reasonable array sizes (prevent abuse)
ALTER TABLE hammerwork_jobs 
ADD CONSTRAINT chk_depends_on_size 
CHECK (array_length(depends_on, 1) IS NULL OR array_length(depends_on, 1) <= 1000);

ALTER TABLE hammerwork_jobs 
ADD CONSTRAINT chk_dependents_size 
CHECK (array_length(dependents, 1) IS NULL OR array_length(dependents, 1) <= 10000);

COMMIT;