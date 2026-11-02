use super::assets::{Asset, AssetRef, execution_assets, infer_media_type};
use super::*;
use axum::http::StatusCode;
use serde::Deserialize;

const COLUMNS: &str = "id,workspace_id,revision_id,entry,action,code,input_assets,image_id,status,stdout,stderr,exit_code,signal,truncated,error,created_at_ms,finished_at_ms,output_files";
pub(super) const ACTIVE: &str = "status IN ('queued','running','cancelling')";

pub(super) fn valid_entry(entry: &str) -> bool {
    crate::artifacts::valid_path(entry)
        && [".js", ".mjs", ".cjs"]
            .iter()
            .any(|ext| entry.ends_with(ext))
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Check,
    Run,
}
impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Run => "run",
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionInput {
    pub id: Uuid,
    pub revision_id: Uuid,
    pub entry: String,
    pub action: Action,
}

#[derive(Clone)]
pub(crate) enum ExecutionSpec {
    Revision(ExecutionInput),
    Compute {
        id: Uuid,
        code: String,
        input_assets: Vec<AssetRef>,
    },
}

impl ExecutionSpec {
    pub(crate) fn id(&self) -> Uuid {
        match self {
            Self::Revision(input) => input.id,
            Self::Compute { id, .. } => *id,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Execution {
    pub id: String,
    pub workspace_id: String,
    pub revision_id: Option<String>,
    pub entry: String,
    pub action: String,
    pub code: Option<String>,
    pub input_assets: Vec<AssetRef>,
    pub image_id: String,
    pub status: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub truncated: bool,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub finished_at_ms: Option<i32_or_i64::Timestamp>,
    #[serde(rename = "outputs", serialize_with = "output_summaries")]
    pub output_files: Vec<File>,
    pub output_assets: Vec<Asset>,
}

mod i32_or_i64 {
    pub type Timestamp = i64;
}

#[derive(Default)]
pub struct ExecutionResult {
    pub status: &'static str,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub truncated: bool,
    pub error: Option<String>,
    pub output_files: Vec<File>,
}

// A bounded observation, frozen with a model request. It grants no execution or
// file authority and contains no captured file contents.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecutionAttachment {
    pub id: String,
    pub workspace_id: String,
    pub revision_id: Option<String>,
    pub entry: String,
    pub action: String,
    pub status: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub truncated: bool,
    pub error: Option<String>,
    pub logs_shortened: bool,
    #[serde(default)]
    pub output_assets: Vec<Asset>,
}

pub(super) fn attachment(c: &Connection, workspace: &str, id: &str) -> Result<ExecutionAttachment> {
    let job = c
        .query_row(
            &format!("SELECT {COLUMNS} FROM executions WHERE id=?1 AND workspace_id=?2"),
            params![id, workspace],
            read_row,
        )
        .optional()?
        .ok_or(ExecutionError(
            StatusCode::NOT_FOUND,
            "Execution result not found in this workspace.",
        ))?;
    if job.is_active() {
        return Err(ExecutionError(
            StatusCode::CONFLICT,
            "Wait for execution cleanup before attaching its result.",
        )
        .into());
    }
    fn excerpt(mut text: String) -> String {
        let mut start = text.len().saturating_sub(8192);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text.drain(..start);
        text
    }
    let output_assets = execution_assets(c, &job.id)?;
    Ok(ExecutionAttachment {
        logs_shortened: job.stdout.len() > 8192 || job.stderr.len() > 8192,
        id: job.id,
        workspace_id: job.workspace_id,
        revision_id: job.revision_id,
        entry: job.entry,
        action: job.action,
        status: job.status,
        stdout: excerpt(job.stdout),
        stderr: excerpt(job.stderr),
        exit_code: job.exit_code,
        signal: job.signal,
        truncated: job.truncated,
        error: job.error,
        output_assets,
    })
}

fn output_summaries<S: serde::Serializer>(
    files: &[File],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq;
    let mut sequence = serializer.serialize_seq(Some(files.len()))?;
    for file in files {
        sequence
            .serialize_element(&serde_json::json!({"path":file.path,"bytes":file.content.len()}))?;
    }
    sequence.end()
}

pub struct WorkerBinding {
    pub path: String,
    pub endpoint: String,
    pub daemon: String,
}

#[derive(Debug)]
pub struct ExecutionError(pub StatusCode, pub &'static str);
impl std::fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.1)
    }
}
impl std::error::Error for ExecutionError {}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Execution> {
    Ok(Execution {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        revision_id: row.get(2)?,
        entry: row.get(3)?,
        action: row.get(4)?,
        code: row.get(5)?,
        input_assets: read_json(row, 6)?,
        image_id: row.get(7)?,
        status: row.get(8)?,
        stdout: row.get(9)?,
        stderr: row.get(10)?,
        exit_code: row.get(11)?,
        signal: row.get(12)?,
        truncated: row.get(13)?,
        error: row.get(14)?,
        created_at_ms: row.get(15)?,
        finished_at_ms: row.get(16)?,
        output_files: read_json(row, 17)?,
        output_assets: Vec::new(),
    })
}

