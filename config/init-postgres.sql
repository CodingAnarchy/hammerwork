-- PostgreSQL initialization script for the docker-compose test database.
--
-- Nothing to set up here: the database and user come from the POSTGRES_* variables in
-- docker-compose.yml, and the schema is created by the migrations, so it always matches
-- the library:
--
--     cargo hammerwork migration run --database-url postgres://postgres:password@localhost:5432/hammerwork_test
--
-- (a pre-created hammerwork_jobs table would be an outdated copy of the schema).
SELECT 1;
