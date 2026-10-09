//! End-to-end tests that run the real `cargo-hammerwork` binary and check its exit status
//! and output: argument handling, configuration loading, logging flags, and a few
//! database-backed commands on both backends.

use std::path::Path;
use std::process::{Command, Output};

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cargo-hammerwork"));
    // Isolate from the developer's own configuration and environment.
    cmd.env_remove("DATABASE_URL")
        .env_remove("HAMMERWORK_DEFAULT_QUEUE")
        .env_remove("HAMMERWORK_DEFAULT_LIMIT")
        .env_remove("HAMMERWORK_LOG_LEVEL")
        .env_remove("HAMMERWORK_POOL_SIZE")
        .env_remove("RUST_LOG")
        .env("NO_COLOR", "1");
    cmd
}

/// A command whose configuration and webhook files live in `dir`.
fn bin_in(dir: &Path) -> Command {
    let mut cmd = bin();
    cmd.env("HAMMERWORK_CONFIG", dir.join("config.toml"))
        .env("HAMMERWORK_WEBHOOKS_FILE", dir.join("webhooks.json"));
    cmd
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn run(cmd: &mut Command) -> (i32, String) {
    let output = cmd.output().expect("binary runs");
    (output.status.code().unwrap_or(-1), text(&output))
}

#[test]
fn help_and_version_describe_the_tool() {
    let (code, out) = run(bin().arg("--help"));
    assert_eq!(code, 0);
    for command in [
        "migration",
        "config",
        "job",
        "worker",
        "queue",
        "monitor",
        "backup",
        "batch",
        "cron",
        "maintenance",
        "workflow",
        "archive",
        "spawn",
        "webhook",
    ] {
        assert!(out.contains(command), "{command} listed in help:\n{out}");
    }
    assert!(out.contains("--verbose") && out.contains("--quiet"));

    let (code, out) = run(bin().arg("--version"));
    assert_eq!(code, 0);
    assert!(out.contains(env!("CARGO_PKG_VERSION")), "{out}");
}

#[test]
fn every_subcommand_has_help() {
    for sub in [
        "migration",
        "config",
        "job",
        "worker",
        "queue",
        "monitor",
        "backup",
        "batch",
        "cron",
        "maintenance",
        "workflow",
        "archive",
        "spawn",
        "webhook",
    ] {
        let (code, out) = run(bin().args([sub, "--help"]));
        assert_eq!(code, 0, "{sub}: {out}");
        assert!(out.contains("Usage"), "{sub}: {out}");
    }
}

#[test]
fn unknown_commands_and_missing_arguments_are_usage_errors() {
    let (code, out) = run(bin().arg("frobnicate"));
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("unrecognized subcommand"), "{out}");

    let (code, _) = run(bin().args(["job", "enqueue"]));
    assert_eq!(code, 2, "--queue and --payload are required");

    let (code, _) = run(&mut bin());
    assert_eq!(code, 2, "a subcommand is required");
}

#[test]
fn invoked_as_a_cargo_subcommand() {
    // `cargo hammerwork <args>` runs `cargo-hammerwork hammerwork <args>`.
    let dir = tempfile::tempdir().unwrap();
    let (code, out) = run(bin_in(dir.path()).args(["hammerwork", "config", "path"]));
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("config.toml"), "{out}");
}

