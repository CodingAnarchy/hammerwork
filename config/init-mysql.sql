-- MySQL initialization script for the docker-compose test database.
--
-- Only the database user's permissions are set up here. The schema is created by the
-- migrations, so it always matches the library:
--
--     cargo hammerwork migration run --database-url mysql://hammerwork:password@localhost:3306/hammerwork_test
--
-- (a pre-created hammerwork_jobs table would be an outdated copy of the schema).

GRANT ALL PRIVILEGES ON hammerwork_test.* TO 'hammerwork'@'%';
GRANT ALL PRIVILEGES ON hammerwork_test.* TO 'hammerwork'@'localhost';
FLUSH PRIVILEGES;
