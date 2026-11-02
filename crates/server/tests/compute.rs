use axum::{
    Router,
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
    sync::{Arc, Mutex},
    time::Duration,
};
use tower::ServiceExt;

async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (address, task)
}

fn config(model_url: String) -> Config {
    Config {
        public_origin: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: std::env::temp_dir(),
        web_dir: std::env::temp_dir(),
        default_backend: "compatible".into(),
        backends: vec![
            BackendConfig {
                kind: deep_server::config::BackendKind::Compatible,
                runtime: None,
                resource_group: None,
                id: "compatible".into(),
                base_url: model_url,
                model: "fixture-model".into(),
                api_key: None,
                timeout: Duration::from_secs(15),
                max_output_tokens: 4096,
            },
            BackendConfig {
                kind: deep_server::config::BackendKind::Demo,
                runtime: None,
                resource_group: None,
                id: "demo".into(),
                base_url: String::new(),
                model: "demo-model".into(),
                api_key: None,
                timeout: Duration::from_secs(5),
                max_output_tokens: 4096,
            },
        ],
        search: None,
        image_profiles: Vec::new(),
        model_catalog: Default::default(),
        worker_image: Some("fixture-worker".into()),
    }
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
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

async fn long_terminal(app: &Router, kind: &str, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(70), async {
        loop {
            let (_, value) = request(app, "GET", &format!("/api/{kind}/{id}"), Value::Null).await;
            if value["status"]
                .as_str()
                .is_some_and(|status| !["queued", "running", "cancelling"].contains(&status))
            {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("run did not settle")
}

fn compute_call(id: &str, code: &str, input_asset_ids: &[&str]) -> Value {
    json!({
        "id": id,
        "type": "function",
        "function": {
            "name": "compute_node",
            "arguments": serde_json::to_string(&json!({
                "code": code,
                "input_asset_ids": input_asset_ids
            })).unwrap()
        }
    })
}

fn model_response(calls: Vec<Value>, text: &str) -> Value {
    json!({
        "model": "fixture-model",
        "choices": [{
            "index": 0,
            "message": {
                "content": text,
                "tool_calls": calls
            },
            "finish_reason": if calls.is_empty() { "stop" } else { "tool_calls" }
        }],
        "usage": {
            "prompt_tokens": 12,
            "completion_tokens": 6,
            "total_tokens": 18
        }
    })
}

#[tokio::test]
async fn url_import_permission_and_whole_batch_validation_are_independent_of_search_and_compute() {
    let batch = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = batch.clone();
    let (model_url, task) = serve(Router::new().route(
        "/chat/completions",
        post(move |axum::Json(input): axum::Json<Value>| {
            let calls = captured.lock().unwrap().clone();
            async move {
                assert!(input["tools"].as_array().is_none_or(|tools| {
                    !tools.iter().any(|t| t["function"]["name"] == "web_search")
                }));
                axum::Json(model_response(calls, ""))
            }
        }),
    ))
    .await;
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(config(model_url), store).unwrap());
    let (_, workspace) = request(
        &app,
        "POST",
        "/api/workspaces",
        json!({"title":"URL permission"}),
    )
    .await;
    let path = format!("/api/workspaces/{}/runs", workspace["id"].as_str().unwrap());
    let import = |id: &str, url: &str| json!({"id":id,"type":"function","function":{"name":"import_url","arguments":json!({"url":url,"name":"age.csv"}).to_string()}});
    for (permitted, calls) in [
        (false, vec![import("i1", "https://example.com/age.csv")]),
        (
            true,
            vec![
                import("i1", "https://example.com/age.csv"),
                compute_call("c1", "console.log(1)", &[]),
            ],
        ),
        (
            true,
            vec![
                import("i1", "https://example.com/age.csv"),
                import("i1", "https://example.com/other.csv"),
            ],
        ),
        (true, vec![import("i1", "http://127.0.0.1/private")]),
    ] {
        *batch.lock().unwrap() = calls;
        let (status, accepted) = request(
            &app,
            "POST",
            &path,
            json!({"prompt":"Import","mode":"chat","allow_url_import":permitted}),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(accepted["import_enabled"], permitted);
        let done = long_terminal(&app, "runs", accepted["id"].as_str().unwrap()).await;
        // With no tools permitted the adapter rejects an unsolicited native call.
        assert_eq!(done["status"], "failed");
        assert_eq!(done["imports"], json!([]));
        assert_eq!(done["tool_executions"], json!([]));
    }

    // Runtime import failure (e.g. non-existent domain or HTML page) returns a structured
    // failure observation to the model and consumes 1 import budget, allowing recovery.
    let round = Arc::new(Mutex::new(0usize));
    let seen_tool_msg = Arc::new(Mutex::new(Value::Null));
    let round_ref = round.clone();
    let seen_ref = seen_tool_msg.clone();
    let (recovery_url, recovery_task) = serve(Router::new().route(
        "/chat/completions",
        post(move |axum::Json(input): axum::Json<Value>| {
            let mut r = round_ref.lock().unwrap();
            *r += 1;
            let current = *r;
            if current == 2
                && let Some(msg) = input["messages"]
                    .as_array()
                    .and_then(|msgs| msgs.iter().rfind(|m| m["role"] == "tool"))
            {
                *seen_ref.lock().unwrap() =
                    serde_json::from_str(msg["content"].as_str().unwrap_or("{}")).unwrap();
            }
            async move {
                if current == 1 {
                    axum::Json(model_response(
                        vec![json!({"id":"imp_fail","type":"function","function":{"name":"import_url","arguments":json!({"url":"https://nonexistent-domain-for-deep-import-test.invalid-tld.org/page","name":"wiki.md"}).to_string()}})],
                        "",
                    ))
                } else {
                    axum::Json(model_response(
                        vec![],
                        "Recovered using search snippets after import failed.",
                    ))
                }
            }
        }),
    ))
    .await;
    let recovery_store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let recovery_app = router(App::new(config(recovery_url), recovery_store).unwrap());
    let (_, rw) = request(
        &recovery_app,
        "POST",
        "/api/workspaces",
        json!({"title":"Import recovery"}),
    )
    .await;
    let rpath = format!("/api/workspaces/{}/runs", rw["id"].as_str().unwrap());
    let (status, accepted) = request(
        &recovery_app,
        "POST",
        &rpath,
        json!({"prompt":"Summarize topic","mode":"chat","allow_url_import":true}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let recovered = long_terminal(&recovery_app, "runs", accepted["id"].as_str().unwrap()).await;
    assert_eq!(recovered["status"], "succeeded", "{recovered}");
    assert_eq!(recovered["imports"], json!([]));
    assert_eq!(recovered["model_rounds"], 2);
    let (_, detail) = request(
        &recovery_app,
        "GET",
        &format!("/api/workspaces/{}", rw["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(
        detail["messages"][1]["content"],
        "Recovered using search snippets after import failed."
    );
    let tool_obs = seen_tool_msg.lock().unwrap().clone();
    assert_eq!(tool_obs["status"], "failed", "{tool_obs}");
    assert!(
        tool_obs["note"]
            .as_str()
            .unwrap_or_default()
            .contains("not HTML webpages"),
        "{tool_obs}"
    );
    recovery_task.abort();

    let (status, _) = request(
        &app,
        "POST",
        &path,
        json!({"prompt":"Import","mode":"chat","backend":"demo","allow_url_import":true}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    task.abort();
}

#[tokio::test]
async fn http_assets_and_asset_proposals_enforce_ownership_immutability_and_exact_base() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(config("http://127.0.0.1:9".into()), store).unwrap());
    let (_, w1) = request(&app, "POST", "/api/workspaces", json!({"title":"Compute"})).await;
    let (_, w2) = request(&app, "POST", "/api/workspaces", json!({"title":"Other"})).await;
    let w1_id = w1["id"].as_str().unwrap();
    let w2_id = w2["id"].as_str().unwrap();

    for invalid in [
        json!({"name": "../escape.csv", "content": "a,b\n1,2"}),
        json!({"name": "CON.csv", "content": "a,b\n1,2"}),
        json!({"name": "", "content": "a,b\n1,2"}),
        json!({"name": "huge.csv", "content": "x".repeat(1_048_577)}),
        json!({"name": "bad.csv", "media_type": "invalid media type", "content": "a,b"}),
    ] {
        let (status, _) = request(
            &app,
            "POST",
            &format!("/api/workspaces/{w1_id}/assets"),
            invalid,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    let (status, csv1) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w1_id}/assets"),
        json!({"name": "sales.csv", "content": "item,amount\nWidget,12\nGadget,18\n"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(csv1["media_type"], "text/csv");
    assert_eq!(csv1["origin"], "upload");
    let csv1_id = csv1["id"].as_str().unwrap();

    let (status, csv2) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w1_id}/assets"),
        json!({"name": "sales.csv", "content": "item,amount\nOther,5\n"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let csv2_id = csv2["id"].as_str().unwrap();
    assert_ne!(csv1_id, csv2_id);

    let (status, fetched) =
        request(&app, "GET", &format!("/api/assets/{csv1_id}"), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["content"], "item,amount\nWidget,12\nGadget,18\n");

    let (_, foreign) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w2_id}/assets"),
        json!({"name": "foreign.csv", "content": "x\n1\n"}),
    )
    .await;
    let foreign_id = foreign["id"].as_str().unwrap();

    // Cross-workspace asset proposal is rejected with 404 (not found in this workspace).
    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w1_id}/asset-proposals"),
        json!({"base_revision": {"id": null}, "asset_ids": [foreign_id]}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Colliding asset names cannot be proposed into the same project snapshot without disambiguation.
    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w1_id}/asset-proposals"),
        json!({"base_revision": {"id": null}, "asset_ids": [csv1_id, csv2_id]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Create a proposal for csv1 against empty project base.
    let (status, prop1) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w1_id}/asset-proposals"),
        json!({"base_revision": {"id": null}, "asset_ids": [csv1_id]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{prop1}");
    assert_eq!(prop1["asset_ids"], json!([csv1_id]));

    // Advance the project head via a manual edit before applying prop1; prop1 must now fail with 409 StaleBase.
    let (_, manual) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w1_id}/proposals"),
        json!({
            "id": uuid::Uuid::new_v4(),
            "base_revision": {"id": null},
            "changes": [{"op": "write", "path": "README.md", "content": "# Project\n"}]
        }),
    )
    .await;
    let (_, rev1) = request(
        &app,
        "POST",
        &format!("/api/proposals/{}/apply", manual["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/proposals/{}/apply", prop1["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Fresh review against rev1 succeeds and preserves asset_ids on the resulting revision.
    let (status, prop2) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w1_id}/asset-proposals"),
        json!({"base_revision": {"id": rev1["id"]}, "asset_ids": [csv1_id]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, rev2) = request(
        &app,
        "POST",
        &format!("/api/proposals/{}/apply", prop2["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rev2["asset_ids"], json!([csv1_id]));
    assert_eq!(rev2["files"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn http_compute_node_validates_batches_and_budgets_before_effects() {
    let calls_to_return: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_calls = calls_to_return.clone();
    let (model_url, model_task) = serve(Router::new().route(
        "/chat/completions",
        post(move || {
            let calls = captured_calls.lock().unwrap().clone();
            async move { axum::Json(model_response(calls, "")) }
        }),
    ))
    .await;
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(config(model_url), store).unwrap());
    let (_, w) = request(
        &app,
        "POST",
        "/api/workspaces",
        json!({"title":"Batch validation"}),
    )
    .await;
    let w_id = w["id"].as_str().unwrap();
    let (_, asset) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w_id}/assets"),
        json!({"name": "input.csv", "content": "a,b\n1,2\n"}),
    )
    .await;
    let asset_id = asset["id"].as_str().unwrap().to_owned();
    let unselected_id = uuid::Uuid::new_v4().to_string();

    for batch in [
        // Unselected asset ID
        vec![compute_call("c1", "console.log(1);", &[&unselected_id])],
        // Duplicate asset ID in same call
        vec![compute_call(
            "c1",
            "console.log(1);",
            &[&asset_id, &asset_id],
        )],
        // Empty code
        vec![compute_call("c1", "   ", &[&asset_id])],
        // Exceeding 2 compute calls in one request
        vec![
            compute_call("c1", "console.log(1);", &[&asset_id]),
            compute_call("c2", "console.log(2);", &[&asset_id]),
            compute_call("c3", "console.log(3);", &[&asset_id]),
        ],
    ] {
        *calls_to_return.lock().unwrap() = batch;
        let (status, receipt) = request(
            &app,
            "POST",
            &format!("/api/workspaces/{w_id}/runs"),
            json!({
                "prompt": "Compute",
                "mode": "chat",
                "node_scope": "compute",
                "input_asset_ids": [asset_id]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let done = long_terminal(&app, "runs", receipt["id"].as_str().unwrap()).await;
        assert_eq!(done["status"], "failed");
        assert!(done["tool_executions"].as_array().unwrap().is_empty());
        let (_, jobs) = request(
            &app,
            "GET",
            &format!("/api/workspaces/{w_id}/executions"),
            Value::Null,
        )
        .await;
        assert!(jobs.as_array().unwrap().is_empty());
    }
    model_task.abort();
}

#[tokio::test]
#[ignore = "requires DEEP_TEST_WORKER_IMAGE and a local Linux Docker engine"]
async fn docker_workspace_computation_automatically_saves_outputs_without_review() {
    let (model,task)=serve(Router::new().route("/chat/completions",post(|axum::Json(body):axum::Json<Value>|async move {
        let has_result=body["messages"].as_array().unwrap().iter().any(|m|m["role"]=="tool");
        axum::Json(if has_result {
            model_response(vec![],"Saved report.md.")
        } else {
            model_response(vec![json!({"id":"compute","type":"function","function":{"name":"compute_node","arguments":json!({"code":"require('node:fs').writeFileSync('/output/report.md', '# Computed report\\n');","input_asset_ids":[]}).to_string()}})],"")
        })
    }))).await;
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let mut settings = config(model);
    settings.worker_image = Some(std::env::var("DEEP_TEST_WORKER_IMAGE").unwrap());
    let app = router(App::new(settings, store.clone()).unwrap());
    let (_, w) = request(
        &app,
        "POST",
        "/api/workspaces",
        json!({"title":"Automatic outputs"}),
    )
    .await;
    let id = w["id"].as_str().unwrap();
    let (status,run)=request(&app,"POST",&format!("/api/workspaces/{id}/runs"),json!({"prompt":"Create a report.","mode":"chat","workspace_tools":true,"node_scope":"compute"})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    let done = long_terminal(&app, "runs", run["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(
        done["tool_execution"]["result"]["status"], "succeeded",
        "{done}"
    );
    let detail = store.workspace(id.into()).await.unwrap().unwrap();
    assert_eq!(detail.assets.len(), 1);
    assert_eq!(detail.revisions.len(), 1);
    assert_eq!(detail.revisions[0].files[0].path, "report.md");
    assert_eq!(detail.revisions[0].files[0].content, "# Computed report\n");
    assert_eq!(detail.proposals[0].status, "applied");
    task.abort();
}

#[tokio::test]
#[ignore = "requires DEEP_TEST_WORKER_IMAGE and a local Linux Docker engine"]
async fn docker_csv_to_svg_temporary_computation_recovers_from_failed_first_script_without_project_revision()
 {
    let image = std::env::var("DEEP_TEST_WORKER_IMAGE").expect("set DEEP_TEST_WORKER_IMAGE");
    let observed_requests: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = observed_requests.clone();
    let asset_holder: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let asset_for_model = asset_holder.clone();

    let (model_url, model_task) = serve(Router::new().route(
        "/chat/completions",
        post(move |axum::Json(body): axum::Json<Value>| {
            let mut requests = captured.lock().unwrap();
            requests.push(body);
            let asset_id = asset_for_model.lock().unwrap().clone();
            let turn = requests.len();
            let reply = match turn {
                1 => model_response(
                    vec![compute_call(
                        "call_fail",
                        "const fs = require('node:fs'); fs.writeFileSync('/output/discard.svg', '<svg/>'); console.error('syntax/logic error in first attempt'); process.exit(1);",
                        &[&asset_id],
                    )],
                    "",
                ),
                2 => model_response(
                    vec![compute_call(
                        "call_ok",
                        &format!(
                            "// {}\n{}",
                            "padding".repeat(512),
                            r#"const fs = require('node:fs');
const lines = fs.readFileSync('sales.csv', 'utf8').trim().split('\n').slice(1);
let total = 0;
for (const line of lines) {
  const [, amount] = line.split(',');
  total += Number(amount);
}
fs.writeFileSync('sales.csv', 'mutated in container only');
fs.writeFileSync('/output/totals.json', JSON.stringify({ total }));
fs.writeFileSync('/output/chart.svg', `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 40"><text x="10" y="25">Total: ${total}</text></svg>`);
console.log(`TOTAL=${total}`);
"#
                        ),
                        &[&asset_id],
                    )],
                    "",
                ),
                _ => model_response(vec![], "The total is 45 and chart.svg has been generated."),
            };
            async move { axum::Json(reply) }
        }),
    ))
    .await;

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("compute.sqlite3")).unwrap();
    let mut settings = config(model_url);
    settings.worker_image = Some(image);
    let app = router(App::new(settings, store).unwrap());

    let (_, w) = request(
        &app,
        "POST",
        "/api/workspaces",
        json!({"title":"CSV to SVG"}),
    )
    .await;
    let w_id = w["id"].as_str().unwrap();
    let csv_body = "region,amount\nNorth,15\nSouth,30\n";
    let (status, uploaded) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w_id}/assets"),
        json!({"name": "sales.csv", "content": csv_body}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let csv_id = uploaded["id"].as_str().unwrap().to_owned();
    *asset_holder.lock().unwrap() = csv_id.clone();

    let (status, receipt) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w_id}/runs"),
        json!({
            "prompt": "Compute the total from sales.csv and create an SVG chart.",
            "mode": "chat",
            "node_scope": "compute",
            "input_asset_ids": [csv_id]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let done = long_terminal(&app, "runs", receipt["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(done["model_rounds"], 3);

    let executions = done["tool_executions"].as_array().unwrap();
    assert_eq!(executions.len(), 2);
    assert_eq!(executions[0]["result"]["status"], "failed");
    assert_eq!(executions[0]["result"]["exit_code"], 1, "{done}");
    assert!(
        executions[0]["result"]["output_assets"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(executions[1]["result"]["status"], "succeeded");
    assert_eq!(executions[1]["result"]["stdout"], "TOTAL=45\n");
    assert!(executions[1]["code"].as_str().unwrap().len() > 2048);
    let output_assets = executions[1]["result"]["output_assets"].as_array().unwrap();
    assert_eq!(output_assets.len(), 2);

    // First model request received only metadata for sales.csv, not its raw CSV lines.
    let requests = observed_requests.lock().unwrap().clone();
    let first_messages = requests[0]["messages"].to_string();
    assert!(first_messages.contains("sales.csv"));
    assert!(!first_messages.contains("North,15"));
    // Tool response to model contains asset metadata, not the SVG body.
    let tool_reply = requests[2]["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap();
    assert!(tool_reply.contains("chart.svg"));
    assert!(!tool_reply.contains("<svg xmlns="));
    assert!(!tool_reply.contains("Total: 45"));

    // Workspace has no project revisions, input CSV is unchanged, and SVG is downloadable.
    let (_, detail) = request(&app, "GET", &format!("/api/workspaces/{w_id}"), Value::Null).await;
    assert!(detail["revisions"].as_array().unwrap().is_empty());
    assert_eq!(detail["assets"].as_array().unwrap().len(), 3);
    let (_, original_csv) =
        request(&app, "GET", &format!("/api/assets/{csv_id}"), Value::Null).await;
    assert_eq!(original_csv["content"], csv_body);
    let svg_meta = output_assets
        .iter()
        .find(|a| a["name"] == "chart.svg")
        .unwrap();
    let (_, svg_detail) = request(
        &app,
        "GET",
        &format!("/api/assets/{}", svg_meta["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert!(
        svg_detail["content"]
            .as_str()
            .unwrap()
            .contains("Total: 45")
    );

    model_task.abort();
}

#[tokio::test]
async fn version_eleven_saved_revision_receipts_and_input_base_proposals_survive_v12_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v11-upgrade.sqlite3");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch(include_str!("fixtures/schema-v4.sql"))
        .unwrap();
    c.pragma_update(None, "foreign_keys", false).unwrap();
    c.execute_batch(include_str!("../src/store/schema-v5.sql"))
        .unwrap();
    c.execute_batch(include_str!("../src/store/schema-v6.sql"))
        .unwrap();
    c.execute_batch(include_str!("../src/store/schema-v7.sql"))
        .unwrap();
    c.execute_batch(include_str!("../src/store/schema-v8.sql"))
        .unwrap();
    c.execute_batch(
        "ALTER TABLE runs ADD COLUMN execution_result TEXT;
         ALTER TABLE runs ADD COLUMN search_config TEXT;
         ALTER TABLE runs ADD COLUMN search_activity TEXT NOT NULL DEFAULT '[]';
         ALTER TABLE runs ADD COLUMN model_rounds INTEGER NOT NULL DEFAULT 0;
         PRAGMA user_version=10;",
    )
    .unwrap();
    c.execute_batch(include_str!("../src/store/schema-v11.sql"))
        .unwrap();
    let rev1_id = uuid::Uuid::new_v4().to_string();
    let exec_id = uuid::Uuid::new_v4().to_string();
    let run_id = uuid::Uuid::new_v4().to_string();
    let edit_id = uuid::Uuid::new_v4().to_string();
    c.execute(
        "INSERT INTO proposals(id,workspace_id,edit_id,status,changes,applied_revision_id) VALUES (?1,'workspace',?2,'applied','[]',?3)",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), edit_id, rev1_id],
    )
    .unwrap();
    c.execute(
        "INSERT INTO revisions(id,workspace_id,edit_id,number,html,files) VALUES (?1,'workspace',?2,3,'','[{\"path\":\"main.cjs\",\"content\":\"console.log(1);\"}]')",
        rusqlite::params![rev1_id, edit_id],
    )
    .unwrap();
    c.execute(
        "INSERT INTO executions(id,workspace_id,revision_id,entry,action,image_id,status,stdout,stderr,exit_code,created_at_ms,finished_at_ms,output_files)
         VALUES (?1,'workspace',?2,'main.cjs','run','sha256:0000000000000000000000000000000000000000000000000000000000000000','succeeded','ok\n','',0,100,200,'[{\"path\":\"generated.txt\",\"content\":\"from rev1\"}]')",
        rusqlite::params![exec_id, rev1_id],
    )
    .unwrap();
    c.execute(
        "INSERT INTO runs(id,workspace_id,status,provider,model,output,stats,created_at_ms,started_at_ms,finished_at_ms,mode,execution_revision_id,model_rounds)
         VALUES (?1,'workspace','succeeded','compatible','fixture','', '{}', 100, 110, 210, 'chat', ?2, 2)",
        rusqlite::params![run_id, rev1_id],
    )
    .unwrap();
    let attachment = json!({
        "id": exec_id,
        "workspace_id": "workspace",
        "revision_id": rev1_id,
        "entry": "main.cjs",
        "action": "run",
        "status": "succeeded",
        "stdout": "ok\n",
        "stderr": "",
        "exit_code": 0,
        "signal": null,
        "truncated": false,
        "error": null,
        "logs_shortened": false
    });
    c.execute(
        "INSERT INTO run_executions(run_id,call_id,execution_id,entry,action,result) VALUES (?1,'call_old',?2,'main.cjs','run',?3)",
        rusqlite::params![run_id, exec_id, attachment.to_string()],
    )
    .unwrap();
    drop(c);

    let store = Store::open(&path).unwrap();
    let migrated_run = store.run(run_id).await.unwrap().unwrap();
    assert_eq!(
        migrated_run.node_scope,
        deep_server::store::NodeScope::Revision
    );
    assert_eq!(
        migrated_run.execution_revision_id.as_deref(),
        Some(rev1_id.as_str())
    );
    assert_eq!(migrated_run.tool_executions.len(), 1);
    assert_eq!(migrated_run.tool_execution.as_ref().unwrap().id, exec_id);

    // Advance workspace head to revision 3, then verify saved-revision output proposal still keeps rev1 as its base.
    let edit = store
        .propose_edits(
            "workspace".into(),
            uuid::Uuid::new_v4().to_string(),
            Some(rev1_id.clone()),
            vec![deep_server::artifacts::FileChange::Write {
                path: "later.txt".into(),
                content: "head advanced".into(),
            }],
            Vec::new(),
        )
        .await
        .unwrap()
        .unwrap();
    let rev2 = store.apply_proposal(edit.id).await.unwrap().unwrap();
    assert_eq!(rev2.number, 4);

    let output_proposal = store
        .propose_execution_outputs(exec_id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        output_proposal.base_revision_id.as_deref(),
        Some(rev1_id.as_str())
    );
    assert_eq!(
        output_proposal.execution_id.as_deref(),
        Some(exec_id.as_str())
    );
}

#[tokio::test]
#[ignore = "requires DEEP_LIVE_MODEL_URL/DEEP_LIVE_MODEL_ID (or .env backend) and DEEP_TEST_WORKER_IMAGE"]
async fn live_model_csv_to_svg_compute_flow() {
    let _ = dotenvy::dotenv();
    let (provider_id, model_url, model_id, api_key) = if let (Ok(url), Ok(id)) = (
        std::env::var("DEEP_LIVE_MODEL_URL"),
        std::env::var("DEEP_LIVE_MODEL_ID"),
    ) {
        (
            "compatible".to_string(),
            url,
            id,
            std::env::var("DEEP_LIVE_MODEL_API_KEY").ok(),
        )
    } else if let (Ok(id), Ok(key)) = (
        std::env::var("DEEP_OPENAI_MODEL"),
        std::env::var("OPENAI_API_KEY"),
    ) {
        (
            "openai".to_string(),
            "https://api.openai.com/v1".into(),
            id,
            Some(key),
        )
    } else if let (Ok(id), Ok(key)) = (
        std::env::var("DEEP_GROQ_MODEL"),
        std::env::var("GROQ_API_KEY"),
    ) {
        (
            "groq".to_string(),
            "https://api.groq.com/openai/v1".into(),
            id,
            Some(key),
        )
    } else {
        panic!("set DEEP_LIVE_MODEL_URL and DEEP_LIVE_MODEL_ID or configure .env");
    };
    let image = std::env::var("DEEP_TEST_WORKER_IMAGE").expect("set DEEP_TEST_WORKER_IMAGE");
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("live-compute.sqlite3")).unwrap();
    let mut settings = config(model_url);
    settings.default_backend = provider_id.clone();
    settings.backends[0].kind = serde_json::from_value(serde_json::json!(provider_id)).unwrap();
    settings.backends[0].id = provider_id;
    settings.backends[0].model = model_id.clone();
    settings.backends[0].api_key = api_key.filter(|v| !v.trim().is_empty());
    settings.backends[0].timeout = Duration::from_secs(60);
    settings.worker_image = Some(image);
    let app = router(App::new(settings, store).unwrap());

    let (_, w) = request(
        &app,
        "POST",
        "/api/workspaces",
        json!({"title":"Live CSV to SVG"}),
    )
    .await;
    let w_id = w["id"].as_str().unwrap();
    let (_, csv) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w_id}/assets"),
        json!({"name": "items.csv", "content": "item,qty\nApples,14\nPears,26\n"}),
    )
    .await;
    let csv_id = csv["id"].as_str().unwrap();
    let (status, receipt) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w_id}/runs"),
        json!({
            "prompt": "Use compute_node with input_asset_ids [\"".to_owned() + csv_id + "\"] to read items.csv from its worker path, compute the total qty, write an SVG bar chart to /output/chart.svg, and print exactly one stdout line in the form TOTAL=<computed sum>. Report the total in your final response.",
            "mode": "chat",
            "node_scope": "compute",
            "input_asset_ids": [csv_id]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    let done = long_terminal(&app, "runs", receipt["id"].as_str().unwrap()).await;
    eprintln!(
        "Live model ({model_id}) status={}, error={:?}, rounds={}, output={:?}, tool_executions={}",
        done["status"],
        done["error"],
        done["model_rounds"],
        done["output"],
        done["tool_executions"]
    );
    assert_eq!(done["status"], "succeeded", "{done}");
    let executions = done["tool_executions"].as_array().unwrap();
    assert!((1..=2).contains(&executions.len()), "{done}");
    let result = &executions.last().unwrap()["result"];
    assert_eq!(result["status"], "succeeded", "{done}");
    assert_eq!(result["stdout"], "TOTAL=40\n", "{done}");
    let chart = result["output_assets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|asset| asset["name"] == "chart.svg")
        .expect("live computation must save chart.svg");
    assert_eq!(chart["media_type"], "image/svg+xml");
    assert_eq!(chart["workspace_id"], w_id);
    assert_eq!(chart["execution_id"], result["id"]);
    let (status, saved_chart) = request(
        &app,
        "GET",
        &format!("/api/assets/{}", chart["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let svg = saved_chart["content"].as_str().unwrap();
    assert!(svg.contains("<svg") && svg.contains("</svg>"), "{svg}");

    let (_, detail) = request(&app, "GET", &format!("/api/workspaces/{w_id}"), Value::Null).await;
    assert!(detail["revisions"].as_array().unwrap().is_empty());
    let (_, original) = request(&app, "GET", &format!("/api/assets/{csv_id}"), Value::Null).await;
    assert_eq!(original["content"], "item,qty\nApples,14\nPears,26\n");
}

#[tokio::test]
async fn turn_deletion_and_workspace_branching_preserve_files_and_unapplied_proposals() {
    let model_requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = model_requests.clone();
    let (model_url, _model_task) = serve(Router::new().route(
        "/chat/completions",
        post(move |axum::Json(payload): axum::Json<Value>| {
            let seen = seen.clone();
            async move {
                let index = {
                    let mut guard = seen.lock().unwrap();
                    guard.push(payload.clone());
                    guard.len()
                };
                let text = match index {
                    1 => r#"{"changes":[{"op":"write","path":"index.html","content":"<h1>Counter v1</h1>\n"}]}"#,
                    2 => r#"{"changes":[{"op":"write","path":"index.html","content":"<h1>Counter v2</h1>\n"},{"op":"write","path":"notes.md","content":"Pending review\n"}]}"#,
                    3 => "Follow-up chat turn three.",
                    _ => "Post-deletion chat turn four.",
                };
                axum::Json(model_response(Vec::new(), text))
            }
        }),
    ))
    .await;

    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(config(model_url), store).unwrap());

    let (_, w) = request(
        &app,
        "POST",
        "/api/workspaces",
        json!({"title": "Branching & Turn Deletion"}),
    )
    .await;
    let w_id = w["id"].as_str().unwrap();

    // Upload a workspace asset as well.
    let (status, asset) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w_id}/assets"),
        json!({"name": "data.csv", "content": "col\n1\n"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let asset_id = asset["id"].as_str().unwrap();

    // Run 1: create and apply initial project files (creates revision 1).
    let (status, run1_receipt) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w_id}/runs"),
        json!({"prompt": "Build counter v1", "mode": "files", "base_revision": {"id": null}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run1_receipt}");
    let run1_id = run1_receipt["id"].as_str().unwrap();
    let run1 = long_terminal(&app, "runs", run1_id).await;
    assert_eq!(run1["status"], "succeeded", "{run1}");

    let (_, detail1) = request(&app, "GET", &format!("/api/workspaces/{w_id}"), Value::Null).await;
    let prop1_id = detail1["proposals"][0]["id"].as_str().unwrap();
    let (status, applied_detail) = request(
        &app,
        "POST",
        &format!("/api/proposals/{prop1_id}/apply"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied_detail}");
    let (_, detail_after_apply1) =
        request(&app, "GET", &format!("/api/workspaces/{w_id}"), Value::Null).await;
    assert_eq!(
        detail_after_apply1["revisions"].as_array().unwrap().len(),
        1
    );
    let rev1_id = detail_after_apply1["revisions"][0]["id"].as_str().unwrap();

    // Run 2: create an UNAPPLIED pending proposal on top of revision 1.
    let (_, run2_receipt) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w_id}/runs"),
        json!({"prompt": "Propose counter v2", "mode": "files", "base_revision": {"id": rev1_id}}),
    )
    .await;
    let run2_id = run2_receipt["id"].as_str().unwrap();
    let run2 = long_terminal(&app, "runs", run2_id).await;
    assert_eq!(run2["status"], "succeeded", "{run2}");

    // Run 3: ordinary chat turn after the unapplied proposal.
    let (_, run3_receipt) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w_id}/runs"),
        json!({"prompt": "Discuss v2", "mode": "chat"}),
    )
    .await;
    let run3_id = run3_receipt["id"].as_str().unwrap();
    let run3 = long_terminal(&app, "runs", run3_id).await;
    assert_eq!(run3["status"], "succeeded", "{run3}");

    // 1. Branch workspace from run 1 (`up_to_run_id: run1_id`):
    //    Chat messages after run 1 are omitted in the branch, while accepted workspace files,
    //    assets, AND the unapplied pending proposal from run 2 remain intact!
    let (status, branch_ws) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w_id}/branch"),
        json!({"up_to_run_id": run1_id}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{branch_ws}");
    let branch_id = branch_ws["id"].as_str().unwrap();
    assert_ne!(branch_id, w_id);
    assert_eq!(branch_ws["title"], "Branching & Turn Deletion (branch)");
    let (_, branched) = request(
        &app,
        "GET",
        &format!("/api/workspaces/{branch_id}"),
        Value::Null,
    )
    .await;
    // Only run 1's user + assistant messages (2 messages) are in the branch.
    let branch_msgs = branched["messages"].as_array().unwrap();
    assert_eq!(branch_msgs.len(), 2, "{branched}");
    // Accepted revision 1 and asset are preserved in the branch.
    assert_eq!(branched["revisions"].as_array().unwrap().len(), 1);
    assert_eq!(
        branched["revisions"][0]["files"][0]["content"],
        "<h1>Counter v1</h1>\n"
    );
    assert_eq!(branched["assets"].as_array().unwrap().len(), 1);
    assert_ne!(branched["assets"][0]["id"].as_str().unwrap(), asset_id);

    // Both proposals (applied prop 1 and pending unapplied prop 2) are preserved in the branch,
    // and the unapplied proposal in the branch can be applied independently!
    let branch_proposals = branched["proposals"].as_array().unwrap();
    assert_eq!(branch_proposals.len(), 2, "{branched}");
    let branch_pending = branch_proposals
        .iter()
        .find(|p| p["status"] == "pending")
        .expect("unapplied proposal must be preserved in branched workspace");
    let branch_pending_id = branch_pending["id"].as_str().unwrap();
    let (status, branch_apply_receipt) = request(
        &app,
        "POST",
        &format!("/api/proposals/{branch_pending_id}/apply"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{branch_apply_receipt}");
    let (_, branch_after_apply) = request(
        &app,
        "GET",
        &format!("/api/workspaces/{branch_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(branch_after_apply["revisions"].as_array().unwrap().len(), 2);

    // Source workspace still has only 1 applied revision and its own pending proposal untouched.
    let (_, source_detail) =
        request(&app, "GET", &format!("/api/workspaces/{w_id}"), Value::Null).await;
    assert_eq!(source_detail["revisions"].as_array().unwrap().len(), 1);
    let source_pending = source_detail["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["status"] == "pending")
        .expect("source pending proposal must still be pending");
    let source_pending_id = source_pending["id"].as_str().unwrap();

    // 2. Delete turn 2 (`run2_id`, which created the unapplied proposal) AND turn 3 (`run3_id`)
    //    in the source workspace.
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/workspaces/{w_id}/messages/{run2_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // Deleting the same turn again is idempotent (204 No Content).
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/workspaces/{w_id}/messages/{run2_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/api/workspaces/{w_id}/messages/{run3_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify source workspace after turn deletion:
    // - Only run 1's 2 messages remain.
    // - Accepted revision 1 and the unapplied pending proposal from run 2 remain intact!
    let (_, after_delete) =
        request(&app, "GET", &format!("/api/workspaces/{w_id}"), Value::Null).await;
    assert_eq!(after_delete["messages"].as_array().unwrap().len(), 2);
    assert_eq!(after_delete["revisions"].as_array().unwrap().len(), 1);
    assert!(
        after_delete["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == source_pending_id && p["status"] == "pending")
    );

    // Run 4: verify deleted turns are excluded from subsequent model context!
    let (_, run4_receipt) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{w_id}/runs"),
        json!({"prompt": "After deleting turns 2 and 3", "mode": "chat"}),
    )
    .await;
    let run4 = long_terminal(&app, "runs", run4_receipt["id"].as_str().unwrap()).await;
    assert_eq!(run4["status"], "succeeded", "{run4}");
    let last_request = model_requests.lock().unwrap().last().cloned().unwrap();
    let serialized = last_request["messages"].to_string();
    assert!(
        !serialized.contains("Propose counter v2") && !serialized.contains("Discuss v2"),
        "deleted turns must not be sent in model context: {serialized}"
    );

    // And the unapplied proposal in the source workspace can still be applied after its turn's
    // chat messages were deleted!
    let (status, source_apply_receipt) = request(
        &app,
        "POST",
        &format!("/api/proposals/{source_pending_id}/apply"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{source_apply_receipt}");
    let (_, source_applied) =
        request(&app, "GET", &format!("/api/workspaces/{w_id}"), Value::Null).await;
    assert_eq!(source_applied["revisions"].as_array().unwrap().len(), 2);
}
