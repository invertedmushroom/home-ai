use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{HeaderMap, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use base64::Engine;
use deep_server::{
    App,
    config::{BackendConfig, BackendKind, Config},
    images::{ImageProfile, ImageProvider},
    router,
    store::Store,
};
use serde_json::{Value, json};
use std::{
    io::Cursor,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;
use tower::ServiceExt;

struct Upstream {
    mode: &'static str,
    image: Vec<u8>,
    calls: Mutex<Vec<Value>>,
    hold: Semaphore,
}

async fn chat(Json(body): Json<Value>) -> Json<Value> {
    if body["messages"].as_array().unwrap().last().unwrap()["role"] == "user"
        && body.get("tools").is_some()
    {
        Json(
            json!({"choices":[{"message":{"content":"","tool_calls":[{"id":"make-image","type":"function","function":{
            "name":"generate_image","arguments":json!({"prompt":"A green square on white."}).to_string()
        }}]},"finish_reason":"tool_calls"}]}),
        )
    } else {
        Json(
            json!({"choices":[{"message":{"content":"Image operation finished."},"finish_reason":"stop"}]}),
        )
    }
}

async fn image_response(
    State(state): State<Arc<Upstream>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert_eq!(headers["authorization"], "Bearer image-fixture-secret");
    state.calls.lock().unwrap().push(body);
    match state.mode {
        "hold" => {
            state.hold.acquire().await.unwrap().forget();
        }
        "http_error" => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"image-fixture-secret echoed"})),
            )
                .into_response();
        }
        "oversize" => {
            return Response::builder()
                .body(Body::from(vec![b' '; 12 * 1024 * 1024]))
                .unwrap();
        }
        _ => {}
    }
    Json(json!({"data":[{"b64_json":if state.mode=="invalid_image" {"invalid".into()}else{base64::engine::general_purpose::STANDARD.encode(&state.image)}}]})).into_response()
}

async fn fixture(
    store: Store,
    mode: &'static str,
) -> (Router, Arc<Upstream>, tokio::task::JoinHandle<()>) {
    let mut buffer = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(16, 16)
        .write_to(&mut buffer, image::ImageFormat::Png)
        .unwrap();
    let state = Arc::new(Upstream {
        mode,
        image: buffer.into_inner(),
        calls: Mutex::default(),
        hold: Semaphore::new(0),
    });
    let upstream = Router::new()
        .route("/v1/chat/completions", post(chat))
        .route("/v1/local/chat/completions", post(chat))
        .route("/v1/images/generations", post(image_response))
        .route("/v1/images/edits", post(image_response))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    let config=Config {
        public_origin:None,bind:"127.0.0.1:0".parse().unwrap(),data_dir:".".into(),web_dir:".".into(),default_backend:"openai".into(),
        backends:vec![BackendConfig {kind:BackendKind::Openai,id:"openai".into(),runtime:None,resource_group:None,
            base_url:endpoint.clone(),model:"vision-test".into(),api_key:Some("chat-fixture-secret".into()),timeout:Duration::from_secs(5),max_output_tokens:1024},
            BackendConfig {kind:BackendKind::Compatible,id:"compatible".into(),runtime:None,resource_group:None,
                base_url:format!("{endpoint}/local"),model:"local".into(),api_key:None,timeout:Duration::from_secs(5),max_output_tokens:1024}],
        worker_image:None,search:None,
        image_profiles:vec![ImageProfile {lora_catalog: Vec::new(), provider:ImageProvider::Openai,api_key_env:Some("OPENAI_API_KEY".into()),api_key:Some("image-fixture-secret".into()),
            resource_group:None,id:"hosted-image".into(),label:"Hosted image".into(),endpoint,
            settings:json!({"model":"gpt-image-2.5-flare","size":"1024x1024","quality":"low"}).as_object().unwrap().clone()}],
        model_catalog:serde_json::from_value(json!({"profiles":[{"id":"vision","label":"Vision","backend":"openai","model":"vision-test","tool_calls":true,"vision":true}],"default_profile":"vision"})).unwrap(),
    };
    (router(App::new(config, store).unwrap()), state, task)
}

