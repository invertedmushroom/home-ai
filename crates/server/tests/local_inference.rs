use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use deep_server::{
    App,
    config::{BackendConfig, Config, LocalRuntime},
    router,
    store::Store,
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;
use tower::ServiceExt;

async fn request(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("Host", "127.0.0.1")
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_097_152).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}

async fn submit(app: &Router, backend: &str) -> Value {
    let (_, workspace) = request(
        app,
        "POST",
        "/api/workspaces",
        json!({"title":"Ownership fixture"}),
    )
    .await;
    let (status, run) = request(
        app,
        "POST",
        &format!("/api/workspaces/{}/runs", workspace["id"].as_str().unwrap()),
        json!({"mode":"chat","prompt":"Fixture request","backend":backend}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    run
}

async fn terminal(app: &Router, run: &Value) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (_, state) = request(
                app,
                "GET",
                &format!("/api/runs/{}", run["id"].as_str().unwrap()),
                Value::Null,
            )
            .await;
            if !matches!(state["status"].as_str(), Some("queued" | "running")) {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

async fn phase(app: &Router, run: &Value, expected: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (_, state) = request(
                app,
                "GET",
                &format!("/api/runs/{}", run["id"].as_str().unwrap()),
                Value::Null,
            )
            .await;
            if state["local_model"]["phase"] == expected {
                assert_eq!(state["local_model"].as_object().unwrap().len(), 3);
                assert!(!state["local_model"].to_string().contains("127.0.0.1"));
                return state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

async fn event(sender: &mpsc::Sender<Result<Bytes, std::io::Error>>, value: Value) {
    sender
        .send(Ok(Bytes::from(format!("data: {value}\n\n"))))
        .await
        .unwrap();
}

async fn completed(sender: &mpsc::Sender<Result<Bytes, std::io::Error>>, reason: &str) {
    event(
        sender,
        json!({"choices":[{"delta":{"content":"Late result"},"finish_reason":reason}]}),
    )
    .await;
    sender
        .send(Ok(Bytes::from_static(b"data: [DONE]\n\n")))
        .await
        .unwrap();
}

async fn ownership(path: &std::path::Path, expected: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let c = rusqlite::Connection::open(path).unwrap();
            let state: String = c
                .query_row(
                    "SELECT status FROM model_calls ORDER BY created_at_ms LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            if state == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn fixture(
    timeout: Duration,
    metrics: Router,
) -> (
    Config,
    tokio::task::JoinHandle<()>,
    mpsc::Sender<Result<Bytes, std::io::Error>>,
    Arc<AtomicUsize>,
) {
    let (sender, receiver) = mpsc::channel(8);
    let receiver = Arc::new(Mutex::new(Some(receiver)));
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let handler = post(move || {
        let first = count.fetch_add(1, Ordering::SeqCst) == 0;
        let receiver = if first {
            receiver.lock().unwrap().take()
        } else {
            None
        };
        async move {
            if let Some(receiver) = receiver {
                (
                    [("Content-Type", "text/event-stream")],
                    Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(receiver)),
                )
                    .into_response()
            } else {
                axum::Json(
                    json!({"choices":[{"message":{"content":"Done"},"finish_reason":"stop"}]}),
                )
                .into_response()
            }
        }
    });
    let slow_count = calls.clone();
    let backend = Router::new()
        .route("/v1/chat/completions", handler.clone())
        .route("/independent/v1/chat/completions", handler)
        .route(
            "/slow/v1/chat/completions",
            post(move || {
                slow_count.fetch_add(1, Ordering::SeqCst);
                async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    axum::Json(
                        json!({"choices":[{"message":{"content":"Done"},"finish_reason":"stop"}]}),
                    )
                }
            }),
        );
    let backend = backend.merge(metrics);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, backend).await.unwrap() });
    let cfg = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        public_origin: None,
        data_dir: ".".into(),
        web_dir: ".".into(),
        default_backend: "compatible".into(),
        worker_image: None,
        search: None,
        image_profiles: Vec::new(),
        model_catalog: Default::default(),
        backends: vec![
            BackendConfig {
                kind: deep_server::config::BackendKind::Compatible,
                runtime: None,
                resource_group: None,
                id: "compatible".into(),
                base_url: base_url.clone(),
                model: "fixture".into(),
                api_key: None,
                timeout,
                max_output_tokens: 128,
            },
            BackendConfig {
                kind: deep_server::config::BackendKind::Openai,
                runtime: None,
                resource_group: None,
                id: "openai".into(),
                base_url,
                model: "hosted-fixture".into(),
                api_key: Some("fixture".into()),
                timeout,
                max_output_tokens: 128,
            },
        ],
    };
    (cfg, task, sender, calls)
}

