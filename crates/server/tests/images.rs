use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use deep_server::{
    App,
    config::{BackendConfig, Config},
    images::ImageProfile,
    router,
    store::Store,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::Cursor,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;
use tower::ServiceExt;

struct Runtime {
    image: Vec<u8>,
    bad: &'static str,
    submissions: AtomicUsize,
    cancellations: AtomicUsize,
    complete: AtomicBool,
    cancelled: AtomicBool,
    fail_reads: AtomicBool,
    settings: Mutex<Vec<Value>>,
    progress: Mutex<Value>,
    uploaded: Mutex<Vec<u8>>,
    hold_upload: Semaphore,
    hold_submit: Semaphore,
    hold_preflight: Semaphore,
    hold_model: Semaphore,
    stages: Mutex<Vec<String>>,
    discovery_calls: Mutex<Vec<Value>>,
}

#[derive(Clone)]
struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(width, height)
        .write_to(&mut buffer, image::ImageFormat::Png)
        .unwrap();
    buffer.into_inner()
}

async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, task)
}

fn remote_job(runtime: &Runtime) -> Value {
    let done = runtime.complete.load(Ordering::SeqCst);
    let cancelled = runtime.cancelled.load(Ordering::SeqCst);
    json!({
        "job_id":"remote_job_one","agent_context":"context_one","done":done,
        "status":if done {"done"}else{"running"},
        "progress":if done {Value::Null}else{runtime.progress.lock().unwrap().clone()},
        "result":if done {json!({
            "success":!cancelled,"cancelled":cancelled,
            "gallery_items":if cancelled {vec![]} else {vec![json!({
                "media_id":"visual:one","media_type":"image","role":"final",
                "task_id":if runtime.bad == "foreign_task" {"mcp_somebody_else_1"}else{"mcp_remote_job_one_1"}
            })]}
        })}else{Value::Null}
    })
}

