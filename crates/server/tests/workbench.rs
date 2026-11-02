use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use deep_server::{
    App,
    artifacts::GeneratedArtifact,
    config::{BackendConfig, Config},
    model::{Mode, Model, extract_html},
    router,
    store::Store,
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc};
use tower::ServiceExt;

fn config() -> Config {
    Config {
        public_origin: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: ".".into(),
        web_dir: ".".into(),
        default_backend: "demo".into(),
        worker_image: None,
        search: None,
        image_profiles: Vec::new(),
        model_catalog: Default::default(),
        backends: vec![BackendConfig {
            kind: deep_server::config::BackendKind::Demo,
            runtime: None,
            resource_group: None,
            id: "demo".into(),
            base_url: String::new(),
            model: "demo-model".into(),
            api_key: None,
            timeout: Duration::from_secs(3),
            max_output_tokens: 4096,
        }],
    }
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    body: Value,
    headers: &[(&str, &str)],
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

async fn request(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = send(app, method, path, body, &[("Host", "127.0.0.1")]).await;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_097_152).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn terminal(app: &Router, id: &str) -> Value {
    for _ in 0..250 {
        let (_, run) = request(app, "GET", &format!("/api/runs/{id}"), Value::Null).await;
        if !matches!(run["status"].as_str(), Some("queued" | "running")) {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("run did not finish");
}

async fn execution_fixture(store: &Store) -> (String, String, String) {
    use deep_server::{
        artifacts::FileChange,
        store::{Action, ExecutionInput},
    };
    let workspace = store
        .create_workspace("Result context".into())
        .await
        .unwrap();
    let proposal = store
        .propose_edits(
            workspace.id.clone(),
            uuid::Uuid::new_v4().to_string(),
            None,
            vec![FileChange::Write {
                path: "main.cjs".into(),
                content: "throw new Error('broken');".into(),
            }],
            Vec::new(),
        )
        .await
        .unwrap()
        .unwrap();
    let revision = store.apply_proposal(proposal.id).await.unwrap().unwrap();
    let (job, _) = store
        .accept_execution(
            workspace.id.clone(),
            ExecutionInput {
                id: uuid::Uuid::new_v4(),
                revision_id: uuid::Uuid::parse_str(&revision.id).unwrap(),
                entry: "main.cjs".into(),
                action: Action::Run,
            },
            "fixture-image".into(),
        )
        .await
        .unwrap();
    (workspace.id, revision.id, job.id)
}

#[tokio::test]
async fn execution_attachment_is_scoped_terminal_and_rejects_before_creating_model_work() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let (workspace, _, job) = execution_fixture(&store).await;
    let other = store.create_workspace("Other".into()).await.unwrap();
    let app = router(App::new(config(), store.clone()).unwrap());
    for (owner, result, expected) in [
        (&workspace, job.clone(), StatusCode::CONFLICT),
        (&other.id, job.clone(), StatusCode::NOT_FOUND),
        (
            &workspace,
            uuid::Uuid::new_v4().to_string(),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            request(
                &app,
                "POST",
                &format!("/api/workspaces/{owner}/runs"),
                json!({"prompt":"Inspect result", "mode":"chat", "execution_result_id":result})
            )
            .await
            .0,
            expected
        );
        let detail = store.workspace(owner.clone()).await.unwrap().unwrap();
        assert!(detail.runs.is_empty() && detail.messages.is_empty());
    }
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("/api/workspaces/{workspace}/runs"),
            json!({"prompt":"Inspect", "mode":"chat", "execution_result_id":"invalid"}),
            &[("Host", "127.0.0.1")]
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Running and cleanup-pending results are equally ineligible.
    store.start_execution(job.clone()).await.unwrap();
    store.cancel_execution(job.clone()).await.unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/api/workspaces/{workspace}/runs"),
            json!({"prompt":"Inspect", "mode":"chat", "execution_result_id":job})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    store
        .finish_execution(
            job.clone(),
            deep_server::store::ExecutionResult {
                status: "cancelled",
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();
    let (_, run) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{workspace}/runs"),
        json!({"prompt":"Inspect", "mode":"chat", "execution_result_id":job}),
    )
    .await;
    assert_eq!(run["execution_result"]["status"], "cancelled");
    assert_eq!(
        terminal(&app, run["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
}

#[tokio::test]
async fn result_logs_are_bounded_frozen_and_sent_only_with_explicit_successful_turns() {
    use deep_server::{
        artifacts::FileChange,
        store::{ExecutionResult, RunOptions},
    };
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = captured.clone();
    let backend = Router::new().route("/custom/v1/chat/completions", post(move |axum::Json(body): axum::Json<Value>| {
        observed.lock().unwrap().push(body);
        async { axum::Json(json!({"choices":[{"message":{"content":"{\"changes\":[{\"op\":\"write\",\"path\":\"main.cjs\",\"content\":\"console.log('fixed');\"}]}"},"finish_reason":"stop"}]})) }
    }));
    let (cfg, task) = compatible_backend(backend).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("results.sqlite3");
    let store = Store::open(&path).unwrap();
    let (workspace, executed_revision, job) = execution_fixture(&store).await;
    store.start_execution(job.clone()).await.unwrap();
    store
        .finish_execution(
            job.clone(),
            ExecutionResult {
                status: "failed",
                exit_code: Some(1),
                stdout: "<img src='http://invalid'> UNTRUSTED_CANARY".into(),
                stderr: format!("PREFIX_CANARY{}TAIL_CANARY", "界🙂".repeat(3000)),
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();
    // A newer accepted head does not alter what was executed.
    let proposal = store
        .propose_edits(
            workspace.clone(),
            uuid::Uuid::new_v4().to_string(),
            Some(executed_revision.clone()),
            vec![FileChange::Write {
                path: "note.txt".into(),
                content: "Current accepted files".into(),
            }],
            Vec::new(),
        )
        .await
        .unwrap()
        .unwrap();
    let current = store.apply_proposal(proposal.id).await.unwrap().unwrap();
    let app = router(App::new(cfg, store.clone()).unwrap());
    let url = format!("/api/workspaces/{workspace}/runs");
    let (_, without) = request(
        &app,
        "POST",
        &url,
        json!({"prompt":"No result", "mode":"chat"}),
    )
    .await;
    terminal(&app, without["id"].as_str().unwrap()).await;
    assert!(
        !captured.lock().unwrap()[0]
            .to_string()
            .contains("UNTRUSTED_CANARY")
    );
    let (status, receipt) = request(
        &app,
        "POST",
        &url,
        json!({"prompt":"Fix the attached result", "mode":"files",
        "base_revision":{"id":current.id}, "execution_result_id":job}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let result = &receipt["execution_result"];
    assert_eq!(result["revision_id"], executed_revision);
    assert_eq!(receipt["base_revision_id"], current.id);
    assert_eq!(result["logs_shortened"], true);
    let stderr = result["stderr"].as_str().unwrap();
    assert!(
        stderr.len() <= 8192
            && stderr.ends_with("TAIL_CANARY")
            && !stderr.contains("PREFIX_CANARY")
    );
    let id = receipt["id"].as_str().unwrap();
    assert_eq!(terminal(&app, id).await["execution_result"], *result);
    let sent = captured.lock().unwrap()[1].to_string();
    assert!(
        sent.contains("UNTRUSTED_CANARY")
            && sent.contains("untrusted program output")
            && sent.contains("Current accepted files")
    );
    // Terminal worker updates cannot rewrite the observation or saved context.
    let frozen = serde_json::to_value(store.run_context(id.into()).await.unwrap()).unwrap();
    store
        .finish_execution(
            job.clone(),
            ExecutionResult {
                status: "succeeded",
                exit_code: Some(0),
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();
    assert_eq!(
        store.execution(job.clone()).await.unwrap().unwrap().status,
        "failed"
    );
    assert_eq!(
        serde_json::to_value(store.run_context(id.into()).await.unwrap()).unwrap(),
        frozen
    );
    let (_, followup) = request(
        &app,
        "POST",
        &url,
        json!({"prompt":"Follow up", "mode":"chat"}),
    )
    .await;
    terminal(&app, followup["id"].as_str().unwrap()).await;
    assert!(
        captured.lock().unwrap()[2]
            .to_string()
            .contains("TAIL_CANARY")
    );
    assert!(followup["execution_result"].is_null());
    let pending = store
        .begin(
            workspace.clone(),
            "Failed turn".into(),
            "demo".into(),
            "demo-model".into(),
            RunOptions {
                model_route: None,
                mode: Mode::Chat,
                base_revision_id: None,
                execution_result_id: Some(job.clone()),
                execution_revision_id: None,
                search: None,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    store
        .terminate(pending.id, "failed", Some("fixture".into()))
        .await
        .unwrap();
    drop(app);
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        serde_json::to_value(store.run(id.into()).await.unwrap().unwrap()).unwrap()["execution_result"],
        *result
    );
    assert_eq!(
        serde_json::to_value(store.run_context(id.into()).await.unwrap()).unwrap(),
        frozen
    );
    let next = store
        .begin(
            workspace.clone(),
            "Check failed turn exclusion".into(),
            "demo".into(),
            "demo-model".into(),
            RunOptions {
                model_route: None,
                mode: Mode::Chat,
                base_revision_id: None,
                execution_result_id: None,
                execution_revision_id: None,
                search: None,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    let history = store.run_context(next.id.clone()).await.unwrap();
    assert_eq!(
        history
            .iter()
            .filter(|message| message.content.contains("Attached execution observation"))
            .count(),
        1
    );
    assert!(
        history
            .iter()
            .all(|message| message.content != "Failed turn")
    );
    store.terminate(next.id, "cancelled", None).await.unwrap();
    // Deletion removes attached snapshots as part of existing workspace history.
    store.delete_workspace(workspace.clone()).await.unwrap();
    assert!(store.workspace(workspace).await.unwrap().is_none());
    task.abort();
}

#[tokio::test]
async fn attached_logs_count_toward_context_budget_without_an_upstream_call() {
    use deep_server::{artifacts::FileChange, store::ExecutionResult};
    let calls = Arc::new(Mutex::new(0));
    let observed = calls.clone();
    let backend = Router::new().route("/custom/v1/chat/completions", post(move || {
        *observed.lock().unwrap() += 1;
        async { axum::Json(json!({"choices":[{"message":{"content":"unexpected"},"finish_reason":"stop"}]})) }
    }));
    let (cfg, task) = compatible_backend(backend).await;
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let (workspace, revision, job) = execution_fixture(&store).await;
    store.start_execution(job.clone()).await.unwrap();
    store
        .finish_execution(
            job.clone(),
            ExecutionResult {
                status: "failed",
                exit_code: Some(1),
                stderr: "x".repeat(16000),
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();
    let edit = store
        .propose_edits(
            workspace.clone(),
            uuid::Uuid::new_v4().to_string(),
            Some(revision),
            vec![FileChange::Write {
                path: "large.txt".into(),
                content: "a".repeat(125_000),
            }],
            Vec::new(),
        )
        .await
        .unwrap()
        .unwrap();
    let head = store.apply_proposal(edit.id).await.unwrap().unwrap();
    let app = router(App::new(cfg, store.clone()).unwrap());
    let (_, run) = request(&app, "POST", &format!("/api/workspaces/{workspace}/runs"),
        json!({"prompt":"Fix", "mode":"files", "base_revision":{"id":head.id}, "execution_result_id":job})).await;
    let done = terminal(&app, run["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "failed");
    assert!(done["error"].as_str().unwrap().contains("128 KiB"));
    assert_eq!(*calls.lock().unwrap(), 0);
    assert!(done["execution_result"].is_object());
    assert_eq!(
        store
            .workspace(workspace)
            .await
            .unwrap()
            .unwrap()
            .revisions
            .len(),
        2
    );
    task.abort();
}

#[test]
fn version_eight_attachment_migration_rolls_back_before_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy8.sqlite3");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch(include_str!("fixtures/schema-v4.sql"))
        .unwrap();
    c.pragma_update(None, "foreign_keys", false).unwrap();
    for sql in [
        include_str!("../src/store/schema-v5.sql"),
        include_str!("../src/store/schema-v6.sql"),
        include_str!("../src/store/schema-v7.sql"),
        include_str!("../src/store/schema-v8.sql"),
    ] {
        c.execute_batch(sql).unwrap();
    }
    c.execute_batch("ALTER TABLE runs ADD COLUMN execution_result TEXT;")
        .unwrap();
    drop(c);
    assert!(Store::open(&path).is_err());
    let c = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        8
    );
    assert_eq!(
        c.query_row("SELECT status FROM runs WHERE id='unfinished'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "queued"
    );
    c.execute_batch("ALTER TABLE runs DROP COLUMN execution_result;")
        .unwrap();
    drop(c);
    let store = Store::open(&path).unwrap();
    drop(store);
    let c = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        23
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM runs WHERE execution_result IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

async fn proposed(app: &Router, workspace: &str, base: Option<&str>) -> Value {
    let (status, run) = request(
        app,
        "POST",
        &format!("/api/workspaces/{workspace}/runs"),
        json!({"prompt":"Propose a counter project","mode":"files","base_revision":{"id":base}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    let completed = terminal(app, run["id"].as_str().unwrap()).await;
    assert_eq!(completed["status"], "succeeded", "{completed}");
    let (_, detail) = request(
        app,
        "GET",
        &format!("/api/workspaces/{workspace}"),
        Value::Null,
    )
    .await;
    detail["proposals"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone()
}

#[tokio::test]
async fn proposals_require_review_apply_once_and_reject_stale_or_settled_actions() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(config(), store.clone()).unwrap());
    let (_, workspace) = request(
        &app,
        "POST",
        "/api/workspaces",
        json!({"title":"File review"}),
    )
    .await;
    let id = workspace["id"].as_str().unwrap();
    let first = proposed(&app, id, None).await;
    let second = proposed(&app, id, None).await;
    let (_, before) = request(&app, "GET", &format!("/api/workspaces/{id}"), Value::Null).await;
    assert!(before["revisions"].as_array().unwrap().is_empty());
    assert_eq!(first["status"], "pending");
    let apply = format!("/api/proposals/{}/apply", first["id"].as_str().unwrap());
    let (status, revision) = request(&app, "POST", &apply, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revision["number"], 1);
    assert_eq!(revision["files"].as_array().unwrap().len(), 3);
    assert_eq!(request(&app, "POST", &apply, Value::Null).await.1, revision);
    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/proposals/{}/apply", second["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let rejected_path = format!("/api/proposals/{}", second["id"].as_str().unwrap());
    let (status, rejected) = request(&app, "DELETE", &rejected_path, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(
        request(&app, "DELETE", &rejected_path, Value::Null).await.1,
        rejected
    );
    assert_eq!(
        request(&app, "POST", &format!("{rejected_path}/apply"), Value::Null)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/api/proposals/{}", first["id"].as_str().unwrap()),
            Value::Null
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, after) = request(&app, "GET", &format!("/api/workspaces/{id}"), Value::Null).await;
    assert_eq!(after["revisions"].as_array().unwrap().len(), 1);
    assert_eq!(after["proposals"][1]["changes"], second["changes"]);
    assert_eq!(after["revisions"][0], revision);
}

#[tokio::test]
async fn manual_edits_are_idempotent_reviewed_and_keep_exact_files_and_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edits.sqlite3");
    let store = Store::open(&path).unwrap();
    let app = router(App::new(config(), store.clone()).unwrap());
    let workspace = store.create_workspace("Manual edits".into()).await.unwrap();
    let url = format!("/api/workspaces/{}/proposals", workspace.id);
    let edit = uuid::Uuid::new_v4().to_string();
    let first_input = json!({"id":edit,"base_revision":{"id":null},"changes":[
        {"op":"write","path":"index.html","content":"<h1>First</h1>"},
        {"op":"write","path":"keep.txt","content":"\u{feff}Exact\r\n界🙂\r\n"},
        {"op":"write","path":"large.txt","content":"x".repeat(70_000)}
    ]});
    let (status, first) = request(&app, "POST", &url, first_input.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}"); // body exceeds the normal 64 KiB route limit
    assert_eq!(first["edit_id"], edit);
    assert!(first["run_id"].is_null() && first["execution_id"].is_null());
    assert_eq!(first["status"], "pending");
    assert_eq!(
        request(&app, "POST", &url, first_input.clone()).await.1,
        first
    );
    let detail = store
        .workspace(workspace.id.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(detail.runs.is_empty() && detail.messages.is_empty() && detail.revisions.is_empty());
    let apply = format!("/api/proposals/{}/apply", first["id"].as_str().unwrap());
    let (_, saved) = request(&app, "POST", &apply, Value::Null).await;
    assert_eq!(saved["edit_id"], edit);
    assert_eq!(request(&app, "POST", &apply, Value::Null).await.1, saved);
    let input = json!({"id":uuid::Uuid::new_v4().to_string(),"base_revision":{"id":saved["id"]},"changes":[
        {"op":"write","path":"index.html","content":"<h1>Second</h1>"},
        {"op":"delete","path":"large.txt"}
    ]});
    let (_, second) = request(&app, "POST", &url, input.clone()).await;
    let (_, next) = request(
        &app,
        "POST",
        &format!("/api/proposals/{}/apply", second["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(next["files"].as_array().unwrap().len(), 2);
    assert_eq!(next["files"][1]["content"], "\u{feff}Exact\r\n界🙂\r\n");
    assert!(next["run_id"].is_null() && next["execution_id"].is_null());
    assert_eq!(
        request(&app, "POST", &url, first_input.clone()).await.1["applied_revision_id"],
        saved["id"]
    );
    let mut conflict = input.clone();
    conflict["changes"][0]["content"] = json!("Different input");
    assert_eq!(
        request(&app, "POST", &url, conflict).await.0,
        StatusCode::CONFLICT
    );
    let other = store.create_workspace("Other".into()).await.unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/api/workspaces/{}/proposals", other.id),
            input.clone()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let mut stale = input.clone();
    stale["id"] = json!(uuid::Uuid::new_v4().to_string());
    assert_eq!(
        request(&app, "POST", &url, stale).await.0,
        StatusCode::CONFLICT
    );
    let detail = store
        .workspace(workspace.id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.proposals.len(), 2);
    assert!(detail.messages.is_empty() && detail.runs.is_empty());
    drop(app);
    drop(store);
    let store = Store::open(&path).unwrap();
    let repeated = store
        .propose_edits(
            workspace.id.clone(),
            input["id"].as_str().unwrap().into(),
            Some(saved["id"].as_str().unwrap().into()),
            serde_json::from_value(input["changes"].clone()).unwrap(),
            Vec::new(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repeated.applied_revision_id.as_deref(), next["id"].as_str());
    let run = store
        .begin(
            workspace.id,
            "Discuss edits".into(),
            "demo".into(),
            "demo-model".into(),
            deep_server::store::RunOptions {
                model_route: None,
                mode: Mode::Chat,
                base_revision_id: None,
                execution_result_id: None,
                execution_revision_id: None,
                search: None,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    let context = serde_json::to_value(store.run_context(run.id).await.unwrap())
        .unwrap()
        .to_string();
    assert!(context.contains(&edit) && context.contains("applied"));
}

#[tokio::test]
async fn manual_edit_input_bounds_do_not_mutate_project_or_relax_model_limits() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(config(), store.clone()).unwrap());
    let workspace = store
        .create_workspace("Manual boundaries".into())
        .await
        .unwrap();
    let path = format!("/api/workspaces/{}/proposals", workspace.id);
    for changes in [
        json!([]),
        json!([{"op":"delete","path":"missing.txt"}]),
        json!([{"op":"write","path":"../escape","content":"x"}]),
        json!([{"op":"write","path":"CON.txt","content":"x"}]),
        json!([{"op":"write","path":"a","content":"x"},{"op":"write","path":"a/b","content":"y"}]),
        json!([{"op":"write","path":"a.txt","content":"x"},{"op":"write","path":"A.txt","content":"y"}]),
        json!([{"op":"write","path":"big.txt","content":"x".repeat(1_048_577)}]),
    ] {
        let (status, _) = request(&app,"POST",&path,json!({"id":uuid::Uuid::new_v4().to_string(),"base_revision":{"id":null},"changes":changes})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    for input in [
        json!({"id":"invalid","base_revision":{"id":null},"changes":[]}),
        json!({"id":uuid::Uuid::new_v4().to_string(),"base_revision":{},"changes":[]}),
        json!({"id":uuid::Uuid::new_v4().to_string(),"changes":[]}),
        json!({"id":uuid::Uuid::new_v4().to_string(),"base_revision":{"id":null},"changes":[],"extra":true}),
    ] {
        assert!(
            send(&app, "POST", &path, input, &[("Host", "127.0.0.1")])
                .await
                .status()
                .is_client_error()
        );
    }
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("/api/workspaces/{}/runs", workspace.id),
            json!({"prompt":"x".repeat(70_000),"mode":"chat"}),
            &[("Host", "127.0.0.1")]
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let detail = store.workspace(workspace.id).await.unwrap().unwrap();
    assert!(
        detail.proposals.is_empty()
            && detail.revisions.is_empty()
            && detail.runs.is_empty()
            && detail.messages.is_empty()
    );
}

#[test]
fn version_six_manual_source_upgrade_preserves_context_and_rolls_back_before_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy6.sqlite3");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch(include_str!("fixtures/schema-v4.sql"))
        .unwrap();
    c.pragma_update(None, "foreign_keys", false).unwrap();
    c.execute_batch(include_str!("../src/store/schema-v5.sql"))
        .unwrap();
    c.execute_batch(include_str!("../src/store/schema-v6.sql"))
        .unwrap();
    c.execute_batch("UPDATE proposals SET rejection_reason='Keep the original' WHERE status='rejected'; UPDATE runs SET context_messages='[{\"role\":\"user\",\"content\":\"frozen\"}]' WHERE id='unfinished'; CREATE TABLE proposals_v7(sentinel TEXT); INSERT INTO proposals_v7 VALUES('keep');").unwrap();
    drop(c);
    assert!(Store::open(&path).is_err());
    let c = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        6
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM pragma_table_info('revisions') WHERE name='edit_id'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT status FROM runs WHERE id='unfinished'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "queued"
    );
    c.execute_batch("DROP TABLE proposals_v7").unwrap();
    drop(c);
    drop(Store::open(&path).unwrap());
    let c = rusqlite::Connection::open(&path).unwrap();
    c.pragma_update(None, "foreign_keys", true).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        23
    );
    assert_eq!(
        c.query_row(
            "SELECT rejection_reason FROM proposals WHERE status='rejected'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "Keep the original"
    );
    assert_eq!(
        c.query_row(
            "SELECT context_messages FROM runs WHERE id='unfinished'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "[{\"role\":\"user\",\"content\":\"frozen\"}]"
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    assert!(
        c.execute(
            "UPDATE proposals SET edit_id='extra-source' WHERE run_id IS NOT NULL",
            []
        )
        .is_err()
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM proposals WHERE edit_id IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn applying_changes_waits_for_active_runs_and_generation_checks_the_base() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(config(), store.clone()).unwrap());
    let (_, workspace) = request(
        &app,
        "POST",
        "/api/workspaces",
        json!({"title":"Base admission"}),
    )
    .await;
    let id = workspace["id"].as_str().unwrap();
    for input in [
        json!({"prompt":"Files","mode":"files"}),
        json!({"prompt":"Files","mode":"files","base_revision":{}}),
    ] {
        let response = send(
            &app,
            "POST",
            &format!("/api/workspaces/{id}/runs"),
            input,
            &[("Host", "127.0.0.1")],
        )
        .await;
        assert!(response.status().is_client_error());
    }
    let proposal = proposed(&app, id, None).await;
    let held = store
        .begin(
            id.into(),
            "Held".into(),
            "demo".into(),
            "demo-model".into(),
            deep_server::store::RunOptions {
                model_route: None,
                mode: Mode::Chat,
                base_revision_id: None,
                execution_result_id: None,
                execution_revision_id: None,
                search: None,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    let apply = format!("/api/proposals/{}/apply", proposal["id"].as_str().unwrap());
    assert_eq!(
        request(&app, "POST", &apply, Value::Null).await.0,
        StatusCode::CONFLICT
    );
    request(
        &app,
        "DELETE",
        &format!("/api/runs/{}", held.id),
        Value::Null,
    )
    .await;
    let (_, revision) = request(&app, "POST", &apply, Value::Null).await;
    let (status, _) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{id}/runs"),
        json!({"prompt":"Stale","mode":"files","base_revision":{"id":null}}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, detail) = request(&app, "GET", &format!("/api/workspaces/{id}"), Value::Null).await;
    assert_eq!(detail["runs"].as_array().unwrap().len(), 2);
    let next = proposed(&app, id, revision["id"].as_str()).await;
    assert_eq!(next["base_revision_id"], revision["id"]);
    assert_eq!(next["changes"].as_array().unwrap().len(), 1);
    let (_, updated) = request(
        &app,
        "POST",
        &format!("/api/proposals/{}/apply", next["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(updated["number"], 2);
    assert_eq!(updated["files"].as_array().unwrap().len(), 3);
    assert_eq!(
        store.workspace(id.into()).await.unwrap().unwrap().revisions[0]
            .files
            .len(),
        3
    );
}

#[tokio::test]
async fn rejection_feedback_is_durable_bounded_and_frozen_for_each_model_request() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let seed = router(App::new(config(), store.clone()).unwrap());
    let workspace = store
        .create_workspace("Proposal feedback".into())
        .await
        .unwrap();
    let first = proposed(&seed, &workspace.id, None).await;
    let revision = store
        .apply_proposal(first["id"].as_str().unwrap().into())
        .await
        .unwrap()
        .unwrap();
    let second = proposed(&seed, &workspace.id, Some(&revision.id)).await;
    let before = serde_json::to_value(
        store
            .workspace(workspace.id.clone())
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let queued = store
        .begin(
            workspace.id.clone(),
            "Already queued".into(),
            "demo".into(),
            "demo-model".into(),
            deep_server::store::RunOptions {
                model_route: None,
                mode: Mode::Chat,
                base_revision_id: None,
                execution_result_id: None,
                execution_revision_id: None,
                search: None,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    let frozen = serde_json::to_value(store.run_context(queued.id.clone()).await.unwrap()).unwrap();
    assert!(frozen.to_string().contains("document.querySelector")); // pending proposals remain available for revision requests
    let reject = format!("/api/proposals/{}/reject", second["id"].as_str().unwrap());
    for input in [
        json!({"reason": "界".repeat(1366)}),
        json!({"reason": 42}),
        json!({"reason":"ok","extra":true}),
    ] {
        assert!(
            send(&seed, "POST", &reject, input, &[("Host", "127.0.0.1")])
                .await
                .status()
                .is_client_error()
        );
    }
    let reason = "Keep the increment at one; improve the styling instead. 界🙂";
    let (status, rejected) = request(
        &seed,
        "POST",
        &reject,
        json!({"reason":format!("  {reason}  ")}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(rejected["rejection_reason"], reason);
    assert_eq!(
        request(&seed, "POST", &reject, json!({"reason":reason}))
            .await
            .1,
        rejected
    );
    assert_eq!(
        request(&seed, "POST", &reject, json!({"reason":"Different reason"}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &seed,
            "DELETE",
            &format!("/api/proposals/{}", second["id"].as_str().unwrap()),
            Value::Null
        )
        .await
        .1,
        rejected
    );
    assert_eq!(
        serde_json::to_value(store.run_context(queued.id.clone()).await.unwrap()).unwrap(),
        frozen
    );
    assert!(!frozen.to_string().contains(reason));
    let after = serde_json::to_value(
        store
            .workspace(workspace.id.clone())
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        &after["messages"].as_array().unwrap()[..4],
        before["messages"].as_array().unwrap().as_slice()
    );
    assert_eq!(after["revisions"], before["revisions"]);
    assert_eq!(after["runs"].as_array().unwrap().len(), 3); // rejection launched no work
    request(
        &seed,
        "DELETE",
        &format!("/api/runs/{}", queued.id),
        Value::Null,
    )
    .await;

    let captured = Arc::new(Mutex::new(None));
    let capture = captured.clone();
    let backend = Router::new().route("/custom/v1/chat/completions", post(move |axum::Json(body): axum::Json<Value>| {
        let capture = capture.clone();
        async move {
            *capture.lock().unwrap() = Some(body);
            axum::Json(json!({"choices":[{"message":{"content":"Acknowledged."},"finish_reason":"stop"}]}))
        }
    }));
    let (cfg, task) = compatible_backend(backend).await;
    let app = router(App::new(cfg, store.clone()).unwrap());
    let (_, run) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{}/runs", workspace.id),
        json!({"mode":"chat","prompt":"What should change next?"}),
    )
    .await;
    assert_eq!(
        terminal(&app, run["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let body = captured.lock().unwrap().clone().unwrap();
    let messages = body["messages"].as_array().unwrap();
    let state_text = messages
        .iter()
        .find_map(|message| {
            message["content"]
                .as_str()
                .filter(|text| text.starts_with("Project state at request submission"))
        })
        .unwrap();
    let state: Value = serde_json::from_str(state_text.split_once('\n').unwrap().1).unwrap();
    assert_eq!(state["accepted_revision"]["id"], revision.id);
    assert_eq!(state["proposals"][0]["status"], "applied");
    assert_eq!(state["proposals"][1]["status"], "rejected");
    assert_eq!(state["proposals"][1]["rejection_reason"], reason);
    assert!(!body.to_string().contains("document.querySelector")); // full proposal source stays in the transcript
    assert!(!body.to_string().contains("Already queued")); // cancelled turns stay excluded
    assert!(!messages.iter().any(|message| {
        message["content"]
            .as_str()
            .unwrap()
            .starts_with("Accepted project snapshot")
    })); // chat mode sends no files
    assert_eq!(
        messages.last().unwrap()["content"],
        "What should change next?"
    );
    task.abort();
}

#[tokio::test]
async fn rejected_proposal_and_saved_context_survive_restart() {
    use deep_server::artifacts::FileChange;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("feedback.sqlite3");
    let store = Store::open(&path).unwrap();
    let workspace = store
        .create_workspace("Saved feedback".into())
        .await
        .unwrap();
    let run = store
        .begin(
            workspace.id.clone(),
            "Create".into(),
            "demo".into(),
            "demo-model".into(),
            deep_server::store::RunOptions {
                model_route: None,
                mode: Mode::Files,
                base_revision_id: None,
                execution_result_id: None,
                execution_revision_id: None,
                search: None,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    store.mark_running(run.id.clone()).await.unwrap();
    store
        .finish(
            run,
            "Original full proposal".into(),
            Some(GeneratedArtifact::Changes(vec![FileChange::Write {
                path: "index.html".into(),
                content: "<h1>Rejected</h1>".into(),
            }])),
        )
        .await
        .unwrap();
    let proposal = store
        .workspace(workspace.id.clone())
        .await
        .unwrap()
        .unwrap()
        .proposals
        .remove(0);
    store
        .reject_proposal(
            proposal.id.clone(),
            Some("Use a Node generator instead".into()),
        )
        .await
        .unwrap();
    let run = store
        .begin(
            workspace.id.clone(),
            "Next request".into(),
            "demo".into(),
            "demo-model".into(),
            deep_server::store::RunOptions {
                model_route: None,
                mode: Mode::Chat,
                base_revision_id: None,
                execution_result_id: None,
                execution_revision_id: None,
                search: None,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    let expected = serde_json::to_value(store.run_context(run.id.clone()).await.unwrap()).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    let detail = store.workspace(workspace.id).await.unwrap().unwrap();
    assert_eq!(
        detail.proposals[0].rejection_reason.as_deref(),
        Some("Use a Node generator instead")
    );
    assert_eq!(detail.messages[1].content, "Original full proposal");
    assert!(detail.revisions.is_empty());
    assert_eq!(
        serde_json::to_value(store.run_context(run.id.clone()).await.unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        store.run(run.id).await.unwrap().unwrap().status,
        "interrupted"
    );
}

#[test]
fn version_five_context_upgrade_rolls_back_before_recovery_and_preserves_legacy_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy5.sqlite3");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch(include_str!("fixtures/schema-v4.sql"))
        .unwrap();
    c.pragma_update(None, "foreign_keys", false).unwrap();
    c.execute_batch(include_str!("../src/store/schema-v5.sql"))
        .unwrap();
    c.execute_batch("ALTER TABLE runs ADD COLUMN context_messages TEXT;")
        .unwrap();
    drop(c);
    assert!(Store::open(&path).is_err());
    let c = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        c.query_row("SELECT status FROM runs WHERE id='unfinished'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "queued"
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM pragma_table_info('proposals') WHERE name='rejection_reason'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    c.execute_batch("ALTER TABLE runs DROP COLUMN context_messages;")
        .unwrap();
    let previous: String = c
        .query_row(
            "SELECT changes FROM proposals WHERE status='rejected'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(c);
    drop(Store::open(&path).unwrap());
    let c = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        23
    );
    assert_eq!(
        c.query_row(
            "SELECT changes FROM proposals WHERE status='rejected'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        previous
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM proposals WHERE rejection_reason IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row(
            "SELECT context_messages FROM runs WHERE id='unfinished'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "[]"
    );
}

#[tokio::test]
async fn file_snapshots_preserve_untouched_text_and_deletions_across_restart() {
    use deep_server::artifacts::{FileChange, GeneratedArtifact};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("files.sqlite3");
    let store = Store::open(&path).unwrap();
    let workspace = store.create_workspace("Exact files".into()).await.unwrap();
    let run = store
        .begin(
            workspace.id.clone(),
            "Files".into(),
            "demo".into(),
            "demo-model".into(),
            deep_server::store::RunOptions {
                model_route: None,
                mode: Mode::Files,
                base_revision_id: None,
                execution_result_id: None,
                execution_revision_id: None,
                search: None,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    store.mark_running(run.id.clone()).await.unwrap();
    store
        .finish(
            run,
            "Proposal".into(),
            Some(GeneratedArtifact::Changes(vec![
                FileChange::Write {
                    path: "keep.txt".into(),
                    content: "Exact\r\n界🙂\n".into(),
                },
                FileChange::Write {
                    path: "remove.txt".into(),
                    content: "old".into(),
                },
            ])),
        )
        .await
        .unwrap();
    let detail = store
        .workspace(workspace.id.clone())
        .await
        .unwrap()
        .unwrap();
    let first = store
        .apply_proposal(detail.proposals[0].id.clone())
        .await
        .unwrap()
        .unwrap();
    let run = store
        .begin(
            workspace.id.clone(),
            "Delete".into(),
            "demo".into(),
            "demo-model".into(),
            deep_server::store::RunOptions {
                model_route: None,
                mode: Mode::Files,
                base_revision_id: Some(first.id.clone()),
                execution_result_id: None,
                execution_revision_id: None,
                search: None,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    store.mark_running(run.id.clone()).await.unwrap();
    store
        .finish(
            run,
            "Delete proposal".into(),
            Some(GeneratedArtifact::Changes(vec![FileChange::Delete {
                path: "remove.txt".into(),
            }])),
        )
        .await
        .unwrap();
    let detail = store
        .workspace(workspace.id.clone())
        .await
        .unwrap()
        .unwrap();
    let proposal_id = detail.proposals[1].id.clone();
    drop(store);
    let store = Store::open(&path).unwrap();
    let second = store.apply_proposal(proposal_id).await.unwrap().unwrap();
    assert_eq!(second.files.len(), 1);
    assert_eq!(second.files[0].content, "Exact\r\n界🙂\n");
    assert_eq!(
        store
            .workspace(workspace.id)
            .await
            .unwrap()
            .unwrap()
            .revisions[0]
            .files,
        first.files
    );
}

#[test]
fn failed_version_three_upgrade_rolls_back_all_pending_migrations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v3-conflict.sqlite3");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!("fixtures/schema-v1.sql"))
        .unwrap();
    connection
        .execute_batch(
            "CREATE TABLE proposals(sentinel TEXT); INSERT INTO proposals VALUES ('keep');",
        )
        .unwrap();
    drop(connection);
    assert!(Store::open(&path).is_err());
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    let stats_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('runs') WHERE name='stats')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let files_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('revisions') WHERE name='files')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let state: String = connection
        .query_row("SELECT status FROM runs WHERE id='pending'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(version, 1);
    assert!(!stats_exists);
    assert!(!files_exists);
    assert_eq!(state, "queued");
}

async fn compatible_backend(backend: Router) -> (Config, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, backend).await.unwrap() });
    let mut cfg = config();
    cfg.default_backend = "compatible".into();
    cfg.backends[0].id = "compatible".into();
    cfg.backends[0].kind = deep_server::config::BackendKind::Compatible;
    cfg.backends[0].model = "test-model".into();
    cfg.backends[0].base_url = format!("http://{address}/custom/v1");
    cfg.backends[0].timeout = Duration::from_secs(30);
    (cfg, task)
}

type StreamSender = mpsc::Sender<Result<axum::body::Bytes, std::io::Error>>;

async fn streaming_backend() -> (Config, tokio::task::JoinHandle<()>, StreamSender) {
    let (sender, receiver) = mpsc::channel(8);
    let receiver = Arc::new(Mutex::new(Some(receiver)));
    let backend = Router::new().route(
        "/custom/v1/chat/completions",
        post(move || {
            let receiver = receiver
                .lock()
                .unwrap()
                .take()
                .expect("one generation request");
            async move {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(receiver)),
                )
            }
        }),
    );
    let (cfg, task) = compatible_backend(backend).await;
    (cfg, task, sender)
}

fn event(body: Value) -> String {
    format!("data: {body}\r\n\r\n")
}

async fn send_stream(sender: &StreamSender, data: &str) {
    sender
        .send(Ok(axum::body::Bytes::copy_from_slice(data.as_bytes())))
        .await
        .unwrap();
}

async fn draft(app: &Router, id: &str, expected: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let (_, run) = request(app, "GET", &format!("/api/runs/{id}"), Value::Null).await;
            if run["output"] == expected {
                break run;
            }
            assert!(
                matches!(run["status"].as_str(), Some("queued" | "running")),
                "{run}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("draft should arrive before upstream completion")
}

#[tokio::test]
async fn streamed_draft_precedes_atomic_publication_and_records_reported_usage() {
    let (cfg, task, sender) = streaming_backend().await;
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(cfg, store.clone()).unwrap());
    let (_, workspace) = request(&app, "POST", "/api/workspaces", json!({"title":"Stream"})).await;
    let workspace_id = workspace["id"].as_str().unwrap();
    let (_, receipt) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{workspace_id}/runs"),
        json!({"prompt":"Make an app","mode":"app"}),
    )
    .await;
    let id = receipt["id"].as_str().unwrap();
    let prefix = "```html\n<h1>界🙂</h1>";
    send_stream(&sender, &event(json!({"model":"reported-revision","choices":[{"index":0,"delta":{"content":prefix},"finish_reason":null}]}))).await;
    let progress = draft(&app, id, prefix).await;
    assert_eq!(progress["status"], "running");
    assert_eq!(progress["model"], "test-model");
    assert_eq!(progress["stats"]["reported_model"], "reported-revision");
    assert!(progress["stats"]["input_tokens"].is_null());
    assert!(progress["stats"]["first_token_ms"].is_number());
    assert!(progress["queue_ms"].is_number());
    let (_, detail) = request(
        &app,
        "GET",
        &format!("/api/workspaces/{workspace_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(detail["messages"].as_array().unwrap().len(), 1);
    assert!(detail["revisions"].as_array().unwrap().is_empty());
    send_stream(&sender, &(event(json!({"model":"reported-revision","choices":[{"index":0,"delta":{"content":"\n```"},"finish_reason":"stop"}]}))
        + &event(json!({"choices":[],"usage":{"prompt_tokens":13,"completion_tokens":8,"total_tokens":21}}))
        + "data: [DONE]\n\n")).await;
    let done = terminal(&app, id).await;
    assert_eq!(done["status"], "succeeded");
    assert_eq!(done["output"], "");
    assert_eq!(done["stats"]["input_tokens"], 13);
    assert_eq!(done["stats"]["output_tokens"], 8);
    assert_eq!(done["stats"]["total_tokens"], 21);
    assert_eq!(done["stats"]["finish_reason"], "stop");
    let duration = done["duration_ms"].as_i64().unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(terminal(&app, id).await["duration_ms"], duration);
    let history = store.history(workspace_id.into()).await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].content, format!("{prefix}\n```"));
    let (_, detail) = request(
        &app,
        "GET",
        &format!("/api/workspaces/{workspace_id}"),
        Value::Null,
    )
    .await;
    assert!(detail["revisions"].as_array().unwrap().is_empty());
    assert_eq!(
        detail["proposals"][0]["changes"][0]["content"],
        "<h1>界🙂</h1>"
    );
    let (_, revision) = request(
        &app,
        "POST",
        &format!(
            "/api/proposals/{}/apply",
            detail["proposals"][0]["id"].as_str().unwrap()
        ),
        Value::Null,
    )
    .await;
    assert_eq!(revision["html"], "<h1>界🙂</h1>");
    task.abort();
}

#[tokio::test]
async fn cancelling_a_stream_keeps_an_incomplete_draft_and_rejects_late_updates() {
    let (cfg, task, sender) = streaming_backend().await;
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(cfg, store.clone()).unwrap());
    let (_, workspace) = request(
        &app,
        "POST",
        "/api/workspaces",
        json!({"title":"Cancel stream"}),
    )
    .await;
    let workspace_id = workspace["id"].as_str().unwrap();
    let (_, receipt) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{workspace_id}/runs"),
        json!({"prompt":"Make an app","mode":"app"}),
    )
    .await;
    let id = receipt["id"].as_str().unwrap();
    send_stream(
        &sender,
        &event(json!({"model":"reported","choices":[{"delta":{"content":"```html\n<h1>Draft"}}]})),
    )
    .await;
    draft(&app, id, "```html\n<h1>Draft").await;
    let (_, cancelled) = request(&app, "DELETE", &format!("/api/runs/{id}"), Value::Null).await;
    assert_eq!(cancelled["status"], "cancelled");
    let mut late = deep_server::model::Completion {
        workspace_artifact: None,
        text: "Late text".into(),
        stats: Default::default(),
        tool_calls: Vec::new(),
    };
    late.stats.reported_model = Some("late-model".into());
    store.progress(id.into(), late).await.unwrap();
    let run = store.run(id.into()).await.unwrap().unwrap();
    assert!(
        !store
            .finish(
                run,
                "Late answer".into(),
                Some(GeneratedArtifact::Html("<h1>Late</h1>".into()))
            )
            .await
            .unwrap()
    );
    let final_state = terminal(&app, id).await;
    assert_eq!(final_state["output"], "```html\n<h1>Draft");
    assert_eq!(final_state["stats"]["reported_model"], "reported");
    assert_eq!(final_state["duration_ms"], cancelled["duration_ms"]);
    assert!(final_state["stats"]["input_tokens"].is_null());
    assert!(store.history(workspace_id.into()).await.unwrap().is_empty());
    let (_, detail) = request(
        &app,
        "GET",
        &format!("/api/workspaces/{workspace_id}"),
        Value::Null,
    )
    .await;
    assert!(detail["revisions"].as_array().unwrap().is_empty());
    assert_eq!(detail["messages"].as_array().unwrap().len(), 1);
    task.abort();
}

#[tokio::test]
async fn incomplete_truncated_or_invalid_streams_never_publish_an_answer() {
    for (reason, usage, done) in [
        (Some("stop"), Value::Null, false),
        (None, Value::Null, true),
        (Some("length"), Value::Null, true),
        (Some("tool_calls"), Value::Null, true),
        (Some("stop"), json!({"prompt_tokens":-1}), true),
    ] {
        let (cfg, task, sender) = streaming_backend().await;
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let app = router(App::new(cfg, store.clone()).unwrap());
        let (_, workspace) = request(
            &app,
            "POST",
            "/api/workspaces",
            json!({"title":"Invalid stream"}),
        )
        .await;
        let id = workspace["id"].as_str().unwrap();
        let (_, run) = request(
            &app,
            "POST",
            &format!("/api/workspaces/{id}/runs"),
            json!({"prompt":"Make an app","mode":"app"}),
        )
        .await;
        let body = event(
            json!({"choices":[{"delta":{"content":"```html\n<h1>Incomplete</h1>\n```"},"finish_reason":reason}]}),
        ) + &event(json!({"choices":[],"usage":usage}))
            + if done { "data: [DONE]\n\n" } else { "" };
        send_stream(&sender, &body).await;
        drop(sender);
        let finished = terminal(&app, run["id"].as_str().unwrap()).await;
        assert_eq!(finished["status"], "failed", "{finished}");
        assert!(store.history(id.into()).await.unwrap().is_empty());
        let (_, detail) = request(&app, "GET", &format!("/api/workspaces/{id}"), Value::Null).await;
        assert!(detail["revisions"].as_array().unwrap().is_empty());
        task.abort();
    }
}

#[tokio::test]
async fn version_one_migration_preserves_history_revisions_and_unknown_metrics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite3");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!("fixtures/schema-v1.sql"))
        .unwrap();
    drop(connection);
    let store = Store::open(&path).unwrap();
    let detail = store.workspace("workspace".into()).await.unwrap().unwrap();
    assert_eq!(detail.messages[1].content, "Original reply");
    assert_eq!(detail.revisions[0].html, "<h1>Original</h1>");
    let run = &detail.runs[0];
    assert_eq!(run.provider, "compatible");
    assert_eq!(run.model, "original-model");
    assert_eq!(run.status, "succeeded");
    assert!(run.stats.input_tokens.is_none());
    assert!(run.duration_ms.is_none());
    assert!(run.queue_ms.is_none());
    assert_eq!(detail.runs[1].status, "interrupted");
    drop(store);
    let store = Store::open(&path).unwrap();
    let run = store
        .begin(
            "workspace".into(),
            "New question".into(),
            "compatible".into(),
            "new-model".into(),
            deep_server::store::RunOptions {
                model_route: None,
                mode: Mode::Chat,
                base_revision_id: None,
                execution_result_id: None,
                execution_revision_id: None,
                search: None,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    store.mark_running(run.id.clone()).await.unwrap();
    let mut completion = deep_server::model::Completion {
        workspace_artifact: None,
        text: "Persisted draft".into(),
        stats: Default::default(),
        tool_calls: Vec::new(),
    };
    completion.stats.input_tokens = Some(17);
    completion.stats.reported_model = Some("reported".into());
    store.progress(run.id.clone(), completion).await.unwrap();
    drop(store);
    let reopened = Store::open(&path).unwrap();
    let recovered = reopened.run(run.id).await.unwrap().unwrap();
    assert_eq!(recovered.status, "interrupted");
    assert_eq!(recovered.output, "Persisted draft");
    assert_eq!(recovered.stats.input_tokens, Some(17));
    assert_eq!(recovered.stats.reported_model.as_deref(), Some("reported"));
    assert!(recovered.duration_ms.is_some());
}

#[tokio::test]
async fn version_two_upgrade_preserves_observations_and_backfills_file_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v2.sqlite3");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!("fixtures/schema-v1.sql"))
        .unwrap();
    connection
        .execute_batch(
            r#"
        ALTER TABLE runs ADD COLUMN output TEXT NOT NULL DEFAULT '';
        ALTER TABLE runs ADD COLUMN stats TEXT NOT NULL DEFAULT '{}';
        ALTER TABLE runs ADD COLUMN created_at_ms INTEGER;
        ALTER TABLE runs ADD COLUMN started_at_ms INTEGER;
        ALTER TABLE runs ADD COLUMN finished_at_ms INTEGER;
        UPDATE runs SET stats='{"input_tokens":17,"reported_model":"reported-v2"}',
            created_at_ms=100,started_at_ms=120,finished_at_ms=200 WHERE id='complete';
        UPDATE runs SET output='Unfinished draft' WHERE id='pending';
        PRAGMA user_version=2;
    "#,
        )
        .unwrap();
    drop(connection);
    let store = Store::open(&path).unwrap();
    let detail = store.workspace("workspace".into()).await.unwrap().unwrap();
    assert!(detail.proposals.is_empty());
    assert_eq!(detail.revisions[0].files[0].path, "index.html");
    assert_eq!(detail.revisions[0].files[0].content, "<h1>Original</h1>");
    assert_eq!(detail.runs[0].mode, "app");
    assert_eq!(detail.runs[0].stats.input_tokens, Some(17));
    assert_eq!(
        detail.runs[0].stats.reported_model.as_deref(),
        Some("reported-v2")
    );
    assert_eq!(detail.runs[0].queue_ms, Some(20));
    assert_eq!(detail.runs[0].duration_ms, Some(80));
    assert_eq!(detail.runs[1].status, "interrupted");
    assert_eq!(detail.runs[1].output, "Unfinished draft");
    assert!(detail.runs[0].base_revision_id.is_none());
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store
            .workspace("workspace".into())
            .await
            .unwrap()
            .unwrap()
            .revisions[0]
            .files,
        detail.revisions[0].files
    );
}

#[test]
fn failed_migration_rolls_back_before_recovery_or_version_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conflicting.sqlite3");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!("fixtures/schema-v1.sql"))
        .unwrap();
    connection
        .execute_batch("ALTER TABLE runs ADD COLUMN stats TEXT;")
        .unwrap();
    drop(connection);
    assert!(Store::open(&path).is_err());
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let has_output: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('runs') WHERE name='output')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let pending: String = connection
        .query_row("SELECT status FROM runs WHERE id='pending'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, 1);
    assert!(!has_output);
    assert_eq!(pending, "queued");
}

#[test]
fn occupied_port_fails_before_opening_or_migrating_the_data_directory() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("untouched");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_deep-server"))
        .current_dir(dir.path())
        .env("DEEP_PROVIDER", "demo")
        .env("DEEP_DEFAULT_BACKEND", "demo")
        .env("DEEP_MODEL_PROFILES_FILE", "")
        .env("DEEP_MODEL_ID", "")
        .env("DEEP_LMSTUDIO_MODEL", "")
        .env("DEEP_GROQ_MODEL", "")
        .env("DEEP_OPENAI_MODEL", "")
        .env("DEEP_BIND", listener.local_addr().unwrap().to_string())
        .env("DEEP_DATA_DIR", &data)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cannot listen on"));
    assert!(!data.exists());
}

#[tokio::test]
async fn demo_http_flow_keeps_revisions_and_enforces_one_run_per_workspace() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(config(), store).unwrap());
    let (status, workspace) =
        request(&app, "POST", "/api/workspaces", json!({"title":"My app"})).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = workspace["id"].as_str().unwrap();
    let path = format!("/api/workspaces/{id}/runs");
    let (status, first) = request(
        &app,
        "POST",
        &path,
        json!({"prompt":"Make a counter","mode":"app"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, _) = request(
        &app,
        "POST",
        &path,
        json!({"prompt":"Another","mode":"app"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        terminal(&app, first["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let (_, second) = request(
        &app,
        "POST",
        &path,
        json!({"prompt":"Keep another revision","mode":"app"}),
    )
    .await;
    assert_eq!(
        terminal(&app, second["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let (_, detail) = request(&app, "GET", &format!("/api/workspaces/{id}"), Value::Null).await;
    assert_eq!(detail["messages"].as_array().unwrap().len(), 4);
    assert!(detail["revisions"].as_array().unwrap().is_empty());
    let proposals = detail["proposals"].as_array().unwrap();
    assert_eq!(proposals.len(), 2);
    assert_eq!(proposals[0]["run_id"], first["id"]);
    assert_eq!(proposals[1]["run_id"], second["id"]);
    let (_, revision) = request(
        &app,
        "POST",
        &format!(
            "/api/proposals/{}/apply",
            proposals[0]["id"].as_str().unwrap()
        ),
        Value::Null,
    )
    .await;
    assert_eq!(revision["number"], 1);
    assert_eq!(
        request(
            &app,
            "POST",
            &format!(
                "/api/proposals/{}/apply",
                proposals[1]["id"].as_str().unwrap()
            ),
            Value::Null
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, repeated) = request(
        &app,
        "POST",
        &format!(
            "/api/proposals/{}/apply",
            proposals[0]["id"].as_str().unwrap()
        ),
        Value::Null,
    )
    .await;
    assert_eq!(revision, repeated);
}

#[tokio::test]
async fn cancellation_is_terminal_and_cancelled_turns_do_not_enter_model_history() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let workspace = store.create_workspace("Cancel".into()).await.unwrap();
    let run = store
        .begin(
            workspace.id.clone(),
            "discard this".into(),
            "demo".into(),
            "demo-model".into(),
            deep_server::store::RunOptions {
                model_route: None,
                mode: Mode::App,
                base_revision_id: None,
                execution_result_id: None,
                execution_revision_id: None,
                search: None,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    store.mark_running(run.id.clone()).await.unwrap();
    store
        .terminate(run.id.clone(), "cancelled", None)
        .await
        .unwrap();
    assert!(
        !store
            .finish(
                run.clone(),
                "late response".into(),
                Some(GeneratedArtifact::Html("<h1>late</h1>".into()))
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .history(workspace.id.clone())
            .await
            .unwrap()
            .is_empty()
    );
    let detail = store.workspace(workspace.id).await.unwrap().unwrap();
    assert!(detail.revisions.is_empty());
    assert_eq!(detail.messages.len(), 1);
    assert_eq!(detail.runs[0].status, "cancelled");
}

#[tokio::test]
async fn restart_preserves_completed_work_and_interrupts_unfinished_work() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.sqlite3");
    let workspace_id;
    {
        let store = Store::open(&path).unwrap();
        let workspace = store.create_workspace("Durable".into()).await.unwrap();
        workspace_id = workspace.id.clone();
        let run = store
            .begin(
                workspace.id.clone(),
                "first".into(),
                "demo".into(),
                "model".into(),
                deep_server::store::RunOptions {
                    model_route: None,
                    mode: Mode::App,
                    base_revision_id: None,
                    execution_result_id: None,
                    execution_revision_id: None,
                    search: None,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
        store.mark_running(run.id.clone()).await.unwrap();
        store
            .finish(
                run,
                "first result".into(),
                Some(GeneratedArtifact::Html("<h1>Original</h1>".into())),
            )
            .await
            .unwrap();
        let detail = store
            .workspace(workspace.id.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(detail.revisions.is_empty());
        store
            .apply_proposal(detail.proposals[0].id.clone())
            .await
            .unwrap();
        store
            .begin(
                workspace.id,
                "pending".into(),
                "demo".into(),
                "model".into(),
                deep_server::store::RunOptions {
                    model_route: None,
                    mode: Mode::Chat,
                    base_revision_id: None,
                    execution_result_id: None,
                    execution_revision_id: None,
                    search: None,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
    }
    let reopened = Store::open(&path).unwrap();
    let detail = reopened.workspace(workspace_id).await.unwrap().unwrap();
    assert_eq!(detail.revisions[0].html, "<h1>Original</h1>");
    assert_eq!(detail.runs[0].status, "succeeded");
    assert_eq!(detail.runs[1].status, "interrupted");
}

#[tokio::test]
async fn a_second_process_cannot_recover_another_processes_active_runs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("owned.sqlite3");
    let store = Store::open(&path).unwrap();
    let workspace = store.create_workspace("Owned".into()).await.unwrap();
    let run = store
        .begin(
            workspace.id,
            "pending".into(),
            "demo".into(),
            "model".into(),
            deep_server::store::RunOptions {
                model_route: None,
                mode: Mode::Chat,
                base_revision_id: None,
                execution_result_id: None,
                execution_revision_id: None,
                search: None,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!(Store::open(&path).is_err());
    assert_eq!(store.run(run.id).await.unwrap().unwrap().status, "queued");
}

#[tokio::test]
async fn cancel_http_keeps_the_exact_receipt_and_does_not_publish_an_app() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(config(), store).unwrap());
    let (_, workspace) = request(&app, "POST", "/api/workspaces", json!({"title":"Cancel"})).await;
    let id = workspace["id"].as_str().unwrap();
    let (_, run) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{id}/runs"),
        json!({"prompt":"Make an app","mode":"app"}),
    )
    .await;
    let run_id = run["id"].as_str().unwrap();
    let (_, cancelled) = request(&app, "DELETE", &format!("/api/runs/{run_id}"), Value::Null).await;
    assert_eq!(cancelled["id"], run["id"]);
    assert_eq!(cancelled["status"], "cancelled");
    tokio::time::sleep(Duration::from_millis(600)).await;
    let (_, detail) = request(&app, "GET", &format!("/api/workspaces/{id}"), Value::Null).await;
    assert!(detail["revisions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn compatible_backend_uses_configured_api_base_and_model() {
    let captured = Arc::new(Mutex::new(None));
    let capture = captured.clone();
    let backend = Router::new().route("/custom/v1/chat/completions", post(move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
        let capture = capture.clone();
        async move { *capture.lock().unwrap() = Some((headers, body)); axum::Json(json!({"choices":[{"message":{"content":"Real adapter result"},"finish_reason":"stop"}]})) }
    }));
    let (mut cfg, task) = compatible_backend(backend).await;
    cfg.backends[0].model = "configured-model".into();
    cfg.backends[0].api_key = Some("test-only-key".into());
    let model = Model::new(cfg.backends.remove(0)).unwrap();
    assert_eq!(
        model
            .complete(
                vec![
                    json!({"role":"system","content":"Transport fixture"}),
                    json!({"role":"user","content":"Hello"})
                ],
                None,
                |_| async { Ok(()) }
            )
            .await
            .unwrap()
            .text,
        "Real adapter result"
    );
    let (headers, body) = captured.lock().unwrap().clone().unwrap();
    assert_eq!(headers["authorization"], "Bearer test-only-key");
    assert_eq!(body["model"], "configured-model");
    assert_eq!(body["messages"][1]["content"], "Hello");
    assert_eq!(
        body["messages"],
        json!([
            {"role":"system","content":"Transport fixture"},
            {"role":"user","content":"Hello"}
        ])
    );
    assert_eq!(body["stream"], true);
    task.abort();
}

#[tokio::test]
async fn named_connections_keep_transport_identity_and_discovery_read_only_and_private() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let capture = calls.clone();
    let inventory_reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reads = inventory_reads.clone();
    let upstream = Router::new()
        .route("/custom/api/v1/models", axum::routing::get(move |headers: HeaderMap| {
            reads.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
            async move {
            assert_eq!(headers["authorization"],"Bearer inventory-fixture-key");
            axum::Json(json!({"models":[{"type":"llm","key":"saluki-model","display_name":"Saluki","loaded_instances":[],"capabilities":{"vision":true,"trained_for_tool_use":true}}],"private":"PRIVATE_UPSTREAM_CONFIG"}))
        }}))
        .route("/custom/v1/chat/completions",post(move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
            capture.lock().unwrap().push((headers,body.clone()));
            async move { axum::Json(json!({"model":body["model"],"choices":[{"message":{"content":"Named connection reply"},"finish_reason":"stop"}]})) }
        }));
    let (mut cfg, task) = compatible_backend(upstream).await;
    let base = cfg.backends[0].base_url.clone();
    cfg.model_catalog = serde_json::from_value(json!({
        "connections":[
            {"id":"desk-text","label":"Desk text","kind":"lmstudio","base_url":base,"api_key_env":"PRIVATE_KEY_ENV"},
            {"id":"cloud-text","label":"Hosted connection","kind":"openai","base_url":base,"api_key_env":"PRIVATE_KEY_ENV"}
        ],
        "default_profile":"saluki",
        "profiles":[
            {"id":"saluki","label":"Saluki","backend":"desk-text","model":"saluki-model","tool_calls":true},
            {"id":"cloud","label":"Cloud model","backend":"cloud-text","model":"cloud-model","tool_calls":true}
        ]
    })).unwrap();
    cfg.backends = cfg
        .model_catalog
        .connections
        .iter()
        .map(|c| {
            c.backend(
                &cfg.model_catalog,
                &|_| Some("inventory-fixture-key".into()),
                Duration::from_secs(3),
                4096,
            )
            .unwrap()
        })
        .collect();
    cfg.default_backend = "desk-text".into();
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(cfg.clone(), store).unwrap());
    let (_, status) = request(&app, "GET", "/api/status", Value::Null).await;
    let text = status.to_string();
    for private in ["inventory-fixture-key", "PRIVATE_KEY_ENV", base.as_str()] {
        assert!(!text.contains(private));
    }
    assert!(status["backends"][0]["profile_required"].as_bool().unwrap());
    let (code, inventory) = request(
        &app,
        "GET",
        "/api/connections/desk-text/models",
        Value::Null,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(inventory["status"], "available");
    assert_eq!(inventory["models"][0]["state"], "unloaded");
    assert!(!inventory.to_string().contains("PRIVATE_UPSTREAM"));
    assert!(calls.lock().unwrap().is_empty());
    let (_, unsupported) = request(
        &app,
        "GET",
        "/api/connections/cloud-text/models",
        Value::Null,
    )
    .await;
    assert_eq!(unsupported["status"], "unsupported");
    let (_, workspace) = request(
        &app,
        "POST",
        "/api/workspaces",
        json!({"title":"Named runtime"}),
    )
    .await;
    let id = workspace["id"].as_str().unwrap();
    for profile in ["saluki", "cloud"] {
        let (code,receipt)=request(&app,"POST",&format!("/api/workspaces/{id}/runs"),json!({"prompt":"Hello","mode":"chat","model_selection":{"mode":"profile","profile_id":profile}})).await;
        assert_eq!(code, StatusCode::ACCEPTED, "{receipt}");
        let done = terminal(&app, receipt["id"].as_str().unwrap()).await;
        assert_eq!(done["status"], "succeeded", "{done}");
    }
    let records = calls.lock().unwrap().clone();
    assert_eq!(records[0].1["max_tokens"], 4096);
    assert!(records[0].1["max_completion_tokens"].is_null());
    assert_eq!(records[1].1["max_completion_tokens"], 4096);
    assert!(records[1].1["max_tokens"].is_null());
    assert_eq!(
        records[0].0["authorization"],
        "Bearer inventory-fixture-key"
    );
    let (code, _) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{id}/runs"),
        json!({"prompt":"No default alias","mode":"chat","backend":"desk-text"}),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (_, receipt) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{id}/runs"),
        json!({"prompt":"Use the configured default profile","mode":"chat"}),
    )
    .await;
    assert_eq!(receipt["model_route"]["profile_id"], "saluki");
    assert_eq!(
        terminal(&app, receipt["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    cfg.public_origin = Some("https://guest.example".into());
    let guest =
        router(App::new(cfg, Store::open(std::path::Path::new(":memory:")).unwrap()).unwrap());
    let session = send(
        &guest,
        "POST",
        "/api/session",
        json!({}),
        &[
            ("Host", "guest.example"),
            ("Origin", "https://guest.example"),
        ],
    )
    .await;
    let cookie = session.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let response = send(
        &guest,
        "GET",
        "/api/connections/desk-text/models",
        Value::Null,
        &[("Host", "guest.example"), ("Cookie", cookie)],
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(inventory_reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn backend_selection_is_persisted_and_keeps_history_and_credentials_separate() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let capture = captured.clone();
    let backend = Router::new().route("/custom/v1/chat/completions", post(move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
        let capture = capture.clone();
        async move {
            capture.lock().unwrap().push((headers, body));
            axum::Json(json!({"choices":[{"message":{"content":"A backend reply"},"finish_reason":"stop"}]}))
        }
    }));
    let (mut cfg, task) = compatible_backend(backend).await;
    cfg.default_backend = "lmstudio".into();
    cfg.backends[0].id = "lmstudio".into();
    cfg.backends[0].kind = deep_server::config::BackendKind::Lmstudio;
    cfg.backends[0].model = "local-model".into();
    cfg.backends[0].api_key = Some("local-test-key".into());
    let mut hosted = cfg.backends[0].clone();
    hosted.id = "openai".into();
    hosted.kind = deep_server::config::BackendKind::Openai;
    hosted.model = "hosted-model".into();
    hosted.api_key = Some("hosted-test-key".into());
    cfg.backends.push(hosted);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routing.sqlite3");
    let store = Store::open(&path).unwrap();
    let app = router(App::new(cfg, store.clone()).unwrap());
    let (_, status) = request(&app, "GET", "/api/status", Value::Null).await;
    assert_eq!(status["provider"], "lmstudio");
    assert_eq!(status["backends"].as_array().unwrap().len(), 2);
    let public = status.to_string();
    assert!(!public.contains("test-key"));
    assert!(!public.contains("base_url"));
    assert!(!public.contains("/custom/v1"));
    let (_, workspace) = request(&app, "POST", "/api/workspaces", json!({"title":"Routing"})).await;
    let id = workspace["id"].as_str().unwrap();
    for (index, backend_id) in [None, Some("openai"), Some("lmstudio")]
        .into_iter()
        .enumerate()
    {
        let mut body = json!({"prompt":format!("Question {index}"),"mode":"chat"});
        if let Some(backend_id) = backend_id {
            body["backend"] = json!(backend_id);
        }
        let (code, receipt) =
            request(&app, "POST", &format!("/api/workspaces/{id}/runs"), body).await;
        assert_eq!(code, StatusCode::ACCEPTED);
        let expected = backend_id.unwrap_or("lmstudio");
        assert_eq!(receipt["provider"], expected);
        let done = terminal(&app, receipt["id"].as_str().unwrap()).await;
        assert_eq!(done["status"], "succeeded");
        assert_eq!(done["provider"], expected);
    }
    let records = captured.lock().unwrap();
    assert_eq!(records.len(), 3);
    for (index, (headers, body)) in records.iter().enumerate() {
        let hosted = index == 1;
        assert_eq!(
            headers["authorization"],
            if hosted {
                "Bearer hosted-test-key"
            } else {
                "Bearer local-test-key"
            }
        );
        assert_eq!(
            body["model"],
            if hosted {
                "hosted-model"
            } else {
                "local-model"
            }
        );
        assert_eq!(body["messages"].as_array().unwrap().len(), 2 + index * 2);
        assert_eq!(body["messages"][1]["content"], "Question 0");
        assert_eq!(
            body[if hosted {
                "max_completion_tokens"
            } else {
                "max_tokens"
            }],
            4096
        );
        assert!(
            body[if hosted {
                "max_tokens"
            } else {
                "max_completion_tokens"
            }]
            .is_null()
        );
    }
    drop(records);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let persisted: Vec<(String, String)> = connection
        .prepare("SELECT provider,model FROM runs ORDER BY rowid")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        persisted,
        vec![
            ("lmstudio".into(), "local-model".into()),
            ("openai".into(), "hosted-model".into()),
            ("lmstudio".into(), "local-model".into())
        ]
    );
    task.abort();
}

#[tokio::test]
async fn model_profiles_route_once_keep_tool_rounds_and_survive_branching_and_restart() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let capture = captured.clone();
    let upstream = Router::new().route("/custom/v1/chat/completions", post(move |axum::Json(body): axum::Json<Value>| {
        let capture = capture.clone();
        async move {
            capture.lock().unwrap().push(body.clone());
            let has_tool = body["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool");
            let message = if body["model"] == "agent-model" && !has_tool {
                json!({"content":null,"tool_calls":[{"id":"inventory","type":"function","function":{"name":"list_files","arguments":"{\"offset\":0}"}}]})
            } else { json!({"content":"A profile reply"}) };
            axum::Json(json!({"model":body["model"],"choices":[{"message":message,"finish_reason":if has_tool || body["model"] != "agent-model" {"stop"} else {"tool_calls"}}]}))
        }
    }));
    let (mut cfg, task) = compatible_backend(upstream).await;
    cfg.model_catalog = serde_json::from_value(json!({
        "profiles":[
            {"id":"small","label":"Small model","backend":"compatible","model":"small-model","tool_calls":false},
            {"id":"agent","label":"Agent model","backend":"compatible","model":"agent-model","tool_calls":true}
        ],
        "preferences":{"conversation":["small","agent"],"coding":["agent"]}
    })).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profiles.sqlite3");
    let store = Store::open(&path).unwrap();
    let app = router(App::new(cfg, store.clone()).unwrap());
    let (_, status) = request(&app, "GET", "/api/status", Value::Null).await;
    assert_eq!(status["model_catalog"]["profiles"][0]["id"], "small");
    assert!(!status.to_string().contains("/custom/v1"));
    let (_, workspace) = request(
        &app,
        "POST",
        "/api/workspaces",
        json!({"title":"Profile switching"}),
    )
    .await;
    let id = workspace["id"].as_str().unwrap();
    let mut receipts = Vec::new();
    for (selection, tools, expected) in [
        (
            json!({"mode":"auto","task":"conversation"}),
            false,
            "small-model",
        ),
        (
            json!({"mode":"auto","task":"conversation"}),
            true,
            "agent-model",
        ),
        (
            json!({"mode":"profile","profile_id":"small"}),
            false,
            "small-model",
        ),
    ] {
        let (code, run) = request(&app,"POST",&format!("/api/workspaces/{id}/runs"),json!({"prompt":"Hello","mode":"chat","workspace_tools":tools,"model_selection":selection})).await;
        assert_eq!(code, StatusCode::ACCEPTED, "{run}");
        assert_eq!(run["model"], expected);
        assert_eq!(run["model_route"]["model"], expected);
        let done = terminal(&app, run["id"].as_str().unwrap()).await;
        assert_eq!(done["status"], "succeeded", "{done}");
        assert_eq!(done["model_route"], run["model_route"]);
        if tools {
            assert_eq!(done["model_rounds"], 2);
        }
        receipts.push(run);
    }
    let models: Vec<_> = captured
        .lock()
        .unwrap()
        .iter()
        .map(|b| b["model"].clone())
        .collect();
    assert_eq!(
        models,
        json!(["small-model", "agent-model", "agent-model", "small-model"])
            .as_array()
            .unwrap()
            .clone()
    );
    for selection in [
        json!({"model_selection":{"mode":"profile","profile_id":"unknown"}}),
        json!({"model_selection":{"mode":"profile","profile_id":"small"},"workspace_tools":true}),
        json!({"backend":"compatible","model_selection":{"mode":"auto","task":"coding"}}),
    ] {
        let mut body = json!({"prompt":"Must not dispatch","mode":"chat"});
        body.as_object_mut()
            .unwrap()
            .extend(selection.as_object().unwrap().clone());
        let (code, _) = request(&app, "POST", &format!("/api/workspaces/{id}/runs"), body).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
    }
    let response = send(&app,"POST",&format!("/api/workspaces/{id}/runs"),json!({"prompt":"Must not dispatch","mode":"chat","model_selection":{"mode":"auto","task":"invented"}}),&[("Host","127.0.0.1")]).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(captured.lock().unwrap().len(), 4);
    assert_eq!(store.history(id.into()).await.unwrap().len(), 6);
    let branch = store
        .branch_workspace(id.into(), Some("Branched profiles".into()), None)
        .await
        .unwrap()
        .unwrap();
    let detail = store.workspace(branch.id.clone()).await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&detail.runs[0].model_route).unwrap(),
        receipts[0]["model_route"]
    );
    drop(app);
    drop(store);
    let reopened = Store::open(&path).unwrap();
    let detail = reopened.workspace(branch.id).await.unwrap().unwrap();
    assert_eq!(detail.runs[1].model, "agent-model");
    assert_eq!(
        serde_json::to_value(&detail.runs[1].model_route).unwrap(),
        receipts[1]["model_route"]
    );
    task.abort();
}

#[tokio::test]
async fn invalid_backend_creates_no_run_and_failure_does_not_fall_back() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let backend = Router::new().route(
        "/custom/v1/chat/completions",
        post(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                (StatusCode::SERVICE_UNAVAILABLE, "private upstream response")
            }
        }),
    );
    let (mut cfg, task) = compatible_backend(backend).await;
    cfg.backends.push(config().backends.remove(0));
    let app =
        router(App::new(cfg, Store::open(std::path::Path::new(":memory:")).unwrap()).unwrap());
    let (_, workspace) = request(
        &app,
        "POST",
        "/api/workspaces",
        json!({"title":"No fallback"}),
    )
    .await;
    let id = workspace["id"].as_str().unwrap();
    let path = format!("/api/workspaces/{id}/runs");
    let (code, _) = request(
        &app,
        "POST",
        &path,
        json!({"prompt":"Hello","mode":"chat","backend":"unconfigured"}),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (_, detail) = request(&app, "GET", &format!("/api/workspaces/{id}"), Value::Null).await;
    assert!(detail["runs"].as_array().unwrap().is_empty());
    assert!(detail["messages"].as_array().unwrap().is_empty());
    let (_, receipt) = request(
        &app,
        "POST",
        &path,
        json!({"prompt":"Hello","mode":"chat","backend":"compatible"}),
    )
    .await;
    let failed = terminal(&app, receipt["id"].as_str().unwrap()).await;
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["provider"], "compatible");
    assert_eq!(failed["error"], "Model backend returned HTTP 503.");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let (_, receipt) = request(
        &app,
        "POST",
        &path,
        json!({"prompt":"Explicit demo","mode":"chat","backend":"demo"}),
    )
    .await;
    assert_eq!(
        terminal(&app, receipt["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    task.abort();
}

#[tokio::test]
async fn execution_failures_persist_without_revisions_or_later_model_history() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let capture = captured.clone();
    let backend = Router::new().route(
        "/custom/v1/chat/completions",
        post(move |axum::Json(body): axum::Json<Value>| {
            let capture = capture.clone();
            async move {
                let prompt = body["messages"].as_array().unwrap().last().unwrap()["content"]
                    .as_str()
                    .unwrap();
                let (content, finish_reason) = match prompt {
                    "incomplete" => ("```html\n<pre>literal ``` without a closing fence</pre>", "stop"),
                    "truncated" => ("```html\n<h1>Truncated</h1>\n```", "length"),
                    "unavailable" => {
                        capture.lock().unwrap().push(body);
                        return (StatusCode::BAD_GATEWAY, "backend-private-detail").into_response();
                    }
                    "recover" => ("```html\r\n<h1>Recovered</h1>\r\n```\r\n", "stop"),
                    _ => ("```html\n<pre>keep ``` intact</pre>\n```", "stop"),
                };
                let response = json!({"choices":[{"message":{"content":content},"finish_reason":finish_reason}]});
                capture.lock().unwrap().push(body);
                axum::Json(response).into_response()
            }
        }),
    );
    // Hosted error/history recovery does not require the local GPU's operator fence.
    // Local unconfirmed completion is covered by local_inference.rs.
    let (mut cfg, task) = compatible_backend(backend).await;
    cfg.default_backend = "openai".into();
    cfg.backends[0].id = "openai".into();
    cfg.backends[0].kind = deep_server::config::BackendKind::Openai;
    cfg.backends[0].api_key = Some("fixture".into());
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(cfg, store.clone()).unwrap());
    let workspace = store
        .create_workspace("Failure recovery".into())
        .await
        .unwrap();
    let path = format!("/api/workspaces/{}/runs", workspace.id);
    for (prompt, expected, error) in [
        ("initial", "succeeded", None),
        ("incomplete", "failed", Some("No complete HTML code fence")),
        ("truncated", "failed", Some("Model output was truncated")),
        (
            "unavailable",
            "failed",
            Some("Model backend returned HTTP 502."),
        ),
        ("recover", "succeeded", None),
    ] {
        let (status, receipt) =
            request(&app, "POST", &path, json!({"prompt":prompt,"mode":"app"})).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let id = receipt["id"].as_str().unwrap();
        let result = terminal(&app, id).await;
        assert_eq!(result["status"], expected);
        if let Some(error) = error {
            assert!(result["error"].as_str().unwrap().contains(error));
            let detail = store
                .workspace(workspace.id.clone())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(detail.revisions.len(), 1);
            assert_eq!(detail.revisions[0].html, "<pre>keep ``` intact</pre>");
            assert_eq!(
                detail
                    .messages
                    .iter()
                    .filter(|m| m.role == "assistant")
                    .count(),
                1
            );
        } else {
            assert!(result["error"].is_null());
            let detail = store
                .workspace(workspace.id.clone())
                .await
                .unwrap()
                .unwrap();
            let proposal = detail
                .proposals
                .iter()
                .find(|proposal| proposal.run_id.as_deref() == Some(id))
                .unwrap();
            store.apply_proposal(proposal.id.clone()).await.unwrap();
        }
        assert_eq!(
            request(&app, "GET", &format!("/api/runs/{id}"), Value::Null)
                .await
                .1,
            result
        );
    }
    let bodies = captured.lock().unwrap().clone();
    let messages = bodies.last().unwrap()["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 6);
    assert_eq!(messages[1]["content"], "initial");
    assert!(
        messages[2]["content"]
            .as_str()
            .unwrap()
            .starts_with("File proposal summary")
    );
    assert!(
        messages[4]["content"]
            .as_str()
            .unwrap()
            .starts_with("Accepted project snapshot")
    );
    assert!(
        messages[3]["content"]
            .as_str()
            .unwrap()
            .starts_with("Project state at request submission")
    );
    assert_eq!(messages[5]["content"], "recover");
    let detail = store
        .workspace(workspace.id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detail.messages.len(), 7);
    assert_eq!(detail.revisions.len(), 2);
    assert_eq!(detail.revisions[1].html, "<h1>Recovered</h1>");
    assert_eq!(store.history(workspace.id).await.unwrap().len(), 4);
    task.abort();
}

#[tokio::test]
async fn auto_model_failure_never_dispatches_the_next_preference() {
    let called = Arc::new(Mutex::new(Vec::new()));
    let capture = called.clone();
    let upstream = Router::new().route(
        "/custom/v1/chat/completions",
        post(move |axum::Json(body): axum::Json<Value>| {
            capture.lock().unwrap().push(body["model"].clone());
            async { StatusCode::SERVICE_UNAVAILABLE }
        }),
    );
    let (mut cfg, task) = compatible_backend(upstream).await;
    cfg.model_catalog = serde_json::from_value(json!({
        "profiles":[
            {"id":"first","label":"First","backend":"compatible","model":"first-model","tool_calls":true},
            {"id":"next","label":"Next","backend":"compatible","model":"next-model","tool_calls":true}
        ],"preferences":{"conversation":["first","next"],"coding":[]}
    })).unwrap();
    let app =
        router(App::new(cfg, Store::open(std::path::Path::new(":memory:")).unwrap()).unwrap());
    let (_, workspace) = request(
        &app,
        "POST",
        "/api/workspaces",
        json!({"title":"No profile fallback"}),
    )
    .await;
    let id = workspace["id"].as_str().unwrap();
    let (code, _) = request(
        &app,
        "POST",
        &format!("/api/workspaces/{id}/runs"),
        json!({"prompt":"Hello","mode":"chat","model_selection":{"mode":"auto","task":"coding"}}),
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (_, detail) = request(&app, "GET", &format!("/api/workspaces/{id}"), Value::Null).await;
    assert!(detail["messages"].as_array().unwrap().is_empty());
    let (_, receipt) = request(&app,"POST",&format!("/api/workspaces/{id}/runs"),json!({"prompt":"Hello","mode":"chat","model_selection":{"mode":"auto","task":"conversation"}})).await;
    let done = terminal(&app, receipt["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "failed");
    assert_eq!(done["model_route"]["profile_id"], "first");
    assert_eq!(*called.lock().unwrap(), vec![json!("first-model")]);
    task.abort();
}

#[tokio::test]
async fn version_twenty_one_upgrade_preserves_historical_model_identity_without_inventing_a_route()
{
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v21.sqlite3");
    let store = Store::open(&path).unwrap();
    let workspace = store
        .create_workspace("Historical route".into())
        .await
        .unwrap();
    let run = store
        .begin(
            workspace.id.clone(),
            "Keep this request".into(),
            "compatible".into(),
            "historical-model".into(),
            Default::default(),
        )
        .await
        .unwrap()
        .unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch("ALTER TABLE runs DROP COLUMN image_attachments; ALTER TABLE runs DROP COLUMN model_route; PRAGMA user_version=21;")
        .unwrap();
    drop(connection);
    let store = Store::open(&path).unwrap();
    let restored = store.run(run.id).await.unwrap().unwrap();
    assert_eq!(restored.model, "historical-model");
    assert!(restored.model_route.is_none());
    assert_eq!(restored.status, "interrupted");
    assert_eq!(
        store
            .workspace(workspace.id)
            .await
            .unwrap()
            .unwrap()
            .messages[0]
            .content,
        "Keep this request"
    );
}

#[tokio::test]
async fn admission_is_bounded_and_only_one_model_call_executes_at_a_time() {
    let gate = Arc::new(Semaphore::new(0));
    let backend_gate = gate.clone();
    let (started, mut calls) = mpsc::unbounded_channel();
    let backend = Router::new().route(
        "/custom/v1/chat/completions",
        post(move || {
            let gate = backend_gate.clone();
            let started = started.clone();
            async move {
                started.send(()).unwrap();
                gate.acquire().await.unwrap().forget();
                axum::Json(
                    json!({"choices":[{"message":{"content":"Done"},"finish_reason":"stop"}]}),
                )
            }
        }),
    );
    let (cfg, task) = compatible_backend(backend).await;
    let app =
        router(App::new(cfg, Store::open(std::path::Path::new(":memory:")).unwrap()).unwrap());
    let (_, limits) = request(&app, "GET", "/api/status", Value::Null).await;
    assert_eq!(limits["max_active"], 16);
    assert_eq!(limits["max_pending_and_active"], 16);
    let mut runs = Vec::new();
    for index in 0..16 {
        let (_, workspace) = request(
            &app,
            "POST",
            "/api/workspaces",
            json!({"title":format!("Slot {index}")}),
        )
        .await;
        let (status, run) = request(
            &app,
            "POST",
            &format!("/api/workspaces/{}/runs", workspace["id"].as_str().unwrap()),
            json!({"prompt":"Hold this slot","mode":"chat"}),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        runs.push(run);
    }
    tokio::time::timeout(Duration::from_secs(3), calls.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), calls.recv())
            .await
            .is_err()
    );
    for (index, run) in runs.iter().enumerate() {
        let (_, state) = request(
            &app,
            "GET",
            &format!("/api/runs/{}", run["id"].as_str().unwrap()),
            Value::Null,
        )
        .await;
        assert_eq!(
            state["status"],
            if index == 0 { "running" } else { "queued" }
        );
    }
    let (_, overflow) = request(&app, "POST", "/api/workspaces", json!({"title":"Overflow"})).await;
    let overflow_id = overflow["id"].as_str().unwrap();
    let path = format!("/api/workspaces/{overflow_id}/runs");
    let (status, error) = request(
        &app,
        "POST",
        &path,
        json!({"prompt":"No room","mode":"chat"}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["error"], "Local queue is full.");
    let (_, rejected) = request(
        &app,
        "GET",
        &format!("/api/workspaces/{overflow_id}"),
        Value::Null,
    )
    .await;
    assert!(rejected["runs"].as_array().unwrap().is_empty());
    assert!(rejected["messages"].as_array().unwrap().is_empty());
    for (index, run) in runs.iter().enumerate() {
        gate.add_permits(1);
        assert_eq!(
            terminal(&app, run["id"].as_str().unwrap()).await["status"],
            "succeeded"
        );
        if index + 1 < runs.len() {
            tokio::time::timeout(Duration::from_secs(3), calls.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(30), calls.recv())
                    .await
                    .is_err()
            );
        }
    }
    let (status, receipt) = request(
        &app,
        "POST",
        &path,
        json!({"prompt":"A released slot","mode":"chat"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    gate.add_permits(1);
    assert_eq!(
        terminal(&app, receipt["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    task.abort();
}

#[tokio::test]
async fn invalid_prompts_and_unknown_api_paths_return_explicit_errors() {
    let app = router(
        App::new(
            config(),
            Store::open(std::path::Path::new(":memory:")).unwrap(),
        )
        .unwrap(),
    );
    for path in [
        "/api",
        "/api/",
        "/api/unknown",
        "/api/workspaces/missing",
        "/api/runs/missing",
    ] {
        let (status, error) = request(&app, "GET", path, Value::Null).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(error["error"].is_string());
    }
    let (_, workspace) = request(
        &app,
        "POST",
        "/api/workspaces",
        json!({"title":"Validation"}),
    )
    .await;
    let id = workspace["id"].as_str().unwrap();
    for prompt in ["   ".into(), "a".repeat(32_769), "界".repeat(10_923)] {
        let (status, error) = request(
            &app,
            "POST",
            &format!("/api/workspaces/{id}/runs"),
            json!({"prompt":prompt,"mode":"chat"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error["error"], "Prompt must contain 1–32768 UTF-8 bytes.");
    }
    let (status, _) = request(
        &app,
        "POST",
        "/api/workspaces/missing/runs",
        json!({"prompt":"Hello","mode":"chat"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, detail) = request(&app, "GET", &format!("/api/workspaces/{id}"), Value::Null).await;
    assert!(detail["messages"].as_array().unwrap().is_empty());
    assert!(detail["runs"].as_array().unwrap().is_empty());
}

#[test]
fn incomplete_html_is_not_published() {
    assert!(extract_html("```html\n<h1>unfinished").is_none());
    assert!(extract_html("plain explanation").is_none());
    assert_eq!(
        extract_html("Answer\n```html\n<h1>Hello</h1>\n```"),
        Some("<h1>Hello</h1>".into())
    );
    assert!(extract_html("```html\n<pre>literal ``` without a closing fence</pre>").is_none());
    let html = "<pre>\n```js\nalert('example');\n```\n</pre><script>const fence = '```';</script>";
    assert_eq!(
        extract_html(&format!(
            "Answer\n```html\n{html}\n```\nFurther explanation."
        )),
        Some(html.into())
    );
    assert_eq!(
        extract_html("```html\r\n<h1>Windows lines</h1>\r\n```\r\n"),
        Some("<h1>Windows lines</h1>".into())
    );
}

#[test]
fn newer_database_schema_is_rejected_without_startup_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.sqlite3");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA user_version=24; CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES ('keep');").unwrap();
    drop(connection);
    assert!(Store::open(&path).is_err());
    let connection = rusqlite::Connection::open(path).unwrap();
    let value: String = connection
        .query_row("SELECT value FROM sentinel", [], |r| r.get(0))
        .unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(value, "keep");
    assert_eq!(version, 24);
}

#[test]
fn cleanup_ledger_upgrade_failure_preserves_version_seven_before_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy7.sqlite3");
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
    c.execute_batch("CREATE TABLE execution_cleanup_ids(sentinel TEXT); INSERT INTO execution_cleanup_ids VALUES('keep');").unwrap();
    drop(c);
    assert!(Store::open(&path).is_err());
    let c = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        c.query_row("SELECT status FROM runs WHERE id='unfinished'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "queued"
    );
    assert_eq!(
        c.query_row("SELECT sentinel FROM execution_cleanup_ids", [], |r| r
            .get::<_, String>(
            0
        ))
        .unwrap(),
        "keep"
    );
    c.execute_batch("DROP TABLE execution_cleanup_ids;")
        .unwrap();
    drop(c);
    drop(Store::open(&path).unwrap());
    let c = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        23
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM execution_cleanup_ids", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT status FROM runs WHERE id='unfinished'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "interrupted"
    );
}

#[tokio::test]
async fn untrusted_authorities_are_rejected_before_api_and_static_handlers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<h1>Private UI</h1>").unwrap();
    std::fs::write(dir.path().join("app.js"), "privateAsset = true;").unwrap();
    let mut cfg = config();
    cfg.web_dir = dir.path().into();
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let app = router(App::new(cfg, store.clone()).unwrap());
    let cases: &[&[(&str, &str)]] = &[
        &[("Host", "rebind.invalid:4317")],
        &[
            ("Host", "rebind.invalid:4317"),
            ("Origin", "http://rebind.invalid:4317"),
        ],
        &[("Host", "localhost.rebind.invalid:4317")],
        &[("Host", "0.0.0.0:4317")],
        &[("Host", "192.168.1.2:4317")],
        &[("Host", "localhost:65536")],
        &[("Host", "localhost:abc")],
        &[("Host", "localhost:")],
        &[("Host", "[::1]:65536")],
        &[("Host", "user@localhost:4317")],
        &[
            ("Host", "localhost:4317"),
            ("Origin", "https://foreign.invalid"),
        ],
        &[("Host", "localhost:4317"), ("Origin", "null")],
        &[
            ("Host", "localhost:4317"),
            ("Origin", "http://localhost:5173"),
        ],
        &[
            ("Host", "localhost:4317"),
            ("Origin", "http://localhost:4317/path"),
        ],
        &[
            ("Host", "localhost:4317"),
            ("Origin", "http://user@localhost:4317"),
        ],
        &[("Host", "localhost:4317"), ("Host", "rebind.invalid:4317")],
        &[
            ("Host", "localhost:4317"),
            ("Origin", "http://localhost:4317"),
            ("Origin", "http://localhost:4317"),
        ],
        &[
            ("Host", "rebind.invalid:4317"),
            ("X-Forwarded-Host", "localhost:4317"),
            ("Forwarded", "host=localhost:4317"),
        ],
        &[],
    ];
    for headers in cases {
        for (method, path) in [
            ("GET", "/api/workspaces"),
            ("POST", "/api/workspaces"),
            ("POST", "/api/workspaces/missing/runs"),
            ("DELETE", "/api/runs/missing"),
            ("GET", "/"),
            ("GET", "/app.js"),
            ("GET", "/spa/deep-link"),
        ] {
            let response = send(
                &app,
                method,
                path,
                json!({"title":"Blocked","prompt":"Blocked","mode":"chat"}),
                headers,
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "{method} {path}: {headers:?}"
            );
            assert_eq!(response.headers()["content-type"], "application/json");
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            let error: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(error["error"].as_str().unwrap().contains("loopback"));
        }
    }
    let response = send(
        &app,
        "GET",
        "http://rebind.invalid/api/status",
        Value::Null,
        &[("Host", "localhost")],
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(store.workspaces().await.unwrap().is_empty());
}

#[tokio::test]
async fn local_authorities_and_same_origin_requests_keep_working() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<h1>Private UI</h1>").unwrap();
    let mut cfg = config();
    cfg.web_dir = dir.path().into();
    cfg.bind = "0.0.0.0:4317".parse().unwrap();
    let app =
        router(App::new(cfg, Store::open(std::path::Path::new(":memory:")).unwrap()).unwrap());
    for (host, origin) in [
        ("localhost", "http://localhost"),
        ("LOCALHOST:5173", "http://localhost:5173"),
        ("127.0.0.1:4317", "http://127.0.0.1:4317"),
        ("127.2.3.4:4317", "http://127.2.3.4:4317"),
        ("[::1]:4317", "http://[::1]:4317"),
        ("[0:0:0:0:0:0:0:1]:4317", "http://[0:0:0:0:0:0:0:1]:4317"),
        ("localhost:80", "http://localhost"),
        ("localhost:443", "https://localhost"),
    ] {
        for headers in [
            vec![("Host", host)],
            vec![("Host", host), ("Origin", origin)],
        ] {
            assert_eq!(
                send(&app, "GET", "/api/status", Value::Null, &headers)
                    .await
                    .status(),
                StatusCode::OK,
                "{headers:?}"
            );
        }
    }
    let response = send(&app, "GET", "/", Value::Null, &[("Host", "localhost")]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        &to_bytes(response.into_body(), 4096).await.unwrap()[..],
        b"<h1>Private UI</h1>"
    );
    let response = send(&app, "GET", "http://localhost/api/status", Value::Null, &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn ipv6_and_wildcard_listeners_enforce_the_same_authority_policy() {
    for bind in ["[::1]:0", "0.0.0.0:0"] {
        let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
        let address = listener.local_addr().unwrap();
        let reachable = if address.is_ipv4() {
            format!("127.0.0.1:{}", address.port())
        } else {
            address.to_string()
        };
        let mut cfg = config();
        cfg.bind = address;
        let app =
            router(App::new(cfg, Store::open(std::path::Path::new(":memory:")).unwrap()).unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let url = format!("http://{reachable}/api/status");
        assert_eq!(
            client.get(&url).send().await.unwrap().status(),
            StatusCode::OK
        );
        assert_eq!(
            client
                .get(&url)
                .header("Host", "rebind.invalid")
                .header("Origin", "http://rebind.invalid")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        task.abort();
    }
}

#[tokio::test]
async fn data_directory_lease_outlives_individual_store_clones() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lease.sqlite3");
    let store = Store::open(&path).unwrap();
    let clone = store.clone();
    drop(store);
    assert!(Store::open(&path).is_err());
    drop(clone);
    assert!(Store::open(&path).is_ok());
}