fn read(c: &Connection, mut job: Execution) -> Result<Execution> {
    job.output_assets = execution_assets(c, &job.id)?;
    Ok(job)
}

impl Execution {
    pub fn is_active(&self) -> bool {
        matches!(self.status.as_str(), "queued" | "running" | "cancelling")
    }

    pub fn matches(&self, workspace: &str, input: &ExecutionInput) -> bool {
        self.workspace_id == workspace
            && self.revision_id.as_deref() == Some(&input.revision_id.to_string())
            && self.code.is_none()
            && self.entry == input.entry
            && self.action == input.action.as_str()
    }

    pub(crate) fn matches_spec(&self, workspace: &str, spec: &ExecutionSpec) -> bool {
        match spec {
            ExecutionSpec::Revision(input) => self.matches(workspace, input),
            ExecutionSpec::Compute {
                code, input_assets, ..
            } => {
                self.workspace_id == workspace
                    && self.revision_id.is_none()
                    && self.code.as_deref() == Some(code.as_str())
                    && &self.input_assets == input_assets
                    && self.entry == "__compute.cjs"
                    && self.action == "run"
            }
        }
    }
}

impl Store {
    pub async fn worker_binding(&self) -> Result<Option<WorkerBinding>> {
        self.call(|c| Ok(c.query_row("SELECT path,endpoint,daemon FROM worker_state WHERE singleton=1 AND endpoint IS NOT NULL", [],
            |r| Ok(WorkerBinding { path: r.get(0)?, endpoint: r.get(1)?, daemon: r.get(2)? })).optional()?)).await
    }