async fn mcp(State(runtime): State<Arc<Runtime>>, Json(body): Json<Value>) -> Response {
    if body["method"] == "notifications/initialized" {
        return StatusCode::ACCEPTED.into_response();
    }
    let result = if body["method"] == "initialize" {
        json!({"protocolVersion":"2025-03-26","serverInfo":{"name":"WanGP"}})
    } else {
        assert_eq!(body["method"], "tools/call");
        let args = &body["params"]["arguments"];
        runtime
            .discovery_calls
            .lock()
            .unwrap()
            .push(body["params"].clone());
        let value = match body["params"]["name"].as_str().unwrap() {
            "wangp_models" => {
                if runtime.bad == "inventory_oversize" {
                    json!({"models":vec![json!({"model_type":"flux2_klein_4b","name":"Klein"});257],"has_more":false})
                } else if runtime.bad == "inventory_cursor" {
                    json!({"models":[],"has_more":true,"next_cursor":"repeated"})
                } else if runtime.bad == "inventory_duplicate" {
                    json!({"models":[{"model_type":"flux2_klein_4b","name":"Klein"},{"model_type":"flux2_klein_4b","name":"Klein"}],"has_more":false})
                } else if args["arguments"]["limit"] == 50 && args["arguments"]["cursor"].is_null()
                {
                    json!({"models":[{"model_type":"flux2_klein_4b","name":"Flux Klein"}],"has_more":true,"next_cursor":"model_page_two"})
                } else if args["arguments"]["cursor"] == "model_page_two" {
                    json!({"models":[{"model_type":"vace_1.3B","name":"Other image-capable model"}],"has_more":false})
                } else {
                    json!({"models":[{"model_type":"flux2_klein_4b"}],"has_more":false})
                }
            }
            "wangp_workspace" => json!({"agent_context":"context_one"}),
            "wangp_model" => {
                if args["action"] == "loras" {
                    if runtime.bad == "inventory_invalid" {
                        json!({"loras":["../private-secret"],"has_more":false})
                    } else {
                        json!({"loras":["selfie.safetensors","neon.safetensors"],"has_more":false})
                    }
                } else {
                    json!({"metadata":{"model_type":"flux2_klein_4b",
                    "capabilities":if runtime.bad == "unsupported_reference" {vec![]} else {vec!["reference_images","lora"]}}})
                }
            }
            "wangp_validate" => {
                if runtime.bad == "hold_preflight" {
                    runtime.hold_preflight.acquire().await.unwrap().forget();
                }
                json!({"valid":true,"submitted":false,"tasks":[{"model_type":"flux2_klein_4b","availability":{"model_type":"flux2_klein_4b","available":true}}]})
            }
            "wangp_generate" => {
                runtime.submissions.fetch_add(1, Ordering::SeqCst);
                runtime.settings.lock().unwrap().push(args.clone());
                if runtime.bad == "queued_work" {
                    runtime
                        .stages
                        .lock()
                        .unwrap()
                        .push(format!("image:{}", args["request_id"].as_str().unwrap()));
                    runtime.hold_submit.acquire().await.unwrap().forget();
                }
                if runtime.bad == "lost_submission" {
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                if runtime.bad == "hold_submit" {
                    runtime.hold_submit.acquire().await.unwrap().forget();
                }
                let mut value = remote_job(&runtime);
                value["request_id"] = args["request_id"].clone();
                value
            }
            "wangp_session" => match args["action"].as_str().unwrap() {
                "create_file_upload" => json!({
                    "upload_url":if runtime.bad == "foreign_upload" {"http://127.0.0.1:9/stolen"} else {"/input"},
                    "method":"PUT","offset":0,"size_bytes":args["arguments"]["size_bytes"]
                }),
                "get_job" => {
                    assert_eq!(args["arguments"]["job_id"], "remote_job_one");
                    if runtime.fail_reads.load(Ordering::SeqCst) {
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    remote_job(&runtime)
                }
                "cancel_job" => {
                    assert_eq!(args["arguments"]["job_id"], "remote_job_one");
                    runtime.cancellations.fetch_add(1, Ordering::SeqCst);
                    runtime.cancelled.store(true, Ordering::SeqCst);
                    runtime.complete.store(true, Ordering::SeqCst);
                    remote_job(&runtime)
                }
                "create_file_download" => {
                    assert_eq!(args["arguments"]["source"], "visual:one");
                    json!({
                        "download_url":if runtime.bad == "foreign_url" {"http://127.0.0.1:9/stolen"}else{"/output"},
                        "method":"GET",
                        "size_bytes":if runtime.bad == "oversized" {8_388_609}else{runtime.image.len()},
                        "mime_type":"image/png",
                        "sha256":if runtime.bad == "wrong_hash" {"0".repeat(64)}else{format!("{:x}",Sha256::digest(&runtime.image))}
                    })
                }
                other => panic!("unexpected session action {other}"),
            },
            other => panic!("unexpected MCP tool {other}"),
        };
        json!({"content":[{"type":"text","text":serde_json::to_string(&value).unwrap()}]})
    };
    let envelope = json!({"jsonrpc":"2.0","id":body["id"],"result":result});
    (
        [
            ("content-type", "text/event-stream"),
            ("mcp-session-id", "fixture_session"),
        ],
        format!("data: {envelope}\n\n"),
    )
        .into_response()
}

fn model_response(calls: Vec<Value>, text: &str) -> Value {
    json!({"choices":[{"index":0,"message":{"content":text,"tool_calls":calls},"finish_reason":if calls.is_empty(){"stop"}else{"tool_calls"}}]})
}

fn image_call(arguments: &str) -> Value {
    json!({"id":"image_call","type":"function","function":{"name":"generate_image","arguments":arguments}})
}

struct Fixture {
    app: Router,
    state: Arc<Runtime>,
    model_requests: Arc<Mutex<Vec<Value>>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn fixture(store: Store, bad: &'static str, malformed: bool, final_failure: bool) -> Fixture {
    let state = Arc::new(Runtime {
        image: if bad == "invalid_bytes" {
            b"not an image".to_vec()
        } else if bad == "dimensions" {
            png(2049, 1)
        } else {
            png(32, 24)
        },
        bad,
        submissions: AtomicUsize::new(0),
        cancellations: AtomicUsize::new(0),
        complete: AtomicBool::new(!matches!(bad, "running" | "hold_submit")),
        cancelled: AtomicBool::new(false),
        fail_reads: AtomicBool::new(bad == "lost_observation"),
        settings: Mutex::default(),
        progress: Mutex::new(Value::Null),
        uploaded: Mutex::default(),
        hold_upload: Semaphore::new(0),
        hold_submit: Semaphore::new(0),
        hold_preflight: Semaphore::new(0),
        hold_model: Semaphore::new(0),
        stages: Mutex::default(),
        discovery_calls: Mutex::default(),
    });
    let output = state.clone();
    let (base, remote_task) = serve(
        Router::new()
            .route("/mcp", post(mcp).delete(|| async { StatusCode::OK }))
            .route("/input", axum::routing::put(|State(runtime):State<Arc<Runtime>>, body:Bytes| async move {
                *runtime.uploaded.lock().unwrap() = body.to_vec();
                if runtime.bad == "hold_upload" {
                    runtime.hold_upload.acquire().await.unwrap().forget();
                }
                Json(json!({"status":"done","offset":body.len(),"size_bytes":body.len(),
                    "path":"@documents/uploads/owned/reference.png","mime_type":"image/png",
                    "sha256":if runtime.bad == "upload_hash" {"0".repeat(64)} else {format!("{:x}",Sha256::digest(&body))}}))
            }))
            .route(
                "/output",
                get(move || {
                    let output = output.clone();
                    async move { ([("content-type", "image/png")], output.image.clone()) }
                }),
            )
            .with_state(state.clone()),
    )
    .await;
    let model_requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = model_requests.clone();
    let model_state = state.clone();
    let (model,model_task) = serve(Router::new().route("/chat/completions",post(move |Json(body):Json<Value>| {
        let calls = captured.clone();
        let runtime = model_state.clone();
        async move {
            let workspace_round = calls.lock().unwrap().len();
            if matches!(bad,"workspace"|"workspace_failure") && workspace_round > 0 {
                let observation:Value=serde_json::from_str(body["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap()).unwrap();
                calls.lock().unwrap().push(body);
                if workspace_round == 1 {
                    let image=observation["output_image"]["id"].as_str().unwrap();
                    return Json(model_response(vec![
                        json!({"id":"save_image","type":"function","function":{"name":"save_asset","arguments":json!({"path":"images/tree.png","asset_id":image}).to_string()}}),
                        json!({"id":"write_html","type":"function","function":{"name":"write_file","arguments":json!({"path":"index.html","content":"<img src=\"images/tree.png\">"}).to_string()}}),
                    ],"")).into_response();
                }
                if bad == "workspace_failure" { return StatusCode::SERVICE_UNAVAILABLE.into_response(); }
                return Json(model_response(vec![],"Saved index.html and images/tree.png.")).into_response();
            }
            let image_tool = body["tools"].as_array().is_some_and(|tools|tools.iter().any(|tool|tool["function"]["name"] == "generate_image"));
            let finish = body["tool_choice"] == "none";
            if bad == "queued_work" && !image_tool && !finish {
                let prompt = body["messages"].as_array().unwrap().iter().rev().find(|m|m["role"] == "user").unwrap()["content"].as_str().unwrap();
                runtime.stages.lock().unwrap().push(prompt.into());
                if prompt == "Hold GPU" {
                    runtime.hold_model.acquire().await.unwrap().forget();
                }
            }
            calls.lock().unwrap().push(body);
            if finish && final_failure { return StatusCode::SERVICE_UNAVAILABLE.into_response(); }
            let arguments = match bad {
                "loras_zero" => json!({"prompt":"tree","loras":[]}).to_string(),
                "loras_one" => json!({"prompt":"selfie","loras":["selfie"]}).to_string(),
                "loras_two" => json!({"prompt":"neon selfie","loras":["neon","selfie"]}).to_string(),
                "loras_unknown" => json!({"prompt":"tree","loras":["../private"]}).to_string(),
                "loras_duplicate" => json!({"prompt":"tree","loras":["selfie","selfie"]}).to_string(),
                "extra_arguments" => json!({"prompt":"tree","profile":"unpermitted"}).to_string(),
                "long_prompt" => json!({"prompt":"x".repeat(4097)}).to_string(),
                "empty_prompt" => json!({"prompt":" \n "}).to_string(),
                _ => json!({"prompt":"A small green tree."}).to_string(),
            };
            let mut native = if image_tool && !finish {vec![image_call(&arguments)]}else{vec![]};
            if bad == "twice" && image_tool && !finish {
                let mut second=image_call(&arguments);
                second["id"]=json!("second_image_call");
                native.push(second);
            }
            if malformed && image_tool && !finish {
                native.push(json!({"id":"not_allowed","type":"function","function":{"name":"web_search","arguments":"{\"query\":\"no permission\"}"}}));
            }
            Json(model_response(native,"The image receipt is ready.")).into_response()
        }
    }))).await;
    let config = Config {
        model_catalog: Default::default(),
        public_origin: if bad == "inventory_guest" {Some("https://guest.example".into())} else {None},
        bind:"127.0.0.1:0".parse().unwrap(),data_dir:".".into(),web_dir:".".into(),
        default_backend:"compatible".into(),worker_image:None,search:None,
        backends:vec![BackendConfig {
kind: deep_server::config::BackendKind::Compatible, runtime: None, resource_group: None,id:"compatible".into(),base_url:model.clone(),model:"fixture-model".into(),api_key:None,timeout:Duration::from_secs(if bad == "queued_work" {1800} else {5}),max_output_tokens:4096},
            BackendConfig {
kind: deep_server::config::BackendKind::Groq, runtime: None, resource_group: None,id:"groq".into(),base_url:model,model:"fixture-model".into(),api_key:Some("fixture-key".into()),timeout:Duration::from_secs(5),max_output_tokens:4096},
            BackendConfig {
kind: deep_server::config::BackendKind::Demo, runtime: None, resource_group: None,id:"demo".into(),base_url:String::new(),model:"demo-model".into(),api_key:None,timeout:Duration::from_secs(5),max_output_tokens:4096}],
        image_profiles:vec![ImageProfile { lora_catalog: if bad.starts_with("loras_") { serde_json::from_value(json!([
            {"id":"selfie","label":"Selfie","description":"Close-up selfie perspective","file":"selfie.safetensors","trigger_words":["selfie"],"strength":0.7},
            {"id":"neon","label":"Neon","description":"Neon light style","file":"neon.safetensors","trigger_words":["neon"],"strength":0.4}
        ])).unwrap() } else {Vec::new()}, provider: Default::default(), api_key_env: None, api_key: None, resource_group: None, id:"klein".into(),label:"Klein test".into(),endpoint:format!("{base}/mcp"),
            settings:json!({"model_type":"flux2_klein_4b","resolution":"512x512","num_inference_steps":4}).as_object().unwrap().clone(),
        }],
    };
    Fixture {
        app: router(App::new(config, store).unwrap()),
        state,
        model_requests,
        tasks: vec![remote_task, model_task],
    }
}

async fn request(app: &Router, method: &str, path: &str, input: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("Host", "127.0.0.1")
                .header("Content-Type", "application/json")
                .body(Body::from(input.to_string()))
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
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)))
        },
    )
}

