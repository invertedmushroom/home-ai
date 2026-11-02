use deep_server::{
    artifacts::{FileChange, GeneratedArtifact},
    model::Mode,
    store::{Action, Execution, ExecutionInput, ExecutionResult, Store},
    worker::Worker,
};
use std::time::Duration;
use uuid::Uuid;

async fn project(store: &Store, source: &str) -> (String, String) {
    let workspace = store
        .create_workspace("Worker checks".into())
        .await
        .unwrap();
    let run = store
        .begin(
            workspace.id.clone(),
            "Project".into(),
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
            Some(GeneratedArtifact::Changes(vec![FileChange::Write {
                path: "main.cjs".into(),
                content: source.into(),
            }])),
        )
        .await
        .unwrap();
    let detail = store
        .workspace(workspace.id.clone())
        .await
        .unwrap()
        .unwrap();
    let revision = store
        .apply_proposal(detail.proposals[0].id.clone())
        .await
        .unwrap()
        .unwrap();
    (workspace.id, revision.id)
}

fn input(revision: &str, action: Action) -> ExecutionInput {
    ExecutionInput {
        id: Uuid::new_v4(),
        revision_id: Uuid::parse_str(revision).unwrap(),
        entry: "main.cjs".into(),
        action,
    }
}

async fn terminal(store: &Store, id: &str) -> Execution {
    tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            let job = store.execution(id.into()).await.unwrap().unwrap();
            if !matches!(job.status.as_str(), "queued" | "running" | "cancelling") {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("execution did not settle")
}

#[tokio::test]
async fn workspace_deletion_is_atomic_scoped_and_blocks_unfinished_work() {
    use deep_server::artifacts::File;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("delete.sqlite3");
    let store = Store::open(&path).unwrap();
    let (workspace, revision) = project(&store, "console.log('delete')").await;
    let (other, other_revision) = project(&store, "console.log('keep')").await;
    let before_other = serde_json::to_value(store.workspace(other.clone()).await.unwrap()).unwrap();
    let run = store
        .begin(
            workspace.clone(),
            "Hold".into(),
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
    assert!(store.delete_workspace(workspace.clone()).await.is_err());
    store
        .terminate(run.id.clone(), "cancelled", None)
        .await
        .unwrap();
    let (job, _) = store
        .accept_execution(
            workspace.clone(),
            input(&revision, Action::Run),
            "image".into(),
        )
        .await
        .unwrap();
    assert!(store.delete_workspace(workspace.clone()).await.is_err());
    store.start_execution(job.id.clone()).await.unwrap();
    assert!(store.delete_workspace(workspace.clone()).await.is_err());
    store.cancel_execution(job.id.clone()).await.unwrap();
    assert!(store.delete_workspace(workspace.clone()).await.is_err());
    store
        .finish_execution(
            job.id.clone(),
            ExecutionResult {
                status: "cancelled",
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();
    let (output, _) = store
        .accept_execution(
            workspace.clone(),
            input(&revision, Action::Run),
            "image".into(),
        )
        .await
        .unwrap();
    store.start_execution(output.id.clone()).await.unwrap();
    store
        .finish_execution(
            output.id.clone(),
            ExecutionResult {
                status: "succeeded",
                exit_code: Some(0),
                output_files: vec![File {
                    path: "index.html".into(),
                    content: "<h1>Output</h1>".into(),
                }],
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();
    let proposal = store
        .propose_execution_outputs(output.id.clone())
        .await
        .unwrap()
        .unwrap();
    let applied = store.apply_proposal(proposal.id).await.unwrap().unwrap();
    let manual = store
        .propose_edits(
            workspace.clone(),
            Uuid::new_v4().to_string(),
            Some(applied.id),
            vec![FileChange::Write {
                path: "manual.txt".into(),
                content: "Manual".into(),
            }],
            Vec::new(),
        )
        .await
        .unwrap()
        .unwrap();
    store.apply_proposal(manual.id).await.unwrap();
    let before_delete =
        serde_json::to_value(store.workspace(workspace.clone()).await.unwrap()).unwrap();
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("CREATE TRIGGER refuse_delete BEFORE DELETE ON workspaces BEGIN SELECT RAISE(ABORT,'injected deletion failure'); END;").unwrap();
    assert!(store.delete_workspace(workspace.clone()).await.is_err());
    assert_eq!(
        serde_json::to_value(store.workspace(workspace.clone()).await.unwrap()).unwrap(),
        before_delete
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM execution_cleanup_ids", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    c.execute_batch("DROP TRIGGER refuse_delete;").unwrap();
    drop(c);
    store.delete_workspace(workspace.clone()).await.unwrap();
    store.delete_workspace(workspace.clone()).await.unwrap();
    assert!(store.owns_execution(job.id.clone()).await.unwrap());
    assert!(
        !store
            .owns_execution(Uuid::new_v4().to_string())
            .await
            .unwrap()
    );
    let mut reused = input(&other_revision, Action::Run);
    reused.id = Uuid::parse_str(&job.id).unwrap();
    assert!(
        store
            .accept_execution(other.clone(), reused, "image".into())
            .await
            .is_err()
    );
    assert!(store.workspace(workspace.clone()).await.unwrap().is_none());
    assert!(store.execution(job.id.clone()).await.unwrap().is_none());
    assert!(store.execution(output.id).await.unwrap().is_none());
    assert!(store.run(run.id).await.unwrap().is_none());
    assert_eq!(
        serde_json::to_value(store.workspace(other.clone()).await.unwrap()).unwrap(),
        before_other
    );
    drop(store);
    let store = Store::open(&path).unwrap();
    assert!(store.owns_execution(job.id).await.unwrap());
    assert!(store.workspace(workspace).await.unwrap().is_none());
    assert!(store.workspace(other).await.unwrap().is_some());
    let c = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    for table in ["messages", "runs", "proposals", "revisions", "executions"] {
        let count: i64 = c.query_row(&format!("SELECT count(*) FROM {table} WHERE workspace_id NOT IN (SELECT id FROM workspaces)"), [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
    }
}

#[tokio::test]
async fn execution_identity_binding_admission_and_terminal_settlement_are_durable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.sqlite3");
    let store = Store::open(&path).unwrap();
    let (workspace, revision) = project(&store, "console.log('original')").await;
    let request = input(&revision, Action::Run);
    let (first, created) = store
        .accept_execution(workspace.clone(), request.clone(), "image-one".into())
        .await
        .unwrap();
    assert!(created);
    assert!(
        !store
            .accept_execution(workspace.clone(), request.clone(), "image-two".into())
            .await
            .unwrap()
            .1
    );
    let mut changed = request.clone();
    changed.action = Action::Check;
    assert!(
        store
            .accept_execution(workspace.clone(), changed, "image".into())
            .await
            .is_err()
    );
    assert!(
        store
            .accept_execution(
                workspace.clone(),
                input(&revision, Action::Check),
                "image".into()
            )
            .await
            .is_err()
    );
    assert!(store.start_execution(first.id.clone()).await.unwrap());
    store.cancel_execution(first.id.clone()).await.unwrap();
    store
        .finish_execution(
            first.id.clone(),
            ExecutionResult {
                status: "succeeded",
                exit_code: Some(0),
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .execution(first.id.clone())
            .await
            .unwrap()
            .unwrap()
            .status,
        "cancelling"
    );
    assert!(
        store
            .accept_execution(
                workspace.clone(),
                input(&revision, Action::Check),
                "image".into()
            )
            .await
            .is_err()
    );
    store
        .finish_execution(
            first.id.clone(),
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
        store
            .execution(first.id.clone())
            .await
            .unwrap()
            .unwrap()
            .status,
        "cancelled"
    );
    store
        .finish_execution(
            first.id.clone(),
            ExecutionResult {
                status: "succeeded",
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    let retried = store
        .accept_execution(workspace.clone(), request, "different-image".into())
        .await
        .unwrap();
    assert!(!retried.1);
    assert_eq!(retried.0.image_id, "image-one");
    assert_eq!(retried.0.status, "cancelled");
    assert_eq!(
        store
            .workspace(workspace.clone())
            .await
            .unwrap()
            .unwrap()
            .revisions
            .len(),
        1
    );
    let other = store.create_workspace("Other".into()).await.unwrap();
    assert!(
        store
            .accept_execution(other.id, input(&revision, Action::Run), "image".into())
            .await
            .is_err()
    );
    let mut invalid = input(&revision, Action::Run);
    invalid.entry = "../main.cjs".into();
    assert!(
        store
            .accept_execution(workspace, invalid, "image".into())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn disabled_worker_does_not_execute_or_change_revisions() {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let (workspace, revision) = project(&store, "console.log('never executed')").await;
    let worker = Worker::new(store.clone(), None);
    assert!(!worker.status().await.enabled);
    assert!(
        worker
            .submit(workspace.clone(), input(&revision, Action::Run))
            .await
            .is_err()
    );
    assert!(store.executions(workspace).await.unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires a local Linux Docker engine and DEEP_TEST_WORKER_IMAGE built from workers/node"]
async fn docker_worker_enforces_isolation_output_deadline_cancellation_and_restart_cleanup() {
    let _ = tracing_subscriber::fmt().try_init();
    let image = std::env::var("DEEP_TEST_WORKER_IMAGE").expect("set DEEP_TEST_WORKER_IMAGE");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("live.sqlite3");
    let store = Store::open(&path).unwrap();
    let code = r#"
const assert=require('node:assert/strict'); const fs=require('node:fs');
assert.equal(process.getuid(),65534); assert.equal(process.getgid(),65534);
assert.equal(process.ppid,1);
assert.equal(fs.readFileSync('/sys/fs/cgroup/memory.max','utf8').trim(),'268435456');
assert.equal(fs.readFileSync('/sys/fs/cgroup/memory.swap.max','utf8').trim(),'0');
assert.equal(fs.readFileSync('/sys/fs/cgroup/pids.max','utf8').trim(),'64');
assert.equal(fs.readFileSync('/sys/fs/cgroup/cpu.max','utf8').trim(),'100000 100000');
assert.throws(()=>process.setuid(0),error=>error.code==='EPERM');
assert.match(fs.readFileSync('/proc/self/status','utf8'),/CapEff:\s+0+\n/);
assert(!Object.keys(process.env).some(key=>/KEY|TOKEN|SECRET/i.test(key)));
assert.throws(()=>fs.writeFileSync('/etc/deep-probe','no'),error=>['EROFS','EACCES'].includes(error.code));
assert.throws(()=>fs.readFileSync('/var/run/docker.sock'),error=>error.code==='ENOENT');
assert.throws(()=>process.kill(1,'SIGSTOP'),error=>error.code==='EPERM');
fs.writeFileSync('/work/output.txt','discarded');
require('node:net').connect({host:'1.1.1.1',port:80}).on('error',()=>console.log('network blocked'));
console.log('isolation passed'); console.error('stderr captured');
"#;
    let (workspace, revision) = project(&store, code).await;
    let worker = Worker::new(store.clone(), Some(image.clone()));
    assert!(worker.status().await.ready);
    let request = input(&revision, Action::Run);
    let job = worker
        .submit(workspace.clone(), request.clone())
        .await
        .unwrap();
    assert_eq!(
        worker
            .submit(workspace.clone(), request.clone())
            .await
            .unwrap()
            .id,
        job.id
    );
    let done = terminal(&store, &job.id).await;
    assert_eq!(done.status, "succeeded", "{done:?}");
    assert!(done.stdout.contains("isolation passed"));
    assert!(done.stdout.contains("network blocked"));
    assert!(done.stderr.contains("stderr captured"));
    assert_eq!(
        store
            .workspace(workspace.clone())
            .await
            .unwrap()
            .unwrap()
            .revisions[0]
            .files
            .len(),
        1
    );
    let (w, r) = project(
        &store,
        "process.stdout.write('x'.repeat(200000));process.stderr.write('y'.repeat(200000));",
    )
    .await;
    let done = terminal(
        &store,
        &worker.submit(w, input(&r, Action::Run)).await.unwrap().id,
    )
    .await;
    assert_eq!(done.status, "succeeded", "{done:?}");
    assert!(done.truncated);
    assert_eq!(done.stdout.len(), 65536);
    assert_eq!(done.stderr.len(), 65536);
    let (w, r) = project(&store, "setInterval(()=>{},1000);").await;
    let job = worker
        .submit(w.clone(), input(&r, Action::Run))
        .await
        .unwrap();
    let done = terminal(&store, &job.id).await;
    assert_eq!(done.status, "timed_out", "{done:?}");
    let job = worker.submit(w, input(&r, Action::Run)).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    // A copied, already-bound database cannot sweep the original's live job.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    let copied_path = dir.path().join("copied.sqlite3");
    std::fs::copy(&path, &copied_path).unwrap();
    let copied = Store::open(&copied_path).unwrap();
    let copied_worker = Worker::new(copied, Some(image));
    assert!(!copied_worker.status().await.ready);
    assert_eq!(
        store
            .execution(job.id.clone())
            .await
            .unwrap()
            .unwrap()
            .status,
        "running"
    );
    worker.cancel(job.id.clone()).await.unwrap();
    assert_eq!(terminal(&store, &job.id).await.status, "cancelled");
    // An unfinished durable receipt is interrupted by a new worker, never replayed.
    let (job, _) = store
        .accept_execution(
            workspace.clone(),
            input(&revision, Action::Run),
            "unused".into(),
        )
        .await
        .unwrap();
    store.start_execution(job.id.clone()).await.unwrap();
    let restarted = Worker::new(store.clone(), None);
    assert!(!restarted.status().await.enabled);
    assert_eq!(terminal(&store, &job.id).await.status, "interrupted");
    assert_eq!(store.executions(workspace).await.unwrap().len(), 2);
}

#[tokio::test]
async fn version_three_upgrade_preserves_proposals_and_failed_upgrade_has_no_partial_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v3.sqlite3");
    let store = Store::open(&path).unwrap();
    let (workspace, revision) = project(&store, "console.log('preserve')").await;
    let before =
        serde_json::to_value(store.workspace(workspace.clone()).await.unwrap().unwrap()).unwrap();
    drop(store);
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("ALTER TABLE runs DROP COLUMN image_attachments; ALTER TABLE runs DROP COLUMN model_route; ALTER TABLE runs DROP COLUMN workspace_tools; DROP TABLE model_calls; DROP TABLE image_assets; DROP TABLE image_jobs; ALTER TABLE runs DROP COLUMN image_config; ALTER TABLE runs DROP COLUMN import_enabled; ALTER TABLE runs DROP COLUMN image_input; DROP TABLE assets; DROP TABLE executions; DROP TABLE worker_state; DROP TABLE execution_cleanup_ids; ALTER TABLE runs DROP COLUMN context_messages; ALTER TABLE runs DROP COLUMN execution_result; DROP TABLE run_executions; ALTER TABLE runs DROP COLUMN execution_revision_id; ALTER TABLE runs DROP COLUMN node_scope; ALTER TABLE runs DROP COLUMN input_assets; ALTER TABLE runs DROP COLUMN search_config; ALTER TABLE runs DROP COLUMN search_activity; ALTER TABLE runs DROP COLUMN model_rounds; DROP INDEX workspaces_guest; ALTER TABLE workspaces DROP COLUMN guest_id; DROP TABLE guests; PRAGMA user_version=3; CREATE TABLE worker_state(sentinel TEXT); INSERT INTO worker_state VALUES('keep');").unwrap();
    drop(c);
    assert!(Store::open(&path).is_err());
    let c = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='executions'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    c.execute_batch("DROP TABLE worker_state").unwrap();
    drop(c);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        serde_json::to_value(store.workspace(workspace.clone()).await.unwrap().unwrap()).unwrap(),
        before
    );
    assert!(
        !store
            .files_at(workspace, Some(revision))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn version_four_upgrade_preserves_every_link_and_rolls_back_before_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy4.sqlite3");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch(include_str!("fixtures/schema-v4.sql"))
        .unwrap();
    c.execute_batch(
        "CREATE TABLE revisions_v5(sentinel TEXT); INSERT INTO revisions_v5 VALUES('keep');",
    )
    .unwrap();
    drop(c);
    assert!(Store::open(&path).is_err());
    let c = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        4
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
            "SELECT count(*) FROM pragma_table_info('executions') WHERE name='output_files'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    c.execute_batch("DROP TABLE revisions_v5").unwrap();
    drop(c);
    let store = Store::open(&path).unwrap();
    let detail = store.workspace("workspace".into()).await.unwrap().unwrap();
    assert_eq!(detail.revisions.len(), 2);
    assert_eq!(detail.proposals.len(), 3);
    assert_eq!(detail.revisions[0].id, "original");
    assert_eq!(detail.revisions[1].id, "applied");
    assert_eq!(detail.revisions[1].files[1].content, "Exact\r\n界🙂");
    assert!(
        detail
            .revisions
            .iter()
            .all(|r| r.execution_id.is_none() && r.run_id.is_some())
    );
    assert_eq!(
        detail.proposals[0].applied_revision_id.as_deref(),
        Some("applied")
    );
    assert_eq!(detail.proposals[1].status, "pending");
    assert_eq!(detail.proposals[2].status, "rejected");
    assert_eq!(detail.messages[1].content, "Original reply");
    assert_eq!(detail.runs[1].base_revision_id.as_deref(), Some("original"));
    assert_eq!(detail.runs[1].stats.input_tokens, Some(17));
    assert_eq!(detail.runs.last().unwrap().status, "interrupted");
    let done = store.execution("done".into()).await.unwrap().unwrap();
    assert_eq!(done.stdout, "old stdout");
    assert!(done.output_files.is_empty());
    assert_eq!(
        store
            .execution("active".into())
            .await
            .unwrap()
            .unwrap()
            .status,
        "queued"
    );
    let binding = store.worker_binding().await.unwrap().unwrap();
    assert_eq!(binding.daemon, "original-daemon");
    let c = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    drop(c);
    store
        .apply_proposal("pending-proposal".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .workspace("workspace".into())
            .await
            .unwrap()
            .unwrap()
            .revisions
            .len(),
        3
    );
}

#[tokio::test]
async fn execution_outputs_become_one_reviewable_proposal_with_exact_source_and_stale_base_checks()
{
    use deep_server::artifacts::File;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("durable-output.sqlite3");
    let store = Store::open(&path).unwrap();
    let (workspace, revision) = project(&store, "// untouched\r\n界🙂").await;
    let (job, _) = store
        .accept_execution(
            workspace.clone(),
            input(&revision, Action::Run),
            "fixture-image".into(),
        )
        .await
        .unwrap();
    assert!(
        store
            .propose_execution_outputs(job.id.clone())
            .await
            .is_err()
    );
    store.start_execution(job.id.clone()).await.unwrap();
    store
        .finish_execution(
            job.id.clone(),
            ExecutionResult {
                status: "succeeded",
                exit_code: Some(0),
                output_files: vec![File {
                    path: "generated.txt".into(),
                    content: "\u{feff}Exact\r\n界🙂\n".into(),
                }],
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();
    let proposal = store
        .propose_execution_outputs(job.id.clone())
        .await
        .unwrap()
        .unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert!(proposal.run_id.is_none());
    assert_eq!(proposal.execution_id.as_deref(), Some(job.id.as_str()));
    assert_eq!(
        proposal.base_revision_id.as_deref(),
        Some(revision.as_str())
    );
    assert_eq!(
        store
            .propose_execution_outputs(job.id.clone())
            .await
            .unwrap()
            .unwrap()
            .id,
        proposal.id
    );
    assert_eq!(
        store
            .workspace(workspace.clone())
            .await
            .unwrap()
            .unwrap()
            .revisions
            .len(),
        1
    );
    let applied = store
        .apply_proposal(proposal.id.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(applied.run_id.is_none());
    assert_eq!(applied.execution_id.as_deref(), Some(job.id.as_str()));
    assert_eq!(applied.files[1].content, "// untouched\r\n界🙂");
    assert_eq!(applied.files[0].content, "\u{feff}Exact\r\n界🙂\n");
    assert_eq!(
        store
            .apply_proposal(proposal.id.clone())
            .await
            .unwrap()
            .unwrap()
            .id,
        applied.id
    );
    assert_eq!(
        store
            .propose_execution_outputs(job.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "applied"
    );
    let (old, _) = store
        .accept_execution(
            workspace.clone(),
            input(&revision, Action::Run),
            "fixture-image".into(),
        )
        .await
        .unwrap();
    store.start_execution(old.id.clone()).await.unwrap();
    store
        .finish_execution(
            old.id.clone(),
            ExecutionResult {
                status: "succeeded",
                exit_code: Some(0),
                output_files: vec![File {
                    path: "other.txt".into(),
                    content: "stale".into(),
                }],
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();
    let stale = store
        .propose_execution_outputs(old.id)
        .await
        .unwrap()
        .unwrap();
    assert!(store.apply_proposal(stale.id.clone()).await.is_err());
    store.reject_proposal(stale.id.clone(), None).await.unwrap();
    assert!(store.apply_proposal(stale.id).await.is_err());
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
}

#[tokio::test]
async fn cancelled_failed_unchanged_and_over_budget_outputs_cannot_change_the_project() {
    use deep_server::artifacts::File;
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let (workspace, revision) = project(&store, "original").await;
    for status in ["failed", "cancelled", "timed_out", "interrupted"] {
        let (job, _) = store
            .accept_execution(
                workspace.clone(),
                input(&revision, Action::Run),
                "image".into(),
            )
            .await
            .unwrap();
        store.start_execution(job.id.clone()).await.unwrap();
        store
            .finish_execution(
                job.id.clone(),
                ExecutionResult {
                    status,
                    output_files: vec![File {
                        path: "a".into(),
                        content: "must discard".into(),
                    }],
                    ..Default::default()
                },
                true,
            )
            .await
            .unwrap();
        assert!(
            store
                .execution(job.id.clone())
                .await
                .unwrap()
                .unwrap()
                .output_files
                .is_empty()
        );
        assert!(store.propose_execution_outputs(job.id).await.is_err());
    }
    let (job, _) = store
        .accept_execution(
            workspace.clone(),
            input(&revision, Action::Run),
            "image".into(),
        )
        .await
        .unwrap();
    store.start_execution(job.id.clone()).await.unwrap();
    store.cancel_execution(job.id.clone()).await.unwrap();
    store
        .finish_execution(
            job.id.clone(),
            ExecutionResult {
                status: "succeeded",
                exit_code: Some(0),
                output_files: vec![File {
                    path: "a".into(),
                    content: "race".into(),
                }],
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();
    assert!(
        store
            .execution(job.id)
            .await
            .unwrap()
            .unwrap()
            .output_files
            .is_empty()
    );
    for files in [
        vec![File {
            path: "main.cjs".into(),
            content: "original".into(),
        }],
        vec![File {
            path: "MAIN.cjs".into(),
            content: "case collision".into(),
        }],
        vec![File {
            path: "huge.txt".into(),
            content: "x".repeat(1048576 - 8),
        }],
    ] {
        let (job, _) = store
            .accept_execution(
                workspace.clone(),
                input(&revision, Action::Run),
                "image".into(),
            )
            .await
            .unwrap();
        store.start_execution(job.id.clone()).await.unwrap();
        store
            .finish_execution(
                job.id.clone(),
                ExecutionResult {
                    status: "succeeded",
                    exit_code: Some(0),
                    output_files: files,
                    ..Default::default()
                },
                true,
            )
            .await
            .unwrap();
        assert!(store.propose_execution_outputs(job.id).await.is_err());
    }
    assert_eq!(
        store
            .workspace(workspace)
            .await
            .unwrap()
            .unwrap()
            .revisions
            .len(),
        1
    );
}

#[tokio::test]
#[ignore = "requires local Linux Docker and DEEP_TEST_WORKER_IMAGE with node-v2 protocol"]
async fn docker_outputs_are_exact_bounded_and_never_follow_filesystem_references() {
    let image = std::env::var("DEEP_TEST_WORKER_IMAGE").expect("set DEEP_TEST_WORKER_IMAGE");
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("outputs.sqlite3")).unwrap();
    let worker = Worker::new(store.clone(), Some(image.clone()));
    assert!(worker.status().await.ready);
    let code = r#"
const fs=require('node:fs');
fs.writeFileSync(process.env.DEEP_OUTPUT_DIR+'/index.html','<!doctype html><h1>Built in worker</h1>');
fs.mkdirSync('/output/nested');fs.writeFileSync('/output/nested/exact.txt','\ufeffExact\r\n界🙂\n');
fs.writeFileSync('/output/empty.txt','');
fs.mkdirSync('output');fs.writeFileSync('output/relative.txt','relative output');
require('node:child_process').spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{detached:true,stdio:['ignore',1,2]}).unref();
console.log('built');
"#;
    let (workspace, revision) = project(&store, code).await;
    let job = worker
        .submit(workspace.clone(), input(&revision, Action::Run))
        .await
        .unwrap();
    let done = terminal(&store, &job.id).await;
    assert_eq!(done.status, "succeeded", "{:?}", done.error);
    assert_eq!(done.output_files.len(), 4);
    assert_eq!(done.output_files[2].content, "\u{feff}Exact\r\n界🙂\n");
    let summary = serde_json::to_value(&done).unwrap();
    assert_eq!(summary["outputs"][2]["bytes"], 18);
    assert!(summary.get("output_files").is_none());
    let proposal = store
        .propose_execution_outputs(job.id.clone())
        .await
        .unwrap()
        .unwrap();
    let applied = store
        .apply_proposal(proposal.id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(applied.files.len(), 5);
    assert_eq!(applied.execution_id.as_deref(), Some(job.id.as_str()));
    assert_eq!(
        store.apply_proposal(proposal.id).await.unwrap().unwrap().id,
        applied.id
    );
    let invalid = [
        "fs.symlinkSync('/etc/passwd','/output/leak.txt');",
        "fs.mkdirSync('/output/real');fs.symlinkSync('/output/real','/output/alias');",
        "fs.writeFileSync('/output/a.txt','x');fs.linkSync('/output/a.txt','/output/b.txt');",
        "require('node:child_process').execFileSync('/usr/bin/mkfifo',['/output/pipe']);",
        "fs.writeFileSync('/output/raw.txt',Buffer.from([255,254]));",
        "fs.writeFileSync('/output/CON.txt','x');",
        "fs.writeFileSync('/output/a.txt','x');fs.writeFileSync('/output/A.txt','y');",
        "for(let i=0;i<65;i++)fs.writeFileSync('/output/f'+i,'x');",
        "fs.writeFileSync('/output/huge.txt','x'.repeat(1048576));",
        "fs.symlinkSync('/etc','/work/output');",
        "fs.writeFileSync('/work/output','not a directory');",
        "fs.mkdirSync('output');fs.symlinkSync('/etc/passwd','output/leak');",
        "fs.mkdirSync('output');fs.writeFileSync('/output/a.txt','x');fs.writeFileSync('output/A.txt','y');",
        "fs.mkdirSync('output');for(let i=0;i<33;i++)fs.writeFileSync('/output/f'+i,'x');for(let i=33;i<65;i++)fs.writeFileSync('output/f'+i,'x');",
        "fs.mkdirSync('output');fs.writeFileSync('/output/a','x'.repeat(600000));fs.writeFileSync('output/b','x'.repeat(600000));",
    ];
    for setup in invalid {
        let (w, r) = project(
            &store,
            &format!("const fs=require('node:fs');{setup}console.log('exited zero');"),
        )
        .await;
        let job = worker.submit(w, input(&r, Action::Run)).await.unwrap();
        let done = terminal(&store, &job.id).await;
        assert_eq!(done.status, "failed", "{setup}: {:?}", done.error);
        assert_eq!(done.exit_code, Some(0));
        assert!(done.output_files.is_empty());
        assert!(store.propose_execution_outputs(job.id).await.is_err());
    }
    // Worst-case JSON escaping must fit the protocol transport, not only storage.
    let (w, r) = project(
        &store,
        "require('node:fs').writeFileSync('/output/large.txt','\\0'.repeat(1048567));",
    )
    .await;
    let done = terminal(
        &store,
        &worker.submit(w, input(&r, Action::Run)).await.unwrap().id,
    )
    .await;
    assert_eq!(done.status, "succeeded", "{:?}", done.error);
    assert_eq!(done.output_files[0].content.len(), 1048567);

    // Deleted history must not make a late stopped container undiscoverable.
    // A matching label without a recorded identity still grants no authority.
    store.delete_workspace(workspace).await.unwrap();
    assert!(store.execution(job.id.clone()).await.unwrap().is_none());
    let binding = store.worker_binding().await.unwrap().unwrap();
    let unknown = Uuid::new_v4().to_string();
    let docker = |args: &[&str]| {
        std::process::Command::new("docker")
            .args(["--host", binding.endpoint.as_str()])
            .args(args)
            .output()
            .unwrap()
    };
    let owner: String = rusqlite::Connection::open(dir.path().join("outputs.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT owner FROM worker_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let owner_label = format!("deep.owner={owner}");
    let mut containers = Vec::new();
    for id in [&job.id, &unknown] {
        let name = format!("deep-exec-{owner}-{id}");
        let job_label = format!("deep.job={id}");
        let created = docker(&[
            "container",
            "create",
            "--name",
            &name,
            "--label",
            &owner_label,
            "--label",
            &job_label,
            &image,
        ]);
        assert!(created.status.success());
        containers.push(String::from_utf8(created.stdout).unwrap().trim().to_owned());
    }
    let ready = Worker::new(store.clone(), Some(image)).status().await.ready;
    let recorded_gone = !docker(&["container", "inspect", &containers[0]])
        .status
        .success();
    let unknown_present = docker(&["container", "inspect", &containers[1]])
        .status
        .success();
    for id in &containers {
        let _ = docker(&["container", "rm", "--force", id]);
    }
    assert!(ready && recorded_gone && unknown_present);
}