#[tokio::test]
async fn stop_detaches_owned_stream_until_terminal_and_blocks_only_conflicting_inference() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep.sqlite3");
    let store = Store::open(&path).unwrap();
    let (cfg, task, sender, calls) = fixture(Duration::from_secs(5), Router::new()).await;
    let app = router(App::new(cfg, store.clone()).unwrap());
    let run = submit(&app, "compatible").await;
    event(&sender, json!({"choices":[{"delta":{"content":"Draft"}}]})).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if store
                .run(run["id"].as_str().unwrap().into())
                .await
                .unwrap()
                .unwrap()
                .output
                == "Draft"
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let (_, cancelled) = request(
        &app,
        "DELETE",
        &format!("/api/runs/{}", run["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(cancelled["status"], "cancelled");
    phase(&app, &run, "draining").await;
    ownership(&path, "dispatching").await;
    let blocked = submit(&app, "compatible").await;
    tokio::time::sleep(Duration::from_millis(75)).await;
    let (_, queued) = request(
        &app,
        "GET",
        &format!("/api/runs/{}", blocked["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(queued["status"], "queued");
    assert_eq!(queued["local_model"]["phase"], "waiting");
    assert!(queued["local_model"]["operation_id"].is_null());
    assert!(queued["duration_ms"].is_null());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/api/workspaces/{}", run["workspace_id"].as_str().unwrap()),
            Value::Null
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let cloud = submit(&app, "openai").await;
    assert_eq!(terminal(&app, &cloud).await["status"], "succeeded");
    completed(&sender, "stop").await;
    ownership(&path, "completed").await;
    assert_eq!(terminal(&app, &blocked).await["status"], "succeeded");
    let next = submit(&app, "compatible").await;
    assert_eq!(terminal(&app, &next).await["status"], "succeeded");
    let final_run = terminal(&app, &run).await;
    phase(&app, &run, "settled").await;
    assert_eq!(final_run["status"], "cancelled");
    assert_eq!(final_run["output"], "Draft");
    assert_eq!(final_run["duration_ms"], cancelled["duration_ms"]);
    assert!(
        store
            .history(run["workspace_id"].as_str().unwrap().into())
            .await
            .unwrap()
            .is_empty()
    );
    task.abort();
}

#[tokio::test]
async fn cancelled_first_stage_waiter_sends_nothing_and_queue_time_ends_at_acquisition() {
    let (cfg, task, sender, calls) = fixture(Duration::from_secs(5), Router::new()).await;
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(cfg, store).unwrap());
    let first = submit(&app, "compatible").await;
    event(
        &sender,
        json!({"choices":[{"delta":{"content":"Holding"}}]}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while calls.load(Ordering::SeqCst) != 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let cancelled = submit(&app, "compatible").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (_, stopped) = request(
        &app,
        "DELETE",
        &format!("/api/runs/{}", cancelled["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(stopped["status"], "cancelled");
    assert!(stopped["local_model"].is_null());
    assert!(stopped["duration_ms"].is_null());
    let next = submit(&app, "compatible").await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    completed(&sender, "stop").await;
    assert_eq!(terminal(&app, &first).await["status"], "succeeded");
    let done = terminal(&app, &next).await;
    assert_eq!(done["status"], "succeeded");
    assert!(done["queue_ms"].as_u64().unwrap() >= 100);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn stop_before_the_first_token_drains_the_exact_lmstudio_or_compatible_call() {
    for backend in ["compatible", "lmstudio"] {
        let (mut cfg, task, sender, calls) = fixture(Duration::from_secs(5), Router::new()).await;
        cfg.backends[0].id = backend.into();
        cfg.backends[0].kind = if backend == "lmstudio" {
            deep_server::config::BackendKind::Lmstudio
        } else {
            deep_server::config::BackendKind::Compatible
        };
        cfg.default_backend = backend.into();
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let app = router(App::new(cfg, store.clone()).unwrap());
        let first = submit(&app, backend).await;
        let active = phase(&app, &first, "working").await;
        assert_eq!(active["output"], "");
        let operation = active["local_model"]["operation_id"].clone();
        let (_, stopped) = request(
            &app,
            "DELETE",
            &format!("/api/runs/{}", first["id"].as_str().unwrap()),
            Value::Null,
        )
        .await;
        assert_eq!(stopped["status"], "cancelled");
        let draining = phase(&app, &first, "draining").await;
        assert_eq!(draining["local_model"]["operation_id"], operation);
        let next = submit(&app, backend).await;
        phase(&app, &next, "waiting").await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        completed(&sender, "stop").await;
        assert_eq!(terminal(&app, &next).await["status"], "succeeded");
        let settled = phase(&app, &first, "settled").await;
        assert_eq!(settled["status"], "cancelled");
        assert_eq!(settled["output"], "");
        assert_eq!(settled["local_model"]["operation_id"], operation);
        assert!(
            store
                .history(first["workspace_id"].as_str().unwrap().into())
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        task.abort();
    }
}

#[tokio::test]
async fn local_transport_timeout_preserves_ownership_and_sends_no_conflicting_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep.sqlite3");
    let store = Store::open(&path).unwrap();
    let (cfg, task, sender, calls) = fixture(Duration::from_millis(150), Router::new()).await;
    let app = router(App::new(cfg, store).unwrap());
    let run = submit(&app, "compatible").await;
    event(&sender,json!({"choices":[{"delta":{"reasoning_content":"Unpublished reasoning"},"finish_reason":null}]})).await;
    let timed_out = terminal(&app, &run).await;
    assert_eq!(timed_out["local_model"]["phase"], "blocked");
    assert_eq!(timed_out["status"], "failed");
    assert_eq!(timed_out["output"], "");
    let error = timed_out["error"].as_str().unwrap();
    assert!(error.contains("0.15-second timeout"), "{error}");
    assert!(error.contains("DEEP_MODEL_TIMEOUT_SECONDS"));
    assert!(!error.contains("127.0.0.1"));
    ownership(&path, "unresolved").await;
    let blocked = submit(&app, "compatible").await;
    let blocked = terminal(&app, &blocked).await;
    assert_eq!(blocked["status"], "failed");
    let error = blocked["error"].as_str().unwrap();
    assert!(error.contains("earlier operation"), "{error}");
    assert!(error.contains("operator must inspect"));
    assert!(!error.contains("127.0.0.1"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn header_timeout_reports_deadline_without_exposing_the_runtime_url() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep.sqlite3");
    let (mut cfg, task, _sender, calls) = fixture(Duration::from_millis(150), Router::new()).await;
    cfg.backends[0].base_url = cfg.backends[0].base_url.replace("/v1", "/slow/v1");
    let app = router(App::new(cfg, Store::open(&path).unwrap()).unwrap());
    let run = submit(&app, "compatible").await;
    let done = terminal(&app, &run).await;
    assert_eq!(done["status"], "failed");
    let error = done["error"].as_str().unwrap();
    assert!(error.contains("0.15-second timeout"), "{error}");
    assert!(!error.contains("127.0.0.1"));
    ownership(&path, "unresolved").await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn connection_refusal_releases_the_group_without_retry_or_manual_recovery() {
    for (backend, runtime) in [
        ("compatible", None),
        ("compatible", Some(LocalRuntime::LlamaCpp)),
        ("lmstudio", None),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep.sqlite3");
        let (mut cfg, fixture_task, sender, calls) =
            fixture(Duration::from_secs(3), Router::new()).await;
        let offline = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = offline.local_addr().unwrap();
        drop(offline);
        cfg.default_backend = backend.into();
        cfg.backends[0].id = backend.into();
        cfg.backends[0].kind = serde_json::from_value(serde_json::json!(backend)).unwrap();
        cfg.backends[0].base_url = format!("http://{address}/v1");
        cfg.backends[0].runtime = runtime;
        // A different backend on this same group must also remain usable.
        cfg.backends[1].resource_group = Some("local-gpu".into());
        let app = router(App::new(cfg, Store::open(&path).unwrap()).unwrap());
        for _ in 0..2 {
            let first = submit(&app, backend).await;
            let done = terminal(&app, &first).await;
            assert_eq!(done["status"], "failed");
            let error = done["error"].as_str().unwrap();
            assert!(error.contains("connection failed"), "{error}");
            assert!(!error.contains("127.0.0.1"));
            let c = rusqlite::Connection::open(&path).unwrap();
            let (state, reason, finished, replies): (String,String,Option<i64>,i64) = c.query_row(
                "SELECT status,reason,finished_at_ms,(SELECT count(*) FROM messages WHERE run_id=?1 AND role='assistant') FROM model_calls WHERE run_id=?1",
                [first["id"].as_str().unwrap()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            ).unwrap();
            assert_eq!(
                (state.as_str(), reason.as_str(), replies),
                ("resolved", "request_not_sent", 0)
            );
            assert!(finished.is_some());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        completed(&sender, "stop").await;
        let independent = submit(&app, "openai").await;
        assert_eq!(terminal(&app, &independent).await["status"], "succeeded");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let restored_calls = Arc::new(AtomicUsize::new(0));
        let observed = restored_calls.clone();
        let restored = Router::new().route("/v1/chat/completions", post(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            async { axum::Json(json!({"choices":[{"message":{"content":"Back online"},"finish_reason":"stop"}]})) }
        }));
        let listener = tokio::net::TcpListener::bind(address).await.unwrap();
        let restored_task =
            tokio::spawn(async move { axum::serve(listener, restored).await.unwrap() });
        let followup = submit(&app, backend).await;
        assert_eq!(terminal(&app, &followup).await["status"], "succeeded");
        assert_eq!(restored_calls.load(Ordering::SeqCst), 1);
        fixture_task.abort();
        restored_task.abort();
    }
}

#[tokio::test]
async fn connection_lost_after_request_bytes_still_requires_completion_evidence() {
    use tokio::io::AsyncReadExt;
    for runtime in [None, Some(LocalRuntime::LlamaCpp)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep.sqlite3");
        let (mut cfg, fixture_task, _sender, _calls) =
            fixture(Duration::from_secs(3), Router::new()).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        cfg.backends[0].base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        cfg.backends[0].runtime = runtime;
        let received = Arc::new(AtomicUsize::new(0));
        let observed = received.clone();
        let broken_task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let count = socket.read(&mut [0; 4096]).await.unwrap();
                assert!(count > 0);
                observed.fetch_add(1, Ordering::SeqCst);
                // The request may have arrived, but no response confirms its outcome.
                drop(socket);
            }
        });
        let app = router(App::new(cfg, Store::open(&path).unwrap()).unwrap());
        let first = submit(&app, "compatible").await;
        let failed = terminal(&app, &first).await;
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["local_model"]["phase"], "blocked");
        ownership(&path, "unresolved").await;
        let second = submit(&app, "compatible").await;
        let blocked = terminal(&app, &second).await;
        assert_eq!(blocked["status"], "failed");
        assert!(blocked["error"].as_str().unwrap().contains("blocked"));
        assert_eq!(received.load(Ordering::SeqCst), 1);
        fixture_task.abort();
        broken_task.abort();
    }
}

#[tokio::test]
async fn native_validation_rejections_retire_ownership_without_hiding_the_failed_run() {
    use tokio_stream::StreamExt;
    for (native, scenario, terminal_response) in [
        (true, "validation", true),
        (true, "context", true),
        (false, "validation", false),
        (true, "malformed", false),
        (true, "wrong_type", false),
        (true, "missing_code", false),
        (true, "oversized", false),
        (true, "truncated", false),
        (true, "stalled", false),
        (true, "wrong_mime", false),
        (true, "server_error", false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep.sqlite3");
        let (mut cfg, unused, _sender, _count) =
            fixture(Duration::from_secs(4), Router::new()).await;
        let count = Arc::new(AtomicUsize::new(0));
        let observed = count.clone();
        let handler=Router::new().route("/v1/chat/completions",post(move || {
            let first=observed.fetch_add(1,Ordering::SeqCst)==0;
            async move {
                if !first {
                    return axum::Json(json!({"choices":[{"message":{"content":"Fresh request works"},"finish_reason":"stop"}]})).into_response();
                }
                let mut value=json!({"error":{"code":400,"type":"invalid_request_error","message":"private-rejection-value"}});
                if scenario=="context" {value["error"]["type"]=json!("exceed_context_size_error");}
                if scenario=="wrong_type" {value["error"]["type"]=json!("server_error");}
                if scenario=="missing_code" {value["error"].as_object_mut().unwrap().remove("code");}
                if scenario=="oversized" {value["error"]["message"]=json!("x".repeat(8193));}
                let bytes=if scenario=="malformed" {"private-rejection-value".to_owned()} else {value.to_string()};
                let body=match scenario {
                    "truncated" => Body::from_stream(tokio_stream::iter([
                        Ok(Bytes::from_static(b"{\"error\":")),
                        Err(std::io::Error::other("private-transport-value")),
                    ])),
                    "stalled" => Body::from_stream(tokio_stream::once(Ok::<_,std::io::Error>(Bytes::from(bytes))).then(|bytes|async move {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        bytes
                    })),
                    _ => Body::from(bytes),
                };
                (if scenario=="server_error" {StatusCode::INTERNAL_SERVER_ERROR}else{StatusCode::BAD_REQUEST},
                 [("Content-Type",if scenario=="wrong_mime" {"text/plain"}else{"application/json; charset=utf-8"})],body).into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        cfg.backends[0].base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        cfg.backends[0].runtime = native.then_some(LocalRuntime::LlamaCpp);
        let mut shared = cfg.backends[0].clone();
        shared.id = "shared-local".into();
        shared.runtime = None;
        cfg.backends.push(shared);
        let remote = tokio::spawn(async move { axum::serve(listener, handler).await.unwrap() });
        let app = router(App::new(cfg, Store::open(&path).unwrap()).unwrap());
        let run = submit(&app, "compatible").await;
        let done = terminal(&app, &run).await;
        assert_eq!(done["status"], "failed", "{scenario}: {done}");
        assert!(!done.to_string().contains("private-"));
        let c = rusqlite::Connection::open(&path).unwrap();
        let (status,reason,finished,replies):(String,String,Option<i64>,i64)=c.query_row(
            "SELECT status,reason,finished_at_ms,(SELECT count(*) FROM messages WHERE run_id=?1 AND role='assistant') FROM model_calls WHERE run_id=?1",
            [run["id"].as_str().unwrap()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ).unwrap();
        assert_eq!(
            status,
            if terminal_response {
                "completed"
            } else {
                "unresolved"
            },
            "{scenario}"
        );
        assert_eq!(
            reason,
            if terminal_response {
                "terminal_response"
            } else {
                "upstream_completion_unconfirmed"
            }
        );
        assert_eq!(finished.is_some(), terminal_response);
        assert_eq!(replies, 0);
        assert_eq!(count.load(Ordering::SeqCst), 1, "No automatic retry");
        let next = submit(&app, "shared-local").await;
        let next = terminal(&app, &next).await;
        assert_eq!(
            next["status"],
            if terminal_response {
                "succeeded"
            } else {
                "failed"
            },
            "{scenario}: {next}"
        );
        assert_eq!(
            count.load(Ordering::SeqCst),
            if terminal_response { 2 } else { 1 }
        );
        unused.abort();
        remote.abort();
    }
}

#[tokio::test]
async fn llamacpp_started_timeout_keeps_group_until_runtime_confirms_idle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep.sqlite3");
    let busy = Arc::new(AtomicBool::new(true));
    let probes = Arc::new(AtomicUsize::new(0));
    let current = busy.clone();
    let observed = probes.clone();
    let metrics = Router::new().route(
        "/metrics",
        get(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            let busy = current.load(Ordering::SeqCst);
            async move {
                format!(
                    "llamacpp:requests_processing {}\nllamacpp:requests_deferred 0\n",
                    u8::from(busy)
                )
            }
        }),
    );
    let (mut cfg, task, sender, calls) = fixture(Duration::from_millis(150), metrics).await;
    cfg.backends[0].runtime = Some(LocalRuntime::LlamaCpp);
    let app = router(App::new(cfg, Store::open(&path).unwrap()).unwrap());
    let first = submit(&app, "compatible").await;
    event(
        &sender,
        json!({"choices":[{"delta":{"reasoning_content":"Reasoning token"},"finish_reason":null}]}),
    )
    .await;
    assert_eq!(terminal(&app, &first).await["status"], "failed");
    phase(&app, &first, "recovering").await;
    ownership(&path, "unresolved").await;
    let second = submit(&app, "compatible").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (_, waiting) = request(
        &app,
        "GET",
        &format!("/api/runs/{}", second["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(waiting["status"], "queued");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    busy.store(false, Ordering::SeqCst);
    assert_eq!(terminal(&app, &second).await["status"], "succeeded");
    ownership(&path, "resolved").await;
    phase(&app, &first, "settled").await;
    assert!(probes.load(Ordering::SeqCst) >= 3);
    let c = rusqlite::Connection::open(&path).unwrap();
    let (state,reason,replies):(String,String,i64) = c.query_row(
        "SELECT r.status,m.reason,(SELECT count(*) FROM messages WHERE run_id=r.id AND role='assistant') FROM runs r JOIN model_calls m ON m.run_id=r.id WHERE r.id=?1",
        [first["id"].as_str().unwrap()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))
    ).unwrap();
    assert_eq!(
        (state.as_str(), reason.as_str(), replies),
        ("failed", "runtime_confirmed_idle", 0)
    );
    task.abort();
}

#[tokio::test]
async fn llamacpp_stop_does_not_release_group_on_deferred_work() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep.sqlite3");
    let deferred = Arc::new(AtomicBool::new(true));
    let queued = deferred.clone();
    let metrics = Router::new().route(
        "/metrics",
        get(move || {
            let deferred = queued.load(Ordering::SeqCst);
            async move {
                format!(
                    "llamacpp:requests_processing 0\nllamacpp:requests_deferred {}\n",
                    u8::from(deferred)
                )
            }
        }),
    );
    let (mut cfg, task, sender, calls) = fixture(Duration::from_secs(5), metrics).await;
    cfg.backends[0].runtime = Some(LocalRuntime::LlamaCpp);
    let app = router(App::new(cfg, Store::open(&path).unwrap()).unwrap());
    let first = submit(&app, "compatible").await;
    event(
        &sender,
        json!({"choices":[{"delta":{"content":"Partial answer"},"finish_reason":null}]}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let (_, run) = request(
                &app,
                "GET",
                &format!("/api/runs/{}", first["id"].as_str().unwrap()),
                Value::Null,
            )
            .await;
            if run["output"] == "Partial answer" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    request(
        &app,
        "DELETE",
        &format!("/api/runs/{}", first["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(terminal(&app, &first).await["status"], "cancelled");
    ownership(&path, "unresolved").await;
    let second = submit(&app, "compatible").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    deferred.store(false, Ordering::SeqCst);
    assert_eq!(terminal(&app, &second).await["status"], "succeeded");
    ownership(&path, "resolved").await;
    assert_eq!(terminal(&app, &first).await["status"], "cancelled");
    task.abort();
}

#[tokio::test]
async fn llamacpp_header_only_timeout_cannot_use_idle_metrics_as_admission_proof() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep.sqlite3");
    let probes = Arc::new(AtomicUsize::new(0));
    let observed = probes.clone();
    let metrics = Router::new().route(
        "/metrics",
        get(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            async { "llamacpp:requests_processing 0\nllamacpp:requests_deferred 0\n" }
        }),
    );
    let (mut cfg, task, _sender, calls) = fixture(Duration::from_millis(150), metrics).await;
    cfg.backends[0].runtime = Some(LocalRuntime::LlamaCpp);
    let app = router(App::new(cfg, Store::open(&path).unwrap()).unwrap());
    let run = submit(&app, "compatible").await;
    assert_eq!(terminal(&app, &run).await["status"], "failed");
    ownership(&path, "unresolved").await;
    let blocked = submit(&app, "compatible").await;
    assert_eq!(terminal(&app, &blocked).await["status"], "failed");
    assert_eq!(probes.load(Ordering::SeqCst), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn llamacpp_unavailable_invalid_or_redirected_metrics_never_resolve_ownership() {
    for (status, body) in [
        (StatusCode::NOT_IMPLEMENTED, "metrics disabled".to_owned()),
        (
            StatusCode::OK,
            "llamacpp:requests_processing 0\n".to_owned(),
        ),
        (StatusCode::OK, "x".repeat(65537)),
        (StatusCode::FOUND, "".to_owned()),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep.sqlite3");
        let probes = Arc::new(AtomicUsize::new(0));
        let observed = probes.clone();
        let metrics = Router::new().route(
            "/metrics",
            get(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                let body = body.clone();
                async move { (status, [("Location", "/v1/chat/completions")], body) }
            }),
        );
        let (mut cfg, task, sender, calls) = fixture(Duration::from_millis(150), metrics).await;
        cfg.backends[0].runtime = Some(LocalRuntime::LlamaCpp);
        let app = router(App::new(cfg, Store::open(&path).unwrap()).unwrap());
        let run = submit(&app, "compatible").await;
        event(
            &sender,
            json!({"choices":[{"delta":{"content":"Generated"},"finish_reason":null}]}),
        )
        .await;
        assert_eq!(terminal(&app, &run).await["status"], "failed");
        ownership(&path, "unresolved").await;
        let blocked = submit(&app, "compatible").await;
        assert_eq!(terminal(&app, &blocked).await["status"], "failed");
        assert_eq!(probes.load(Ordering::SeqCst), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        task.abort();
    }
}

#[tokio::test]
async fn streamed_errors_drain_to_completion_but_remain_fenced_without_a_marker() {
    for (frame, expected) in [
        (
            r#"{"error":{"message":"private upstream detail"}}"#,
            "Model stream reported an error.",
        ),
        ("{invalid-json", "Model stream returned invalid JSON."),
        (r#"{"model":42}"#, "Model returned invalid model identity."),
    ] {
        for terminal_marker in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("deep.sqlite3");
            let (cfg, task, sender, calls) = fixture(Duration::from_secs(5), Router::new()).await;
            let app = router(App::new(cfg, Store::open(&path).unwrap()).unwrap());
            let run = submit(&app, "compatible").await;
            sender
                .send(Ok(Bytes::from(format!("data: {frame}\n\n"))))
                .await
                .unwrap();
            if terminal_marker {
                // Separate chunks exercise continued reading after the rejected frame.
                tokio::time::sleep(Duration::from_millis(20)).await;
                sender
                    .send(Ok(Bytes::from_static(b"data: [DONE]\n\n")))
                    .await
                    .unwrap();
            }
            drop(sender);
            let failed = terminal(&app, &run).await;
            assert_eq!(failed["status"], "failed");
            assert_eq!(failed["error"], expected);
            ownership(
                &path,
                if terminal_marker {
                    "completed"
                } else {
                    "unresolved"
                },
            )
            .await;
            let c = rusqlite::Connection::open(&path).unwrap();
            let replies: i64 = c
                .query_row(
                    "SELECT count(*) FROM messages WHERE run_id=?1 AND role='assistant'",
                    [run["id"].as_str().unwrap()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(replies, 0);
            let next = submit(&app, "compatible").await;
            assert_eq!(
                terminal(&app, &next).await["status"],
                if terminal_marker {
                    "succeeded"
                } else {
                    "failed"
                }
            );
            assert_eq!(
                calls.load(Ordering::SeqCst),
                if terminal_marker { 2 } else { 1 }
            );
            task.abort();
        }
    }
}

#[tokio::test]
async fn terminal_evidence_releases_ownership_even_when_the_model_output_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep.sqlite3");
    let (cfg, task, sender, calls) = fixture(Duration::from_secs(5), Router::new()).await;
    let app = router(App::new(cfg, Store::open(&path).unwrap()).unwrap());
    let run = submit(&app, "compatible").await;
    completed(&sender, "length").await;
    assert_eq!(terminal(&app, &run).await["status"], "failed");
    ownership(&path, "completed").await;
    let next = submit(&app, "compatible").await;
    assert_eq!(terminal(&app, &next).await["status"], "succeeded");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn subscriber_storage_failure_does_not_abort_owned_transport_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep.sqlite3");
    let (cfg, task, sender, _) = fixture(Duration::from_secs(5), Router::new()).await;
    let app = router(App::new(cfg, Store::open(&path).unwrap()).unwrap());
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("CREATE TRIGGER fail_draft BEFORE UPDATE OF output ON runs WHEN NEW.output <> '' BEGIN SELECT RAISE(FAIL,'fixture storage failure'); END;").unwrap();
    let run = submit(&app, "compatible").await;
    event(&sender, json!({"choices":[{"delta":{"content":"Draft"}}]})).await;
    assert_eq!(terminal(&app, &run).await["status"], "failed");
    ownership(&path, "dispatching").await;
    completed(&sender, "stop").await;
    ownership(&path, "completed").await;
    c.execute_batch("DROP TRIGGER fail_draft;").unwrap();
    let next = submit(&app, "compatible").await;
    assert_eq!(terminal(&app, &next).await["status"], "succeeded");
    task.abort();
}

#[test]
fn ownership_cli_requires_an_offline_database_and_explicit_resolution_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep.sqlite3");
    let store = Store::open(&path).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_deep-server"))
        .arg("model-calls")
        .env("DEEP_DATA_DIR", dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("another Deep process"));
    drop(store);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_deep-server"))
        .arg("model-calls")
        .env("DEEP_DATA_DIR", dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!([])
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_deep-server"))
        .args(["model-calls", "--resolve", "any-id"])
        .env("DEEP_DATA_DIR", dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--confirm-idle"));
}

#[tokio::test]
async fn independent_managed_groups_can_complete_while_another_model_is_running() {
    let (mut cfg, task, sender, calls) = fixture(Duration::from_secs(5), Router::new()).await;
    let mut second = cfg.backends[0].clone();
    second.id = "lmstudio".into();
    second.kind = deep_server::config::BackendKind::Lmstudio;
    second.resource_group = Some("independent-gpu".into());
    second.base_url = format!(
        "{}/independent/v1",
        second.base_url.strip_suffix("/v1").unwrap()
    );
    cfg.backends.push(second);
    let app =
        router(App::new(cfg, Store::open(std::path::Path::new(":memory:")).unwrap()).unwrap());
    let first = submit(&app, "compatible").await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while calls.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let second = submit(&app, "lmstudio").await;
    assert_eq!(terminal(&app, &second).await["status"], "succeeded");
    let (_, still_running) = request(
        &app,
        "GET",
        &format!("/api/runs/{}", first["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(still_running["status"], "running");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    completed(&sender, "stop").await;
    assert_eq!(terminal(&app, &first).await["status"], "succeeded");
    task.abort();
}
