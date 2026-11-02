use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode},
    routing::post,
};
use base64::Engine;
use deep_server::{
    App,
    config::{BackendConfig, BackendKind, Config},
    router,
    store::Store,
};
use serde_json::{Value, json};
use std::{
    io::Cursor,
    sync::{Arc, Mutex},
    time::Duration,
};
use tower::ServiceExt;
use uuid::Uuid;

type Calls = Arc<Mutex<Vec<Value>>>;

async fn model(State(calls): State<Calls>, Json(body): Json<Value>) -> Json<Value> {
    let first = {
        let mut calls = calls.lock().unwrap();
        let first = calls.is_empty();
        calls.push(body.clone());
        first
    };
    if first && body.get("tools").is_some() {
        Json(json!({"choices":[{"message":{"content":"","tool_calls":[{
            "id":"save-report", "type":"function", "function":{"name":"write_file",
            "arguments":json!({"path":"inspection.txt","content":"A visual report."}).to_string()}
        }]},"finish_reason":"tool_calls"}]}))
    } else {
        Json(
            json!({"choices":[{"message":{"content":"Inspected the attached images."},"finish_reason":"stop"}]}),
        )
    }
}

async fn fixture(store: Store, kind: BackendKind) -> (Router, Calls, tokio::task::JoinHandle<()>) {
    let calls: Calls = Default::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let upstream = Router::new()
        .route("/chat/completions", post(model))
        .with_state(calls.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    let config = Config {
        public_origin: None, bind: "127.0.0.1:0".parse().unwrap(), data_dir: ".".into(), web_dir: ".".into(),
        default_backend: kind.as_str().into(),
        backends: vec![BackendConfig { id: kind.as_str().into(), kind, runtime: None, resource_group: None,
            base_url: endpoint, model: "vision-test".into(), api_key: Some("fixture-key".into()),
            timeout: Duration::from_secs(5), max_output_tokens: 4096 }],
        worker_image: None, search: None, image_profiles: Vec::new(),
        model_catalog: serde_json::from_value(json!({
            "profiles":[
                {"id":"vision","label":"Vision","backend":kind.as_str(),"model":"vision-test","tool_calls":true,"vision":true},
                {"id":"text","label":"Text","backend":kind.as_str(),"model":"text-test","tool_calls":true}
            ], "default_profile":"vision"
        })).unwrap(),
    };
    (router(App::new(config, store).unwrap()), calls, task)
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

fn image(format: image::ImageFormat, padding: usize) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(8, 6)
        .write_to(&mut out, format)
        .unwrap();
    let mut bytes = out.into_inner();
    if padding > bytes.len() {
        bytes.resize(padding, 0);
    }
    bytes
}

async fn settled(store: &Store, id: &str) -> deep_server::store::Run {
    for _ in 0..300 {
        let run = store.run(id.into()).await.unwrap().unwrap();
        if !matches!(run.status.as_str(), "queued" | "running") {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("vision fixture did not settle");
}

#[tokio::test]
async fn pixels_reach_each_provider_and_tool_continuation_without_entering_saved_context() {
    for (kind, tools) in [
        (BackendKind::Compatible, true),
        (BackendKind::Lmstudio, false),
        (BackendKind::Openai, true),
        (BackendKind::Groq, false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vision.sqlite3");
        let store = Store::open(&path).unwrap();
        let workspace = store.create_workspace("Vision".into()).await.unwrap();
        // Larger than the text-context budget, but within the separate image budget.
        let png = image(image::ImageFormat::Png, 180_000);
        let jpeg = image(image::ImageFormat::Jpeg, 0);
        let mut assets = Vec::new();
        for (name, content, media) in [
            ("scene.png", png.clone(), "image/png"),
            ("detail.jpg", jpeg.clone(), "image/jpeg"),
        ] {
            assets.push(
                store
                    .upload_image(
                        workspace.id.clone(),
                        Uuid::new_v4().to_string(),
                        name.into(),
                        content,
                        media.into(),
                    )
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        let (app, calls, task) = fixture(store.clone(), kind).await;
        let ids: Vec<_> = assets.iter().map(|image| image.id.clone()).collect();
        let (status, receipt) = request(&app, "POST", &format!("/api/workspaces/{}/runs", workspace.id), json!({
            "prompt":"Describe these images and save a report if tools are enabled.", "mode":"chat",
            "workspace_tools":tools, "image_attachment_ids":ids
        })).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
        assert_eq!(receipt["image_attachments"], json!(assets));
        let run = settled(&store, receipt["id"].as_str().unwrap()).await;
        assert_eq!(run.status, "succeeded", "{:?}", run.error);
        assert_eq!(run.model_rounds, if tools { 2 } else { 0 });
        let recorded = calls.lock().unwrap().clone();
        assert_eq!(recorded.len(), if tools { 2 } else { 1 });
        for body in &recorded {
            let multipart: Vec<_> = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["content"].is_array())
                .collect();
            assert_eq!(multipart.len(), 1);
            assert_eq!(multipart[0]["role"], "user");
            let parts = multipart[0]["content"].as_array().unwrap();
            assert_eq!(parts.len(), 3);
            assert!(
                parts[0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("Describe these images")
            );
            for (part, (media, expected)) in parts[1..]
                .iter()
                .zip([("image/png", &png), ("image/jpeg", &jpeg)])
            {
                let url = part["image_url"]["url"].as_str().unwrap();
                let encoded = url.strip_prefix(&format!("data:{media};base64,")).unwrap();
                assert_eq!(
                    base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .unwrap(),
                    *expected
                );
            }
        }
        let context =
            serde_json::to_string(&store.run_context(run.id.clone()).await.unwrap()).unwrap();
        assert!(!context.contains("base64,"));
        assert!(!serde_json::to_string(&run).unwrap().contains("base64,"));
        let (status, next) = request(&app,"POST",&format!("/api/workspaces/{}/runs",workspace.id),json!({
            "prompt":"Continue in text.","mode":"chat","model_selection":{"mode":"profile","profile_id":"text"}
        })).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(
            settled(&store, next["id"].as_str().unwrap()).await.status,
            "succeeded"
        );
        let last = calls.lock().unwrap().last().unwrap().clone();
        assert!(!last.to_string().contains("base64,"));
        assert!(last.to_string().contains("pixels are not included"));
        let branch = store
            .branch_workspace(workspace.id.clone(), None, None)
            .await
            .unwrap()
            .unwrap();
        let detail = store.workspace(branch.id.clone()).await.unwrap().unwrap();
        let branched = &detail.runs[0];
        assert_eq!(branched.image_attachments.len(), 2);
        for (original, copied) in assets.iter().zip(&branched.image_attachments) {
            assert_ne!(original.id, copied.id);
            assert_eq!(copied.workspace_id, branch.id);
            assert_eq!(original.sha256, copied.sha256);
            assert!(detail.image_assets.contains(copied));
        }
        task.abort();
        drop(app);
        drop(store);
        let reopened = Store::open(&path).unwrap();
        let saved = reopened.run(run.id).await.unwrap().unwrap();
        assert_eq!(saved.image_attachments, assets);
    }
}

#[tokio::test]
async fn version_22_upgrade_preserves_images_and_historical_runs_with_empty_attachments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.sqlite3");
    let store = Store::open(&path).unwrap();
    let workspace = store
        .create_workspace("Keep this workspace".into())
        .await
        .unwrap();
    let asset = store
        .upload_image(
            workspace.id.clone(),
            Uuid::new_v4().to_string(),
            "keep.png".into(),
            image(image::ImageFormat::Png, 0),
            "image/png".into(),
        )
        .await
        .unwrap()
        .unwrap();
    let run = store
        .begin(
            workspace.id,
            "Existing prompt".into(),
            "demo".into(),
            "demo".into(),
            Default::default(),
        )
        .await
        .unwrap()
        .unwrap();
    store
        .terminate(run.id.clone(), "failed", Some("Existing failure".into()))
        .await
        .unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch("ALTER TABLE runs DROP COLUMN image_attachments; PRAGMA user_version=22;")
        .unwrap();
    drop(connection);
    let store = Store::open(&path).unwrap();
    let saved = store.run(run.id).await.unwrap().unwrap();
    assert!(saved.image_attachments.is_empty());
    assert_eq!(saved.error.as_deref(), Some("Existing failure"));
    assert_eq!(
        store.image_asset(asset.id.clone()).await.unwrap().unwrap(),
        asset
    );
}

#[tokio::test]
async fn invalid_or_unauthorized_image_attachments_reject_before_any_run_or_model_call() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("vision.sqlite3")).unwrap();
    let workspace = store.create_workspace("Images".into()).await.unwrap();
    let foreign = store.create_workspace("Other".into()).await.unwrap();
    let mut assets = Vec::new();
    for index in 0..5 {
        assets.push(
            store
                .upload_image(
                    workspace.id.clone(),
                    Uuid::new_v4().to_string(),
                    format!("{index}.png"),
                    image(image::ImageFormat::Png, 0),
                    "image/png".into(),
                )
                .await
                .unwrap()
                .unwrap(),
        );
    }
    let foreign_image = store
        .upload_image(
            foreign.id,
            Uuid::new_v4().to_string(),
            "other.png".into(),
            image(image::ImageFormat::Png, 0),
            "image/png".into(),
        )
        .await
        .unwrap()
        .unwrap();
    let (app, calls, task) = fixture(store.clone(), BackendKind::Compatible).await;
    for ids in [
        vec![assets[0].id.clone(); 2],
        assets.iter().map(|a| a.id.clone()).collect(),
        vec![foreign_image.id],
        vec![Uuid::new_v4().to_string()],
    ] {
        let (status, _) = request(
            &app,
            "POST",
            &format!("/api/workspaces/{}/runs", workspace.id),
            json!({"prompt":"Inspect","mode":"chat","image_attachment_ids":ids}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    for selection in [
        json!({"model_selection":{"mode":"profile","profile_id":"text"}}),
        json!({"backend":"compatible"}),
    ] {
        let mut input =
            json!({"prompt":"Inspect","mode":"chat","image_attachment_ids":[assets[0].id]});
        input
            .as_object_mut()
            .unwrap()
            .extend(selection.as_object().unwrap().clone());
        assert_eq!(
            request(
                &app,
                "POST",
                &format!("/api/workspaces/{}/runs", workspace.id),
                input
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut large = Vec::new();
    for index in 0..3 {
        large.push(
            store
                .upload_image(
                    workspace.id.clone(),
                    Uuid::new_v4().to_string(),
                    format!("large{index}.png"),
                    image(image::ImageFormat::Png, 6 * 1024 * 1024),
                    "image/png".into(),
                )
                .await
                .unwrap()
                .unwrap()
                .id,
        );
    }
    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{}/runs", workspace.id),
        json!({"prompt":"Inspect","mode":"chat","image_attachment_ids":large}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        store
            .workspace(workspace.id)
            .await
            .unwrap()
            .unwrap()
            .runs
            .is_empty()
    );
    assert!(calls.lock().unwrap().is_empty());
    task.abort();
}
