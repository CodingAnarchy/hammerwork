//! Runs the real `hammerwork-web` binary: argument handling and, against each database
//! backend, a complete start-up that serves HTTP and shuts down cleanly on SIGINT.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hammerwork-web"));
    cmd.env_remove("DATABASE_URL")
        .env_remove("RUST_LOG")
        .env("NO_COLOR", "1");
    cmd
}

fn run(cmd: &mut Command) -> (i32, String) {
    let output = cmd.output().expect("binary runs");
    (
        output.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[test]
fn help_and_version() {
    let (code, out) = run(bin().arg("--help"));
    assert_eq!(code, 0);
    for flag in [
        "--config",
        "--database-url",
        "--bind",
        "--port",
        "--static-dir",
        "--cors",
        "--auth",
        "--username",
        "--password",
        "--password-file",
    ] {
        assert!(out.contains(flag), "{flag} in help:\n{out}");
    }
    let (code, out) = run(bin().arg("--version"));
    assert_eq!(code, 0);
    assert!(out.contains(env!("CARGO_PKG_VERSION")), "{out}");
}

#[test]
fn bad_arguments_are_usage_errors() {
    let (code, out) = run(bin().args(["--port", "99999"]));
    assert_eq!(code, 2, "{out}");
    let (code, out) = run(bin().arg("--frobnicate"));
    assert_eq!(code, 2, "{out}");
}

#[test]
fn startup_errors_exit_with_status_1_and_a_message() {
    let (code, out) = run(bin().args(["--config", "/no/such/file.toml"]));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("/no/such/file.toml"), "{out}");

    let (code, out) = run(bin().args(["--auth"]));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("no password provided"), "{out}");

    let port = free_port().to_string();
    let (code, out) = run(bin().args(["-d", "sqlite://x.db", "-p", &port, "--static-dir", "."]));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("Unsupported database URL"), "{out}");
}

/// A running dashboard process, stopped with SIGINT (so it exits normally and flushes
/// coverage data) when dropped.
struct Server {
    child: Child,
    port: u16,
}

impl Server {
    fn start(args: &[&str], port: u16) -> Self {
        let child = bin()
            .args(args)
            .args(["-p", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        Self { child, port }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    async fn wait_until_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!("the dashboard exited early: {status}");
            }
            if reqwest::get(self.url("/health")).await.is_ok() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the dashboard never became ready"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// SIGINT, then wait for a clean exit.
    fn stop(mut self) -> std::process::ExitStatus {
        let _ = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("the dashboard did not stop on SIGINT");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

async fn serve_and_query(database_url: &str, database_name: &str) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<html>dashboard</html>").unwrap();
    let static_dir = dir.path().to_str().unwrap();

    // --- open dashboard
    let mut server = Server::start(
        &["-d", database_url, "--static-dir", static_dir],
        free_port(),
    );
    server.wait_until_ready().await;

    let health: serde_json::Value = reqwest::get(server.url("/health"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["status"], "healthy");
    let page = reqwest::get(server.url("/"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("dashboard"));
    let info: serde_json::Value = reqwest::get(server.url("/api/system/info"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        info["data"]["database_info"]["database_type"],
        database_name
    );
    assert_eq!(info["data"]["database_info"]["connection_health"], true);
    let queues = reqwest::get(server.url("/api/queues")).await.unwrap();
    assert_eq!(queues.status(), 200);
    let unknown = reqwest::get(server.url("/api/nothing")).await.unwrap();
    assert_eq!(unknown.status(), 404);

    // A job posted over HTTP shows up in the listing.
    let queue = format!("binary_{}", uuid::Uuid::new_v4().simple());
    let client = reqwest::Client::new();
    let created: serde_json::Value = client
        .post(server.url("/api/jobs"))
        .json(&serde_json::json!({"queue_name": queue, "payload": {"hello": "world"}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let job_id = created["data"]["job_id"].as_str().unwrap().to_string();
    let listing: serde_json::Value = reqwest::get(server.url(&format!("/api/jobs?queue={queue}")))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listing["data"]["items"][0]["id"], job_id.as_str());
    client
        .post(server.url(&format!("/api/jobs/{job_id}/actions")))
        .json(&serde_json::json!({"action": "delete"}))
        .send()
        .await
        .unwrap();

    let status = server.stop();
    assert!(status.success(), "clean shutdown on SIGINT: {status}");

    // --- protected dashboard, configured through a password file
    let hash = {
        #[cfg(feature = "auth")]
        {
            bcrypt::hash("open-sesame", 4).unwrap()
        }
        #[cfg(not(feature = "auth"))]
        {
            "open-sesame".to_string()
        }
    };
    let hash_file = dir.path().join("hash.txt");
    std::fs::write(&hash_file, format!("{hash}\n")).unwrap();
    let mut server = Server::start(
        &[
            "-d",
            database_url,
            "--static-dir",
            static_dir,
            "--cors",
            "--auth",
            "--username",
            "ops",
            "--password-file",
            hash_file.to_str().unwrap(),
        ],
        free_port(),
    );
    server.wait_until_ready().await;
    assert_eq!(
        reqwest::get(server.url("/health")).await.unwrap().status(),
        200
    );
    assert_eq!(
        reqwest::get(server.url("/api/queues"))
            .await
            .unwrap()
            .status(),
        401
    );
    let denied = client
        .get(server.url("/api/queues"))
        .basic_auth("ops", Some("wrong"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);
    let allowed = client
        .get(server.url("/api/queues"))
        .basic_auth("ops", Some("open-sesame"))
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);
    // CORS is on: a cross-origin preflight is answered.
    let preflight = client
        .request(reqwest::Method::OPTIONS, server.url("/api/queues"))
        .header("origin", "http://example.com")
        .header("access-control-request-method", "GET")
        .send()
        .await
        .unwrap();
    assert!(
        preflight
            .headers()
            .contains_key("access-control-allow-origin")
    );
    assert!(server.stop().success());
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires DATABASE_URL (PostgreSQL)"]
async fn test_binary_serves_postgres() {
    serve_and_query(
        &std::env::var("DATABASE_URL").expect("DATABASE_URL"),
        "PostgreSQL",
    )
    .await;
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires MYSQL_DATABASE_URL"]
async fn test_binary_serves_mysql() {
    serve_and_query(
        &std::env::var("MYSQL_DATABASE_URL").expect("MYSQL_DATABASE_URL"),
        "MySQL",
    )
    .await;
}
