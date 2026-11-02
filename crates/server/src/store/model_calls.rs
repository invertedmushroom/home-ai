use super::{Store, now_ms};
use anyhow::{Result, ensure};
use rusqlite::params;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

pub(super) const ACTIVE: &str = "status IN ('dispatching','unresolved')";

pub(crate) enum ModelCallOutcome {
    Completed,
    NotSubmitted,
    Unresolved,
}

#[derive(Clone, Copy, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocalModelPhase {
    Waiting,
    Working,
    Draining,
    Recovering,
    Blocked,
    Settled,
}

#[derive(Clone, Serialize, Debug)]
pub struct LocalModelActivity {
    pub phase: LocalModelPhase,
    pub round: u32,
    pub operation_id: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct LiveModelStages(Arc<Mutex<HashMap<String, (Uuid, LocalModelActivity)>>>);

pub(crate) struct ModelStage {
    live: LiveModelStages,
    run: String,
    identity: Uuid,
}

impl LiveModelStages {
    pub(crate) fn enter(&self, run: &str, round: u32) -> ModelStage {
        let identity = Uuid::new_v4();
        self.0.lock().expect("model stages lock").insert(
            run.into(),
            (
                identity,
                LocalModelActivity {
                    phase: LocalModelPhase::Waiting,
                    round,
                    operation_id: None,
                },
            ),
        );
        ModelStage {
            live: self.clone(),
            run: run.into(),
            identity,
        }
    }

    fn get(&self, run: &str) -> Option<LocalModelActivity> {
        self.0
            .lock()
            .expect("model stages lock")
            .get(run)
            .map(|(_, activity)| activity.clone())
    }
}

impl Drop for ModelStage {
    fn drop(&mut self) {
        let mut live = self.live.0.lock().expect("model stages lock");
        if live
            .get(&self.run)
            .is_some_and(|(identity, _)| *identity == self.identity)
        {
            live.remove(&self.run);
        }
    }
}

impl ModelStage {
    pub(crate) fn dispatched(&self, operation_id: String) {
        let mut live = self.live.0.lock().expect("model stages lock");
        let (identity, activity) = live.get_mut(&self.run).expect("live model stage");
        assert_eq!(*identity, self.identity, "Model stage ownership changed.");
        activity.operation_id = Some(operation_id);
        activity.phase = LocalModelPhase::Working;
    }

    pub(crate) fn recovering(&self) {
        let mut live = self.live.0.lock().expect("model stages lock");
        let (identity, activity) = live.get_mut(&self.run).expect("live model stage");
        assert_eq!(*identity, self.identity, "Model stage ownership changed.");
        activity.phase = LocalModelPhase::Recovering;
    }
}

pub(super) fn activity(
    c: &rusqlite::Connection,
    run: &super::Run,
    live: &LiveModelStages,
) -> Result<Option<LocalModelActivity>> {
    use rusqlite::OptionalExtension;
    let stage = live.get(&run.id);
    if matches!(run.status.as_str(), "queued" | "running")
        && stage
            .as_ref()
            .is_some_and(|stage| stage.phase == LocalModelPhase::Waiting)
    {
        return Ok(stage);
    }
    let call: Option<(String, u32, String)> = c
        .query_row(
            "SELECT id,round,status FROM model_calls WHERE run_id=?1 ORDER BY round DESC LIMIT 1",
            [&run.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((id, round, status)) = call else {
        return Ok(None);
    };
    let owned = stage
        .as_ref()
        .is_some_and(|stage| stage.operation_id.as_deref() == Some(&id));
    let phase = match status.as_str() {
        "dispatching" if owned => {
            if run.status == "running" {
                LocalModelPhase::Working
            } else {
                LocalModelPhase::Draining
            }
        }
        "unresolved"
            if owned
                && stage
                    .as_ref()
                    .is_some_and(|s| s.phase == LocalModelPhase::Recovering) =>
        {
            LocalModelPhase::Recovering
        }
        "dispatching" | "unresolved" => LocalModelPhase::Blocked,
        "completed" | "resolved" => LocalModelPhase::Settled,
        _ => anyhow::bail!("Invalid local model ownership state."),
    };
    Ok(Some(LocalModelActivity {
        phase,
        round,
        operation_id: Some(id),
    }))
}

pub(super) fn resource_busy(
    c: &rusqlite::Connection,
    group: &str,
    runtime: Option<&str>,
    image_job: Option<&str>,
) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM model_calls WHERE status IN ('dispatching','unresolved') AND (resource_group=?1 OR runtime_base_url=?2))
         OR EXISTS(SELECT 1 FROM image_jobs j JOIN runs r ON r.id=j.run_id
           WHERE j.status IN ('submitting','running','cancelling','unresolved') AND j.id IS NOT ?3
             AND (COALESCE(json_extract(r.image_config,'$.resource_group'),CASE WHEN json_extract(r.image_config,'$.provider')='openai' THEN 'openai-images' ELSE 'local-gpu' END)=?1 OR json_extract(r.image_config,'$.endpoint')=?2))",
        params![group,runtime,image_job], |row| row.get(0),
    )?)
}