async fn request(app: &Router, method: &str, path: &str, value: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("Host", "127.0.0.1")
                .header("Content-Type", "application/json")
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_097_152).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn settled(store: &Store, id: &str) -> deep_server::store::Run {
    for _ in 0..500 {
        let run = store.run(id.into()).await.unwrap().unwrap();
        if !matches!(run.status.as_str(), "queued" | "running") {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("image test did not settle");
}

#[tokio::test]
async fn generation_editing_and_vision_share_saved_assets_without_persisting_keys_or_pixels_in_runs()
 {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("images.sqlite3");
    let store = Store::open(&path).unwrap();
    let workspace = store
        .create_workspace("OpenAI images".into())
        .await
        .unwrap();
    let (app, upstream, task) = fixture(store.clone(), "success").await;
    let mut input_image = None;
    for editing in [false, true] {
        let mut input =
            json!({"prompt":"Create an image","mode":"chat","image_profile_id":"hosted-image"});
        if editing {
            input["image_input"] = json!({"role":"reference","asset_id":input_image});
        }
        let (status, receipt) = request(
            &app,
            "POST",
            &format!("/api/workspaces/{}/runs", workspace.id),
            input,
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
        let run = settled(&store, receipt["id"].as_str().unwrap()).await;
        assert_eq!(run.status, "succeeded", "{:?}", run.error);
        let job = run.image_job.as_ref().unwrap();
        assert_eq!(job.status, "succeeded", "{:?}", job.error);
        assert_eq!(job.profile.provider, ImageProvider::Openai);
        assert!(job.remote_job_id.is_none());
        assert_eq!(job.input_image.is_some(), editing);
        let output = job.output_image.as_ref().unwrap();
        assert_eq!(
            store
                .image_content(output.id.clone())
                .await
                .unwrap()
                .unwrap()
                .1,
            upstream.image
        );
        input_image = Some(output.id.clone());
        let wire = upstream.calls.lock().unwrap().last().unwrap().clone();
        assert_eq!(wire["n"], 1);
        assert_eq!(wire["output_format"], "png");
        assert_eq!(wire["model"], "gpt-image-2.5-flare");
        assert_eq!(wire.get("images").is_some(), editing);
        if editing {
            assert_eq!(
                wire["images"][0]["image_url"],
                format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&upstream.image)
                )
            );
        }
        let public = serde_json::to_string(&run).unwrap();
        assert!(!public.contains("secret"));
        assert!(!public.contains("base64,"));
    }
    let (status, receipt) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{}/runs", workspace.id),
        json!({
            "prompt":"Inspect the saved output.","mode":"chat","image_attachment_ids":[input_image]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let run = settled(&store, receipt["id"].as_str().unwrap()).await;
    assert_eq!(run.status, "succeeded");
    assert_eq!(run.image_attachments[0].origin, "generation");
    let connection = rusqlite::Connection::open(&path).unwrap();
    let private: String = connection
        .query_row(
            "SELECT image_config FROM runs WHERE image_config IS NOT NULL LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!private.contains("image-fixture-secret"));
    let settings: String = connection
        .query_row(
            "SELECT settings FROM image_jobs ORDER BY rowid DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!settings.contains("base64,"));
    task.abort();
}

#[tokio::test]
async fn provider_rejections_invalid_images_and_lost_outcomes_remain_distinct() {
    for (mode, expected) in [
        ("http_error", "failed"),
        ("invalid_image", "failed"),
        ("oversize", "unresolved"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("images.sqlite3")).unwrap();
        let workspace = store.create_workspace("Failure".into()).await.unwrap();
        let (app, upstream, task) = fixture(store.clone(), mode).await;
        let (_, receipt) = request(
            &app,
            "POST",
            &format!("/api/workspaces/{}/runs", workspace.id),
            json!({"prompt":"Generate","mode":"chat","image_profile_id":"hosted-image"}),
        )
        .await;
        let run = settled(&store, receipt["id"].as_str().unwrap()).await;
        let job = run.image_job.unwrap();
        assert_eq!(job.status, expected, "{mode}: {:?}", job.error);
        assert!(!job.error.unwrap().contains("secret"));
        assert_eq!(upstream.calls.lock().unwrap().len(), 1);
        assert!(
            store
                .workspace(workspace.id.clone())
                .await
                .unwrap()
                .unwrap()
                .image_assets
                .is_empty()
        );
        if expected == "unresolved" {
            let (status, _) = request(
                &app,
                "POST",
                &format!("/api/workspaces/{}/runs", workspace.id),
                json!({"prompt":"Again","mode":"chat","image_profile_id":"hosted-image"}),
            )
            .await;
            assert_eq!(status, StatusCode::CONFLICT);
            // Hosted uncertainty must not acquire the local GPU's durable fence.
            let local = store.create_workspace("Local".into()).await.unwrap();
            let (status, local_run) = request(
                &app,
                "POST",
                &format!("/api/workspaces/{}/runs", local.id),
                json!({"prompt":"Local work","mode":"chat","backend":"compatible"}),
            )
            .await;
            assert_eq!(status, StatusCode::ACCEPTED);
            assert_eq!(
                settled(&store, local_run["id"].as_str().unwrap())
                    .await
                    .status,
                "succeeded"
            );
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &format!("/api/image-jobs/{}/confirm-stopped", job.id),
                    json!({"confirm_remote_stopped":true})
                )
                .await
                .0,
                StatusCode::BAD_REQUEST
            );
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &format!("/api/image-jobs/{}/accept-unknown", job.id),
                    json!({"accept_unknown_outcome":false})
                )
                .await
                .0,
                StatusCode::BAD_REQUEST
            );
            let (status, resolved) = request(
                &app,
                "POST",
                &format!("/api/image-jobs/{}/accept-unknown", job.id),
                json!({"accept_unknown_outcome":true}),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(resolved["status"], "interrupted");
            assert!(
                resolved["error"]
                    .as_str()
                    .unwrap()
                    .contains("may have completed")
            );
        }
        task.abort();
    }
}

#[tokio::test]
async fn cancelled_parent_drains_openai_response_and_never_publishes_its_image() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("images.sqlite3")).unwrap();
    let workspace = store.create_workspace("Cancel".into()).await.unwrap();
    let (app, upstream, task) = fixture(store.clone(), "hold").await;
    let (_, receipt) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{}/runs", workspace.id),
        json!({"prompt":"Generate","mode":"chat","image_profile_id":"hosted-image"}),
    )
    .await;
    for _ in 0..300 {
        if !upstream.calls.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(upstream.calls.lock().unwrap().len(), 1);
    let id = receipt["id"].as_str().unwrap();
    assert_eq!(
        request(&app, "DELETE", &format!("/api/runs/{id}"), Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    upstream.hold.add_permits(1);
    let mut terminal = false;
    for _ in 0..300 {
        let run = store.run(id.into()).await.unwrap().unwrap();
        if run.image_job.is_some_and(|job| job.status == "cancelled") {
            terminal = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(terminal);
    assert!(
        store
            .workspace(workspace.id)
            .await
            .unwrap()
            .unwrap()
            .image_assets
            .is_empty()
    );
    task.abort();
}
