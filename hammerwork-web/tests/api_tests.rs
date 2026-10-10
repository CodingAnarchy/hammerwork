//! HTTP-level tests of the dashboard: the complete route tree (`/health`, `/api/...`, `/ws`,
//! static files) served over a real database, once per backend, with authentication on and
//! off. Every endpoint is exercised for its success and its error paths.

use hammerwork::archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason};
use hammerwork::queue::DatabaseQueue;
use hammerwork::{Job, JobPriority, JobQueue};
use hammerwork_web::api::history::JobHistory;
use hammerwork_web::api::system::SystemState;
use hammerwork_web::auth::AuthState;
use hammerwork_web::config::{AuthConfig, DashboardConfig};
use hammerwork_web::security::AllowedOrigins;
use hammerwork_web::server::app_routes;
use hammerwork_web::websocket::WebSocketState;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::RwLock;
use warp::Filter;

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

/// A test server: the full route tree for `queue`.
struct App<Q: JobHistory + 'static> {
    queue: Arc<Q>,
    routes: Box<dyn Fn() -> Router + Send + Sync>,
    _static_dir: tempfile::TempDir,
}

type Router = warp::filters::BoxedFilter<(Box<dyn warp::Reply>,)>;

impl<Q: JobHistory + 'static> App<Q> {
    fn new(queue: Arc<Q>, auth: AuthConfig, database_type: &str) -> Self {
        let static_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            static_dir.path().join("index.html"),
            "<html><body>SPA</body></html>",
        )
        .unwrap();
        std::fs::create_dir(static_dir.path().join("assets")).unwrap();
        std::fs::write(static_dir.path().join("asset.txt"), "static file").unwrap();

        let config = DashboardConfig {
            static_dir: static_dir.path().to_path_buf(),
            auth: auth.clone(),
            ..DashboardConfig::default()
        };
        let system_state = Arc::new(RwLock::new(SystemState::new(
            config.clone(),
            database_type.to_string(),
            config.pool_size,
        )));
        let websocket_state = Arc::new(RwLock::new(WebSocketState::new(config.websocket.clone())));
        let auth_state = AuthState::new(auth);
        let dir = static_dir.path().to_path_buf();
        let q = queue.clone();
        let routes = move || -> Router {
            app_routes(
                q.clone(),
                auth_state.clone(),
                system_state.clone(),
                websocket_state.clone(),
                dir.clone(),
                AllowedOrigins::new(["https://ops.example.com"]).unwrap(),
            )
            .map(|reply| Box::new(reply) as Box<dyn warp::Reply>)
            .boxed()
        };
        Self {
            queue,
            routes: Box::new(routes),
            _static_dir: static_dir,
        }
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        auth: Option<&str>,
    ) -> (u16, String, Value) {
        let mut request = warp::test::request().method(method).path(path);
        if let Some(body) = body {
            request = request.json(&body);
        }
        if let Some(auth) = auth {
            request = request.header("authorization", auth);
        }
        let response = request.reply(&(self.routes)()).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        let json = serde_json::from_str(&text).unwrap_or(Value::Null);
        (status, text, json)
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let (status, _, json) = self.call("GET", path, None, None).await;
        (status, json)
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let (status, _, json) = self.call("POST", path, Some(body), None).await;
        (status, json)
    }

    async fn delete(&self, path: &str, body: Value) -> (u16, Value) {
        let (status, _, json) = self.call("DELETE", path, Some(body), None).await;
        (status, json)
    }
}

fn job_ids(listing: &Value) -> Vec<String> {
    listing["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["id"].as_str().unwrap().to_string())
        .collect()
}

/// Enqueue a job in `queue_name` and return its id.
async fn enqueue<Q: DatabaseQueue>(queue: &Q, queue_name: &str, payload: Value) -> uuid::Uuid {
    queue
        .enqueue(Job::new(queue_name.to_string(), payload))
        .await
        .unwrap()
}

async fn health_and_static_files<Q: JobHistory + 'static>(app: &App<Q>) {
    let (status, body) = app.get("/health").await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "healthy");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));

    // The single-page app is served at the root, for client-side routes and under /static.
    for path in ["/", "/queues/some-queue", "/jobs"] {
        let (status, text, _) = app.call("GET", path, None, None).await;
        assert_eq!(status, 200, "{path}");
        assert!(text.contains("SPA"), "{path}: {text}");
    }
    let (status, text, _) = app.call("GET", "/static/asset.txt", None, None).await;
    assert_eq!((status, text.as_str()), (200, "static file"));
    let (status, _, _) = app.call("GET", "/static/missing.txt", None, None).await;
    assert!(
        status == 404 || status == 200,
        "missing static files fall back or 404"
    );

    // API paths never fall back to the HTML page.
    let (status, text, json) = app
        .call("GET", "/api/definitely/not/here", None, None)
        .await;
    assert_eq!(status, 404, "{text}");
    assert!(
        json["error"].as_str().unwrap().contains("not found"),
        "{text}"
    );
    let (status, _, _) = app.call("GET", "/api", None, None).await;
    assert_eq!(status, 404);
}