    pub async fn bind_worker(&self, endpoint: String, daemon: String) -> Result<()> {
        let path = self.worker_path.clone().ok_or(ExecutionError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Native execution requires a lossless UTF-8 database path.",
        ))?;
        self.call(move |c| {
            c.execute("UPDATE worker_state SET path=?1,endpoint=?2,daemon=?3 WHERE singleton=1 AND endpoint IS NULL", params![path,endpoint,daemon])?;
            Ok(())
        }).await
    }

    pub async fn execution(&self, id: String) -> Result<Option<Execution>> {
        self.call(move |c| {
            let Some(job) = c
                .query_row(
                    &format!("SELECT {COLUMNS} FROM executions WHERE id=?1"),
                    [id],
                    read_row,
                )
                .optional()?
            else {
                return Ok(None);
            };
            Ok(Some(read(c, job)?))
        })
        .await
    }

    pub async fn owns_execution(&self, id: String) -> Result<bool> {
        self.call(move |c| {
            Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM executions WHERE id=?1) OR EXISTS(SELECT 1 FROM execution_cleanup_ids WHERE id=?1)", [id], |r| r.get(0))?)
        }).await
    }

    pub async fn executions(&self, workspace: String) -> Result<Vec<Execution>> {
        self.call(move |c| {
            let jobs = c
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM executions WHERE workspace_id=?1 ORDER BY rowid"
                ))?
                .query_map([workspace], read_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            jobs.into_iter().map(|job| read(c, job)).collect()
        })
        .await
    }

    pub async fn unfinished_executions(&self) -> Result<Vec<Execution>> {
        self.call(|c| {
            let jobs = c
                .prepare(&format!("SELECT {COLUMNS} FROM executions WHERE {ACTIVE}"))?
                .query_map([], read_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            jobs.into_iter().map(|job| read(c, job)).collect()
        })
        .await
    }

    pub(crate) async fn execution_files(&self, id: String) -> Result<Vec<File>> {
        self.call(move |c| {
            let job = c
                .query_row(
                    &format!("SELECT {COLUMNS} FROM executions WHERE id=?1"),
                    [&id],
                    read_row,
                )
                .optional()?
                .ok_or_else(|| anyhow::anyhow!("Execution record disappeared."))?;
            if let Some(revision_id) = &job.revision_id {
                return base_files(c, &job.workspace_id, Some(revision_id));
            }
            let code = job
                .code
                .ok_or_else(|| anyhow::anyhow!("Missing temporary computation code."))?;
            let mut files = Vec::with_capacity(job.input_assets.len() + 1);
            for asset in &job.input_assets {
                let content: String = c
                    .query_row(
                        "SELECT content FROM assets WHERE id=?1 AND workspace_id=?2",
                        params![asset.id, job.workspace_id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .ok_or_else(|| anyhow::anyhow!("Input asset disappeared."))?;
                files.push(File {
                    path: asset.path.clone(),
                    content,
                });
            }
            files.push(File {
                path: "__compute.cjs".into(),
                content: code,
            });
            validate_files(&files)?;
            Ok(files)
        })
        .await
    }

    pub async fn accept_execution(
        &self,
        workspace: String,
        input: ExecutionInput,
        image: String,
    ) -> Result<(Execution, bool)> {
        self.admit_execution(workspace, ExecutionSpec::Revision(input), image, None)
            .await
    }

    pub(crate) async fn admit_execution(
        &self,
        workspace: String,
        spec: ExecutionSpec,
        image: String,
        parent: Option<String>,
    ) -> Result<(Execution, bool)> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let id_str = spec.id().to_string();
            if let Some(parent) = &parent {
                super::tool_executions::authorize(&tx, parent, &workspace, &spec)?;
            } else {
                let reserved: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM run_executions WHERE execution_id=?1)",
                    [&id_str],
                    |r| r.get(0),
                )?;
                anyhow::ensure!(!reserved, "This execution identity belongs to a model tool call.");
            }
            if let Some(existing) = tx
                .query_row(
                    &format!("SELECT {COLUMNS} FROM executions WHERE id=?1"),
                    [&id_str],
                    read_row,
                )
                .optional()?
            {
                if !existing.matches_spec(&workspace, &spec) {
                    return Err(ExecutionError(
                        StatusCode::CONFLICT,
                        "This execution ID already names different input.",
                    )
                    .into());
                }
                return Ok((read(&tx, existing)?, false));
            }
            let retired: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM execution_cleanup_ids WHERE id=?1)",
                [&id_str],
                |r| r.get(0),
            )?;
            if retired {
                return Err(ExecutionError(
                    StatusCode::CONFLICT,
                    "This execution ID belongs to deleted workspace history; use a new ID.",
                )
                .into());
            }
            let (revision_id, entry, action, code, input_assets) = match &spec {
                ExecutionSpec::Revision(input) => {
                    let rev_str = input.revision_id.to_string();
                    let files = tx
                        .query_row(
                            &format!(
                                "SELECT {REVISION_COLUMNS} FROM revisions WHERE id=?1 AND workspace_id=?2"
                            ),
                            params![rev_str, workspace],
                            read_revision,
                        )
                        .optional()?
                        .ok_or(ExecutionError(
                            StatusCode::NOT_FOUND,
                            "Revision not found in this workspace.",
                        ))?
                        .files;
                    if !valid_entry(&input.entry)
                        || !files.iter().any(|file| file.path == input.entry)
                    {
                        return Err(ExecutionError(
                            StatusCode::BAD_REQUEST,
                            "Choose a JavaScript entry present in the saved revision.",
                        )
                        .into());
                    }
                    (
                        Some(rev_str),
                        input.entry.clone(),
                        input.action.as_str(),
                        None,
                        Vec::new(),
                    )
                }
                ExecutionSpec::Compute {
                    code, input_assets, ..
                } => {
                    let mut files = Vec::with_capacity(input_assets.len() + 1);
                    for asset in input_assets {
                        let content: String = tx
                            .query_row(
                                "SELECT content FROM assets WHERE id=?1 AND workspace_id=?2",
                                params![asset.id, workspace],
                                |r| r.get(0),
                            )
                            .optional()?
                            .ok_or(ExecutionError(
                                StatusCode::NOT_FOUND,
                                "Selected computation asset was not found in this workspace.",
                            ))?;
                        files.push(File {
                            path: asset.path.clone(),
                            content,
                        });
                    }
                    files.push(File {
                        path: "__compute.cjs".into(),
                        content: code.clone(),
                    });
                    validate_files(&files).map_err(|_| {
                        ExecutionError(
                            StatusCode::BAD_REQUEST,
                            "Temporary computation script and inputs exceed worker file budgets.",
                        )
                    })?;
                    (
                        None,
                        "__compute.cjs".to_string(),
                        "run",
                        Some(code.clone()),
                        input_assets.clone(),
                    )
                }
            };
            let busy: bool = tx.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM executions WHERE {ACTIVE})"),
                [],
                |r| r.get(0),
            )?;
            if busy {
                return Err(ExecutionError(
                    StatusCode::CONFLICT,
                    "An execution is active or awaiting cleanup.",
                )
                .into());
            }
            tx.execute(
                "INSERT INTO executions(id,workspace_id,revision_id,entry,action,code,input_assets,image_id,status,created_at_ms)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'queued',?9)",
                params![
                    id_str,
                    workspace,
                    revision_id,
                    entry,
                    action,
                    code,
                    serde_json::to_string(&input_assets)?,
                    image,
                    now_ms()
                ],
            )?;
            let job = tx.query_row(
                &format!("SELECT {COLUMNS} FROM executions WHERE id=?1"),
                [id_str],
                read_row,
            )?;
            let job = read(&tx, job)?;
            tx.commit()?;
            Ok((job, true))
        })
        .await
    }

    pub async fn start_execution(&self, id: String) -> Result<bool> {
        self.call(move |c| {
            Ok(c.execute(
                "UPDATE executions SET status='running' WHERE id=?1 AND status='queued'",
                [id],
            )? == 1)
        })
        .await
    }

    pub async fn cancel_execution(&self, id: String) -> Result<Option<Execution>> {
        self.call(move |c| {
            c.execute("UPDATE executions SET status='cancelling' WHERE id=?1 AND status IN ('queued','running')", [&id])?;
            let Some(job) = c.query_row(&format!("SELECT {COLUMNS} FROM executions WHERE id=?1"), [id], read_row).optional()? else {
                return Ok(None);
            };
            Ok(Some(read(c, job)?))
        }).await
    }

    pub async fn finish_execution(
        &self,
        id: String,
        result: ExecutionResult,
        cleaned: bool,
    ) -> Result<()> {
        self.call(move |c| {
            validate_files(&result.output_files)?;
            let tx = c.transaction()?;
            let now = now_ms();
            let changed = tx.execute(&format!("UPDATE executions SET
                status=CASE WHEN NOT ?2 THEN 'cancelling' WHEN ?3='interrupted' THEN 'interrupted' WHEN status='cancelling' THEN 'cancelled' ELSE ?3 END,
                stdout=?4,stderr=?5,exit_code=?6,signal=?7,truncated=?8,error=?9,finished_at_ms=CASE WHEN ?2 THEN ?10 ELSE NULL END,
                output_files=CASE WHEN ?2 AND ?3='succeeded' AND status!='cancelling' THEN ?11 ELSE '[]' END
                WHERE id=?1 AND {ACTIVE}"), params![id, cleaned, result.status, result.stdout, result.stderr, result.exit_code, result.signal, result.truncated,
                    if cleaned { result.error } else { Some("Cleanup is pending. Restore Docker and refresh the worker before starting another execution.".into()) }, now, serde_json::to_string(&result.output_files)?])?;
            if changed == 1 {
                let row: (String, Option<String>, String) = tx.query_row(
                    "SELECT workspace_id,revision_id,status FROM executions WHERE id=?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?;
                let (workspace_id, revision_id, final_status) = row;
                if revision_id.is_none() && final_status == "succeeded" {
                    for file in &result.output_files {
                        let media_type = infer_media_type(&file.path, None)
                            .unwrap_or_else(|_| "text/plain".into());
                        tx.execute(
                            "INSERT INTO assets(id,workspace_id,name,media_type,bytes,content,origin,execution_id,created_at_ms)
                             VALUES (?1,?2,?3,?4,?5,?6,'execution',?7,?8)",
                            params![
                                Uuid::new_v4().to_string(),
                                workspace_id,
                                file.path,
                                media_type,
                                file.content.len() as i64,
                                file.content,
                                id,
                                now
                            ],
                        )?;
                    }
                }
            }
            tx.commit()?;
            Ok(())
        }).await
    }

    pub async fn propose_execution_outputs(&self, id: String) -> Result<Option<Proposal>> {
        self.call(move |c| {
            let tx = c.transaction()?;
            if let Some(proposal) = tx.query_row(&format!("SELECT {PROPOSAL_COLUMNS} FROM proposals WHERE execution_id=?1"), [&id], read_proposal).optional()? {
                return Ok(Some(proposal));
            }
            let Some(job) = tx.query_row(&format!("SELECT {COLUMNS} FROM executions WHERE id=?1"), [&id], read_row).optional()? else { return Ok(None); };
            if job.status != "succeeded" { return Err(ExecutionError(StatusCode::BAD_REQUEST, "Only successful, cleaned-up executions can propose output changes.").into()); }
            let Some(revision_id) = job.revision_id.as_deref() else {
                return Err(ExecutionError(
                    StatusCode::BAD_REQUEST,
                    "Temporary computations have no input revision; review selected output assets instead.",
                )
                .into());
            };
            validate_files(&job.output_files)?;
            let base = base_files(&tx, &job.workspace_id, Some(revision_id))?;
            let changes: Vec<_> = job.output_files.into_iter().filter(|file| !base.contains(file))
                .map(|file| FileChange::Write { path: file.path, content: file.content }).collect();
            if changes.is_empty() { return Err(ExecutionError(StatusCode::BAD_REQUEST, "This execution produced no changed output files.").into()); }
            let (files,bindings)=apply_project_changes(&base,
                &project_assets::base_bindings(&tx,&job.workspace_id,Some(revision_id))?,&changes,&[])
                .map_err(|_| ExecutionError(StatusCode::BAD_REQUEST, "Output changes exceed project limits or conflict with existing paths."))?;
            project_assets::resolve(&tx,&job.workspace_id,&files,&bindings)
                .map_err(|_|ExecutionError(StatusCode::BAD_REQUEST,"Output changes conflict with project asset bindings."))?;
            let proposal_id = Uuid::new_v4().to_string();
            tx.execute("INSERT INTO proposals(id,workspace_id,execution_id,base_revision_id,status,changes) VALUES (?1,?2,?3,?4,'pending',?5)",
                params![proposal_id,job.workspace_id,job.id,revision_id,serde_json::to_string(&changes)?])?;
            let proposal = tx.query_row(&format!("SELECT {PROPOSAL_COLUMNS} FROM proposals WHERE id=?1"), [proposal_id], read_proposal)?;
            tx.commit()?;
            Ok(Some(proposal))
        }).await
    }
}