#[test]
fn config_commands_read_and_write_the_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config.toml");

    let (code, out) = run(bin_in(dir.path()).args(["config", "path"]));
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(file.to_str().unwrap()), "{out}");
    assert!(out.contains("File does not exist"), "{out}");

    let (code, out) = run(bin_in(dir.path()).args(["config", "set", "default_queue", "emails"]));
    assert_eq!(code, 0, "{out}");
    assert!(file.exists());
    assert!(std::fs::read_to_string(&file).unwrap().contains("emails"));

    let (code, out) = run(bin_in(dir.path()).args(["config", "get", "default_queue"]));
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "emails");

    // The environment overrides the file.
    let (code, out) = run(bin_in(dir.path())
        .env("HAMMERWORK_DEFAULT_QUEUE", "from_env")
        .args(["config", "get", "default_queue"]));
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "from_env");

    let (code, out) = run(bin_in(dir.path()).args(["config", "show"]));
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("default_queue") && out.contains("emails"),
        "{out}"
    );

    let (code, out) = run(bin_in(dir.path()).args(["config", "path"]));
    assert_eq!(code, 0);
    assert!(out.contains("File exists"), "{out}");

    // Invalid values fail with exit code 1 and a message, and change nothing.
    let (code, out) = run(bin_in(dir.path()).args(["config", "set", "log_level", "loud"]));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("log_level must be one of"), "{out}");
    let (_, out) = run(bin_in(dir.path()).args(["config", "get", "log_level"]));
    assert_eq!(out.trim(), "info");

    let (code, out) = run(bin_in(dir.path()).args(["config", "reset"]));
    assert_eq!(code, 0);
    assert!(out.contains("--confirm"), "{out}");
    let (code, _) = run(bin_in(dir.path()).args(["config", "reset", "--confirm"]));
    assert_eq!(code, 0);
    let (_, out) = run(bin_in(dir.path()).args(["config", "get", "default_queue"]));
    assert_eq!(out.trim(), "Not set");
}

#[test]
fn a_corrupt_config_file_warns_and_falls_back_to_defaults() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "default_limit = \"many\"").unwrap();

    let (code, out) = run(bin_in(dir.path()).args(["config", "get", "default_limit"]));
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Could not load config"), "{out}");
    assert!(out.contains("Invalid config file"), "{out}");
    assert_eq!(out.lines().next(), Some("50"), "defaults are used: {out}");

    // --quiet suppresses the warning.
    let (code, out) = run(bin_in(dir.path()).args(["-q", "config", "get", "default_limit"]));
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "50");
}

#[test]
fn commands_that_need_a_database_say_so() {
    let dir = tempfile::tempdir().unwrap();
    let (code, out) = run(bin_in(dir.path()).args(["queue", "list"]));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("Database URL is required"), "{out}");

    let (code, out) = run(bin_in(dir.path()).args(["queue", "list", "-d", "sqlite://x.db"]));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("Unsupported database URL"), "{out}");
}

#[test]
fn webhook_registry_commands_work_without_a_database() {
    let dir = tempfile::tempdir().unwrap();
    let (code, out) = run(bin_in(dir.path()).args([
        "webhook",
        "add",
        "-n",
        "ci",
        "-u",
        "http://localhost:9/hook",
        "--events",
        "failed",
    ]));
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("added successfully"), "{out}");

    let (code, out) = run(bin_in(dir.path()).args(["webhook", "list"]));
    assert_eq!(code, 0);
    assert!(
        out.contains("ci") && out.contains("http://localhost:9/hook") && out.contains("Failed"),
        "{out}"
    );

    let (code, out) = run(bin_in(dir.path()).args(["webhook", "remove", "-w", "ci"]));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("--confirm"), "{out}");
    let (code, _) = run(bin_in(dir.path()).args(["webhook", "remove", "-w", "ci", "--confirm"]));
    assert_eq!(code, 0);
}

#[test]
fn logging_flags_control_what_is_printed() {
    let dir = tempfile::tempdir().unwrap();
    // `webhook add` reports success through the logger.
    let add = |flags: &[&str], name: &str| {
        let mut cmd = bin_in(dir.path());
        cmd.args(flags);
        cmd.args(["webhook", "add", "-n", name, "-u", "http://localhost:9/x"]);
        run(&mut cmd)
    };
    let (code, out) = add(&[], "a");
    assert_eq!(code, 0);
    assert!(
        out.contains("added successfully"),
        "info is shown by default: {out}"
    );

    let (code, out) = add(&["-q"], "b");
    assert_eq!(code, 0);
    assert!(
        !out.contains("added successfully"),
        "--quiet hides info: {out}"
    );

    let (code, out) = add(&["-v"], "c");
    assert_eq!(code, 0);
    assert!(
        out.contains("Command completed successfully"),
        "--verbose adds detail: {out}"
    );
}

// ---- database-backed commands ----

