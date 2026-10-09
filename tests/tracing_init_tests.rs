//! `init_tracing` / `shutdown_tracing` against a local OTLP collector.
//!
//! `init_tracing` installs process-wide state (the global subscriber and tracer
//! provider), so everything runs in one test in its own test binary.

#![cfg(feature = "tracing")]

use hammerwork::{
    Job,
    tracing::{TracingConfig, create_job_span, init_tracing, shutdown_tracing},
};
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc;

/// A fake OTLP/gRPC trace collector: answers every export with an empty
/// `ExportTraceServiceResponse` and forwards the raw request bodies.
async fn fake_collector() -> (String, mpsc::UnboundedReceiver<Vec<u8>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let tx = tx.clone();
            tokio::spawn(async move {
                let Ok(mut connection) = h2::server::handshake(socket).await else {
                    return;
                };
                // The connection must keep being polled (by `accept`) while a request
                // body is read, so each request is handled in its own task.
                while let Some(Ok((request, mut respond))) = connection.accept().await {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let mut body = request.into_body();
                        let mut bytes = Vec::new();
                        while let Some(Ok(chunk)) = body.data().await {
                            let _ = body.flow_control().release_capacity(chunk.len());
                            bytes.extend_from_slice(&chunk);
                        }
                        let _ = tx.send(bytes);
                        let response = http::Response::builder()
                            .status(200)
                            .header("content-type", "application/grpc")
                            .body(())
                            .unwrap();
                        let Ok(mut stream) = respond.send_response(response, false) else {
                            return;
                        };
                        // An empty, uncompressed message.
                        let _ = stream.send_data(bytes::Bytes::from_static(&[0; 5]), false);
                        let mut trailers = http::HeaderMap::new();
                        trailers.insert("grpc-status", "0".parse().unwrap());
                        let _ = stream.send_trailers(trailers);
                    });
                }
            });
        }
    });
    (format!("http://{addr}"), rx)
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[tokio::test(flavor = "current_thread")]
async fn test_init_export_and_shutdown_tracing() {
    // An endpoint that is not a URI fails before anything global is installed.
    let err = init_tracing(TracingConfig::new().with_otlp_endpoint("not a uri"))
        .await
        .expect_err("an invalid endpoint is rejected");
    assert!(err.to_string().contains("OTLP"), "{err}");

    let (endpoint, mut exports) = fake_collector().await;
    init_tracing(
        TracingConfig::new()
            .with_service_name("hw-tracing-test")
            .with_service_version("9.9.9")
            .with_environment("ci")
            .with_resource_attribute("team", "queues")
            .with_otlp_endpoint(endpoint),
    )
    .await
    .expect("tracing initializes");

    let job = Job::new("traced_queue".to_string(), json!({}))
        .with_correlation_id("order-77")
        .with_trace_id("trace-77");
    {
        let span = create_job_span(&job, "job.traced");
        let _entered = span.enter();
        tracing::info!("processing");
    }

    // Shutting down flushes the batch to the collector. On a current-thread runtime
    // this used to block the only runtime thread, which the exporter needs, until the
    // provider's 5 second shutdown timeout.
    let started = std::time::Instant::now();
    shutdown_tracing().await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "shutdown took {:?}",
        started.elapsed()
    );
    let body = tokio::time::timeout(Duration::from_secs(10), exports.recv())
        .await
        .expect("spans are exported on shutdown")
        .expect("collector running");
    for expected in [
        "hw-tracing-test",
        "9.9.9",
        "deployment.environment",
        "ci",
        "team",
        "queues",
        "job.traced",
        "traced_queue",
        "order-77",
        &job.id.to_string(),
    ] {
        assert!(contains(&body, expected), "export is missing {expected:?}");
    }

    // A second shutdown has nothing left to shut down.
    shutdown_tracing().await;

    // The global subscriber can only be installed once.
    let err = init_tracing(TracingConfig::new().with_console_exporter(true))
        .await
        .expect_err("a second initialization fails");
    assert!(
        err.to_string()
            .contains("Failed to initialize tracing subscriber"),
        "{err}"
    );
    let err = init_tracing(TracingConfig::new())
        .await
        .expect_err("a second initialization fails");
    assert!(err.to_string().contains("tracing subscriber"), "{err}");
    shutdown_tracing().await;
}