async fn workspace(app: &Router) -> String {
    request(
        app,
        "POST",
        "/api/workspaces",
        json!({"title":"Image test"}),
    )
    .await
    .1["id"]
        .as_str()
        .unwrap()
        .into()
}

async fn submit(app: &Router, workspace: &str) -> Value {
    let (status, run) = request(
        app,
        "POST",
        &format!("/api/workspaces/{workspace}/runs"),
        json!({"prompt":"Generate an image.","mode":"chat","image_profile_id":"klein"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    run
}

async fn terminal(app: &Router, run: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (_, value) = request(app, "GET", &format!("/api/runs/{run}"), Value::Null).await;
            if !matches!(value["status"].as_str(), Some("queued" | "running")) {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("run must settle")
}

async fn wait_for(app: &Router, run: &str, status: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (_, value) = request(app, "GET", &format!("/api/runs/{run}"), Value::Null).await;
            if value["image_job"]["status"] == status {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("image job must reach expected state")
}

#[tokio::test]
async fn image_inventory_reads_bounded_pages_without_generation_or_private_metadata() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let f = fixture(store.clone(), "", false, false).await;
    let (status, inventory) = request(
        &f.app,
        "GET",
        "/api/image-profiles/klein/inventory",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{inventory}");
    assert_eq!(inventory["profile_id"], "klein");
    assert_eq!(inventory["models"].as_array().unwrap().len(), 2);
    assert_eq!(
        inventory["loras"],
        json!(["selfie.safetensors", "neon.safetensors"])
    );
    assert_eq!(inventory["available"], true);
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 0);
    assert!(store.workspaces().await.unwrap().is_empty());
    let calls = f.state.discovery_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 5);
    assert!(calls.iter().all(|c| matches!(
        c["name"].as_str(),
        Some("wangp_models" | "wangp_model" | "wangp_validate")
    )));
    assert!(!inventory.to_string().contains("runtime_config"));
    assert_eq!(
        request(
            &f.app,
            "GET",
            "/api/image-profiles/missing/inventory",
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    for case in [
        "inventory_invalid",
        "inventory_oversize",
        "inventory_cursor",
        "inventory_duplicate",
    ] {
        let invalid = fixture(store.clone(), case, false, false).await;
        let (status, body) = request(
            &invalid.app,
            "GET",
            "/api/image-profiles/klein/inventory",
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(!body.to_string().contains("private-secret"));
        assert_eq!(invalid.state.submissions.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn authenticated_guests_cannot_inspect_a_configured_wangp_runtime() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let f = fixture(store, "inventory_guest", false, false).await;
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/session")
                .header("Host", "guest.example")
                .header("Origin", "https://guest.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/image-profiles/klein/inventory")
                .header("Host", "guest.example")
                .header("Cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(f.state.discovery_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn catalog_loras_resolve_in_call_order_and_survive_reload_and_configuration_changes() {
    for (case, files, strengths, ids) in [
        ("loras_zero", vec![], "", vec![]),
        (
            "loras_one",
            vec!["selfie.safetensors"],
            "0.7",
            vec!["selfie"],
        ),
        (
            "loras_two",
            vec!["neon.safetensors", "selfie.safetensors"],
            "0.4 0.7",
            vec!["neon", "selfie"],
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep.sqlite3");
        let store = Store::open(&path).unwrap();
        let f = fixture(store.clone(), case, false, false).await;
        let w = workspace(&f.app).await;
        let accepted = submit(&f.app, &w).await;
        let run = terminal(&f.app, accepted["id"].as_str().unwrap()).await;
        assert_eq!(run["status"], "succeeded", "{run}");
        assert_eq!(
            run["image_job"]["generation"],
            json!({"loras":files,"lora_strengths":strengths,"selected_lora_ids":ids})
        );
        let sent = f.state.settings.lock().unwrap()[0]["settings"].clone();
        assert_eq!(sent["activated_loras"], json!(files));
        assert_eq!(sent["loras_multipliers"], strengths);
        assert_eq!(f.state.submissions.load(Ordering::SeqCst), 1);
        let requests = f.model_requests.lock().unwrap().clone();
        let tool = &requests[0]["tools"][0]["function"];
        assert!(
            tool["description"]
                .as_str()
                .unwrap()
                .contains("Close-up selfie perspective")
        );
        assert_eq!(
            tool["parameters"]["properties"]["loras"]["items"]["enum"],
            json!(["selfie", "neon"])
        );
        drop(requests);
        drop(f);
        drop(store);
        let reopened = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(store) = Store::open(&path) {
                    break store;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        // Current presets have no catalog. Saved settings and the admitted
        // catalog, not that new configuration, own the historical receipt.
        let new_app = fixture(reopened.clone(), "", false, false).await;
        let (_, saved) = request(
            &new_app.app,
            "GET",
            &format!("/api/runs/{}", run["id"].as_str().unwrap()),
            Value::Null,
        )
        .await;
        assert_eq!(
            saved["image_job"]["generation"],
            run["image_job"]["generation"]
        );
        assert_eq!(
            saved["image_profile"]["fingerprint"],
            run["image_profile"]["fingerprint"]
        );
    }
}

#[tokio::test]
async fn invalid_catalog_lora_calls_reject_before_any_wangp_io() {
    for case in ["loras_unknown", "loras_duplicate"] {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let f = fixture(store, case, false, false).await;
        let w = workspace(&f.app).await;
        let accepted = submit(&f.app, &w).await;
        let run = terminal(&f.app, accepted["id"].as_str().unwrap()).await;
        assert_eq!(run["status"], "failed", "{run}");
        assert!(run["image_job"].is_null());
        assert!(f.state.discovery_calls.lock().unwrap().is_empty());
    }
}

async fn reference_run(
    f: &Fixture,
    store: &Store,
    workspace: &str,
) -> (Value, deep_server::store::ImageAsset) {
    let image = store
        .upload_image(
            workspace.into(),
            uuid::Uuid::new_v4().to_string(),
            "reference.png".into(),
            png(16, 12),
            "image/png".into(),
        )
        .await
        .unwrap()
        .unwrap();
    let (status, run) = request(
        &f.app,
        "POST",
        &format!("/api/workspaces/{workspace}/runs"),
        json!({
            "prompt":"Edit the selected image.","mode":"chat","image_profile_id":"klein",
            "image_input":{"role":"reference","asset_id":image.id}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    (run, image)
}

#[tokio::test]
async fn reference_input_is_frozen_transferred_and_preserved_with_output_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep.sqlite3");
    let store = Store::open(&path).unwrap();
    let f = fixture(store.clone(), "", false, false).await;
    let w = workspace(&f.app).await;
    let (run, image) = reference_run(&f, &store, &w).await;
    let run = terminal(&f.app, run["id"].as_str().unwrap()).await;
    assert_eq!(run["status"], "succeeded", "{run}");
    assert_eq!(run["image_input"]["role"], "reference");
    assert_eq!(run["image_input"]["asset"]["id"], image.id);
    assert_eq!(run["image_job"]["input_image"], run["image_input"]);
    assert_eq!(run["image_job"]["status"], "succeeded");
    assert_eq!(*f.state.uploaded.lock().unwrap(), png(16, 12));
    let settings = f.state.settings.lock().unwrap()[0]["settings"].clone();
    assert_eq!(
        settings["image_refs"],
        json!(["@documents/uploads/owned/reference.png"])
    );
    assert_eq!(settings["video_prompt_type"], "KI");
    let requests = serde_json::to_string(&*f.model_requests.lock().unwrap()).unwrap();
    assert!(requests.contains(&image.id));
    assert!(
        !requests.contains("@documents")
            && !requests.contains("base64")
            && !requests.contains("image_url")
    );
    let detail = store.workspace(w.clone()).await.unwrap().unwrap();
    assert_eq!(detail.image_assets.len(), 2);
    assert!(detail.revisions.is_empty() && detail.proposals.is_empty());
    drop(f);
    drop(store);
    let reopened = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(store) = Store::open(&path) {
                break store;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let saved = reopened
        .run(run["id"].as_str().unwrap().into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.image_input.unwrap().asset, image);
    assert_eq!(saved.image_job.unwrap().input_image.unwrap().asset, image);
    assert_eq!(
        reopened.image_content(image.id).await.unwrap().unwrap().1,
        png(16, 12)
    );
}

#[tokio::test]
async fn reference_inputs_require_profile_valid_role_and_same_workspace_before_admission() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let f = fixture(store.clone(), "", false, false).await;
    let own = workspace(&f.app).await;
    let other = workspace(&f.app).await;
    let image = store
        .upload_image(
            other,
            uuid::Uuid::new_v4().to_string(),
            "other.png".into(),
            png(8, 8),
            "image/png".into(),
        )
        .await
        .unwrap()
        .unwrap();
    for input in [
        json!({"role":"reference","asset_id":image.id}),
        json!({"role":"reference","asset_id":uuid::Uuid::new_v4()}),
        json!({"role":"start_image","asset_id":image.id}),
        json!({"role":"reference","asset_id":image.id,"path":"private"}),
    ] {
        let (status, _) = request(
            &f.app,
            "POST",
            &format!("/api/workspaces/{own}/runs"),
            json!({
                "prompt":"Edit","mode":"chat","image_profile_id":"klein","image_input":input
            }),
        )
        .await;
        assert!(status.is_client_error());
    }
    let (status, _) = request(
        &f.app,
        "POST",
        &format!("/api/workspaces/{own}/runs"),
        json!({
            "prompt":"Edit","mode":"chat","image_input":{"role":"reference","asset_id":image.id}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(store.workspace(own).await.unwrap().unwrap().runs.is_empty());
    assert!(f.model_requests.lock().unwrap().is_empty());
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unsupported_models_and_unverified_uploads_never_submit_generation() {
    for bad in ["unsupported_reference", "foreign_upload", "upload_hash"] {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let f = fixture(store.clone(), bad, false, false).await;
        let w = workspace(&f.app).await;
        let (run, _) = reference_run(&f, &store, &w).await;
        let run = terminal(&f.app, run["id"].as_str().unwrap()).await;
        assert_eq!(run["image_job"]["status"], "failed", "{bad}: {run}");
        assert!(run["image_job"]["output_image"].is_null());
        assert_eq!(f.state.submissions.load(Ordering::SeqCst), 0);
        if bad != "upload_hash" {
            assert!(f.state.uploaded.lock().unwrap().is_empty());
        }
        assert_eq!(
            store
                .workspace(w)
                .await
                .unwrap()
                .unwrap()
                .image_assets
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn cancellation_during_reference_upload_preserves_input_and_prevents_generation() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let f = fixture(store.clone(), "hold_upload", false, false).await;
    let w = workspace(&f.app).await;
    let (run, image) = reference_run(&f, &store, &w).await;
    let id = run["id"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.state.uploaded.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    request(&f.app, "DELETE", &format!("/api/runs/{id}"), Value::Null).await;
    f.state.hold_upload.add_permits(1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let run = terminal(&f.app, id).await;
    assert_eq!(run["status"], "cancelled");
    assert_eq!(run["image_job"]["status"], "interrupted");
    assert_eq!(run["image_job"]["input_image"]["asset"]["id"], image.id);
    assert!(run["image_job"]["output_image"].is_null());
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn owned_progress_updates_are_advisory_redacted_and_do_not_complete_generation() {
    let logs = Arc::new(Mutex::new(Vec::new()));
    let writer = LogWriter(logs.clone());
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let f = fixture(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
        "running",
        false,
        false,
    )
    .await;
    *f.state.progress.lock().unwrap() = json!({
        "phase":"inference", "current_step":2, "total_steps":4,
        "task_id":"mcp_remote_job_one_1", "status":"private prompt and path"
    });
    let w = workspace(&f.app).await;
    let run = submit(&f.app, &w).await;
    let id = run["id"].as_str().unwrap();
    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value = request(&f.app, "GET", &format!("/api/runs/{id}"), Value::Null)
                .await
                .1;
            if value["image_job"]["progress"]["current_step"] == 2 {
                break value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        observed["image_job"]["progress"],
        json!({"phase":"inference","current_step":2,"total_steps":4})
    );
    assert_eq!(observed["status"], "running");
    assert!(observed["image_job"]["output_image"].is_null());
    let contact = observed["image_job"]["last_contact_at_ms"]
        .as_i64()
        .unwrap();
    *f.state.progress.lock().unwrap() =
        json!({"phase":"inference","current_step":999,"total_steps":4});
    let refreshed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value = request(&f.app, "GET", &format!("/api/runs/{id}"), Value::Null)
                .await
                .1;
            if value["image_job"]["last_contact_at_ms"]
                .as_i64()
                .is_some_and(|time| time > contact)
            {
                break value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        refreshed["image_job"]["progress"],
        observed["image_job"]["progress"]
    );
    assert_eq!(refreshed["status"], "running");
    *f.state.progress.lock().unwrap() = observed["image_job"]["progress"].clone();
    let contact = refreshed["image_job"]["last_contact_at_ms"]
        .as_i64()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value = request(&f.app, "GET", &format!("/api/runs/{id}"), Value::Null)
                .await
                .1;
            if value["image_job"]["last_contact_at_ms"]
                .as_i64()
                .is_some_and(|time| time > contact)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    f.state.complete.store(true, Ordering::SeqCst);
    let done = terminal(&f.app, id).await;
    assert_eq!(done["image_job"]["status"], "succeeded");
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 1);
    assert!(
        !f.model_requests
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .to_string()
            .contains("private prompt and path")
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if String::from_utf8_lossy(&logs.lock().unwrap())
                .lines()
                .any(|line| line.contains("run task finished") && line.contains(id))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let logs = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    for field in [
        id,
        observed["image_job"]["id"].as_str().unwrap(),
        "remote_job_one",
        "image progress changed",
        "model_round",
        "model call completed",
    ] {
        assert!(logs.contains(field), "missing {field} in {logs}");
    }
    assert!(!logs.contains("private prompt and path"));
    assert!(!logs.contains("A small green tree."));
    assert_eq!(logs.matches("image progress changed").count(), 1);
    let progress_line = logs
        .lines()
        .find(|line| line.contains("image progress changed"))
        .unwrap();
    assert!(progress_line.contains(id));
    assert!(progress_line.contains(observed["image_job"]["id"].as_str().unwrap()));
    assert!(progress_line.contains("remote_job_one"));
}

#[tokio::test]
async fn native_workspace_image_generation_and_file_writes_share_one_atomic_revision() {
    for failure in [false, true] {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let f = fixture(
            store.clone(),
            if failure {
                "workspace_failure"
            } else {
                "workspace"
            },
            false,
            false,
        )
        .await;
        let w = workspace(&f.app).await;
        let (status,run)=request(&f.app,"POST",&format!("/api/workspaces/{w}/runs"),json!({"prompt":"Generate an image and a page.","mode":"chat","workspace_tools":true,"image_profile_id":"klein"})).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{run}");
        let done = terminal(&f.app, run["id"].as_str().unwrap()).await;
        assert_eq!(
            done["status"],
            if failure { "failed" } else { "succeeded" },
            "{done}"
        );
        let detail = store.workspace(w).await.unwrap().unwrap();
        assert_eq!(detail.image_assets.len(), 1);
        assert_eq!(f.state.submissions.load(Ordering::SeqCst), 1);
        if failure {
            assert!(detail.revisions.is_empty());
            assert!(detail.proposals.is_empty());
        } else {
            assert_eq!(detail.revisions.len(), 1);
            let revision = &detail.revisions[0];
            assert_eq!(revision.files[0].path, "index.html");
            assert_eq!(revision.asset_bindings[0].path, "images/tree.png");
            assert_eq!(
                revision.asset_bindings[0].asset_id,
                detail.image_assets[0].id
            );
            assert_eq!(detail.proposals[0].status, "applied");
        }
    }
}

#[tokio::test]
async fn image_tool_publishes_exact_binary_receipt_without_project_changes_or_model_pixels() {
    let f = fixture(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
        "",
        false,
        false,
    )
    .await;
    let w = workspace(&f.app).await;
    let run = submit(&f.app, &w).await;
    let done = terminal(&f.app, run["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(done["model_rounds"], 2);
    assert_eq!(done["image_job"]["status"], "succeeded");
    let image = &done["image_job"]["output_image"];
    assert_eq!(image["width"], 32);
    assert_eq!(image["height"], 24);
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 1);
    let path = format!(
        "/api/image-assets/{}/content",
        image["id"].as_str().unwrap()
    );
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&path)
                .header("Host", "127.0.0.1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        to_bytes(response.into_body(), 8_388_608)
            .await
            .unwrap()
            .as_ref(),
        f.state.image
    );
    let (_, detail) = request(&f.app, "GET", &format!("/api/workspaces/{w}"), Value::Null).await;
    assert!(detail["assets"].as_array().unwrap().is_empty());
    assert!(detail["revisions"].as_array().unwrap().is_empty());
    assert!(detail["proposals"].as_array().unwrap().is_empty());
    assert_eq!(detail["image_assets"].as_array().unwrap().len(), 1);
    assert!(!detail.to_string().contains("/mcp"));
    {
        let requests = f.model_requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["tools"].as_array().unwrap().len(), 1);
        assert_eq!(requests[1]["tool_choice"], "none");
        assert!(!requests[1]["messages"].to_string().contains("download_url"));
        assert!(!requests[1]["messages"].to_string().contains("/mcp"));
        assert!(
            requests[1]["messages"]
                .to_string()
                .contains(image["id"].as_str().unwrap())
        );
    }
    let (status,binding)=request(&f.app,"POST",&format!("/api/workspaces/{w}/proposals"),json!({
        "id":uuid::Uuid::new_v4().to_string(),"base_revision":{"id":null},
        "changes":[{"op":"write","path":"index.html","content":"<img src=\"assets/generated.png\">"}],
        "asset_changes":[{"op":"bind","path":"assets/generated.png","asset_id":image["id"]}]
    })).await;
    assert_eq!(status, StatusCode::OK, "{binding}");
    let (status, revision) = request(
        &f.app,
        "POST",
        &format!("/api/proposals/{}/apply", binding["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revision}");
    assert_eq!(revision["asset_bindings"][0]["asset_id"], image["id"]);
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 1);
    let (status, next) = request(
        &f.app,
        "POST",
        &format!("/api/workspaces/{w}/runs"),
        json!({"prompt":"Discuss the previous image.","mode":"chat"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        terminal(&f.app, next["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let requests = f.model_requests.lock().unwrap();
    let context = requests.last().unwrap()["messages"].to_string();
    assert!(
        context.contains("Saved image observation")
            && context.contains(image["id"].as_str().unwrap())
    );
    assert!(!context.contains("iVBORw0KGgo"));
}

#[tokio::test]
async fn unpermitted_native_calls_reject_without_effects_while_valid_batch_members_proceed() {
    let f = fixture(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
        "",
        true,
        false,
    )
    .await;
    let w = workspace(&f.app).await;
    for input in [
        json!({"prompt":"x","mode":"chat","image_profile_id":"unknown"}),
        json!({"prompt":"x","mode":"chat","backend":"demo","image_profile_id":"klein"}),
        json!({"prompt":"x","mode":"chat","search":true,"image_profile_id":"klein"}),
        json!({"prompt":"x","mode":"chat","node_scope":"compute","image_profile_id":"klein"}),
    ] {
        let (status, value) =
            request(&f.app, "POST", &format!("/api/workspaces/{w}/runs"), input).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{value}");
    }

    assert!(f.model_requests.lock().unwrap().is_empty());
    let run = submit(&f.app, &w).await;
    let done = terminal(&f.app, run["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert!(!done["image_job"].is_null(), "{done}");
    assert!(done["searches"].as_array().unwrap().is_empty());
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 1);
    let context = f.model_requests.lock().unwrap().last().unwrap()["messages"].to_string();
    assert!(context.contains("Search permission or budget is unavailable."));
    assert!(!context.contains("Untrusted search snippets"));
}

#[tokio::test]
async fn second_image_calls_extra_authority_and_prompt_overflows_have_zero_effects() {
    // A spent image budget inside one batch rejects the second call per call;
    // the valid first call still generates exactly once.
    let f = fixture(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
        "twice",
        false,
        false,
    )
    .await;
    let w = workspace(&f.app).await;
    let run = submit(&f.app, &w).await;
    let done = terminal(&f.app, run["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert!(!done["image_job"].is_null(), "{done}");
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 1);
    let context = f.model_requests.lock().unwrap().last().unwrap()["messages"].to_string();
    assert!(context.contains("Image permission or budget is unavailable."));

    // Schema-invalid image calls fail the whole batch before any effect.
    for bad in ["extra_arguments", "long_prompt", "empty_prompt"] {
        let f = fixture(
            Store::open(std::path::Path::new(":memory:")).unwrap(),
            bad,
            false,
            false,
        )
        .await;
        let w = workspace(&f.app).await;
        let run = submit(&f.app, &w).await;
        let done = terminal(&f.app, run["id"].as_str().unwrap()).await;
        assert_eq!(done["status"], "failed", "{bad}: {done}");
        assert!(done["image_job"].is_null());
        assert_eq!(f.state.submissions.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn invalid_or_foreign_image_outputs_are_explicit_failures_without_saved_bytes() {
    for bad in [
        "wrong_hash",
        "foreign_url",
        "oversized",
        "invalid_bytes",
        "dimensions",
        "foreign_task",
    ] {
        let f = fixture(
            Store::open(std::path::Path::new(":memory:")).unwrap(),
            bad,
            false,
            false,
        )
        .await;
        let w = workspace(&f.app).await;
        let run = submit(&f.app, &w).await;
        let done = terminal(&f.app, run["id"].as_str().unwrap()).await;
        assert_eq!(done["image_job"]["status"], "failed", "{bad}: {done}");
        assert!(done["image_job"]["output_image"].is_null());
        assert!(
            done["image_job"]["error"]
                .as_str()
                .is_some_and(|e| !e.is_empty())
        );
        let (_, detail) =
            request(&f.app, "GET", &format!("/api/workspaces/{w}"), Value::Null).await;
        assert!(detail["image_assets"].as_array().unwrap().is_empty());
    }
}

#[tokio::test]
async fn cancellation_retains_ownership_and_stops_only_the_bound_image_job() {
    for bad in ["running", "hold_submit", "hold_preflight"] {
        let f = fixture(
            Store::open(std::path::Path::new(":memory:")).unwrap(),
            bad,
            false,
            false,
        )
        .await;
        let w = workspace(&f.app).await;
        let run = submit(&f.app, &w).await;
        let id = run["id"].as_str().unwrap();
        wait_for(
            &f.app,
            id,
            if bad == "running" {
                "running"
            } else if bad == "hold_submit" {
                "submitting"
            } else {
                "planned"
            },
        )
        .await;
        let (_, cancelled) =
            request(&f.app, "DELETE", &format!("/api/runs/{id}"), Value::Null).await;
        assert_eq!(cancelled["status"], "cancelled");
        f.state.hold_submit.add_permits(1);
        f.state.hold_preflight.add_permits(1);
        let settled = wait_for(
            &f.app,
            id,
            if bad == "hold_preflight" {
                "interrupted"
            } else {
                "cancelled"
            },
        )
        .await;
        assert!(settled["image_job"]["output_image"].is_null());
        assert_eq!(f.model_requests.lock().unwrap().len(), 1);
        assert_eq!(
            f.state.submissions.load(Ordering::SeqCst),
            usize::from(bad != "hold_preflight")
        );
        assert_eq!(
            f.state.cancellations.load(Ordering::SeqCst),
            usize::from(bad != "hold_preflight")
        );
    }
}

#[tokio::test]
async fn multiple_image_workspaces_share_fifo_with_text_and_cancel_only_their_own_waiter() {
    let f = fixture(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
        "queued_work",
        false,
        false,
    )
    .await;
    let first_text_workspace = workspace(&f.app).await;
    let (status, text) = request(
        &f.app,
        "POST",
        &format!("/api/workspaces/{first_text_workspace}/runs"),
        json!({"prompt":"Hold GPU","mode":"chat","backend":"compatible"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.state.stages.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    let mut images = Vec::new();
    for _ in 0..2 {
        let w = workspace(&f.app).await;
        let (status, run) = request(&f.app, "POST", &format!("/api/workspaces/{w}/runs"),
            json!({"prompt":"Generate an image.","mode":"chat","backend":"groq","image_profile_id":"klein"})).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{run}");
        images.push(wait_for(&f.app, run["id"].as_str().unwrap(), "planned").await);
        // Let the owned task enter its group wait before submitting the next stage.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let text_workspace = workspace(&f.app).await;
    let (status, next_text) = request(
        &f.app,
        "POST",
        &format!("/api/workspaces/{text_workspace}/runs"),
        json!({"prompt":"Next text","mode":"chat","backend":"compatible"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(next_text["status"], "queued");
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 0);

    // Neither the old 300-second image timer nor the old 360-second parent
    // timer may consume time spent behind another workspace's GPU operation.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(601)).await;
    tokio::time::resume();
    for run in &images {
        let (_, waiting) = request(
            &f.app,
            "GET",
            &format!("/api/runs/{}", run["id"].as_str().unwrap()),
            Value::Null,
        )
        .await;
        assert_eq!(waiting["status"], "running", "{waiting}");
        assert_eq!(waiting["image_job"]["status"], "planned", "{waiting}");
        assert!(waiting["image_job"]["remote_job_id"].is_null());
    }
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 0);

    f.state.hold_model.add_permits(1);
    wait_for(&f.app, images[0]["id"].as_str().unwrap(), "submitting").await;
    // A third workspace must also be admitted while an image is actually submitted.
    let w = workspace(&f.app).await;
    let (status, cancelled) = request(&f.app, "POST", &format!("/api/workspaces/{w}/runs"),
        json!({"prompt":"Generate an image.","mode":"chat","backend":"groq","image_profile_id":"klein"})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{cancelled}");
    let cancelled_id = cancelled["id"].as_str().unwrap();
    wait_for(&f.app, cancelled_id, "planned").await;
    request(
        &f.app,
        "DELETE",
        &format!("/api/runs/{cancelled_id}"),
        Value::Null,
    )
    .await;
    wait_for(&f.app, cancelled_id, "interrupted").await;

    f.state.hold_submit.add_permits(1);
    wait_for(&f.app, images[1]["id"].as_str().unwrap(), "submitting").await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.state.submissions.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // Text cannot overtake the image stage that entered this group first.
    assert_eq!(
        *f.state.stages.lock().unwrap(),
        vec![
            "Hold GPU".to_owned(),
            format!("image:{}", images[0]["image_job"]["id"].as_str().unwrap()),
            format!("image:{}", images[1]["image_job"]["id"].as_str().unwrap()),
        ]
    );
    f.state.hold_submit.add_permits(1);
    for run in [&text, &images[0], &images[1], &next_text] {
        let done = terminal(&f.app, run["id"].as_str().unwrap()).await;
        assert_eq!(done["status"], "succeeded", "{done}");
        if !done["image_job"].is_null() {
            assert_eq!(done["image_job"]["status"], "succeeded");
        }
    }
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 2);
    assert_eq!(f.state.cancellations.load(Ordering::SeqCst), 0);
    assert_eq!(f.state.stages.lock().unwrap().last().unwrap(), "Next text");
}

#[tokio::test]
async fn image_continuation_can_wait_behind_another_workspaces_long_model_call() {
    let f = fixture(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
        "queued_work",
        false,
        false,
    )
    .await;
    let image_workspace = workspace(&f.app).await;
    let image = submit(&f.app, &image_workspace).await;
    let image_id = image["id"].as_str().unwrap();
    wait_for(&f.app, image_id, "submitting").await;
    let text_workspace = workspace(&f.app).await;
    let (status, text) = request(
        &f.app,
        "POST",
        &format!("/api/workspaces/{text_workspace}/runs"),
        json!({"prompt":"Hold GPU","mode":"chat","backend":"compatible"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(text["status"], "queued");
    f.state.hold_submit.add_permits(1);
    wait_for(&f.app, image_id, "succeeded").await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f
            .state
            .stages
            .lock()
            .unwrap()
            .iter()
            .any(|stage| stage == "Hold GPU")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(601)).await;
    tokio::time::resume();
    let (_, waiting) = request(&f.app, "GET", &format!("/api/runs/{image_id}"), Value::Null).await;
    assert_eq!(waiting["status"], "running", "{waiting}");
    assert_eq!(waiting["image_job"]["status"], "succeeded");
    assert_eq!(waiting["model_rounds"], 1);
    f.state.hold_model.add_permits(1);
    assert_eq!(
        terminal(&f.app, text["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let done = terminal(&f.app, image_id).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(done["model_rounds"], 2);
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn lost_submission_stays_unresolved_and_blocks_images_and_deletion_but_not_chat() {
    let f = fixture(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
        "lost_submission",
        false,
        false,
    )
    .await;
    let w = workspace(&f.app).await;
    let run = submit(&f.app, &w).await;
    let done = terminal(&f.app, run["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "failed");
    assert_eq!(done["image_job"]["status"], "unresolved");
    let job = done["image_job"]["id"].as_str().unwrap();
    let other = workspace(&f.app).await;
    assert_eq!(
        request(
            &f.app,
            "POST",
            &format!("/api/workspaces/{other}/runs"),
            json!({"prompt":"again","mode":"chat","image_profile_id":"klein"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &f.app,
            "DELETE",
            &format!("/api/workspaces/{w}"),
            Value::Null
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            &format!("/api/image-jobs/{job}/reconcile"),
            Value::Null
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            &format!("/api/workspaces/{other}/runs"),
            json!({"prompt":"Ordinary chat","mode":"chat"})
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 1);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        request(
            &f.app,
            "POST",
            &format!("/api/image-jobs/{job}/confirm-stopped"),
            json!({"confirm_remote_stopped":false})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, resolved) = request(
        &f.app,
        "POST",
        &format!("/api/image-jobs/{job}/confirm-stopped"),
        json!({"confirm_remote_stopped":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(resolved["status"], "interrupted");
    assert!(resolved["output_image"].is_null());
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn restart_reconciles_only_a_saved_remote_job_without_replaying_generation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restart.sqlite3");
    let mut f = fixture(
        Store::open(&path).unwrap(),
        "lost_observation",
        false,
        false,
    )
    .await;
    let w = workspace(&f.app).await;
    let run = submit(&f.app, &w).await;
    terminal(&f.app, run["id"].as_str().unwrap()).await;
    let done = wait_for(&f.app, run["id"].as_str().unwrap(), "unresolved").await;
    let job = done["image_job"]["id"].as_str().unwrap().to_owned();
    tokio::time::sleep(Duration::from_millis(100)).await;
    f.app = Router::new();
    let store = Store::open(&path).unwrap();
    f.state.fail_reads.store(false, Ordering::SeqCst);
    deep_server::images::Images::new(store.clone(), &[])
        .recover()
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let receipt = store.image_job(job.clone()).await.unwrap().unwrap();
            if receipt.status == "cancelled" {
                assert!(receipt.output_image.is_none());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn known_job_reconciliation_discards_late_output_and_does_not_resubmit_generation() {
    let f = fixture(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
        "lost_observation",
        false,
        false,
    )
    .await;
    let w = workspace(&f.app).await;
    let run = submit(&f.app, &w).await;
    let id = run["id"].as_str().unwrap();
    terminal(&f.app, id).await;
    let done = wait_for(&f.app, id, "unresolved").await;
    f.state.fail_reads.store(false, Ordering::SeqCst);
    let job = done["image_job"]["id"].as_str().unwrap();
    assert_eq!(
        request(
            &f.app,
            "POST",
            &format!("/api/image-jobs/{job}/reconcile"),
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    let settled = wait_for(&f.app, id, "cancelled").await;
    assert!(settled["image_job"]["output_image"].is_null());
    assert_eq!(f.state.submissions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn image_success_survives_a_later_parent_model_failure_and_reloads_exact_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("images.sqlite3");
    let f = fixture(Store::open(&path).unwrap(), "", false, true).await;
    let w = workspace(&f.app).await;
    let run = submit(&f.app, &w).await;
    let done = terminal(&f.app, run["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "failed");
    assert_eq!(done["image_job"]["status"], "succeeded");
    let asset = done["image_job"]["output_image"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let expected = f.state.image.clone();
    drop(f);
    // Let the parent response and owned MCP session cleanup release their Store clones.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.image_content(asset).await.unwrap().unwrap().1,
        expected
    );
    // The saved image survives, while the unconfirmed second local model call
    // continues to fence deletion until the operator confirms runtime idleness.
    assert!(store.delete_workspace(w).await.is_err());
}