fn db_flow(url: &str) {
    let dir = tempfile::tempdir().unwrap();
    let queue = format!("e2e_{}", uuid::Uuid::new_v4().simple());
    let hostile = format!("it's; DROP TABLE x -- {}", uuid::Uuid::new_v4().simple());
    let cli = |args: &[&str]| {
        let mut cmd = bin_in(dir.path());
        cmd.env("DATABASE_URL", url);
        cmd.args(args);
        run(&mut cmd)
    };

    let (code, out) = cli(&["migration", "status"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("tables exist"), "{out}");
    assert!(out.contains("queue_name"), "{out}");

    // enqueue, list and show
    let (code, out) = cli(&[
        "job",
        "enqueue",
        "-n",
        &queue,
        "-j",
        r#"{"to": "a@example.com"}"#,
        "-r",
        "high",
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Job enqueued successfully"), "{out}");
    let job_id = out
        .split("Job enqueued successfully: ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("job id in output")
        .trim()
        .to_string();
    let (code, out) = cli(&["job", "enqueue", "-n", &hostile, "-j", "{}"]);
    assert_eq!(code, 0, "{out}");

    let (code, out) = cli(&["job", "list", "-n", &queue]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(&queue) && out.contains("Pending") && out.contains("high"),
        "{out}"
    );
    assert!(out.contains(&job_id[..8]), "{out}");

    let (code, out) = cli(&["job", "show", &job_id]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Job Details") && out.contains(&queue) && out.contains("a@example.com"),
        "{out}"
    );
    assert!(out.contains("Priority: high"), "{out}");

    let (code, out) = cli(&["job", "show", &uuid::Uuid::new_v4().to_string()]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("Job not found"), "{out}");

    // queue commands and their output
    let (code, out) = cli(&["queue", "stats", "-n", &queue]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Queue Statistics")
            && out.contains("Pending")
            && out.contains("Total jobs: 1"),
        "{out}"
    );
    let (code, out) = cli(&["queue", "stats", "-n", &hostile, "--detailed"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Detailed Queue Statistics") && out.contains("normal"),
        "{out}"
    );

    let (code, out) = cli(&["queue", "pause", "-n", &queue]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("has been paused"), "{out}");
    let (code, out) = cli(&["queue", "paused"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(&queue) && out.contains("cli"), "{out}");
    let (code, out) = cli(&["queue", "list"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(&queue) && out.contains("Paused"), "{out}");
    let (code, out) = cli(&["queue", "resume", "-n", &queue]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("has been resumed"), "{out}");
    let (_, out) = cli(&["queue", "resume", "-n", &queue]);
    assert!(out.contains("was not paused"), "{out}");

    let (code, out) = cli(&["queue", "health", "-n", &queue]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Queue Health") && out.contains("Total Jobs"),
        "{out}"
    );

    let (code, out) = cli(&["monitor", "health", "--format", "json"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("\"checks\"") && out.contains("Database"),
        "{out}"
    );
    let (code, out) = cli(&["monitor", "metrics", "-n", &queue, "-t", "1h"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("Throughput") && out.contains("Total Jobs: 1"),
        "{out}"
    );

    // clear: refuses without --confirm, then removes only that queue
    let (code, out) = cli(&["queue", "clear", "-n", &queue]);
    assert_eq!(code, 0);
    assert!(out.contains("--confirm"), "{out}");
    let (code, out) = cli(&["queue", "clear", "-n", &queue, "--confirm"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Cleared 1 all jobs"), "{out}");
    let (code, out) = cli(&["queue", "clear", "-n", &hostile, "--confirm"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Cleared 1"), "{out}");
}

#[test]
#[ignore = "requires DATABASE_URL (PostgreSQL)"]
fn test_database_commands_end_to_end_postgres() {
    db_flow(&std::env::var("DATABASE_URL").expect("DATABASE_URL"));
}

#[test]
#[ignore = "requires MYSQL_DATABASE_URL"]
fn test_database_commands_end_to_end_mysql() {
    db_flow(&std::env::var("MYSQL_DATABASE_URL").expect("MYSQL_DATABASE_URL"));
}
