use super::assets::{AssetRef, resolve_input_assets};
use super::executions::ExecutionSpec;
use super::*;
use anyhow::ensure;
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolExecution {
    pub id: String,
    pub call_id: String,
    pub entry: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default)]
    pub input_assets: Vec<AssetRef>,
    pub result: Option<ExecutionAttachment>,
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ToolExecution> {
    Ok(ToolExecution {
        id: row.get(0)?,
        call_id: row.get(1)?,
        entry: row.get(2)?,
        action: row.get(3)?,
        code: row.get(4)?,
        input_assets: read_json(row, 5)?,
        result: read_optional_json(row, 6)?,
    })
}

pub(super) fn read_all(c: &Connection, id: &str) -> Result<Vec<ToolExecution>> {
    Ok(c.prepare(
        "SELECT execution_id,call_id,entry,action,code,input_assets,result FROM run_executions WHERE run_id=?1 ORDER BY seq",
    )?
    .query_map([id], read_row)?
    .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn read_by_execution(
    c: &Connection,
    run: &str,
    execution_id: &str,
) -> Result<Option<ToolExecution>> {
    Ok(c.query_row(
        "SELECT execution_id,call_id,entry,action,code,input_assets,result FROM run_executions WHERE run_id=?1 AND execution_id=?2",
        params![run, execution_id],
        read_row,
    )
    .optional()?)
}

pub(super) fn authorize(
    c: &Connection,
    run: &str,
    workspace: &str,
    spec: &ExecutionSpec,
) -> Result<()> {
    let binding: Option<(String, String, Option<String>)> = c.query_row(
        "SELECT workspace_id,node_scope,execution_revision_id FROM runs WHERE id=?1 AND status='running' AND node_scope!='off'",
        [run],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional()?;
    let (owner, scope, revision) =
        binding.ok_or_else(|| anyhow::anyhow!("Execution permission is no longer active."))?;
    ensure!(
        owner == workspace,
        "Execution input does not match the permitted workspace."
    );
    let exec_id = spec.id().to_string();
    let intent = read_by_execution(c, run, &exec_id)?
        .ok_or_else(|| anyhow::anyhow!("Missing execution intent."))?;
    match (scope.as_str(), spec) {
        ("revision", ExecutionSpec::Revision(input)) => {
            ensure!(
                revision.as_deref() == Some(&input.revision_id.to_string())
                    && intent.id == exec_id
                    && intent.code.is_none()
                    && intent.entry == input.entry
                    && intent.action == input.action.as_str(),
                "Execution input does not match the permitted saved revision and intent."
            );
        }
        (
            "compute",
            ExecutionSpec::Compute {
                code, input_assets, ..
            },
        ) => {
            ensure!(
                revision.is_none()
                    && intent.id == exec_id
                    && intent.code.as_deref() == Some(code.as_str())
                    && &intent.input_assets == input_assets
                    && intent.entry == "__compute.cjs"
                    && intent.action == "run",
                "Execution input does not match the permitted temporary computation intent."
            );
        }
        _ => anyhow::bail!("Execution input does not match the active Node scope."),
    }
    Ok(())
}

pub(super) fn cancel_child(c: &Connection, id: &str) -> Result<()> {
    c.execute(
        "UPDATE executions SET status='cancelling' WHERE id IN (SELECT execution_id FROM run_executions WHERE run_id=?1) AND status IN ('queued','running')",
        [id],
    )?;
    Ok(())
}

impl Store {
    pub(crate) async fn plan_tool_execution(
        &self,
        id: String,
        call_id: String,
        entry: String,
        action: Action,
    ) -> Result<ToolExecution> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let binding: Option<(String, Option<String>)> = tx.query_row(
                "SELECT workspace_id,execution_revision_id FROM runs WHERE id=?1 AND status='running' AND node_scope='revision' AND execution_revision_id IS NOT NULL",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional()?;
            let (workspace, Some(revision)) =
                binding.ok_or_else(|| anyhow::anyhow!("Execution permission is no longer active."))?
            else {
                anyhow::bail!("Execution permission is no longer active.");
            };
            let existing_count: i64 = tx.query_row(
                "SELECT count(*) FROM run_executions WHERE run_id=?1",
                [&id],
                |r| r.get(0),
            )?;
            ensure!(existing_count == 0, "Saved-script execution budget is exhausted.");
            let files = base_files(&tx, &workspace, Some(&revision))?;
            ensure!(
                files.iter().any(|file| file.path == entry) && super::executions::valid_entry(&entry),
                "Choose a script in the permitted saved revision."
            );
            let intent = ToolExecution {
                id: Uuid::new_v4().to_string(),
                call_id,
                entry,
                action: action.as_str().into(),
                code: None,
                input_assets: Vec::new(),
                result: None,
            };
            tx.execute(
                "INSERT INTO run_executions(execution_id,run_id,seq,call_id,entry,action,code,input_assets)
                 VALUES (?1,?2,1,?3,?4,?5,NULL,'[]')",
                params![intent.id, id, intent.call_id, intent.entry, intent.action],
            )?;
            tx.commit()?;
            Ok(intent)
        })
        .await
    }

    pub(crate) async fn plan_compute_execution(
        &self,
        id: String,
        call_id: String,
        code: String,
        input_asset_ids: Vec<String>,
    ) -> Result<ToolExecution> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let binding: Option<(String, Vec<AssetRef>)> = tx
                .query_row(
                    "SELECT workspace_id,input_assets FROM runs WHERE id=?1 AND status='running' AND node_scope='compute'",
                    [&id],
                    |r| Ok((r.get(0)?, read_json(r, 1)?)),
                )
                .optional()?;
            let (workspace, run_inputs) = binding.ok_or_else(|| {
                anyhow::anyhow!("Temporary computation permission is no longer active.")
            })?;
            let existing = read_all(&tx, &id)?;
            ensure!(
                existing.len() < 2,
                "Temporary computation budget is exhausted."
            );
            let mut allowed_ids: HashSet<String> =
                run_inputs.into_iter().map(|item| item.id).collect();
            allowed_ids.extend(super::assets::imports_for_run(&tx, &id)?.into_iter().map(|asset| asset.id));
            for prior in &existing {
                if let Some(result) = &prior.result
                    && result.status == "succeeded"
                {
                    allowed_ids.extend(result.output_assets.iter().map(|asset| asset.id.clone()));
                }
            }
            ensure!(
                input_asset_ids.iter().all(|asset_id| allowed_ids.contains(asset_id)),
                "Input asset ID is not permitted for this request."
            );
            let resolved = resolve_input_assets(&tx, &workspace, &input_asset_ids)?;
            let mut files: Vec<File> = resolved
                .iter()
                .map(|(meta, content)| File {
                    path: meta.path.clone(),
                    content: content.clone(),
                })
                .collect();
            files.push(File {
                path: "__compute.cjs".into(),
                content: code.clone(),
            });
            validate_files(&files)?;
            let input_assets: Vec<AssetRef> = resolved.into_iter().map(|(meta, _)| meta).collect();
            let seq = (existing.len() + 1) as i64;
            let intent = ToolExecution {
                id: Uuid::new_v4().to_string(),
                call_id,
                entry: "__compute.cjs".into(),
                action: "run".into(),
                code: Some(code),
                input_assets,
                result: None,
            };
            tx.execute(
                "INSERT INTO run_executions(execution_id,run_id,seq,call_id,entry,action,code,input_assets)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    intent.id,
                    id,
                    seq,
                    intent.call_id,
                    intent.entry,
                    intent.action,
                    intent.code,
                    serde_json::to_string(&intent.input_assets)?
                ],
            )?;
            tx.commit()?;
            Ok(intent)
        })
        .await
    }

    pub(crate) async fn tool_execution_spec(
        &self,
        id: String,
        execution_id: Option<String>,
    ) -> Result<(String, ExecutionSpec)> {
        self.call(move |c| {
            let (workspace, scope, revision): (String, String, Option<String>) = c.query_row(
                "SELECT workspace_id,node_scope,execution_revision_id FROM runs WHERE id=?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            let intent = match execution_id {
                Some(exec_id) => read_by_execution(c, &id, &exec_id)?,
                None => read_all(c, &id)?.into_iter().last(),
            }
            .ok_or_else(|| anyhow::anyhow!("Missing execution intent."))?;
            let spec = match scope.as_str() {
                "revision" => {
                    let rev =
                        revision.ok_or_else(|| anyhow::anyhow!("Missing permitted revision."))?;
                    ExecutionSpec::Revision(ExecutionInput {
                        id: Uuid::parse_str(&intent.id)?,
                        revision_id: Uuid::parse_str(&rev)?,
                        entry: intent.entry,
                        action: match intent.action.as_str() {
                            "check" => Action::Check,
                            "run" => Action::Run,
                            _ => anyhow::bail!("Invalid saved execution action."),
                        },
                    })
                }
                "compute" => ExecutionSpec::Compute {
                    id: Uuid::parse_str(&intent.id)?,
                    code: intent
                        .code
                        .ok_or_else(|| anyhow::anyhow!("Missing temporary computation code."))?,
                    input_assets: intent.input_assets,
                },
                _ => anyhow::bail!("Execution permission is not active."),
            };
            authorize(c, &id, &workspace, &spec)?;
            Ok((workspace, spec))
        })
        .await
    }

    #[cfg(test)]
    pub(crate) async fn record_tool_execution(&self, id: String) -> Result<ExecutionAttachment> {
        self.record_tool_execution_by_id(id, None).await
    }

    pub(crate) async fn record_tool_execution_by_id(
        &self,
        id: String,
        execution_id: Option<String>,
    ) -> Result<ExecutionAttachment> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let workspace: Option<String> = tx
                .query_row(
                    "SELECT workspace_id FROM runs WHERE id=?1 AND status='running'",
                    [&id],
                    |r| r.get(0),
                )
                .optional()?;
            let workspace =
                workspace.ok_or_else(|| anyhow::anyhow!("Model run is no longer active."))?;
            let intent = match execution_id {
                Some(exec_id) => read_by_execution(&tx, &id, &exec_id)?,
                None => read_all(&tx, &id)?.into_iter().last(),
            }
            .ok_or_else(|| anyhow::anyhow!("Missing execution intent."))?;
            if let Some(result) = intent.result {
                return Ok(result);
            }
            let result = super::executions::attachment(&tx, &workspace, &intent.id)?;
            tx.execute(
                "UPDATE run_executions SET result=?3 WHERE run_id=?1 AND execution_id=?2 AND result IS NULL",
                params![id, intent.id, serde_json::to_string(&result)?],
            )?;
            tx.commit()?;
            Ok(result)
        })
        .await
    }

    pub(crate) async fn linked_execution_ids(&self, id: String) -> Result<Vec<String>> {
        self.call(move |c| Ok(read_all(c, &id)?.into_iter().map(|item| item.id).collect()))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn project(path: &Path) -> (Store, Workspace, Revision) {
        let store = Store::open(path).unwrap();
        let workspace = store
            .create_workspace("Tool ownership".into())
            .await
            .unwrap();
        let run = store
            .begin(
                workspace.id.clone(),
                "Seed files".into(),
                "demo".into(),
                "demo".into(),
                RunOptions {
                    model_route: None,
                    workspace_tools: false,
                    mode: Mode::Files,
                    base_revision_id: None,
                    execution_result_id: None,
                    search: None,
                    allow_url_import: false,
                    execution_revision_id: None,
                    node_scope: NodeScope::Off,
                    input_asset_ids: Vec::new(),
                    image_profile: None,
                    image_input: None,
                    image_attachment_ids: Vec::new(),
                },
            )
            .await
            .unwrap()
            .unwrap();
        store.mark_running(run.id.clone()).await.unwrap();
        store
            .finish(
                run,
                "Seed proposal".into(),
                Some(GeneratedArtifact::Changes(vec![FileChange::Write {
                    path: "main.cjs".into(),
                    content: "console.log(42);".into(),
                }])),
            )
            .await
            .unwrap();
        let proposal = store
            .workspace(workspace.id.clone())
            .await
            .unwrap()
            .unwrap()
            .proposals[0]
            .id
            .clone();
        let revision = store.apply_proposal(proposal).await.unwrap().unwrap();
        (store, workspace, revision)
    }

    async fn native(store: &Store, workspace: &Workspace, revision: &Revision) -> Run {
        let run = store
            .begin(
                workspace.id.clone(),
                "Run saved script".into(),
                "compatible".into(),
                "test".into(),
                RunOptions {
                    model_route: None,
                    workspace_tools: false,
                    mode: Mode::Chat,
                    base_revision_id: None,
                    execution_result_id: None,
                    search: None,
                    allow_url_import: false,
                    execution_revision_id: Some(revision.id.clone()),
                    node_scope: NodeScope::Revision,
                    input_asset_ids: Vec::new(),
                    image_profile: None,
                    image_input: None,
                    image_attachment_ids: Vec::new(),
                },
            )
            .await
            .unwrap()
            .unwrap();
        store.mark_running(run.id.clone()).await.unwrap();
        store
            .plan_tool_execution(
                run.id.clone(),
                "native_call".into(),
                "main.cjs".into(),
                Action::Run,
            )
            .await
            .unwrap();
        run
    }

    async fn admit(store: &Store, run: &Run) -> Execution {
        let (workspace, spec) = store
            .tool_execution_spec(run.id.clone(), None)
            .await
            .unwrap();
        let (job, created) = store
            .admit_execution(
                workspace.clone(),
                spec.clone(),
                "original-image".into(),
                Some(run.id.clone()),
            )
            .await
            .unwrap();
        assert!(created);
        let (same, created) = store
            .admit_execution(
                workspace,
                spec,
                "another-image".into(),
                Some(run.id.clone()),
            )
            .await
            .unwrap();
        assert!(!created);
        assert_eq!(same.id, job.id);
        assert_eq!(same.image_id, "original-image");
        job
    }

    #[tokio::test]
    async fn reserved_identity_and_terminal_parent_refuse_new_or_mismatched_admission() {
        let (store, workspace, revision) = project(Path::new(":memory:")).await;
        let run = native(&store, &workspace, &revision).await;
        let (owner, spec) = store
            .tool_execution_spec(run.id.clone(), None)
            .await
            .unwrap();
        let ExecutionSpec::Revision(input) = &spec else {
            panic!("expected revision spec");
        };
        assert!(
            store
                .accept_execution(owner.clone(), input.clone(), "image".into())
                .await
                .is_err()
        );
        let mut wrong = input.clone();
        wrong.action = Action::Check;
        assert!(
            store
                .admit_execution(
                    owner.clone(),
                    ExecutionSpec::Revision(wrong),
                    "image".into(),
                    Some(run.id.clone())
                )
                .await
                .is_err()
        );
        assert!(
            store
                .plan_tool_execution(
                    run.id.clone(),
                    "second".into(),
                    "main.cjs".into(),
                    Action::Check
                )
                .await
                .is_err()
        );
        store
            .terminate(run.id.clone(), "cancelled", None)
            .await
            .unwrap();
        assert!(
            store
                .admit_execution(owner, spec.clone(), "image".into(), Some(run.id.clone()))
                .await
                .is_err()
        );
        assert!(
            store
                .execution(spec.id().to_string())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !store
                .finish(run.clone(), "late".into(), None)
                .await
                .unwrap()
        );
        assert!(
            store
                .run(run.id)
                .await
                .unwrap()
                .unwrap()
                .tool_execution
                .unwrap()
                .result
                .is_none()
        );
    }

    #[tokio::test]
    async fn every_parent_termination_marks_its_child_and_refuses_late_publication() {
        for status in ["failed", "cancelled", "interrupted"] {
            let (store, workspace, revision) = project(Path::new(":memory:")).await;
            let run = native(&store, &workspace, &revision).await;
            let job = admit(&store, &run).await;
            store.start_execution(job.id.clone()).await.unwrap();
            store.terminate(run.id.clone(), status, None).await.unwrap();
            assert_eq!(
                store
                    .execution(job.id.clone())
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                "cancelling"
            );
            assert!(store.delete_workspace(workspace.id.clone()).await.is_err());
            store
                .finish_execution(
                    job.id.clone(),
                    ExecutionResult {
                        status: "succeeded",
                        stdout: "late".into(),
                        exit_code: Some(0),
                        ..Default::default()
                    },
                    true,
                )
                .await
                .unwrap();
            assert_eq!(
                store.execution(job.id).await.unwrap().unwrap().status,
                "cancelled"
            );
            assert!(store.record_tool_execution(run.id.clone()).await.is_err());
            assert!(
                !store
                    .finish(run.clone(), "late answer".into(), None)
                    .await
                    .unwrap()
            );
            assert_eq!(store.run(run.id).await.unwrap().unwrap().status, status);
        }
    }

    #[tokio::test]
    async fn pending_results_block_success_and_only_completed_turns_project_native_evidence() {
        for outcome in ["succeeded", "failed", "cancelled", "interrupted"] {
            let (store, workspace, revision) = project(Path::new(":memory:")).await;
            let run = native(&store, &workspace, &revision).await;
            let job = admit(&store, &run).await;
            assert!(
                store
                    .finish(run.clone(), "premature".into(), None)
                    .await
                    .is_err()
            );
            assert_eq!(
                store.run(run.id.clone()).await.unwrap().unwrap().status,
                "running"
            );
            store
                .finish_execution(
                    job.id.clone(),
                    ExecutionResult {
                        status: "succeeded",
                        stdout: "42".into(),
                        exit_code: Some(0),
                        ..Default::default()
                    },
                    true,
                )
                .await
                .unwrap();
            let observed = store.record_tool_execution(run.id.clone()).await.unwrap();
            assert_eq!(observed.id, job.id);
            assert_eq!(observed.revision_id.as_deref(), Some(revision.id.as_str()));
            if outcome == "succeeded" {
                store
                    .finish(run.clone(), "Observed answer".into(), None)
                    .await
                    .unwrap();
            } else {
                store
                    .terminate(run.id.clone(), outcome, None)
                    .await
                    .unwrap();
            }
            let next = store
                .begin(
                    workspace.id.clone(),
                    "Discuss the result".into(),
                    "compatible".into(),
                    "test".into(),
                    RunOptions {
                        model_route: None,
                        workspace_tools: false,
                        mode: Mode::Chat,
                        base_revision_id: None,
                        execution_result_id: None,
                        search: None,
                        allow_url_import: false,
                        execution_revision_id: None,
                        node_scope: NodeScope::Off,
                        input_asset_ids: Vec::new(),
                        image_profile: None,
                        image_input: None,
                        image_attachment_ids: Vec::new(),
                    },
                )
                .await
                .unwrap()
                .unwrap();
            assert!(next.execution_revision_id.is_none() && next.tool_execution.is_none());
            let context = store.run_context(next.id).await.unwrap();
            assert_eq!(
                context
                    .iter()
                    .any(|m| m.content.contains(&job.id) && m.content.contains("42")),
                outcome == "succeeded"
            );
            assert_eq!(
                store
                    .run(run.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .tool_execution
                    .unwrap()
                    .result
                    .unwrap()
                    .stdout,
                "42"
            );
        }
    }

    #[tokio::test]
    async fn termination_leaves_an_unrelated_manual_job_running_and_deletion_removes_native_evidence()
     {
        let (store, workspace, revision) = project(Path::new(":memory:")).await;
        let run = native(&store, &workspace, &revision).await;
        let job = admit(&store, &run).await;
        store
            .finish_execution(
                job.id.clone(),
                ExecutionResult {
                    status: "succeeded",
                    exit_code: Some(0),
                    ..Default::default()
                },
                true,
            )
            .await
            .unwrap();
        store.record_tool_execution(run.id.clone()).await.unwrap();
        let (unrelated, _) = store
            .accept_execution(
                workspace.id.clone(),
                ExecutionInput {
                    id: Uuid::new_v4(),
                    revision_id: Uuid::parse_str(&revision.id).unwrap(),
                    entry: "main.cjs".into(),
                    action: Action::Check,
                },
                "image".into(),
            )
            .await
            .unwrap();
        store.terminate(run.id, "cancelled", None).await.unwrap();
        assert_eq!(
            store
                .execution(unrelated.id.clone())
                .await
                .unwrap()
                .unwrap()
                .status,
            "queued"
        );
        store
            .finish_execution(
                unrelated.id,
                ExecutionResult {
                    status: "succeeded",
                    exit_code: Some(0),
                    ..Default::default()
                },
                true,
            )
            .await
            .unwrap();
        store.delete_workspace(workspace.id).await.unwrap();
        store
            .call(move |c| {
                assert_eq!(
                    c.query_row("SELECT count(*) FROM run_executions", [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
                assert!(c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM execution_cleanup_ids WHERE id=?1)",
                    [job.id],
                    |r| r.get::<_, bool>(0)
                )?);
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn restart_preserves_linked_cleanup_without_replaying_or_fabricating_a_result() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep.sqlite3");
        let (store, workspace, revision) = project(&path).await;
        let run = native(&store, &workspace, &revision).await;
        let job = admit(&store, &run).await;
        store.start_execution(job.id.clone()).await.unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        let recovered = store.run(run.id).await.unwrap().unwrap();
        assert_eq!(recovered.status, "interrupted");
        let intent = recovered.tool_execution.unwrap();
        assert_eq!(intent.id, job.id);
        assert!(intent.result.is_none());
        assert_eq!(
            store.execution(job.id).await.unwrap().unwrap().status,
            "cancelling"
        );
        assert_eq!(
            store
                .workspace(workspace.id)
                .await
                .unwrap()
                .unwrap()
                .revisions
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn version_ten_upgrade_rolls_back_before_recovery_and_preserves_old_permission_defaults()
    {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep.sqlite3");
        let (store, workspace, revision) = project(&path).await;
        let run = store
            .begin(
                workspace.id.clone(),
                "Old queued request".into(),
                "compatible".into(),
                "test".into(),
                RunOptions {
                    model_route: None,
                    workspace_tools: false,
                    mode: Mode::Chat,
                    base_revision_id: None,
                    execution_result_id: None,
                    search: None,
                    allow_url_import: false,
                    execution_revision_id: None,
                    node_scope: NodeScope::Off,
                    input_asset_ids: Vec::new(),
                    image_profile: None,
                    image_input: None,
                    image_attachment_ids: Vec::new(),
                },
            )
            .await
            .unwrap()
            .unwrap();
        drop(store);
        let c = Connection::open(&path).unwrap();
        c.execute_batch("ALTER TABLE runs DROP COLUMN image_attachments; ALTER TABLE runs DROP COLUMN model_route; ALTER TABLE runs DROP COLUMN workspace_tools; DROP TABLE model_calls; DROP TABLE image_assets; DROP TABLE image_jobs; ALTER TABLE runs DROP COLUMN image_config; ALTER TABLE runs DROP COLUMN import_enabled; ALTER TABLE runs DROP COLUMN image_input; DROP TABLE assets; DROP TABLE run_executions; ALTER TABLE runs DROP COLUMN execution_revision_id; ALTER TABLE runs DROP COLUMN node_scope; ALTER TABLE runs DROP COLUMN input_assets; DROP INDEX workspaces_guest; ALTER TABLE workspaces DROP COLUMN guest_id; DROP TABLE guests; PRAGMA user_version=10; CREATE TABLE run_executions(sentinel TEXT); INSERT INTO run_executions VALUES('keep');").unwrap();
        drop(c);
        assert!(Store::open(&path).is_err());
        let c = Connection::open(&path).unwrap();
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            10
        );
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM pragma_table_info('runs') WHERE name='execution_revision_id'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            c.query_row("SELECT sentinel FROM run_executions", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        assert_eq!(
            c.query_row("SELECT status FROM runs WHERE id=?1", [&run.id], |r| r
                .get::<_, String>(
                0
            ))
            .unwrap(),
            "queued"
        );
        c.execute_batch("DROP TABLE run_executions;").unwrap();
        drop(c);
        let store = Store::open(&path).unwrap();
        let recovered = store.run(run.id).await.unwrap().unwrap();
        assert_eq!(recovered.status, "interrupted");
        assert!(recovered.execution_revision_id.is_none() && recovered.tool_execution.is_none());
        assert_eq!(
            store
                .workspace(workspace.id)
                .await
                .unwrap()
                .unwrap()
                .revisions[0]
                .id,
            revision.id
        );
    }

    #[tokio::test]
    async fn temporary_compute_supports_correction_chaining_and_asset_proposals_without_hidden_revisions()
     {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let workspace = store
            .create_workspace("Compute slice".into())
            .await
            .unwrap();
        let csv = store
            .create_asset(
                workspace.id.clone(),
                "sales.csv".into(),
                None,
                "item,amount\nA,10\nB,15\n".into(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(csv.media_type, "text/csv");
        assert_eq!(csv.origin, "upload");
        let other = store
            .create_workspace("Other workspace".into())
            .await
            .unwrap();
        let foreign = store
            .create_asset(other.id.clone(), "secret.csv".into(), None, "secret".into())
            .await
            .unwrap()
            .unwrap();
        assert!(
            store
                .begin(
                    workspace.id.clone(),
                    "Cross-workspace asset".into(),
                    "compatible".into(),
                    "test".into(),
                    RunOptions {
                        model_route: None,
                        mode: Mode::Chat,
                        node_scope: NodeScope::Compute,
                        input_asset_ids: vec![foreign.id.clone()],
                        ..Default::default()
                    },
                )
                .await
                .is_err()
        );

        let run = store
            .begin(
                workspace.id.clone(),
                "Compute totals and chart".into(),
                "compatible".into(),
                "test".into(),
                RunOptions {
                    model_route: None,
                    mode: Mode::Chat,
                    node_scope: NodeScope::Compute,
                    input_asset_ids: vec![csv.id.clone()],
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.node_scope, NodeScope::Compute);
        assert_eq!(run.input_assets.len(), 1);
        assert_eq!(run.input_assets[0].path, "sales.csv");
        store.mark_running(run.id.clone()).await.unwrap();
        store.tool_round(run.id.clone(), 1).await.unwrap();

        // First call fails and publishes no eligible output assets.
        let first_intent = store
            .plan_compute_execution(
                run.id.clone(),
                "call_1".into(),
                "throw new Error('bad parse');".into(),
                vec![csv.id.clone()],
            )
            .await
            .unwrap();
        let (owner, first_spec) = store
            .tool_execution_spec(run.id.clone(), Some(first_intent.id.clone()))
            .await
            .unwrap();
        let (first_job, _) = store
            .admit_execution(owner, first_spec, "image".into(), Some(run.id.clone()))
            .await
            .unwrap();
        let materialized = store.execution_files(first_job.id.clone()).await.unwrap();
        assert_eq!(materialized.len(), 2);
        assert!(
            materialized
                .iter()
                .any(|f| f.path == "sales.csv" && f.content.contains("A,10"))
        );
        assert!(
            materialized
                .iter()
                .any(|f| f.path == "__compute.cjs" && f.content.contains("bad parse"))
        );
        store.start_execution(first_job.id.clone()).await.unwrap();
        store
            .finish_execution(
                first_job.id.clone(),
                ExecutionResult {
                    status: "failed",
                    stderr: "Error: bad parse".into(),
                    exit_code: Some(1),
                    output_files: vec![File {
                        path: "discarded.svg".into(),
                        content: "<svg/>".into(),
                    }],
                    ..Default::default()
                },
                true,
            )
            .await
            .unwrap();
        let first_obs = store
            .record_tool_execution_by_id(run.id.clone(), Some(first_intent.id.clone()))
            .await
            .unwrap();
        assert_eq!(first_obs.status, "failed");
        assert!(first_obs.output_assets.is_empty());

        // Second call succeeds with corrected code and produces downloadable assets.
        store.tool_round(run.id.clone(), 2).await.unwrap();
        let second_intent = store
            .plan_compute_execution(
                run.id.clone(),
                "call_2".into(),
                "console.log('total=25');".into(),
                vec![csv.id.clone()],
            )
            .await
            .unwrap();
        let (owner, second_spec) = store
            .tool_execution_spec(run.id.clone(), Some(second_intent.id.clone()))
            .await
            .unwrap();
        let (second_job, _) = store
            .admit_execution(owner, second_spec, "image".into(), Some(run.id.clone()))
            .await
            .unwrap();
        store.start_execution(second_job.id.clone()).await.unwrap();
        store
            .finish_execution(
                second_job.id.clone(),
                ExecutionResult {
                    status: "succeeded",
                    stdout: "total=25\n".into(),
                    exit_code: Some(0),
                    output_files: vec![
                        File {
                            path: "chart.svg".into(),
                            content: "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".into(),
                        },
                        File {
                            path: "totals.json".into(),
                            content: "{\"total\":25}".into(),
                        },
                    ],
                    ..Default::default()
                },
                true,
            )
            .await
            .unwrap();
        let second_obs = store
            .record_tool_execution_by_id(run.id.clone(), Some(second_intent.id.clone()))
            .await
            .unwrap();
        assert_eq!(second_obs.status, "succeeded");
        assert_eq!(second_obs.output_assets.len(), 2);

        // Budget rejects a third temporary computation call.
        assert!(
            store
                .plan_compute_execution(
                    run.id.clone(),
                    "call_3".into(),
                    "console.log('extra');".into(),
                    vec![],
                )
                .await
                .is_err()
        );
        store.tool_round(run.id.clone(), 3).await.unwrap();
        store
            .finish(run.clone(), "Computed total 25 and chart.svg".into(), None)
            .await
            .unwrap();

        let detail = store
            .workspace(workspace.id.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(detail.revisions.is_empty());
        assert_eq!(detail.assets.len(), 3);
        let saved_run = &detail.runs[0];
        assert_eq!(saved_run.tool_executions.len(), 2);
        assert_eq!(
            saved_run.tool_executions[0].code.as_deref(),
            Some("throw new Error('bad parse');")
        );
        assert_eq!(
            saved_run.tool_executions[1].code.as_deref(),
            Some("console.log('total=25');")
        );
        let svg_asset = detail
            .assets
            .iter()
            .find(|a| a.name == "chart.svg")
            .unwrap();
        assert_eq!(svg_asset.media_type, "image/svg+xml");
        assert_eq!(
            svg_asset.execution_id.as_deref(),
            Some(second_job.id.as_str())
        );
        assert_eq!(
            store
                .asset(svg_asset.id.clone())
                .await
                .unwrap()
                .unwrap()
                .content,
            "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"
        );
        assert_eq!(
            store.asset(csv.id.clone()).await.unwrap().unwrap().content,
            "item,amount\nA,10\nB,15\n"
        );

        // Unselected existing assets cannot be used in a new request, but assets produced
        // in the active request can be chained by a second call in that same request.
        let chain_run = store
            .begin(
                workspace.id.clone(),
                "Chain two steps".into(),
                "compatible".into(),
                "test".into(),
                RunOptions {
                    model_route: None,
                    mode: Mode::Chat,
                    node_scope: NodeScope::Compute,
                    input_asset_ids: vec![],
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
        store.mark_running(chain_run.id.clone()).await.unwrap();
        assert!(
            store
                .plan_compute_execution(
                    chain_run.id.clone(),
                    "unselected".into(),
                    "console.log(1);".into(),
                    vec![svg_asset.id.clone()],
                )
                .await
                .is_err()
        );
        let step1 = store
            .plan_compute_execution(
                chain_run.id.clone(),
                "step1".into(),
                "console.log('step1');".into(),
                vec![],
            )
            .await
            .unwrap();
        let (owner, spec1) = store
            .tool_execution_spec(chain_run.id.clone(), Some(step1.id.clone()))
            .await
            .unwrap();
        let (job1, _) = store
            .admit_execution(owner, spec1, "image".into(), Some(chain_run.id.clone()))
            .await
            .unwrap();
        store.start_execution(job1.id.clone()).await.unwrap();
        store
            .finish_execution(
                job1.id.clone(),
                ExecutionResult {
                    status: "succeeded",
                    exit_code: Some(0),
                    output_files: vec![File {
                        path: "step1.json".into(),
                        content: "{\"ok\":true}".into(),
                    }],
                    ..Default::default()
                },
                true,
            )
            .await
            .unwrap();
        let obs1 = store
            .record_tool_execution_by_id(chain_run.id.clone(), Some(step1.id.clone()))
            .await
            .unwrap();
        let produced_id = obs1.output_assets[0].id.clone();
        let step2 = store
            .plan_compute_execution(
                chain_run.id.clone(),
                "step2".into(),
                "console.log('step2');".into(),
                vec![produced_id.clone()],
            )
            .await
            .unwrap();
        assert_eq!(step2.input_assets[0].id, produced_id);
        store
            .terminate(chain_run.id, "cancelled", None)
            .await
            .unwrap();

        // Explicit review of selected text assets into the project captures current base and preserves asset_ids.
        let proposal = store
            .propose_assets(workspace.id.clone(), None, vec![svg_asset.id.clone()])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            proposal.asset_ids.as_ref().unwrap(),
            &vec![svg_asset.id.clone()]
        );
        let revision = store
            .apply_proposal(proposal.id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            revision.asset_ids.as_ref().unwrap(),
            &vec![svg_asset.id.clone()]
        );
        assert_eq!(revision.files.len(), 1);
        assert_eq!(revision.files[0].path, "chart.svg");
        assert!(
            store
                .propose_assets(workspace.id.clone(), None, vec![csv.id.clone()])
                .await
                .is_err()
        );
    }
}
