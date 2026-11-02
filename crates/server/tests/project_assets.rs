use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    routing::post,
};
use deep_server::{
    App,
    config::{BackendConfig, Config},
    router,
    store::Store,
};
use serde_json::{Value, json};
use std::{
    io::{Cursor, Read},
    sync::{Arc, Mutex},
    time::Duration,
};
use tower::ServiceExt;
use uuid::Uuid;

fn png(color: u8) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(8, 6, image::Rgb([color, 100, 50]));
    let mut buffer = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut buffer, image::ImageFormat::Png)
        .unwrap();
    buffer.into_inner()
}

fn app(store: Store, model: Option<String>) -> Router {
    let compatible = model.is_some();
    router(
        App::new(
            Config {
                public_origin: None,
                bind: "127.0.0.1:0".parse().unwrap(),
                data_dir: ".".into(),
                web_dir: ".".into(),
                default_backend: if compatible { "compatible" } else { "demo" }.into(),
                backends: vec![BackendConfig {
                    kind: if compatible {
                        deep_server::config::BackendKind::Compatible
                    } else {
                        deep_server::config::BackendKind::Demo
                    },
                    runtime: None,
                    resource_group: None,
                    id: if compatible { "compatible" } else { "demo" }.into(),
                    base_url: model.unwrap_or_default(),
                    model: "asset-fixture".into(),
                    api_key: None,
                    timeout: Duration::from_secs(5),
                    max_output_tokens: 4096,
                }],
                worker_image: None,
                search: None,
                image_profiles: Vec::new(),
                model_catalog: Default::default(),
            },
            store,
        )
        .unwrap(),
    )
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
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}

async fn workspace(app: &Router) -> String {
    request(
        app,
        "POST",
        "/api/workspaces",
        json!({"title":"Reusable assets"}),
    )
    .await
    .1["id"]
        .as_str()
        .unwrap()
        .into()
}