#[derive(Debug, Serialize)]
pub struct ModelCall {
    pub id: String,
    pub workspace_id: String,
    pub run_id: String,
    pub round: u32,
    pub resource_group: String,
    pub backend: String,
    pub model: String,
    pub runtime_base_url: String,
    pub runtime_fingerprint: String,
    pub status: String,
    pub reason: String,
    pub created_at_ms: i64,
    pub finished_at_ms: Option<i64>,
}

impl Store {
    pub(crate) async fn check_resource(
        &self,
        group: String,
        runtime: Option<String>,
        image_job: Option<String>,
    ) -> Result<()> {
        self.call(move |c| {
            ensure!(!resource_busy(c,&group,runtime.as_deref(),image_job.as_deref())?,
                "Shared local generation is blocked by an earlier operation without confirmed completion. The operator must inspect local model/image ownership and verify the affected runtime is idle before resolving it.");
            Ok(())
        }).await
    }

    pub(crate) async fn begin_model_call(
        &self,
        run: String,
        round: u32,
        runtime_base_url: String,
        group: String,
    ) -> Result<String> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let (workspace, backend, model, status): (String, String, String, String) = tx.query_row(
                "SELECT workspace_id,provider,model,status FROM runs WHERE id=?1", [&run],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            )?;
            ensure!(status == "running", "Run ended before local inference dispatch.");
            let blocked = resource_busy(&tx,&group,Some(&runtime_base_url),None)?;
            ensure!(!blocked, "Local GPU ownership is active or unresolved; wait for cleanup or ask the operator to inspect it.");
            let id = Uuid::new_v4().to_string();
            let runtime_fingerprint = format!("{:x}", Sha256::digest(runtime_base_url.as_bytes()));
            tx.execute(
                "INSERT INTO model_calls(id,workspace_id,run_id,round,resource_group,backend,model,runtime_base_url,runtime_fingerprint,status,reason,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'dispatching','dispatch_intent',?10)",
                params![id,workspace,run,round,group,backend,model,runtime_base_url,runtime_fingerprint,now_ms()],
            )?;
            tx.commit()?;
            Ok(id)
        }).await
    }

    pub(crate) async fn finish_model_call(
        &self,
        id: String,
        outcome: ModelCallOutcome,
    ) -> Result<()> {
        self.call(move |c| {
            let (status, reason, finished) = match outcome {
                ModelCallOutcome::Completed => ("completed", "terminal_response", Some(now_ms())),
                ModelCallOutcome::NotSubmitted => ("resolved", "request_not_sent", Some(now_ms())),
                ModelCallOutcome::Unresolved => ("unresolved", "upstream_completion_unconfirmed", None),
            };
            ensure!(c.execute(
                "UPDATE model_calls SET status=?2,reason=?3,finished_at_ms=?4 WHERE id=?1 AND status='dispatching'",
                params![id, status, reason, finished],
            )? == 1, "Local inference ownership did not settle; operator inspection is required.");
            Ok(())
        }).await
    }

    pub(crate) async fn confirm_model_idle(&self, id: String) -> Result<()> {
        self.call(move |c| {
            ensure!(c.execute(
                "UPDATE model_calls SET status='resolved',reason='runtime_confirmed_idle',finished_at_ms=?2 WHERE id=?1 AND status='unresolved'",
                params![id,now_ms()],
            )? == 1, "Only the exact unresolved model call can be confirmed idle.");
            Ok(())
        }).await
    }

    pub async fn model_calls(&self) -> Result<Vec<ModelCall>> {
        ensure!(
            self.maintenance,
            "Inspect local inference ownership through offline maintenance."
        );
        self.call(|c| {
            Ok(c.prepare("SELECT id,workspace_id,run_id,round,resource_group,backend,model,runtime_base_url,runtime_fingerprint,status,reason,created_at_ms,finished_at_ms FROM model_calls ORDER BY created_at_ms,id")?
                .query_map([], |row| Ok(ModelCall { id:row.get(0)?,workspace_id:row.get(1)?,run_id:row.get(2)?,round:row.get(3)?,resource_group:row.get(4)?,backend:row.get(5)?,model:row.get(6)?,runtime_base_url:row.get(7)?,runtime_fingerprint:row.get(8)?,status:row.get(9)?,reason:row.get(10)?,created_at_ms:row.get(11)?,finished_at_ms:row.get(12)? }))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        }).await
    }

    pub async fn resolve_model_call(&self, id: String) -> Result<()> {
        ensure!(
            self.maintenance,
            "Stop Deep before resolving local inference ownership."
        );
        self.call(move |c| {
            ensure!(c.execute(
                &format!("UPDATE model_calls SET status='resolved',reason='operator_confirmed_idle',finished_at_ms=?2 WHERE id=?1 AND {ACTIVE}"),
                params![id,now_ms()],
            )? == 1, "Only an exact active or unresolved local model call can be resolved.");
            Ok(())
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_guards_clear_cancelled_waits_without_erasing_a_replacement() {
        let live = LiveModelStages::default();
        let first = live.enter("run", 1);
        assert_eq!(live.get("run").unwrap().phase, LocalModelPhase::Waiting);
        let next = live.enter("run", 2);
        drop(first);
        assert_eq!(live.get("run").unwrap().round, 2);
        next.dispatched("operation".into());
        next.recovering();
        assert_eq!(live.get("run").unwrap().phase, LocalModelPhase::Recovering);
        drop(next);
        assert!(live.get("run").is_none());
    }

    async fn running(store: &Store) -> (String, String) {
        let workspace = store
            .create_workspace("Preserved work".into())
            .await
            .unwrap()
            .id;
        let run = Uuid::new_v4().to_string();
        let w = workspace.clone();
        let r = run.clone();
        store.call(move |c| {
            c.execute("INSERT INTO runs(id,workspace_id,status,provider,model) VALUES(?1,?2,'running','compatible','fixture')", params![r,w])?;
            Ok(())
        }).await.unwrap();
        (run, workspace)
    }

    #[tokio::test]
    async fn ownership_survives_parent_termination_and_blocks_deletion_and_image_dispatch() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let (run, workspace) = running(&store).await;
        let owned = store
            .begin_model_call(run.clone(), 1, "fingerprint".into(), "local-gpu".into())
            .await
            .unwrap();
        store
            .terminate(run.clone(), "cancelled", None)
            .await
            .unwrap();
        assert!(store.delete_workspace(workspace.clone()).await.is_err());
        assert!(
            store
                .delete_turn(workspace.clone(), run.clone())
                .await
                .is_err()
        );
        assert!(
            store
                .branch_workspace(workspace.clone(), None, None)
                .await
                .is_err()
        );
        assert!(store.resolve_model_call(owned.clone()).await.is_err());

        let (other, other_workspace) = running(&store).await;
        assert!(
            store
                .begin_model_call(other.clone(), 1, "other".into(), "local-gpu".into())
                .await
                .is_err()
        );
        let r = other.clone();
        let w = other_workspace.clone();
        store.call(move |c| {
            c.execute("UPDATE runs SET image_config=?2 WHERE id=?1",params![r,serde_json::json!({"id":"fixture","label":"Fixture","endpoint":"http://127.0.0.1:8000/mcp","settings":{}}).to_string()])?;
            c.execute("INSERT INTO image_jobs(id,workspace_id,run_id,call_id,prompt,seed,settings,status,created_at_ms) VALUES('image',?1,?2,'call','prompt',1,'{}','planned',1)",params![w,r])?;
            Ok(())
        }).await.unwrap();
        assert!(
            store
                .image_dispatch("image".into(), "context".into(), serde_json::json!({}))
                .await
                .is_err()
        );
        store
            .finish_model_call(owned, ModelCallOutcome::Completed)
            .await
            .unwrap();
        assert!(
            store
                .image_dispatch("image".into(), "context".into(), serde_json::json!({}))
                .await
                .unwrap()
        );
        // Planned jobs do not own GPU execution. Committed image dispatch does.
        assert!(
            store
                .begin_model_call(other, 1, "other".into(), "local-gpu".into())
                .await
                .is_err()
        );
        store.delete_workspace(workspace).await.unwrap();
    }

    #[tokio::test]
    async fn unresolved_runtime_cannot_escape_by_renaming_its_group() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let (run, _) = running(&store).await;
        let id = store
            .begin_model_call(run, 1, "http://runtime/v1".into(), "old-group".into())
            .await
            .unwrap();
        store
            .finish_model_call(id, ModelCallOutcome::Unresolved)
            .await
            .unwrap();
        assert!(
            store
                .resources
                .acquire(&store, "new-group", Some("http://runtime/v1"), None)
                .await
                .is_err()
        );
        assert!(
            store
                .resources
                .acquire(&store, "old-group", None, None)
                .await
                .is_err()
        );
        assert!(
            store
                .resources
                .acquire(&store, "independent", Some("http://other/v1"), None)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn maintenance_preserves_intent_and_restart_requires_exact_offline_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep.sqlite3");
        let store = Store::open(&path).unwrap();
        let (run, _) = running(&store).await;
        let id = store
            .begin_model_call(run.clone(), 1, "frozen".into(), "local-gpu".into())
            .await
            .unwrap();
        store
            .terminate(run.clone(), "cancelled", None)
            .await
            .unwrap();
        assert!(Store::open_for_maintenance(&path).is_err());
        drop(store);

        let maintenance = Store::open_for_maintenance(&path).unwrap();
        let calls = maintenance.model_calls().await.unwrap();
        assert_eq!(calls[0].status, "dispatching");
        assert_eq!(calls[0].runtime_base_url, "frozen");
        assert_eq!(calls[0].runtime_fingerprint.len(), 64);
        drop(maintenance);
        let store = Store::open(&path).unwrap();
        let recovered = store.run(run.clone()).await.unwrap().unwrap();
        assert_eq!(recovered.status, "cancelled");
        assert_eq!(
            recovered.local_model.unwrap().phase,
            LocalModelPhase::Blocked
        );
        let (next, _) = running(&store).await;
        assert!(
            store
                .begin_model_call(next, 1, "changed".into(), "local-gpu".into())
                .await
                .is_err()
        );
        drop(store);
        let maintenance = Store::open_for_maintenance(&path).unwrap();
        let calls = maintenance.model_calls().await.unwrap();
        assert_eq!(calls[0].status, "unresolved");
        assert_eq!(calls[0].reason, "process_restarted");
        assert!(
            maintenance
                .resolve_model_call("wrong-id".into())
                .await
                .is_err()
        );
        maintenance.resolve_model_call(id.clone()).await.unwrap();
        assert!(maintenance.resolve_model_call(id).await.is_err());
        let calls = maintenance.model_calls().await.unwrap();
        assert_eq!(calls[0].status, "resolved");
        assert_eq!(calls[0].reason, "operator_confirmed_idle");
        assert!(calls[0].finished_at_ms.is_some());
        drop(maintenance);
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .run(run)
                .await
                .unwrap()
                .unwrap()
                .local_model
                .unwrap()
                .phase,
            LocalModelPhase::Settled
        );
        let (next, _) = running(&store).await;
        assert!(
            store
                .begin_model_call(next, 1, "new".into(), "local-gpu".into())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn failed_v19_upgrade_precedes_recovery_and_preserves_v18_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep.sqlite3");
        let store = Store::open(&path).unwrap();
        let (run, workspace) = running(&store).await;
        drop(store);
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch("ALTER TABLE runs DROP COLUMN image_attachments; ALTER TABLE runs DROP COLUMN model_route; ALTER TABLE runs DROP COLUMN workspace_tools; DROP TABLE model_calls; PRAGMA user_version=18; CREATE TABLE model_calls(sentinel TEXT); INSERT INTO model_calls VALUES('keep');").unwrap();
        drop(c);
        assert!(Store::open(&path).is_err());
        let c = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            18
        );
        assert_eq!(
            c.query_row("SELECT status FROM runs WHERE id=?1", [&run], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "running"
        );
        assert_eq!(
            c.query_row("SELECT sentinel FROM model_calls", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        c.execute_batch("DROP TABLE model_calls;").unwrap();
        drop(c);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.run(run).await.unwrap().unwrap().status, "interrupted");
        assert_eq!(
            store
                .workspace(workspace)
                .await
                .unwrap()
                .unwrap()
                .workspace
                .title,
            "Preserved work"
        );
        drop(store);
        let maintenance = Store::open_for_maintenance(&path).unwrap();
        assert!(maintenance.model_calls().await.unwrap().is_empty());
    }
}