async fn job_endpoints<Q: JobHistory + 'static>(app: &App<Q>) {
    let q = unique("jobs");
    let token = unique("needle");

    // --- create
    let (status, body) = app
        .post(
            "/api/jobs",
            json!({
                "queue_name": q,
                "payload": {"token": token, "n": 1},
                "priority": "high",
                "max_attempts": 5,
                "trace_id": "trace-1",
                "correlation_id": "corr-1"
            }),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["message"], "Job created successfully");
    let job_id = body["data"]["job_id"].as_str().unwrap().to_string();

    for (request, expected) in [
        (
            json!({"queue_name": "", "payload": {}}),
            "queue_name must not be empty",
        ),
        (
            json!({"queue_name": q, "payload": {}, "priority": "urgent"}),
            "Invalid priority 'urgent'",
        ),
        (
            json!({"queue_name": q, "payload": {}, "max_attempts": 0}),
            "max_attempts must be at least 1",
        ),
        (
            json!({"queue_name": q, "payload": {}, "cron_schedule": "* * * * *"}),
            "Invalid cron schedule",
        ),
    ] {
        let (status, body) = app.post("/api/jobs", request.clone()).await;
        assert_eq!(status, 400, "{request}: {body}");
        assert!(body["error"].as_str().unwrap().contains(expected), "{body}");
    }
    let (status, _, _) = app
        .call("POST", "/api/jobs", Some(json!({"payload": {}})), None)
        .await;
    assert_eq!(status, 400, "a body missing queue_name is rejected");

    // a recurring job and a scheduled one
    let (status, body) = app
        .post(
            "/api/jobs",
            json!({"queue_name": q, "payload": {"token": token, "kind": "cron"}, "cron_schedule": "0 0 * * * *"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let cron_id = body["data"]["job_id"].as_str().unwrap().to_string();
    let future = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
    let (status, body) = app
        .post(
            "/api/jobs",
            json!({"queue_name": q, "payload": {"kind": "later"}, "scheduled_at": future, "priority": "low"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let later_id = body["data"]["job_id"].as_str().unwrap().to_string();

    // --- get
    let (status, body) = app.get(&format!("/api/jobs/{job_id}")).await;
    assert_eq!(status, 200);
    let job = &body["data"];
    assert_eq!(job["queue_name"], q.as_str());
    assert_eq!(job["status"], "Pending");
    assert_eq!(job["priority"], "high");
    assert_eq!(
        (job["attempts"].as_i64(), job["max_attempts"].as_i64()),
        (Some(0), Some(5))
    );
    assert_eq!(job["payload"]["n"], 1);
    assert_eq!(job["trace_id"], "trace-1");
    assert_eq!(job["correlation_id"], "corr-1");
    assert_eq!(job["is_recurring"], false);
    let (_, body) = app.get(&format!("/api/jobs/{cron_id}")).await;
    assert_eq!(body["data"]["is_recurring"], true);
    assert_eq!(body["data"]["cron_schedule"], "0 0 * * * *");
    let (status, body) = app
        .get(&format!("/api/jobs/{}", uuid::Uuid::new_v4()))
        .await;
    assert_eq!(status, 404);
    assert!(body["error"].as_str().unwrap().contains("not found"));
    let (status, body) = app.get("/api/jobs/not-a-uuid").await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("Invalid job ID"));

    // --- list: filters, sorting, pagination
    let (status, listing) = app.get(&format!("/api/jobs?queue={q}")).await;
    assert_eq!(status, 200);
    let ids = job_ids(&listing);
    assert!(ids.contains(&job_id) && ids.contains(&cron_id), "{ids:?}");
    assert_eq!(listing["data"]["pagination"]["total"], ids.len());
    assert_eq!(
        ids.len(),
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        "a recurring job is listed once"
    );

    let (_, by_priority) = app.get(&format!("/api/jobs?queue={q}&priority=HIGH")).await;
    assert_eq!(job_ids(&by_priority), vec![job_id.clone()]);
    let (_, recurring) = app
        .get(&format!("/api/jobs?queue={q}&status=recurring"))
        .await;
    assert_eq!(job_ids(&recurring), vec![cron_id.clone()]);
    let (_, pending) = app
        .get(&format!("/api/jobs?queue={q}&status=pending"))
        .await;
    assert!(job_ids(&pending).contains(&job_id));
    let (_, completed) = app
        .get(&format!("/api/jobs?queue={q}&status=completed"))
        .await;
    assert!(job_ids(&completed).is_empty());

    let (_, asc) = app
        .get(&format!(
            "/api/jobs?queue={q}&sort_by=created_at&sort_order=asc"
        ))
        .await;
    let (_, desc) = app
        .get(&format!(
            "/api/jobs?queue={q}&sort_by=created_at&sort_order=desc"
        ))
        .await;
    let mut reversed = job_ids(&asc);
    reversed.reverse();
    assert_eq!(job_ids(&desc), reversed);
    let (_, default_order) = app.get(&format!("/api/jobs?queue={q}")).await;
    assert_eq!(job_ids(&default_order), job_ids(&desc));
    let (_, by_priority_sort) = app
        .get(&format!(
            "/api/jobs?queue={q}&sort_by=priority&sort_order=desc"
        ))
        .await;
    assert_eq!(
        job_ids(&by_priority_sort)[0],
        job_id,
        "high outranks normal"
    );
    let (status, _) = app
        .get(&format!(
            "/api/jobs?queue={q}&sort_by=scheduled_at&sort_order=asc"
        ))
        .await;
    assert_eq!(status, 200);

    let (_, page1) = app
        .get(&format!("/api/jobs?queue={q}&limit=1&page=1"))
        .await;
    let (_, page2) = app
        .get(&format!("/api/jobs?queue={q}&limit=1&page=2"))
        .await;
    assert_eq!(job_ids(&page1).len(), 1);
    assert_eq!(job_ids(&page2).len(), 1);
    assert_ne!(job_ids(&page1), job_ids(&page2));
    let meta = &page2["data"]["pagination"];
    assert_eq!(
        (meta["page"].as_u64(), meta["limit"].as_u64()),
        (Some(2), Some(1))
    );
    assert_eq!(meta["has_prev"], true);
    let (_, huge_limit) = app.get(&format!("/api/jobs?queue={q}&limit=5000")).await;
    assert_eq!(
        huge_limit["data"]["pagination"]["limit"], 100,
        "the page size is capped, and reported as such"
    );
    let (_, offset_page) = app
        .get(&format!("/api/jobs?queue={q}&limit=1&offset=1"))
        .await;
    assert_eq!(job_ids(&offset_page), job_ids(&page2));
    let (status, _) = app.get("/api/jobs?limit=abc").await;
    assert_eq!(status, 400, "bad query parameters are rejected");

    // --- search
    let (status, found) = app
        .post("/api/jobs/search", json!({"query": token.to_uppercase()}))
        .await;
    assert_eq!(status, 200);
    let found_ids = job_ids(&found);
    assert_eq!(found_ids.len(), 2, "{found_ids:?}");
    assert!(found_ids.contains(&job_id) && found_ids.contains(&cron_id));
    let (_, only_queue) = app
        .post("/api/jobs/search", json!({"query": "later", "queues": [q]}))
        .await;
    assert_eq!(
        job_ids(&only_queue).len(),
        0,
        "scheduled-in-the-future jobs are not ready"
    );
    let (_, by_status) = app
        .post(
            "/api/jobs/search",
            json!({"query": token, "statuses": ["Pending"], "priorities": ["High"], "queues": [q]}),
        )
        .await;
    assert_eq!(job_ids(&by_status), vec![job_id.clone()]);
    let (_, none) = app
        .post(
            "/api/jobs/search",
            json!({"query": token, "statuses": ["Dead"]}),
        )
        .await;
    assert!(job_ids(&none).is_empty());
    let tomorrow = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    let yesterday = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();
    let (_, window) = app
        .post(
            "/api/jobs/search",
            json!({"query": token, "created_after": yesterday, "created_before": tomorrow}),
        )
        .await;
    assert_eq!(job_ids(&window).len(), 2);
    let (_, too_old) = app
        .post(
            "/api/jobs/search",
            json!({"query": token, "created_before": yesterday}),
        )
        .await;
    assert!(job_ids(&too_old).is_empty());
    let (_, too_new) = app
        .post(
            "/api/jobs/search",
            json!({"query": token, "created_after": tomorrow}),
        )
        .await;
    assert!(job_ids(&too_new).is_empty());
    let (_, by_id) = app
        .post("/api/jobs/search", json!({"query": &job_id[..13]}))
        .await;
    assert!(job_ids(&by_id).contains(&job_id));
    let (status, _, _) = app
        .call("POST", "/api/jobs/search", Some(json!({"nope": 1})), None)
        .await;
    assert_eq!(status, 400);

    // --- actions
    let (status, body) = app
        .post(
            &format!("/api/jobs/{job_id}/actions"),
            json!({"action": "retry"}),
        )
        .await;
    assert_eq!(status, 409, "a pending job cannot be retried: {body}");
    let (status, _) = app
        .post(
            &format!("/api/jobs/{job_id}/actions"),
            json!({"action": "explode"}),
        )
        .await;
    assert_eq!(status, 400);
    let (status, _) = app
        .post("/api/jobs/nope/actions", json!({"action": "retry"}))
        .await;
    assert_eq!(status, 400);
    let missing = uuid::Uuid::new_v4();
    for action in ["retry", "delete", "cancel"] {
        let (status, body) = app
            .post(
                &format!("/api/jobs/{missing}/actions"),
                json!({"action": action}),
            )
            .await;
        assert_eq!(status, 404, "{action}: {body}");
    }

    // a dead job is listed as failed and can be retried
    let dead = enqueue(app.queue.as_ref(), &q, json!({"kind": "doomed"})).await;
    let claimed = app
        .queue
        .dequeue(&q)
        .await
        .unwrap()
        .expect("a job to claim");
    app.queue
        .mark_job_dead(claimed.id, "boom: it broke")
        .await
        .unwrap();
    let dead_id = claimed.id.to_string();
    let _ = dead;
    let (_, body) = app.get(&format!("/api/jobs/{dead_id}")).await;
    assert_eq!(body["data"]["status"], "Dead");
    assert_eq!(body["data"]["error_message"], "boom: it broke");
    let (_, failed) = app.get(&format!("/api/jobs?queue={q}&status=failed")).await;
    assert!(
        job_ids(&failed).contains(&dead_id),
        "failed covers dead jobs"
    );
    let (_, dead_list) = app.get(&format!("/api/jobs?queue={q}&status=dead")).await;
    assert!(job_ids(&dead_list).contains(&dead_id));
    let (status, body) = app
        .post(
            &format!("/api/jobs/{dead_id}/actions"),
            json!({"action": "retry", "reason": "fixed"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let (_, body) = app.get(&format!("/api/jobs/{dead_id}")).await;
    assert_eq!(body["data"]["status"], "Pending");

    // delete / cancel
    let (status, _) = app
        .post(
            &format!("/api/jobs/{later_id}/actions"),
            json!({"action": "delete"}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(app.get(&format!("/api/jobs/{later_id}")).await.0, 404);
    let (status, _) = app
        .post(
            &format!("/api/jobs/{cron_id}/actions"),
            json!({"action": "cancel"}),
        )
        .await;
    assert_eq!(status, 200);

    // --- bulk
    let a = enqueue(app.queue.as_ref(), &q, json!({"bulk": "a"})).await;
    let b = enqueue(app.queue.as_ref(), &q, json!({"bulk": "b"})).await;
    let (status, body) = app
        .post(
            "/api/jobs/bulk",
            json!({"job_ids": [a.to_string(), b.to_string(), "garbage", missing.to_string()], "action": "delete"}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        (
            body["data"]["successful"].as_i64(),
            body["data"]["failed"].as_i64()
        ),
        (Some(2), Some(2))
    );
    let errors = body["data"]["errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap().contains("Invalid job ID: garbage"))
    );
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap().contains("not found"))
    );
    assert_eq!(app.get(&format!("/api/jobs/{a}")).await.0, 404);
    let (status, body) = app
        .post(
            "/api/jobs/bulk",
            json!({"job_ids": [job_id.clone()], "action": "retry"}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["failed"], 1, "a pending job cannot be retried");
    let (status, _) = app
        .post(
            "/api/jobs/bulk",
            json!({"job_ids": [], "action": "explode"}),
        )
        .await;
    assert_eq!(status, 400);

    // clean up
    for id in [&job_id, &dead_id] {
        app.post(
            &format!("/api/jobs/{id}/actions"),
            json!({"action": "delete"}),
        )
        .await;
    }
}

async fn queue_endpoints<Q: JobHistory + 'static>(app: &App<Q>) {
    let q = unique("queues");
    let ids = [
        enqueue(app.queue.as_ref(), &q, json!({"i": 1})).await,
        enqueue(app.queue.as_ref(), &q, json!({"i": 2})).await,
    ];
    // one completed job, one dead job
    let done = app.queue.dequeue(&q).await.unwrap().unwrap();
    app.queue.complete_job(done.id).await.unwrap();
    let doomed = app.queue.dequeue(&q).await.unwrap().unwrap();
    app.queue
        .mark_job_dead(doomed.id, "queue test failure")
        .await
        .unwrap();
    let pending = enqueue(app.queue.as_ref(), &q, json!({"i": 3})).await;
    let _ = (ids, pending);

    // --- list
    let (status, body) = app.get("/api/queues?limit=1000").await;
    assert_eq!(status, 200);
    let ours = body["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == q.as_str())
        .expect("queue listed")
        .clone();
    assert_eq!(ours["pending_count"], 1);
    assert_eq!(ours["completed_count"], 1);
    assert_eq!(ours["dead_count"], 1);
    assert_eq!(ours["failed_count"], 1);
    assert_eq!(ours["is_paused"], false);
    assert!(ours["last_job_at"].is_string());
    assert!(ours["oldest_pending_job"].is_string());
    let (_, paged) = app.get("/api/queues?limit=1&page=1").await;
    assert_eq!(paged["data"]["items"].as_array().unwrap().len(), 1);
    let (_, beyond) = app.get("/api/queues?limit=10&offset=100000").await;
    assert!(beyond["data"]["items"].as_array().unwrap().is_empty());

    // --- detail
    let (status, body) = app.get(&format!("/api/queues/{q}")).await;
    assert_eq!(status, 200, "{body}");
    let detail = &body["data"];
    assert_eq!(detail["queue_info"]["name"], q.as_str());
    assert_eq!(detail["status_breakdown"]["Pending"], 1);
    assert!(detail["priority_breakdown"].is_object());
    assert_eq!(detail["hourly_throughput"].as_array().unwrap().len(), 24);
    let errors = detail["recent_errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e["error_message"] == "queue test failure"),
        "{errors:?}"
    );
    let (status, body) = app.get(&format!("/api/queues/{}", unique("nope"))).await;
    assert_eq!(status, 404);
    assert!(body["error"].as_str().unwrap().contains("not found"));

    // --- jobs of the queue
    let (status, jobs) = app.get(&format!("/api/queues/{q}/jobs")).await;
    assert_eq!(status, 200);
    assert!(
        jobs["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|j| j["queue_name"] == q.as_str())
    );
    let (_, failed) = app
        .get(&format!("/api/queues/{q}/jobs?status=failed"))
        .await;
    assert_eq!(job_ids(&failed).len(), 1);

    // --- actions
    let (status, body) = app
        .post(
            &format!("/api/queues/{q}/actions"),
            json!({"action": "pause"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["action"], "pause");
    let (_, body) = app.get(&format!("/api/queues/{q}")).await;
    assert_eq!(body["data"]["queue_info"]["is_paused"], true);
    assert_eq!(body["data"]["queue_info"]["paused_by"], "web-ui");
    assert!(body["data"]["queue_info"]["paused_at"].is_string());
    let (status, _) = app
        .post(
            &format!("/api/queues/{q}/actions"),
            json!({"action": "resume"}),
        )
        .await;
    assert_eq!(status, 200);
    let (_, body) = app.get(&format!("/api/queues/{q}")).await;
    assert_eq!(body["data"]["queue_info"]["is_paused"], false);

    let (status, body) = app
        .post(
            &format!("/api/queues/{q}/actions"),
            json!({"action": "clear_completed", "confirm": true}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["cleared_count"], 1);
    let (_, body) = app.get(&format!("/api/queues/{q}")).await;
    assert_eq!(body["data"]["queue_info"]["completed_count"], 0);
    // dead jobs younger than a week are kept
    let (status, body) = app
        .post(
            &format!("/api/queues/{q}/actions"),
            json!({"action": "clear_dead"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["count"], 0);
    assert_eq!(app.get(&format!("/api/jobs/{}", doomed.id)).await.0, 200);
    let (status, body) = app
        .post(
            &format!("/api/queues/{q}/actions"),
            json!({"action": "dance"}),
        )
        .await;
    assert_eq!(status, 400);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("Unknown action: dance")
    );
    let (status, _, _) = app
        .call(
            "POST",
            &format!("/api/queues/{q}/actions"),
            Some(json!({})),
            None,
        )
        .await;
    assert_eq!(status, 400);

    // clean up
    for job in app.queue.get_ready_jobs(&q, 100).await.unwrap() {
        app.queue.delete_job(job.id).await.unwrap();
    }
    app.queue.delete_job(doomed.id).await.unwrap();
}

async fn stats_endpoints<Q: JobHistory + 'static>(app: &App<Q>) {
    let q = unique("stats");
    let done = enqueue(app.queue.as_ref(), &q, json!({})).await;
    let claimed = app.queue.dequeue(&q).await.unwrap().unwrap();
    assert_eq!(claimed.id, done);
    app.queue.complete_job(done).await.unwrap();
    let failing = enqueue(app.queue.as_ref(), &q, json!({})).await;
    let claimed = app.queue.dequeue(&q).await.unwrap().unwrap();
    app.queue
        .mark_job_dead(claimed.id, "TimeoutError: upstream timed out after 30s")
        .await
        .unwrap();
    let _ = failing;
    enqueue(app.queue.as_ref(), &q, json!({})).await;

    // --- overview
    let (status, body) = app.get("/api/stats/overview").await;
    assert_eq!(status, 200, "{body}");
    let overview = &body["data"];
    assert!(overview["total_queues"].as_u64().unwrap() >= 1);
    assert!(overview["completed_jobs"].as_u64().unwrap() >= 1);
    assert!(overview["dead_jobs"].as_u64().unwrap() >= 1);
    assert!(overview["pending_jobs"].as_u64().unwrap() >= 1);
    let total = overview["total_jobs"].as_u64().unwrap();
    assert!(total >= 3);
    assert!(overview["overall_error_rate"].as_f64().unwrap() > 0.0);
    assert!(overview["uptime_seconds"].is_u64());
    assert!(
        overview["system_health"]["database_healthy"]
            .as_bool()
            .unwrap()
    );

    // --- detailed
    let (status, body) = app.get("/api/stats/detailed").await;
    assert_eq!(status, 200, "{body}");
    let detailed = &body["data"];
    let ours = detailed["queue_stats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == q.as_str())
        .expect("queue in detailed stats");
    assert_eq!(
        (
            ours["pending"].as_u64(),
            ours["completed_total"].as_u64(),
            ours["dead_total"].as_u64()
        ),
        (Some(1), Some(1), Some(1))
    );
    assert!(ours["oldest_pending_age_seconds"].is_number());
    assert!(ours["priority_distribution"].is_object());
    assert_eq!(detailed["hourly_trends"].as_array().unwrap().len(), 24);
    assert!(detailed["performance_metrics"]["database_response_time_ms"].is_number());
    let patterns = detailed["error_patterns"].as_array().unwrap();
    assert!(
        patterns.iter().any(|p| p["error_type"]
            .as_str()
            .is_some_and(|t| t.contains("Timeout"))),
        "{patterns:?}"
    );
    let (status, body) = app.get("/api/stats/detailed?hours=6").await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["hourly_trends"].as_array().unwrap().len(), 6);

    // --- trends
    let (status, body) = app.get("/api/stats/trends").await;
    assert_eq!(status, 200);
    let trends = body["data"].as_array().unwrap();
    assert_eq!(trends.len(), 24);
    let completed: u64 = trends
        .iter()
        .map(|t| t["completed"].as_u64().unwrap())
        .sum();
    let failed: u64 = trends.iter().map(|t| t["failed"].as_u64().unwrap()).sum();
    assert!(completed >= 1 && failed >= 1);
    let (_, body) = app.get("/api/stats/trends?hours=1").await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    for bad in ["hours=0", "hours=100000"] {
        let (status, body) = app.get(&format!("/api/stats/trends?{bad}")).await;
        assert_eq!(status, 400, "{bad}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("hours must be between")
        );
    }
    assert_eq!(app.get("/api/stats/trends?hours=abc").await.0, 400);

    // --- health
    let (status, body) = app.get("/api/stats/health").await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["database_healthy"], true);
    assert!(body["data"]["status"].is_string());

    // clean up
    for job in app.queue.get_ready_jobs(&q, 100).await.unwrap() {
        app.queue.delete_job(job.id).await.unwrap();
    }
    for job in app
        .queue
        .get_dead_jobs_by_queue(&q, Some(100), Some(0))
        .await
        .unwrap()
    {
        app.queue.delete_job(job.id).await.unwrap();
    }
    app.queue.delete_job(done).await.unwrap();
}

async fn system_endpoints<Q: JobHistory + 'static>(app: &App<Q>, database_type: &str) {
    let (status, body) = app.get("/api/system/info").await;
    assert_eq!(status, 200, "{body}");
    let info = &body["data"];
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(info["database_info"]["database_type"], database_type);
    assert_eq!(info["database_info"]["connection_url"], "***masked***");
    assert_eq!(info["database_info"]["connection_health"], true);
    assert_eq!(info["runtime_info"]["process_id"], std::process::id());
    assert!(
        info["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "postgres" || f == "mysql")
    );
    assert!(info["started_at"].is_string() && info["uptime_seconds"].is_u64());

    let (status, body) = app.get("/api/system/config").await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["bind_address"], "127.0.0.1");
    assert_eq!(body["data"]["authentication_enabled"], false);
    assert_eq!(body["data"]["websocket_max_connections"], 100);

    let (status, body) = app.get("/api/system/metrics").await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["prometheus_enabled"], false);
    assert!(body["data"]["custom_metrics_count"].is_null());

    let (status, body) = app.get("/api/version").await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["name"], "hammerwork-web");
    assert_eq!(body["data"]["version"], env!("CARGO_PKG_VERSION"));

    // maintenance
    let (status, body) = app
        .post(
            "/api/system/maintenance",
            json!({"operation": "cleanup", "dry_run": true}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["dry_run"], true);
    assert!(body["data"]["estimated_deletions"].is_u64());
    let (status, body) = app
        .post("/api/system/maintenance", json!({"operation": "cleanup"}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["dry_run"], false);
    assert!(body["data"]["deletions"].is_u64());
    for (operation, postgres_statement) in [
        ("vacuum", "VACUUM (ANALYZE) hammerwork_jobs"),
        ("reindex", "REINDEX TABLE hammerwork_jobs"),
        ("optimize", "ANALYZE hammerwork_jobs"),
    ] {
        let expected = if database_type == "PostgreSQL" {
            postgres_statement
        } else {
            "OPTIMIZE TABLE hammerwork_jobs"
        };
        // Every Hammerwork table that exists, one statement each.
        let (status, body) = app
            .post("/api/system/maintenance", json!({"operation": operation}))
            .await;
        assert_eq!(status, 200, "{operation}: {body}");
        assert_eq!(body["data"]["operation"], operation);
        assert_eq!(body["data"]["dry_run"], false);
        let statements = body["data"]["statements"].as_array().unwrap();
        let tables: Vec<&str> = statements
            .iter()
            .map(|s| s["table"].as_str().unwrap())
            .collect();
        for table in ["hammerwork_jobs", "hammerwork_jobs_archive"] {
            assert!(tables.contains(&table), "{operation}: {tables:?}");
        }
        assert!(tables.iter().all(|t| t.starts_with("hammerwork_")));
        assert!(
            statements
                .iter()
                .any(|s| s["statement"] == expected && s["table"] == "hammerwork_jobs"),
            "{operation}: {body}"
        );
        if database_type == "MySQL" {
            assert!(
                statements
                    .iter()
                    .all(|s| !s["messages"].as_array().unwrap().is_empty()),
                "{body}"
            );
        }

        // One table, as a dry run: nothing runs.
        let (status, body) = app
            .post(
                "/api/system/maintenance",
                json!({"operation": operation, "target": "hammerwork_jobs", "dry_run": true}),
            )
            .await;
        assert_eq!(status, 200, "{operation}: {body}");
        assert_eq!(body["data"]["dry_run"], true);
        assert_eq!(
            body["data"]["statements"],
            json!([{"table": "hammerwork_jobs", "statement": expected, "messages": []}])
        );

        // Anything that is not a Hammerwork table is refused.
        let (status, body) = app
            .post(
                "/api/system/maintenance",
                json!({"operation": operation, "target": "hammerwork_jobs; DROP TABLE x"}),
            )
            .await;
        assert_eq!(status, 400, "{operation}: {body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("not a Hammerwork table")
        );
    }
    let (status, body) = app
        .post("/api/system/maintenance", json!({"operation": "defrag"}))
        .await;
    assert_eq!(status, 400);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("Unknown maintenance operation")
    );
    assert_eq!(
        app.call("POST", "/api/system/maintenance", Some(json!({})), None)
            .await
            .0,
        400
    );
    assert_eq!(
        app.call("GET", "/api/system/maintenance", None, None)
            .await
            .0,
        405
    );
}

/// Complete a job in `q` and archive it, returning its id.
async fn archive_one<Q: DatabaseQueue>(queue: &Q, q: &str, payload: Value, by: &str) -> uuid::Uuid {
    let id = queue
        .enqueue(Job::new(q.to_string(), payload).with_priority(JobPriority::Normal))
        .await
        .unwrap();
    let claimed = queue.dequeue(q).await.unwrap().unwrap();
    assert_eq!(claimed.id, id);
    queue.complete_job(id).await.unwrap();
    let policy = ArchivalPolicy::new().archive_completed_after(chrono::Duration::zero());
    let stats = queue
        .archive_jobs(
            Some(q),
            &policy,
            &ArchivalConfig::new(),
            ArchivalReason::Manual,
            Some(by),
        )
        .await
        .unwrap();
    assert_eq!(stats.jobs_archived, 1);
    id
}

async fn archive_endpoints<Q: JobHistory + 'static>(app: &App<Q>) {
    let q = unique("archive");
    let first = archive_one(
        app.queue.as_ref(),
        &q,
        json!({"n": 1, "pad": "x".repeat(2000)}),
        "alice",
    )
    .await;
    let second = archive_one(app.queue.as_ref(), &q, json!({"n": 2}), "bob").await;

    // --- list and filters
    let (status, body) = app.get(&format!("/api/archive/jobs?queue={q}")).await;
    assert_eq!(status, 200, "{body}");
    let ids = job_ids(&body);
    assert_eq!(ids.len(), 2);
    assert_eq!(body["data"]["pagination"]["total"], 2);
    let item = body["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["id"] == first.to_string())
        .unwrap()
        .clone();
    assert_eq!(item["queue_name"], q.as_str());
    assert_eq!(item["archival_reason"], "Manual");
    assert_eq!(item["archived_by"], "alice");
    assert_eq!(
        item["payload_compressed"], true,
        "a large payload is compressed"
    );

    let (_, by_alice) = app
        .get(&format!("/api/archive/jobs?queue={q}&archived_by=alice"))
        .await;
    assert_eq!(job_ids(&by_alice), vec![first.to_string()]);
    let (_, compressed) = app
        .get(&format!("/api/archive/jobs?queue={q}&compressed=false"))
        .await;
    assert_eq!(job_ids(&compressed), vec![second.to_string()]);
    let (_, reason) = app
        .get(&format!("/api/archive/jobs?queue={q}&reason=manual"))
        .await;
    assert_eq!(job_ids(&reason).len(), 2);
    let (_, other_reason) = app
        .get(&format!("/api/archive/jobs?queue={q}&reason=compliance"))
        .await;
    assert!(job_ids(&other_reason).is_empty());
    let (_, status_filter) = app
        .get(&format!(
            "/api/archive/jobs?queue={q}&original_status=completed"
        ))
        .await;
    assert_eq!(job_ids(&status_filter).len(), 2);
    let (_, failed_only) = app
        .get(&format!(
            "/api/archive/jobs?queue={q}&original_status=failed"
        ))
        .await;
    assert!(job_ids(&failed_only).is_empty());
    let future = (chrono::Utc::now() + chrono::Duration::days(1))
        .to_rfc3339()
        .replace('+', "%2B");
    let (_, after) = app
        .get(&format!(
            "/api/archive/jobs?queue={q}&archived_after={future}"
        ))
        .await;
    assert!(job_ids(&after).is_empty());
    let (_, before) = app
        .get(&format!(
            "/api/archive/jobs?queue={q}&archived_before={future}"
        ))
        .await;
    assert_eq!(job_ids(&before).len(), 2);
    let (_, page) = app
        .get(&format!("/api/archive/jobs?queue={q}&limit=1&page=2"))
        .await;
    assert_eq!(job_ids(&page).len(), 1);
    assert_eq!(
        page["data"]["pagination"]["total"], 2,
        "a full page still reports the true total"
    );
    let (_, filtered_page) = app
        .get(&format!(
            "/api/archive/jobs?queue={q}&reason=manual&limit=1&page=2"
        ))
        .await;
    assert_eq!(job_ids(&filtered_page).len(), 1);
    assert_eq!(filtered_page["data"]["pagination"]["total"], 2);
    let (_, other_queue) = app
        .get(&format!("/api/archive/jobs?queue={}", unique("empty")))
        .await;
    assert!(job_ids(&other_queue).is_empty());

    // --- stats
    let (status, body) = app.get(&format!("/api/archive/stats?queue={q}")).await;
    assert_eq!(status, 200, "{body}");
    assert!(body["data"]["stats"]["jobs_archived"].as_u64().unwrap() >= 2);
    assert!(
        body["data"]["by_queue"].as_object().unwrap().is_empty(),
        "a queue filter skips the breakdown"
    );
    assert!(
        body["data"]["recent_operations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let (status, body) = app.get("/api/archive/stats").await;
    assert_eq!(status, 200);
    assert!(body["data"]["by_queue"].as_object().unwrap().len() <= 1000);

    // --- archive through the API: dry run, then for real
    let completed = app
        .queue
        .enqueue(Job::new(q.clone(), json!({"later": true})))
        .await
        .unwrap();
    app.queue.dequeue(&q).await.unwrap();
    app.queue.complete_job(completed).await.unwrap();
    let policy = serde_json::to_value(
        ArchivalPolicy::new().archive_completed_after(chrono::Duration::zero()),
    )
    .unwrap();
    let (status, body) = app
        .post(
            "/api/archive/jobs",
            json!({"queue_name": q, "reason": "Manual", "archived_by": "api", "dry_run": true, "policy": policy}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["dry_run"], true);
    let (_, job) = app.get(&format!("/api/jobs/{completed}")).await;
    assert_eq!(
        job["data"]["status"], "Completed",
        "a dry run archives nothing"
    );
    let (status, body) = app
        .post(
            "/api/archive/jobs",
            json!({"queue_name": q, "reason": "Compliance", "archived_by": "api", "dry_run": false, "policy": policy, "config": serde_json::to_value(ArchivalConfig::new().with_compression_level(3)).unwrap()}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["dry_run"], false);
    assert_eq!(body["data"]["stats"]["jobs_archived"], 1);
    let (_, job) = app.get(&format!("/api/jobs/{completed}")).await;
    assert_eq!(job["data"]["status"], "Archived");
    let (_, by_api) = app
        .get(&format!(
            "/api/archive/jobs?queue={q}&reason=compliance&archived_by=api"
        ))
        .await;
    assert_eq!(job_ids(&by_api), vec![completed.to_string()]);
    assert_eq!(
        app.call(
            "POST",
            "/api/archive/jobs",
            Some(json!({"queue_name": q})),
            None
        )
        .await
        .0,
        400
    );

    // --- restore
    let (status, body) = app
        .post(
            &format!("/api/archive/jobs/{first}/restore"),
            json!({"restored_by": "carol"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["restored_by"], "carol");
    assert_eq!(body["data"]["job"]["id"], first.to_string());
    assert_eq!(app.get(&format!("/api/jobs/{first}")).await.0, 200);
    let (status, body) = app
        .post(&format!("/api/archive/jobs/{first}/restore"), json!({}))
        .await;
    assert_eq!(status, 404, "already restored: {body}");
    let (status, _) = app
        .post("/api/archive/jobs/not-a-uuid/restore", json!({}))
        .await;
    assert_eq!(status, 400);
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/archive/jobs/{second}/restore"),
            None,
            None
        )
        .await
        .0,
        415,
        "a JSON body is required"
    );

    // --- purge: the dry run counts only what the real purge would delete
    let long_ago = (chrono::Utc::now() - chrono::Duration::days(365)).to_rfc3339();
    let tomorrow = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    let (status, body) = app
        .delete(
            "/api/archive/purge",
            json!({"older_than": long_ago, "dry_run": true}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["jobs_purged"], 0, "nothing is that old");
    let (_, body) = app
        .delete(
            "/api/archive/purge",
            json!({"older_than": tomorrow, "dry_run": true}),
        )
        .await;
    let would_purge = body["data"]["jobs_purged"].as_u64().unwrap();
    assert!(
        would_purge >= 2,
        "second and the API-archived job are archived: {would_purge}"
    );
    assert_eq!(body["data"]["dry_run"], true);
    let (_, still_there) = app.get(&format!("/api/archive/jobs?queue={q}")).await;
    assert_eq!(job_ids(&still_there).len(), 2, "a dry run deletes nothing");
    // purge only this test's jobs: nothing outside is older than the cutoff of "now" minus a bit
    let (status, body) = app
        .delete(
            "/api/archive/purge",
            json!({"older_than": tomorrow, "dry_run": false, "purged_by": "admin"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["data"]["jobs_purged"].as_u64().unwrap() >= 2);
    let (_, gone) = app.get(&format!("/api/archive/jobs?queue={q}")).await;
    assert!(job_ids(&gone).is_empty());
    assert_eq!(
        app.call(
            "DELETE",
            "/api/archive/purge",
            Some(json!({"dry_run": true})),
            None
        )
        .await
        .0,
        400
    );

    // clean up
    app.queue.delete_job(first).await.unwrap();
}

#[cfg(feature = "auth")]
async fn authentication<Q: JobHistory + 'static>(queue: Arc<Q>, database_type: &str) {
    let hash = bcrypt::hash("s3cret", 4).unwrap();
    let app = App::new(
        queue,
        AuthConfig {
            enabled: true,
            username: "admin".into(),
            password_hash: hash.clone(),
            max_failed_attempts: 1000,
            ..AuthConfig::default()
        },
        database_type,
    );
    use base64::Engine;
    let basic = |user: &str, pass: &str| {
        format!(
            "Basic {}",
            base64::prelude::BASE64_STANDARD.encode(format!("{user}:{pass}"))
        )
    };

    // public: the health check and the single-page app
    assert_eq!(app.call("GET", "/health", None, None).await.0, 200);
    let (status, text, _) = app.call("GET", "/", None, None).await;
    assert_eq!((status, text.contains("SPA")), (200, true));

    // everything under /api needs credentials - and never answers with the HTML page
    for path in [
        "/api/queues",
        "/api/jobs",
        "/api/stats/overview",
        "/api/system/info",
        "/api/archive/stats",
        "/api/version",
    ] {
        let (status, text, _) = app.call("GET", path, None, None).await;
        assert_eq!(status, 401, "{path}");
        assert!(!text.contains("SPA"), "{path}");
        let (status, _, json) = app
            .call("GET", path, None, Some(&basic("admin", "wrong")))
            .await;
        assert_eq!(status, 401, "{path}");
        assert_eq!(json["error"], "Invalid credentials");
    }
    let (status, _, json) = app
        .call("GET", "/api/queues", None, Some("Bearer abc"))
        .await;
    assert_eq!(status, 400);
    assert_eq!(json["error"], "Invalid authentication format");
    let (status, _, _) = app
        .call(
            "POST",
            "/api/jobs",
            Some(json!({"queue_name": "x", "payload": {}})),
            None,
        )
        .await;
    assert_eq!(status, 401, "writes are protected too");

    let (status, _, json) = app
        .call("GET", "/api/version", None, Some(&basic("admin", "s3cret")))
        .await;
    assert_eq!(status, 200);
    assert_eq!(json["success"], true);
    // unknown API paths are still 404s for authenticated users
    let (status, _, _) = app
        .call("GET", "/api/nope", None, Some(&basic("admin", "s3cret")))
        .await;
    assert_eq!(status, 404);
    // the WebSocket endpoint is protected as well
    let denied = warp::test::ws().path("/ws").handshake((app.routes)()).await;
    assert!(denied.is_err());
    let allowed = warp::test::ws()
        .path("/ws")
        .header("authorization", basic("admin", "s3cret"))
        .handshake((app.routes)())
        .await;
    assert!(allowed.is_ok());

    // repeated failures lock the account: even the right password is refused for a while
    let strict = App::new(
        app.queue.clone(),
        AuthConfig {
            enabled: true,
            username: "admin".into(),
            password_hash: hash,
            max_failed_attempts: 3,
            ..AuthConfig::default()
        },
        database_type,
    );
    for _ in 0..3 {
        strict
            .call("GET", "/api/version", None, Some(&basic("admin", "nope")))
            .await;
    }
    let (status, _, json) = strict
        .call("GET", "/api/version", None, Some(&basic("admin", "s3cret")))
        .await;
    assert_eq!(status, 429);
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("temporarily locked")
    );
    assert_eq!(
        strict.call("GET", "/health", None, None).await.0,
        200,
        "health stays public"
    );
}

async fn websocket_through_the_router<Q: JobHistory + 'static>(app: &App<Q>) {
    let mut client = warp::test::ws()
        .path("/ws")
        .handshake((app.routes)())
        .await
        .expect("handshake");
    client.send_text(r#"{"type": "Ping"}"#).await;
    let message = tokio::time::timeout(std::time::Duration::from_secs(2), client.recv())
        .await
        .expect("a reply")
        .expect("open socket");
    assert!(message.to_str().unwrap().contains("Pong"));
    // /ws is only for upgrades
    let (status, _, _) = app.call("GET", "/ws", None, None).await;
    assert_ne!(status, 200, "a plain GET to /ws is not the single-page app");
}

/// The dashboard script may only call endpoints the server has.
async fn dashboard_script_calls_only_existing_endpoints<Q: JobHistory + 'static>(app: &App<Q>) {
    let script = include_str!("../assets/dashboard.js");
    let job = enqueue(app.queue.as_ref(), &unique("script"), json!({}))
        .await
        .to_string();
    let mut checked = 0;
    for (index, _) in script.match_indices("apiCall(") {
        let rest = &script[index + "apiCall(".len()..];
        let Some(quote) = rest.chars().next().filter(|c| ['\'', '`', '"'].contains(c)) else {
            continue; // apiCall(url, ...) inside the helper itself
        };
        let end = rest[1..].find(quote).unwrap();
        let template = &rest[1..1 + end];
        let after = &rest[2 + end..rest.len().min(2 + end + 300)];
        let call_end = after.find(");").unwrap_or(after.len());
        let args = &after[..call_end];
        let method = ["POST", "DELETE", "PUT", "PATCH"]
            .iter()
            .find(|m| args.contains(&format!("'{m}'")) || args.contains(&format!("method: '{m}'")))
            .copied()
            .unwrap_or("GET");
        // fill the placeholders: ids become a job UUID, anything else a harmless value
        let mut path = String::new();
        let mut chars = template.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '$' && chars.peek() == Some(&'{') {
                let expr: String = chars.by_ref().skip(1).take_while(|c| *c != '}').collect();
                path.push_str(if expr.contains("ob") || expr.contains("selectedJob") {
                    &job
                } else if expr.contains("queueName") {
                    "q"
                } else if expr.contains("params") {
                    "limit=5"
                } else if expr.contains("period") {
                    "24h"
                } else {
                    "x"
                });
            } else {
                path.push(c);
            }
        }
        if !path.starts_with("/api/") {
            continue;
        }
        let body = match method {
            "GET" => None,
            _ => Some(
                json!({"action": "retry", "reason": null, "restored_by": null, "queue_name": "q", "payload": {}, "older_than": "2020-01-01T00:00:00Z", "dry_run": true, "reason_": null}),
            ),
        };
        let (status, text, _) = app.call(method, &path, body, None).await;
        assert!(
            status != 404 || text.contains("not found"),
            "{method} {path}: {status} {text}"
        );
        assert_ne!(status, 405, "{method} {path} is not routed: {text}");
        assert!(
            !text.contains("SPA"),
            "{method} {path} fell through to the single-page app"
        );
        // 404s must come from a missing job/queue, never from a missing route
        if status == 404 {
            assert!(
                !text.contains("Resource not found"),
                "{method} {path} has no route: {text}"
            );
        }
        checked += 1;
    }
    assert!(checked >= 12, "only {checked} script calls were checked");
    app.queue.delete_job(job.parse().unwrap()).await.unwrap();
}

/// Issue #64 regressions: cross-site writes (H8), body limits and paging the archive in the
/// database (M11), and clamped pagination (M13).
async fn request_hardening<Q: JobHistory + 'static>(app: &App<Q>) {
    let q = unique("hardening");
    let reply = |request: warp::test::RequestBuilder| {
        let routes = (app.routes)();
        async move {
            let response = request.reply(&routes).await;
            let text = String::from_utf8_lossy(response.body()).to_string();
            (response.status().as_u16(), text)
        }
    };
    let body = json!({"queue_name": q, "payload": {"n": 1}}).to_string();
    let post = |content_type: Option<&str>| {
        let mut request = warp::test::request()
            .method("POST")
            .path("/api/jobs")
            .header("content-length", body.len().to_string())
            .body(body.clone());
        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }
        request
    };

    // --- H8: a body without the JSON content type, or from another site, is refused.
    let (status, text) = reply(post(None)).await;
    assert_eq!(status, 415, "{text}");
    let (status, text) = reply(post(Some("text/plain"))).await;
    assert_eq!(status, 415, "{text}");
    let (status, text) = reply(
        post(Some("application/json"))
            .header("origin", "http://evil.example")
            .header("sec-fetch-site", "cross-site"),
    )
    .await;
    assert_eq!(status, 403, "{text}");
    assert!(text.contains("Cross-origin request refused"), "{text}");
    let (_, listing) = app.get(&format!("/api/jobs?queue={q}")).await;
    assert!(job_ids(&listing).is_empty(), "nothing was enqueued");
    // The dashboard's own page and an allowed origin can write.
    let (status, text) = reply(
        post(Some("application/json"))
            .header("origin", "http://127.0.0.1:8080")
            .header("sec-fetch-site", "same-origin"),
    )
    .await;
    assert_eq!(status, 200, "{text}");
    let (status, text) = reply(
        post(Some("application/json; charset=utf-8"))
            .header("origin", "https://ops.example.com")
            .header("sec-fetch-site", "cross-site"),
    )
    .await;
    assert_eq!(status, 200, "{text}");

    // --- M11: bounded bodies and bulk requests.
    let (status, _) = reply(
        warp::test::request()
            .method("POST")
            .path("/api/jobs")
            .json(&json!({"queue_name": q, "payload": "x".repeat(2 * 1024 * 1024)})),
    )
    .await;
    assert_eq!(status, 413);
    let ids: Vec<String> = (0..=hammerwork_web::api::jobs::MAX_BULK_JOB_IDS)
        .map(|_| uuid::Uuid::new_v4().to_string())
        .collect();
    let (status, body) = app
        .post(
            "/api/jobs/bulk",
            json!({"job_ids": ids, "action": "delete"}),
        )
        .await;
    assert_eq!(status, 400, "{body}");

    // --- M11 / M13: the archive is filtered, counted and paged in the database.
    let archive = unique("archive_pages");
    let mut archived = Vec::new();
    for (n, by) in ["ann", "ann", "ann", "ben"].iter().enumerate() {
        archived.push(archive_one(app.queue.as_ref(), &archive, json!({ "n": n }), by).await);
    }
    let (status, page) = app
        .get(&format!("/api/archive/jobs?queue={archive}&limit=2&page=2"))
        .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(job_ids(&page).len(), 2);
    assert_eq!(page["data"]["pagination"]["total"], 4);
    let (_, filtered) = app
        .get(&format!(
            "/api/archive/jobs?queue={archive}&archived_by=ann&limit=2&page=2"
        ))
        .await;
    assert_eq!(job_ids(&filtered).len(), 1, "{filtered}");
    assert_eq!(filtered["data"]["pagination"]["total"], 3);
    assert_eq!(filtered["data"]["pagination"]["total_pages"], 2);
    // Pages cover every job exactly once, newest first.
    let mut seen = Vec::new();
    for page in 1..=4 {
        let (_, listing) = app
            .get(&format!(
                "/api/archive/jobs?queue={archive}&limit=1&page={page}"
            ))
            .await;
        seen.extend(job_ids(&listing));
    }
    seen.sort();
    let mut expected: Vec<String> = archived.iter().map(|id| id.to_string()).collect();
    expected.sort();
    assert_eq!(seen, expected);
    // M13: an oversized limit is clamped before the offset is computed.
    let (status, clamped) = app
        .get(&format!(
            "/api/archive/jobs?queue={archive}&limit=5000&page=2"
        ))
        .await;
    assert_eq!(status, 200, "{clamped}");
    assert_eq!(clamped["data"]["pagination"]["limit"], 1000);
    assert_eq!(clamped["data"]["pagination"]["offset"], 1000);
    assert!(job_ids(&clamped).is_empty());
    let (status, overflow) = app
        .get(&format!(
            "/api/archive/jobs?queue={archive}&limit=4294967295&page=4294967295"
        ))
        .await;
    assert_eq!(status, 200, "no overflow: {overflow}");
    assert!(job_ids(&overflow).is_empty());
    assert_eq!(overflow["data"]["pagination"]["total"], 4);
    // The purge dry run counts in the database too.
    let (status, dry) = app
        .delete(
            "/api/archive/purge",
            json!({"older_than": chrono::Utc::now() + chrono::Duration::days(1), "dry_run": true}),
        )
        .await;
    assert_eq!(status, 200, "{dry}");
    assert!(dry["data"]["jobs_purged"].as_u64().unwrap() >= 4);
    for id in archived {
        app.queue.restore_archived_job(id).await.unwrap();
        app.queue.delete_job(id).await.unwrap();
    }
}

async fn run_all<Q: JobHistory + 'static>(queue: Arc<Q>, database_type: &str) {
    let app = App::new(
        queue.clone(),
        AuthConfig {
            enabled: false,
            ..AuthConfig::default()
        },
        database_type,
    );
    health_and_static_files(&app).await;
    job_endpoints(&app).await;
    queue_endpoints(&app).await;
    stats_endpoints(&app).await;
    system_endpoints(&app, database_type).await;
    archive_endpoints(&app).await;
    request_hardening(&app).await;
    websocket_through_the_router(&app).await;
    dashboard_script_calls_only_existing_endpoints(&app).await;
    #[cfg(feature = "auth")]
    authentication(queue, database_type).await;
    #[cfg(not(feature = "auth"))]
    let _ = queue;
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires DATABASE_URL (PostgreSQL)"]
async fn test_http_api_postgres() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap();
    run_all(Arc::new(JobQueue::new(pool)), "PostgreSQL").await;
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires MYSQL_DATABASE_URL"]
async fn test_http_api_mysql() {
    let url = std::env::var("MYSQL_DATABASE_URL").expect("MYSQL_DATABASE_URL");
    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap();
    run_all(Arc::new(JobQueue::new(pool)), "MySQL").await;
}

/// Without a database the API reports 500s with JSON bodies instead of hanging or panicking.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn test_http_api_reports_a_dead_database() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(200))
        .connect_lazy("postgres://nobody:nothing@127.0.0.1:1/none")
        .unwrap();
    let app = App::new(
        Arc::new(JobQueue::new(pool)),
        AuthConfig {
            enabled: false,
            ..AuthConfig::default()
        },
        "PostgreSQL",
    );
    for path in [
        "/api/queues",
        "/api/queues/q",
        "/api/jobs",
        "/api/jobs/00000000-0000-0000-0000-000000000000",
        "/api/stats/overview",
        "/api/stats/detailed",
        "/api/stats/trends",
        "/api/archive/jobs",
        "/api/archive/stats",
    ] {
        let (status, _, json) = app.call("GET", path, None, None).await;
        assert_eq!(status, 500, "{path}");
        assert_eq!(json["success"], false, "{path}");
        assert!(json["error"].is_string(), "{path}");
    }
    // The health summary degrades instead of failing, and says why.
    let (status, json) = app.get("/api/stats/health").await;
    assert_eq!(status, 200);
    assert_eq!(json["data"]["database_healthy"], false);
    assert_eq!(json["data"]["status"], "critical");
    let (_, json) = app.get("/api/system/info").await;
    assert_eq!(json["data"]["database_info"]["connection_health"], false);

    for (method, path, body) in [
        (
            "POST",
            "/api/jobs",
            json!({"queue_name": "q", "payload": {}}),
        ),
        ("POST", "/api/jobs/search", json!({"query": "x"})),
        ("POST", "/api/queues/q/actions", json!({"action": "pause"})),
        ("POST", "/api/queues/q/actions", json!({"action": "resume"})),
        (
            "POST",
            "/api/system/maintenance",
            json!({"operation": "cleanup"}),
        ),
        (
            "POST",
            "/api/system/maintenance",
            json!({"operation": "vacuum"}),
        ),
        (
            "POST",
            "/api/archive/jobs",
            json!({"reason": "Manual", "dry_run": false}),
        ),
        (
            "DELETE",
            "/api/archive/purge",
            json!({"older_than": "2020-01-01T00:00:00Z", "dry_run": false}),
        ),
    ] {
        let (status, _, json) = app.call(method, path, Some(body), None).await;
        assert_eq!(status, 500, "{method} {path}");
        assert_eq!(json["success"], false);
    }
}