async fn upload(app: &Router, workspace: &str, id: &str, bytes: &[u8]) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/workspaces/{workspace}/image-assets?id={id}&name=portrait.png"
                ))
                .header("Host", "127.0.0.1")
                .header("Content-Type", "image/png")
                .body(Body::from(bytes.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65_536).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn edit(
    app: &Router,
    workspace: &str,
    base: Option<&str>,
    changes: Value,
    asset_changes: Value,
) -> Value {
    let (status,proposal)=request(app,"POST",&format!("/api/workspaces/{workspace}/proposals"),
        json!({"id":Uuid::new_v4().to_string(),"base_revision":{"id":base},"changes":changes,"asset_changes":asset_changes})).await;
    assert_eq!(status, StatusCode::OK, "{proposal}");
    proposal
}

async fn apply(app: &Router, proposal: &Value) -> Value {
    let (status, revision) = request(
        app,
        "POST",
        &format!("/api/proposals/{}/apply", proposal["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revision}");
    revision
}

async fn archive(
    app: &Router,
    workspace: &str,
    revision: &Value,
) -> zip::ZipArchive<Cursor<Vec<u8>>> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/workspaces/{workspace}/project-export"))
                .header("Host", "127.0.0.1")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({"files":revision["files"],"asset_bindings":revision["asset_bindings"]})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/zip");
    let bytes = to_bytes(response.into_body(), 34_668_544)
        .await
        .unwrap()
        .to_vec();
    zip::ZipArchive::new(Cursor::new(bytes)).unwrap()
}

#[tokio::test]
async fn image_upload_is_immutable_idempotent_and_does_not_create_model_or_project_work() {
    let a = app(Store::open(std::path::Path::new(":memory:")).unwrap(), None);
    let w = workspace(&a).await;
    let id = Uuid::new_v4().to_string();
    let bytes = png(10);
    let (status, image) = upload(&a, &w, &id, &bytes).await;
    assert_eq!(status, StatusCode::CREATED, "{image}");
    assert_eq!(image["id"], id);
    assert_eq!(image["origin"], "upload");
    assert!(image["image_job_id"].is_null());
    assert_eq!(image["name"], "portrait.png");
    assert_eq!(image["bytes"], bytes.len());
    assert_eq!(upload(&a, &w, &id, &bytes).await.1, image);
    assert_eq!(upload(&a, &w, &id, &png(20)).await.0, StatusCode::CONFLICT);
    let other = workspace(&a).await;
    assert_eq!(
        upload(&a, &other, &id, &bytes).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        upload(&a, &w, &Uuid::new_v4().to_string(), b"not an image")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (_, detail) = request(&a, "GET", &format!("/api/workspaces/{w}"), Value::Null).await;
    assert_eq!(detail["image_assets"].as_array().unwrap().len(), 1);
    for field in ["runs", "revisions", "proposals", "assets"] {
        assert!(detail[field].as_array().unwrap().is_empty(), "{field}");
    }
    let (_, text) = request(
        &a,
        "POST",
        &format!("/api/workspaces/{w}/assets"),
        json!({"name":"table.csv","content":"a,b\n1,2\n"}),
    )
    .await;
    assert_eq!(
        upload(&a, &w, text["id"].as_str().unwrap(), &bytes).await.0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn reviewed_bindings_and_historical_exports_pin_exact_asset_bytes() {
    let a = app(Store::open(std::path::Path::new(":memory:")).unwrap(), None);
    let w = workspace(&a).await;
    let first = Uuid::new_v4().to_string();
    let second = Uuid::new_v4().to_string();
    let first_bytes = png(10);
    let second_bytes = png(200);
    upload(&a, &w, &first, &first_bytes).await;
    upload(&a, &w, &second, &second_bytes).await;
    let html = "<html><img src=\"assets/hero.png\"></html>";
    let input_id = Uuid::new_v4().to_string();
    let input = json!({"id":input_id,"base_revision":{"id":null},
        "changes":[{"op":"write","path":"index.html","content":html}],
        "asset_changes":[{"op":"bind","path":"assets/hero.png","asset_id":first}]});
    let (status, proposal) = request(
        &a,
        "POST",
        &format!("/api/workspaces/{w}/proposals"),
        input.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{proposal}");
    let (_, before) = request(&a, "GET", &format!("/api/workspaces/{w}"), Value::Null).await;
    assert!(before["revisions"].as_array().unwrap().is_empty());
    let rev1 = apply(&a, &proposal).await;
    assert_eq!(
        request(&a, "POST", &format!("/api/workspaces/{w}/proposals"), input)
            .await
            .1,
        proposal_with_application(proposal.clone(), &rev1)
    );
    let replace = edit(
        &a,
        &w,
        rev1["id"].as_str(),
        json!([]),
        json!([{"op":"bind","path":"assets/hero.png","asset_id":second}]),
    )
    .await;
    let rev2 = apply(&a, &replace).await;
    assert_eq!(rev1["asset_bindings"][0]["asset_id"], first);
    assert_eq!(rev2["asset_bindings"][0]["asset_id"], second);
    for (revision, expected) in [(&rev1, &first_bytes), (&rev2, &second_bytes)] {
        let mut zip = archive(&a, &w, revision).await;
        assert_eq!(zip.len(), 2);
        let mut content = Vec::new();
        zip.by_name("assets/hero.png")
            .unwrap()
            .read_to_end(&mut content)
            .unwrap();
        assert_eq!(&content, expected);
        let mut source = String::new();
        zip.by_name("index.html")
            .unwrap()
            .read_to_string(&mut source)
            .unwrap();
        assert_eq!(source, html);
    }
    let remove = edit(
        &a,
        &w,
        rev2["id"].as_str(),
        json!([]),
        json!([{"op":"unbind","path":"assets/hero.png"}]),
    )
    .await;
    let rev3 = apply(&a, &remove).await;
    assert!(rev3["asset_bindings"].as_array().unwrap().is_empty());
    let (_, detail) = request(&a, "GET", &format!("/api/workspaces/{w}"), Value::Null).await;
    assert_eq!(detail["image_assets"].as_array().unwrap().len(), 2);
    assert_eq!(
        detail["revisions"][0]["asset_bindings"][0]["asset_id"],
        first
    );
}

fn proposal_with_application(mut proposal: Value, revision: &Value) -> Value {
    proposal["status"] = json!("applied");
    proposal["applied_revision_id"] = revision["id"].clone();
    proposal
}

#[tokio::test]
async fn asset_only_edits_use_exact_base_checks_and_reject_foreign_or_conflicting_paths() {
    let a = app(Store::open(std::path::Path::new(":memory:")).unwrap(), None);
    let w = workspace(&a).await;
    let other = workspace(&a).await;
    let image = Uuid::new_v4().to_string();
    let foreign = Uuid::new_v4().to_string();
    upload(&a, &w, &image, &png(10)).await;
    upload(&a, &other, &foreign, &png(20)).await;
    for (files, bindings) in [
        (
            json!([]),
            json!([{"op":"bind","path":"assets/hero.png","asset_id":foreign}]),
        ),
        (
            json!([]),
            json!([{"op":"bind","path":"../hero.png","asset_id":image}]),
        ),
        (
            json!([{"op":"write","path":"assets","content":"collision"}]),
            json!([{"op":"bind","path":"assets/hero.png","asset_id":image}]),
        ),
        (
            json!([{"op":"write","path":"hero.png","content":"collision"}]),
            json!([{"op":"bind","path":"hero.png","asset_id":image}]),
        ),
        (
            json!([]),
            json!([{"op":"bind","path":"wrong.jpg","asset_id":image}]),
        ),
    ] {
        let (status,value)=request(&a,"POST",&format!("/api/workspaces/{w}/proposals"),
            json!({"id":Uuid::new_v4().to_string(),"base_revision":{"id":null},"changes":files,"asset_changes":bindings})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{value}");
    }
    let old = edit(
        &a,
        &w,
        None,
        json!([]),
        json!([{"op":"bind","path":"hero.png","asset_id":image}]),
    )
    .await;
    let source = edit(
        &a,
        &w,
        None,
        json!([{"op":"write","path":"index.html","content":"hello"}]),
        json!([]),
    )
    .await;
    apply(&a, &source).await;
    assert_eq!(
        request(
            &a,
            "POST",
            &format!("/api/proposals/{}/apply", old["id"].as_str().unwrap()),
            Value::Null
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &a,
            "POST",
            &format!("/api/workspaces/{w}/project-export"),
            json!({"files":[],"asset_bindings":[{"path":"foreign.png","asset_id":foreign}]})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn source_generation_receives_binding_metadata_never_pixels_and_preserves_resources() {
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = captured.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let model = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener,Router::new().route("/chat/completions",post(move |Json(body):Json<Value>| {
            observed.lock().unwrap().push(body);
            async {Json(json!({"choices":[{"index":0,"message":{"content":json!({"changes":[{"op":"write","path":"index.html","content":"<html><h1>Hero</h1><img src=\"assets/hero.png\"></html>"}]}).to_string()},"finish_reason":"stop"}]}))}
        }))).await.unwrap();
    });
    let a = app(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
        Some(model),
    );
    let w = workspace(&a).await;
    let image = Uuid::new_v4().to_string();
    upload(&a, &w, &image, &png(20)).await;
    let binding = edit(
        &a,
        &w,
        None,
        json!([]),
        json!([{"op":"bind","path":"assets/hero.png","asset_id":image}]),
    )
    .await;
    let base = apply(&a, &binding).await;
    let (status,run)=request(&a,"POST",&format!("/api/workspaces/{w}/runs"),json!({
        "prompt":"Create an app using the bound hero image.","mode":"files","base_revision":{"id":base["id"]}
    })).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (_, value) = request(
                &a,
                "GET",
                &format!("/api/runs/{}", run["id"].as_str().unwrap()),
                Value::Null,
            )
            .await;
            if value["status"] == "succeeded" {
                break;
            }
            assert_ne!(value["status"], "failed", "{value}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let (_, detail) = request(&a, "GET", &format!("/api/workspaces/{w}"), Value::Null).await;
    let generated = detail["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|proposal| proposal["run_id"] == run["id"])
        .unwrap();
    let revision = apply(&a, generated).await;
    assert_eq!(revision["asset_bindings"], base["asset_bindings"]);
    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let body = requests[0].to_string();
    assert!(body.contains("assets/hero.png") && body.contains(&image));
    assert!(body.contains("metadata") && body.contains("image/png"));
    assert!(!body.contains("iVBORw0KGgo"));
    assert!(!body.contains("data:image/"));
    assert!(!body.contains("/image-assets/"));
    server.abort();
}

#[tokio::test]
async fn project_asset_count_and_aggregate_byte_limits_are_measured_before_export() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let w = store.create_workspace("Budget".into()).await.unwrap();
    let id = Uuid::new_v4().to_string();
    let mut bytes = png(10);
    bytes.resize(8_388_608, 0);
    store
        .upload_image(
            w.id.clone(),
            id.clone(),
            "large.png".into(),
            bytes,
            "image/png".into(),
        )
        .await
        .unwrap();
    let bindings = |count| {
        (0..count)
            .map(|i| deep_server::artifacts::AssetBinding {
                path: format!("assets/{i}.png"),
                asset_id: id.clone(),
            })
            .collect::<Vec<_>>()
    };
    assert!(
        store
            .export_project(w.id.clone(), Vec::new(), bindings(5))
            .await
            .unwrap_err()
            .to_string()
            .contains("32 MiB")
    );
    let valid = store
        .export_project(w.id.clone(), Vec::new(), bindings(4))
        .await
        .unwrap();
    assert_eq!(zip::ZipArchive::new(Cursor::new(valid)).unwrap().len(), 4);
    assert!(
        store
            .export_project(w.id, Vec::new(), bindings(65))
            .await
            .unwrap_err()
            .to_string()
            .contains("64")
    );
}
