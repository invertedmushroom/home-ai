use axum::{Router, body::Bytes, extract::State, http::StatusCode, routing::post};
use opentelemetry_proto::tonic::collector::{
    metrics::v1::ExportMetricsServiceRequest, trace::v1::ExportTraceServiceRequest,
};
use prost::Message;
use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    (
        origin,
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
    )
}

async fn start(
    directory: &Path,
    model: &str,
    endpoint: Option<&str>,
    settings: &[(&str, &str)],
) -> (String, Server) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut command = Command::new(env!("CARGO_BIN_EXE_deep-server"));
    command.current_dir(directory).env_clear();
    if let Ok(root) = std::env::var("SystemRoot") {
        command.env("SystemRoot", root);
    }
    command
        .env("DEEP_BIND", address.to_string())
        .env("DEEP_DATA_DIR", directory.join("data"))
        .env("DEEP_DEFAULT_BACKEND", "compatible")
        .env("DEEP_MODEL_PROFILES_FILE", "")
        .env("DEEP_MODEL_ID", "PRIVATE_MODEL_LABEL")
        .env(
            "DEEP_MODEL_BASE_URL",
            format!("{model}/PRIVATE_RUNTIME_PATH/v1"),
        )
        .env("DEEP_MODEL_API_KEY", "PRIVATE_MODEL_CREDENTIAL")
        .env("DEEP_MODEL_TIMEOUT_SECONDS", "3")
        .env("RUST_LOG", "off")
        .env("OTEL_RESOURCE_ATTRIBUTES", "private=PRIVATE_RESOURCE")
        .env("OTEL_SERVICE_NAME", "PRIVATE_SERVICE_OVERRIDE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            std::fs::File::create(directory.join("server.log")).unwrap(),
        ));
    if let Some(endpoint) = endpoint {
        command.env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint);
    }
    command.envs(settings.iter().copied());
    let mut server = Server(command.spawn().unwrap());
    let origin = format!("http://{address}");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let end = Instant::now() + Duration::from_secs(8);
    loop {
        if client
            .get(format!("{origin}/api/status"))
            .send()
            .await
            .is_ok()
        {
            break;
        }
        if let Some(status) = server.0.try_wait().unwrap() {
            panic!(
                "Deep exited {status}: {}",
                std::fs::read_to_string(directory.join("server.log")).unwrap()
            );
        }
        assert!(Instant::now() < end, "Deep startup timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (origin, server)
}

async fn run(origin: &str) -> Value {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let workspace: Value = client
        .post(format!("{origin}/api/workspaces"))
        .json(&json!({"title":"PRIVATE_WORKSPACE_TITLE"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let accepted = client
        .post(format!(
            "{origin}/api/workspaces/{}/runs",
            workspace["id"].as_str().unwrap()
        ))
        .header("cookie", "PRIVATE_COOKIE")
        .json(&json!({"backend":"compatible","mode":"chat","prompt":"PRIVATE_PROMPT"}))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let accepted: Value = accepted.json().await.unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let state: Value = client
            .get(format!(
                "{origin}/api/runs/{}",
                accepted["id"].as_str().unwrap()
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if !matches!(state["status"].as_str(), Some("queued" | "running")) {
            return state;
        }
        assert!(Instant::now() < end, "Run did not finish");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn model() -> Router {
    Router::new().route("/PRIVATE_RUNTIME_PATH/v1/chat/completions",post(|| async {
        axum::Json(json!({"model":"PRIVATE_MODEL_LABEL","choices":[{"message":{"content":"PRIVATE_RESPONSE"},"finish_reason":"stop"}]}))
    }))
}

#[tokio::test]
async fn failed_dispatch_and_unresolved_ownership_reach_operator_metrics() {
    use opentelemetry_proto::tonic::metrics::v1::{
        metric::Data, number_data_point::Value as Number,
    };
    let captures: Arc<Mutex<Vec<Bytes>>> = Arc::default();
    let received = captures.clone();
    let (collector, collector_task) = serve(
        Router::new()
            .route("/v1/traces", post(|| async { StatusCode::OK }))
            .route(
                "/v1/metrics",
                post(move |body: Bytes| {
                    received.lock().unwrap().push(body);
                    async { StatusCode::OK }
                }),
            ),
    )
    .await;
    for connected in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (model, model_task) = if connected {
            let (origin, task) = serve(Router::new().route(
                "/PRIVATE_RUNTIME_PATH/v1/chat/completions",
                post(|| async {
                    (
                        [("Content-Type", "text/event-stream")],
                        axum::body::Body::from_stream(tokio_stream::pending::<
                            Result<Bytes, std::io::Error>,
                        >()),
                    )
                }),
            ))
            .await;
            (origin, Some(task))
        } else {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            drop(listener);
            (origin, None)
        };
        captures.lock().unwrap().clear();
        let (origin, server) = start(dir.path(), &model, Some(&collector), &[]).await;
        assert_eq!(run(&origin).await["status"], "failed");
        if connected {
            assert_eq!(run(&origin).await["status"], "failed");
        }
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            let messages: Vec<_> = captures
                .lock()
                .unwrap()
                .iter()
                .map(|body| ExportMetricsServiceRequest::decode(body.clone()).unwrap())
                .collect();
            let metrics: Vec<_> = messages
                .iter()
                .flat_map(|m| &m.resource_metrics)
                .flat_map(|r| &r.scope_metrics)
                .flat_map(|s| &s.metrics)
                .collect();
            let model_outcome = if connected {
                "unresolved"
            } else {
                "request_not_sent"
            };
            let has_outcome = metrics.iter().any(|m| {
                m.name == "deep.model.calls" && format!("{:?}", m.data).contains(model_outcome)
            });
            let has_gauge=metrics.iter().any(|m| m.name=="deep.ownership.unresolved"&&matches!(&m.data,Some(Data::Gauge(g)) if g.data_points.iter().any(|p|matches!(p.value,Some(Number::AsInt(n)) if n==i64::from(connected)))));
            if has_outcome && has_gauge {
                assert!(
                    metrics
                        .iter()
                        .any(|m| m.name == "deep.model.transport.failures")
                );
                if connected {
                    assert!(
                        metrics
                            .iter()
                            .any(|m| m.name == "deep.resource.acquisitions"
                                && format!("{:?}", m.data).contains("ownership_blocked"))
                    );
                }
                break;
            }
            assert!(
                Instant::now() < end,
                "Missing failure outcome or durable ownership gauge"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        drop(server);
        if let Some(task) = model_task {
            task.abort();
        }
    }
    collector_task.abort();
}

#[tokio::test]
async fn real_run_exports_correlated_allowlisted_spans_and_metrics() {
    type Captures = Arc<Mutex<(Vec<Bytes>, Vec<Bytes>)>>;
    async fn traces(State(captures): State<Captures>, bytes: Bytes) -> StatusCode {
        captures.lock().unwrap().0.push(bytes);
        StatusCode::OK
    }
    async fn metrics(State(captures): State<Captures>, bytes: Bytes) -> StatusCode {
        captures.lock().unwrap().1.push(bytes);
        StatusCode::OK
    }
    let captures = Captures::default();
    let (collector, collector_task) = serve(
        Router::new()
            .route("/v1/traces", post(traces))
            .route("/v1/metrics", post(metrics))
            .with_state(captures.clone()),
    )
    .await;
    let (model, model_task) = serve(model()).await;
    let dir = tempfile::tempdir().unwrap();
    let (origin, _server) = start(dir.path(), &model, Some(&collector), &[]).await;
    let completed = run(&origin).await;
    assert_eq!(completed["status"], "succeeded", "{completed}");
    let run_id = completed["id"].as_str().unwrap();
    let end = Instant::now() + Duration::from_secs(12);
    let (traces, metrics) = loop {
        let (traces, metrics) = captures.lock().unwrap().clone();
        let traces: Vec<_> = traces
            .iter()
            .map(|bytes| ExportTraceServiceRequest::decode(bytes.clone()).unwrap())
            .collect();
        let metrics: Vec<_> = metrics
            .iter()
            .map(|bytes| ExportMetricsServiceRequest::decode(bytes.clone()).unwrap())
            .collect();
        if format!("{traces:?}").contains(run_id)
            && format!("{metrics:?}").contains("deep.run.finished")
        {
            break (traces, metrics);
        }
        assert!(
            Instant::now() < end,
            "Missing run trace or metric: traces={}, metrics={}",
            traces.len(),
            metrics.len()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let spans: Vec<_> = traces
        .iter()
        .flat_map(|r| &r.resource_spans)
        .flat_map(|r| &r.scope_spans)
        .flat_map(|s| &s.spans)
        .collect();
    let root = spans.iter().find(|s| s.name == "run").unwrap();
    assert!(root.parent_span_id.is_empty());
    for name in [
        "model_stage_wait",
        "resource.wait",
        "model.operation",
        "model_call",
    ] {
        let span = spans
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("Missing {name}"));
        assert_eq!(
            span.trace_id, root.trace_id,
            "{name} detached from run trace"
        );
        assert!(!span.parent_span_id.is_empty());
    }
    let all = format!("{traces:?}{metrics:?}");
    for private in [
        "PRIVATE_PROMPT",
        "PRIVATE_RESPONSE",
        "PRIVATE_MODEL_LABEL",
        "PRIVATE_MODEL_CREDENTIAL",
        "PRIVATE_RUNTIME_PATH",
        "PRIVATE_COOKIE",
        "PRIVATE_WORKSPACE_TITLE",
        "PRIVATE_RESOURCE",
        "PRIVATE_SERVICE_OVERRIDE",
    ] {
        assert!(!all.contains(private), "Telemetry leaked {private}");
    }
    assert!(
        !format!("{metrics:?}").contains(run_id),
        "Run identity became a metric dimension"
    );
    assert!(format!("{metrics:?}").contains("deep.observability.snapshot.timestamp"));
    collector_task.abort();
    model_task.abort();
}

#[tokio::test]
async fn disabled_export_and_a_stalled_collector_do_not_delay_generation() {
    let requests = Arc::new(AtomicUsize::new(0));
    let seen = requests.clone();
    let (collector, collector_task) = serve(Router::new().fallback(post(move || {
        seen.fetch_add(1, Ordering::SeqCst);
        async { std::future::pending::<StatusCode>().await }
    })))
    .await;
    let (model, model_task) = serve(model()).await;
    for endpoint in [None, Some(collector.as_str())] {
        let dir = tempfile::tempdir().unwrap();
        let (origin, _server) = start(dir.path(), &model, endpoint, &[]).await;
        assert_eq!(run(&origin).await["status"], "succeeded");
        tokio::time::sleep(Duration::from_millis(1200)).await;
        if endpoint.is_none() {
            assert_eq!(requests.load(Ordering::SeqCst), 0);
        } else {
            assert!(
                requests.load(Ordering::SeqCst) > 0,
                "No export attempted against stalled collector"
            );
        }
        let started = Instant::now();
        assert_eq!(run(&origin).await["status"], "succeeded");
        assert!(
            started.elapsed() < Duration::from_millis(700),
            "Collector delayed generation"
        );
    }
    collector_task.abort();
    model_task.abort();
}

#[tokio::test]
async fn owned_native_recovery_remains_in_the_run_trace_after_parent_failure() {
    use axum::routing::get;
    use tokio_stream::StreamExt;
    type Captures = Arc<Mutex<(Vec<Bytes>, Vec<Bytes>)>>;
    let captures = Captures::default();
    let traces = captures.clone();
    let metrics = captures.clone();
    let (collector, collector_task) = serve(
        Router::new()
            .route(
                "/v1/traces",
                post(move |bytes: Bytes| {
                    traces.lock().unwrap().0.push(bytes);
                    async { StatusCode::OK }
                }),
            )
            .route(
                "/v1/metrics",
                post(move |bytes: Bytes| {
                    metrics.lock().unwrap().1.push(bytes);
                    async { StatusCode::OK }
                }),
            ),
    )
    .await;
    let (model,model_task)=serve(Router::new()
        .route("/metrics",get(||async {"llamacpp:requests_processing 0\nllamacpp:requests_deferred 0\n"}))
        .route("/v1/chat/completions",post(||async {
            let start=tokio_stream::iter([Ok(Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"PRIVATE_REASONING\"},\"finish_reason\":null}]}\n\n"))]);
            let rest=tokio_stream::pending::<Result<Bytes,std::io::Error>>();
            ([("Content-Type","text/event-stream")],axum::body::Body::from_stream(start.chain(rest)))
        }))).await;
    let dir = tempfile::tempdir().unwrap();
    let base = format!("{model}/v1");
    let (origin, _server) = start(
        dir.path(),
        &model,
        Some(&collector),
        &[
            ("DEEP_MODEL_RUNTIME", "llamacpp"),
            ("DEEP_MODEL_BASE_URL", &base),
            ("DEEP_MODEL_TIMEOUT_SECONDS", "1"),
        ],
    )
    .await;
    let failed = run(&origin).await;
    assert_eq!(failed["status"], "failed");
    let end = Instant::now() + Duration::from_secs(12);
    loop {
        let messages = captures.lock().unwrap().clone();
        let metrics: Vec<_> = messages
            .1
            .iter()
            .map(|bytes| ExportMetricsServiceRequest::decode(bytes.clone()).unwrap())
            .collect();
        if format!("{metrics:?}").contains("confirmed_idle") {
            assert!(format!("{metrics:?}").contains("deep.model.recoveries"));
            let traces: Vec<_> = messages
                .0
                .iter()
                .map(|bytes| ExportTraceServiceRequest::decode(bytes.clone()).unwrap())
                .collect();
            assert!(!format!("{traces:?}{metrics:?}").contains("PRIVATE_REASONING"));
            let spans: Vec<_> = traces
                .iter()
                .flat_map(|r| &r.resource_spans)
                .flat_map(|r| &r.scope_spans)
                .flat_map(|r| &r.spans)
                .collect();
            let root = spans.iter().find(|s| s.name == "run").unwrap();
            let recovery = spans.iter().find(|s| s.name == "model.recovery").unwrap();
            assert_eq!(root.trace_id, recovery.trace_id);
            assert!(!recovery.parent_span_id.is_empty());
            let connection =
                rusqlite::Connection::open(dir.path().join("data/deep.sqlite3")).unwrap();
            let state: (String, String) = connection
                .query_row(
                    "SELECT status,reason FROM model_calls WHERE run_id=?1",
                    [failed["id"].as_str().unwrap()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(
                state,
                ("resolved".to_owned(), "runtime_confirmed_idle".to_owned())
            );
            break;
        }
        assert!(Instant::now() < end, "Missing native recovery metric");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    collector_task.abort();
    model_task.abort();
}
