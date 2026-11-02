use crate::model::{Completion, ContextMessage, ModelStats};
use crate::{
    artifacts::{
        AssetBinding, AssetChange, File, FileChange, GeneratedArtifact, apply_project_changes,
        validate_files, validate_project,
    },
    model::Mode,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

mod assets;
mod branching;
mod context;
mod edits;
mod executions;
mod guests;
pub(crate) mod images;
mod migrations;
mod model_calls;
mod project_assets;
mod searches;
mod telemetry;
mod tool_executions;
mod workspace_files;
pub use assets::{Asset, AssetDetail, AssetRef, ImportSource, InvalidAsset};
pub use edits::InvalidEdit;
pub(crate) use executions::ExecutionSpec;
pub use executions::{
    Action, Execution, ExecutionAttachment, ExecutionError, ExecutionInput, ExecutionResult,
    WorkerBinding,
};
pub use guests::GuestCleanup;
pub use images::{ImageAsset, ImageInput, ImageJob};
pub(crate) use model_calls::LiveModelStages;
pub use model_calls::ModelCall;
pub(crate) use model_calls::ModelCallOutcome;
pub use model_calls::{LocalModelActivity, LocalModelPhase};
pub use project_assets::ProjectAsset;
pub use tool_executions::ToolExecution;
pub(crate) use workspace_files::{
    FileInvocation, WorkspaceFiles, definitions as workspace_file_definitions,
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeScope {
    #[default]
    Off,
    Revision,
    Compute,
}

impl NodeScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Revision => "revision",
            Self::Compute => "compute",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "revision" => Some(Self::Revision),
            "compute" => Some(Self::Compute),
            _ => None,
        }
    }
}

const ACTIVE_RUN_PREDICATE: &str = "status IN ('queued','running')";
const RUN_COLUMNS: &str = "id,workspace_id,status,error,provider,model,output,stats,created_at_ms,started_at_ms,finished_at_ms,mode,base_revision_id,execution_result,search_config,search_activity,model_rounds,execution_revision_id,node_scope,input_assets,image_config,image_input,import_enabled,workspace_tools,model_route,image_attachments";
const REVISION_COLUMNS: &str =
    "id,run_id,execution_id,edit_id,asset_ids,number,html,files,asset_bindings";
const PROPOSAL_COLUMNS: &str = "id,workspace_id,run_id,execution_id,edit_id,asset_ids,base_revision_id,status,changes,applied_revision_id,rejection_reason,asset_changes";

#[derive(Debug)]
pub enum ArtifactConflict {
    StaleBase,
    ActiveRun,
    Settled,
    EditIdentity,
    WorkspaceBusy,
}
impl std::fmt::Display for ArtifactConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::StaleBase => {
                "The base revision is stale; refresh and generate against the current revision."
            }
            Self::ActiveRun => {
                "Wait for or cancel this workspace's active run before applying changes."
            }
            Self::Settled => "This proposal has already been settled.",
            Self::EditIdentity => "This edit ID is already bound to different input.",
            Self::WorkspaceBusy => {
                "Stop or wait for active model, Node and image jobs before deleting this workspace."
            }
        })
    }
}
impl std::error::Error for ArtifactConflict {}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[derive(Clone)]
pub struct Store {
    pub(crate) resources: crate::resources::Resources,
    pub(crate) model_stages: LiveModelStages,
    maintenance: bool,
    pub(crate) worker_owner: String,
    pub(crate) worker_path: Option<String>,
    connection: Arc<Mutex<Connection>>,
    _lease: Arc<Option<std::fs::File>>,
}

#[derive(Serialize)]
pub struct Workspace {
    pub id: String,
    pub title: String,
}

#[derive(Serialize, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub run_id: String,
}

#[derive(Serialize)]
pub struct Revision {
    pub id: String,
    pub run_id: Option<String>,
    pub execution_id: Option<String>,
    pub edit_id: Option<String>,
    pub asset_ids: Option<Vec<String>>,
    pub number: i64,
    pub html: String,
    pub files: Vec<File>,
    pub asset_bindings: Vec<AssetBinding>,
}

#[derive(Serialize)]
pub struct Proposal {
    pub id: String,
    pub workspace_id: String,
    pub run_id: Option<String>,
    pub execution_id: Option<String>,
    pub edit_id: Option<String>,
    pub asset_ids: Option<Vec<String>>,
    pub base_revision_id: Option<String>,
    pub status: String,
    pub changes: Vec<FileChange>,
    pub asset_changes: Vec<AssetChange>,
    pub applied_revision_id: Option<String>,
    pub rejection_reason: Option<String>,
}

#[derive(Serialize)]
pub struct WorkspaceDetail {
    pub workspace: Workspace,
    pub messages: Vec<Message>,
    pub revisions: Vec<Revision>,
    pub assets: Vec<Asset>,
    pub image_assets: Vec<ImageAsset>,
    pub runs: Vec<Run>,
    pub proposals: Vec<Proposal>,
}

#[derive(Serialize, Clone)]
pub struct Run {
    pub local_model: Option<LocalModelActivity>,
    pub model_route: Option<crate::routing::ModelRoute>,
    pub workspace_tools: bool,
    pub id: String,
    pub workspace_id: String,
    pub status: String,
    pub error: Option<String>,
    pub provider: String,
    pub model: String,
    pub output: String,
    pub stats: ModelStats,
    pub queue_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub mode: String,
    pub base_revision_id: Option<String>,
    pub execution_result: Option<ExecutionAttachment>,
    pub search_enabled: bool,
    pub import_enabled: bool,
    pub imports: Vec<Asset>,
    pub searches: Vec<crate::search::SearchReceipt>,
    pub model_rounds: u32,
    pub execution_revision_id: Option<String>,
    pub node_scope: NodeScope,
    pub input_assets: Vec<AssetRef>,
    pub tool_execution: Option<ToolExecution>,
    pub tool_executions: Vec<ToolExecution>,
    pub image_profile: Option<crate::images::ImageProfileInfo>,
    pub image_input: Option<ImageInput>,
    pub image_attachments: Vec<ImageAsset>,
    pub image_job: Option<ImageJob>,
}

impl Run {
    pub(crate) fn max_model_rounds(&self) -> usize {
        if self.workspace_tools {
            16
        } else if self.image_profile.is_some()
            && !self.search_enabled
            && !self.import_enabled
            && self.node_scope == NodeScope::Off
        {
            2
        } else if self.import_enabled || self.node_scope == NodeScope::Compute {
            5
        } else if self.node_scope == NodeScope::Revision {
            4
        } else {
            3
        }
    }
}

#[derive(Default)]
pub struct RunOptions {
    pub model_route: Option<crate::routing::ModelRoute>,
    pub workspace_tools: bool,
    pub mode: Mode,
    pub base_revision_id: Option<String>,
    pub execution_result_id: Option<String>,
    pub search: Option<crate::search::SearchConfig>,
    pub allow_url_import: bool,
    pub execution_revision_id: Option<String>,
    pub node_scope: NodeScope,
    pub input_asset_ids: Vec<String>,
    pub image_profile: Option<crate::images::ImageProfile>,
    pub image_input: Option<crate::images::ImageInputRequest>,
    pub image_attachment_ids: Vec<String>,
}

fn read_json<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<T> {
    serde_json::from_str(&row.get::<_, String>(index)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn read_optional_json<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<T>> {
    row.get::<_, Option<String>>(index)?
        .map(|text| {
            serde_json::from_str(&text).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()
}

fn read_revision(row: &rusqlite::Row<'_>) -> rusqlite::Result<Revision> {
    let files: Vec<File> = read_json(row, 7)?;
    let asset_bindings: Vec<AssetBinding> = read_json(row, 8)?;
    validate_project(&files, &asset_bindings).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, error.into())
    })?;
    Ok(Revision {
        id: row.get(0)?,
        run_id: row.get(1)?,
        execution_id: row.get(2)?,
        edit_id: row.get(3)?,
        asset_ids: read_optional_json(row, 4)?,
        number: row.get(5)?,
        html: row.get(6)?,
        files,
        asset_bindings,
    })
}

fn read_proposal(row: &rusqlite::Row<'_>) -> rusqlite::Result<Proposal> {
    Ok(Proposal {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        run_id: row.get(2)?,
        execution_id: row.get(3)?,
        edit_id: row.get(4)?,
        asset_ids: read_optional_json(row, 5)?,
        base_revision_id: row.get(6)?,
        status: row.get(7)?,
        changes: read_json(row, 8)?,
        applied_revision_id: row.get(9)?,
        rejection_reason: row.get(10)?,
        asset_changes: read_json(row, 11)?,
    })
}

fn head(c: &Connection, workspace_id: &str) -> Result<Option<Revision>> {
    Ok(c.query_row(&format!("SELECT {REVISION_COLUMNS} FROM revisions WHERE workspace_id=?1 ORDER BY number DESC LIMIT 1"), [workspace_id], read_revision).optional()?)
}

fn base_files(c: &Connection, workspace_id: &str, base_id: Option<&str>) -> Result<Vec<File>> {
    match base_id {
        None => Ok(Vec::new()),
        Some(id) => Ok(c
            .query_row(
                &format!(
                    "SELECT {REVISION_COLUMNS} FROM revisions WHERE workspace_id=?1 AND id=?2"
                ),
                params![workspace_id, id],
                read_revision,
            )
            .optional()?
            .ok_or(ArtifactConflict::StaleBase)?
            .files),
    }
}

fn insert_revision(
    c: &Connection,
    proposal: &Proposal,
    files: Vec<File>,
    asset_bindings: Vec<AssetBinding>,
) -> Result<Revision> {
    validate_project(&files, &asset_bindings)?;
    let id = Uuid::new_v4().to_string();
    let html = files
        .iter()
        .find(|f| f.path == "index.html")
        .map(|f| f.content.as_str())
        .unwrap_or_default();
    let asset_ids = proposal
        .asset_ids
        .as_deref()
        .map(serde_json::to_string)
        .transpose()?;
    c.execute(
        "INSERT INTO revisions(id,workspace_id,run_id,execution_id,edit_id,asset_ids,number,html,files,asset_bindings)
        SELECT ?1,?2,?3,?6,?7,?8,COALESCE(MAX(number),0)+1,?4,?5,?9 FROM revisions WHERE workspace_id=?2",
        params![
            id,
            proposal.workspace_id,
            proposal.run_id,
            html,
            serde_json::to_string(&files)?,
            proposal.execution_id,
            proposal.edit_id,
            asset_ids,
            serde_json::to_string(&asset_bindings)?
        ],
    )?;
    Ok(c.query_row(
        &format!("SELECT {REVISION_COLUMNS} FROM revisions WHERE id=?1"),
        [id],
        read_revision,
    )?)
}

fn read_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    let created: Option<i64> = row.get(8)?;
    let started: Option<i64> = row.get(9)?;
    let finished: Option<i64> = row.get(10)?;
    let scope_raw: String = row.get(18)?;
    let node_scope = NodeScope::parse(&scope_raw).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            18,
            rusqlite::types::Type::Text,
            format!("invalid node_scope {scope_raw}").into(),
        )
    })?;
    let now = now_ms();
    Ok(Run {
        local_model: None,
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        status: row.get(2)?,
        error: row.get(3)?,
        provider: row.get(4)?,
        model: row.get(5)?,
        output: row.get(6)?,
        stats: read_json(row, 7)?,
        queue_ms: created.map(|time| (started.or(finished).unwrap_or(now) - time).max(0)),
        duration_ms: started.map(|time| (finished.unwrap_or(now) - time).max(0)),
        mode: row.get(11)?,
        base_revision_id: row.get(12)?,
        execution_result: read_optional_json(row, 13)?,
        search_enabled: row.get::<_, Option<String>>(14)?.is_some(),
        searches: read_json(row, 15)?,
        model_rounds: row.get(16)?,
        execution_revision_id: row.get(17)?,
        node_scope,
        input_assets: read_json(row, 19)?,
        tool_execution: None,
        tool_executions: Vec::new(),
        image_profile: read_optional_json::<crate::images::ImageProfile>(row, 20)?
            .map(|p| p.info()),
        image_job: None,
        image_input: read_optional_json(row, 21)?,
        import_enabled: row.get(22)?,
        imports: Vec::new(),
        workspace_tools: row.get(23)?,
        model_route: read_optional_json(row, 24)?,
        image_attachments: read_json(row, 25)?,
    })
}

fn workspace_busy(c: &Connection, id: &str) -> Result<bool> {
    Ok(c.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM runs WHERE workspace_id=?1 AND {ACTIVE_RUN_PREDICATE}) OR EXISTS(SELECT 1 FROM executions WHERE workspace_id=?1 AND {}) OR EXISTS(SELECT 1 FROM image_jobs WHERE workspace_id=?1 AND {}) OR EXISTS(SELECT 1 FROM model_calls WHERE workspace_id=?1 AND {})", executions::ACTIVE, images::ACTIVE, model_calls::ACTIVE),
        [id], |row| row.get(0),
    )?)
}

fn delete_workspace(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<()> {
    if workspace_busy(tx, id)? {
        return Err(ArtifactConflict::WorkspaceBusy.into());
    }
    // Preserve the scoped cleanup ledger while deferring circular artifact references.
    tx.pragma_update(None, "defer_foreign_keys", true)?;
    tx.execute("INSERT OR IGNORE INTO execution_cleanup_ids(id) SELECT id FROM executions WHERE workspace_id=?1", [id])?;
    for table in [
        "model_calls",
        "messages",
        "proposals",
        "revisions",
        "assets",
        "image_assets",
        "image_jobs",
        "executions",
        "runs",
    ] {
        tx.execute(&format!("DELETE FROM {table} WHERE workspace_id=?1"), [id])?;
    }
    tx.execute("DELETE FROM workspaces WHERE id=?1", [id])?;
    Ok(())
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_database(path, true)
    }

    pub fn open_for_maintenance(path: &Path) -> Result<Self> {
        ensure!(
            path.is_file(),
            "The data directory does not contain a Deep database."
        );
        Self::open_database(path, false)
    }

    fn open_database(path: &Path, recover: bool) -> Result<Self> {
        let lease = if path == Path::new(":memory:") {
            None
        } else {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path.with_extension("lock"))?;
            file.try_lock()
                .context("another Deep process owns this data directory")?;
            Some(file)
        };
        let mut connection = Connection::open(path)?;
        if recover {
            migrations::migrate(&mut connection)?;
        } else {
            let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            ensure!(
                version == 23,
                "Start this Deep version once before using maintenance."
            );
            connection.execute_batch("PRAGMA foreign_keys=ON;")?;
        }
        let worker_owner: String = connection.query_row(
            "SELECT owner FROM worker_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        Uuid::parse_str(&worker_owner).context("invalid worker ownership identity")?;
        if recover {
            let recovery = connection.transaction()?;
            recovery.execute("UPDATE model_calls SET status='unresolved',reason='process_restarted' WHERE status='dispatching'", [])?;
            let unfinished = recovery
                .prepare(&format!("SELECT id FROM runs WHERE {ACTIVE_RUN_PREDICATE}"))?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for id in unfinished {
                searches::close_pending(&recovery, &id, "interrupted")?;
                tool_executions::cancel_child(&recovery, &id)?;
                images::close_pending(&recovery, &id)?;
            }
            recovery.execute(&format!("UPDATE runs SET status='interrupted',
            error='Server restarted; work was not replayed.', finished_at_ms=?1 WHERE {ACTIVE_RUN_PREDICATE}"), [now_ms()])?;
            recovery.commit()?;
        }
        Ok(Self {
            resources: crate::resources::Resources::default(),
            model_stages: LiveModelStages::default(),
            maintenance: !recover,
            worker_owner,
            worker_path: if path == Path::new(":memory:") {
                Some(format!("memory:{}", Uuid::new_v4()))
            } else {
                path.canonicalize()?.to_str().map(str::to_owned)
            },
            connection: Arc::new(Mutex::new(connection)),
            _lease: Arc::new(lease),
        })
    }

    async fn call<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = connection
                .lock()
                .map_err(|_| anyhow::anyhow!("database lock poisoned"))?;
            f(&mut guard)
        })
        .await?
    }

    pub async fn create_workspace(&self, title: String) -> Result<Workspace> {
        self.call(move |c| {
            let workspace = Workspace {
                id: Uuid::new_v4().to_string(),
                title,
            };
            c.execute(
                "INSERT INTO workspaces(id,title) VALUES (?1,?2)",
                params![workspace.id, workspace.title],
            )?;
            Ok(workspace)
        })
        .await
    }

    pub async fn workspaces(&self) -> Result<Vec<Workspace>> {
        self.call(|c| {
            Ok(
                c.prepare("SELECT id,title FROM workspaces ORDER BY rowid DESC")?
                    .query_map([], |r| {
                        Ok(Workspace {
                            id: r.get(0)?,
                            title: r.get(1)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            )
        })
        .await
    }

    pub async fn delete_workspace(&self, id: String) -> Result<()> {
        self.call(move |c| {
            let tx = c.transaction()?;
            delete_workspace(&tx, &id)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn workspace(&self, id: String) -> Result<Option<WorkspaceDetail>> {
        let stages = self.model_stages.clone();
        self.call(move |c| {
            let workspace = c
                .query_row("SELECT id,title FROM workspaces WHERE id=?1", [&id], |r| {
                    Ok(Workspace {
                        id: r.get(0)?,
                        title: r.get(1)?,
                    })
                })
                .optional()?;
            let Some(workspace) = workspace else {
                return Ok(None);
            };
            let messages = c
                .prepare(
                    "SELECT role,content,run_id FROM messages WHERE workspace_id=?1 ORDER BY seq",
                )?
                .query_map([&id], |r| {
                    Ok(Message {
                        role: r.get(0)?,
                        content: r.get(1)?,
                        run_id: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let revisions = c
                .prepare(&format!(
                    "SELECT {REVISION_COLUMNS} FROM revisions WHERE workspace_id=?1 ORDER BY number"
                ))?
                .query_map([&id], read_revision)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let assets = assets::workspace_assets(c, &id)?;
            let mut runs = c
                .prepare(&format!(
                    "SELECT {RUN_COLUMNS} FROM runs WHERE workspace_id=?1 ORDER BY rowid"
                ))?
                .query_map([&id], read_run)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for run in &mut runs {
                run.local_model = model_calls::activity(c, run, &stages)?;
                run.tool_executions = tool_executions::read_all(c, &run.id)?;
                run.tool_execution = run.tool_executions.last().cloned();
                run.image_job = images::for_run(c, &run.id)?;
                run.imports = assets::imports_for_run(c, &run.id)?;
            }
            let proposals = c
                .prepare(&format!(
                    "SELECT {PROPOSAL_COLUMNS} FROM proposals WHERE workspace_id=?1 ORDER BY rowid"
                ))?
                .query_map([&id], read_proposal)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(Some(WorkspaceDetail {
                workspace,
                messages,
                revisions,
                assets,
                image_assets: images::workspace_assets(c, &id)?,
                runs,
                proposals,
            }))
        })
        .await
    }

    pub async fn run(&self, id: String) -> Result<Option<Run>> {
        let stages = self.model_stages.clone();
        self.call(move |c| {
            let mut run = c
                .query_row(
                    &format!("SELECT {RUN_COLUMNS} FROM runs WHERE id=?1"),
                    [id],
                    read_run,
                )
                .optional()?;
            if let Some(run) = &mut run {
                run.local_model = model_calls::activity(c, run, &stages)?;
                run.tool_executions = tool_executions::read_all(c, &run.id)?;
                run.tool_execution = run.tool_executions.last().cloned();
                run.image_job = images::for_run(c, &run.id)?;
                run.imports = assets::imports_for_run(c, &run.id)?;
            }
            Ok(run)
        })
        .await
    }

    // None means a previous run still owns this workspace's generation slot.
    pub async fn begin(
        &self,
        workspace_id: String,
        prompt: String,
        provider: String,
        model: String,
        options: RunOptions,
    ) -> Result<Option<Run>> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let RunOptions { model_route, workspace_tools, mode, base_revision_id, execution_result_id, search, allow_url_import, execution_revision_id, node_scope, input_asset_ids, image_profile, image_input, image_attachment_ids } = options;
            ensure!(model_route.as_ref().is_none_or(|route| route.backend == provider && route.model == model), "Model route must match the admitted backend and model.");
            if !image_attachment_ids.is_empty() && (provider == "demo" || !model_route.as_ref().is_some_and(|route| route.vision)) {
                return Err(InvalidAsset("Select a model profile with vision enabled before attaching images to chat.".into()).into());
            }
            let image_attachments = images::resolve_attachments(&tx, &workspace_id, &image_attachment_ids)?;
            ensure!(!workspace_tools || (mode == Mode::Chat && provider != "demo"), "Workspace tools require Conversation mode and a real model backend.");
            let image_input = image_input.map(|input| {
                if image_profile.is_none() {
                    return Err(InvalidAsset("Select an image profile before attaching a reference image.".into()).into());
                }
                let asset = tx.query_row(
                    &format!("SELECT {} FROM image_assets WHERE id=?1 AND workspace_id=?2", images::ASSET_COLUMNS),
                    params![input.asset_id.to_string(),workspace_id], images::read_asset,
                ).optional()?.ok_or_else(|| InvalidAsset("Choose a saved image in this workspace.".into()))?;
                Ok::<_, anyhow::Error>(ImageInput {role: input.role, asset})
            }).transpose()?;
            if let Some(profile) = &image_profile {
                profile.validate()?;
                if provider == "demo" {
                    return Err(InvalidAsset("Image generation requires a real model backend.".into()).into());
                }
                images::check_admission(&tx, profile)?;
            }
            if let Some(config) = &search { config.validate()?; }
            ensure!(!allow_url_import || provider != "demo", "URL import requires a real model backend.");
            let node_scope = match (node_scope, execution_revision_id.is_some()) {
                (NodeScope::Off, true) | (NodeScope::Revision, true) => NodeScope::Revision,
                (NodeScope::Revision, false) => {
                    return Err(ExecutionError(
                        axum::http::StatusCode::BAD_REQUEST,
                        "Select a saved revision before enabling saved-revision execution.",
                    )
                    .into());
                }
                (NodeScope::Compute, true) => {
                    return Err(ExecutionError(
                        axum::http::StatusCode::BAD_REQUEST,
                        "Choose one Node scope: either temporary computation or a saved revision.",
                    )
                    .into());
                }
                (scope, false) => scope,
            };
            if node_scope != NodeScope::Compute && !input_asset_ids.is_empty() {
                return Err(InvalidAsset(
                    "Enable temporary Node computation before selecting input assets.".into(),
                )
                .into());
            }
            let busy: bool = tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM runs WHERE workspace_id=?1 AND {ACTIVE_RUN_PREDICATE})"),
                [&workspace_id], |r| r.get(0))?;
            if busy { return Ok(None) }
            let current = head(&tx, &workspace_id)?;
            if mode == Mode::Files && current.as_ref().map(|r| r.id.as_str()) != base_revision_id.as_deref() {
                return Err(ArtifactConflict::StaleBase.into());
            }
            if let Some(requested) = &execution_revision_id {
                let revision = current.as_ref().filter(|r| &r.id == requested)
                    .ok_or(ExecutionError(axum::http::StatusCode::CONFLICT, "Saved revision changed; refresh before allowing script execution."))?;
                if !revision.files.iter().any(|file| executions::valid_entry(&file.path)) {
                    return Err(ExecutionError(axum::http::StatusCode::BAD_REQUEST, "Apply a project containing a saved JavaScript script before enabling execution.").into());
                }
            }
            let input_assets = if node_scope == NodeScope::Compute {
                assets::resolve_input_assets(&tx, &workspace_id, &input_asset_ids)?
                    .into_iter()
                    .map(|(meta, _)| meta)
                    .collect()
            } else {
                Vec::new()
            };
            let base_revision_id = if mode == Mode::App || workspace_tools { current.map(|r| r.id) } else { base_revision_id };
            let execution_result = execution_result_id.as_deref()
                .map(|id| executions::attachment(&tx, &workspace_id, id)).transpose()?;
            let run = Run { local_model: None, model_route, workspace_tools, id: Uuid::new_v4().to_string(), workspace_id, status: "queued".into(), error: None, provider, model,
                output: String::new(), stats: ModelStats::default(), queue_ms: Some(0), duration_ms: None,
                mode: mode.as_str().into(), base_revision_id, execution_result, search_enabled: search.is_some(), searches: Vec::new(), import_enabled: allow_url_import, imports: Vec::new(),
                model_rounds: 0, execution_revision_id, node_scope, input_assets, tool_execution: None, tool_executions: Vec::new(),
                image_profile:image_profile.as_ref().map(|p|p.info()),image_input,image_attachments,image_job:None };
            let mut context = context::capture(&tx, &run.workspace_id)?;
            if let Some(result) = &run.execution_result { context.push(context::execution_observation(result)?); }
            let context = serde_json::to_string(&context)?;
            tx.execute("INSERT INTO runs(id,workspace_id,status,provider,model,created_at_ms,mode,base_revision_id,context_messages,execution_result,search_config,execution_revision_id,node_scope,input_assets,image_config,image_input,import_enabled,workspace_tools,model_route,image_attachments) VALUES (?1,?2,'queued',?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
                params![run.id, run.workspace_id, run.provider, run.model, now_ms(), run.mode, run.base_revision_id, context,
                    run.execution_result.as_ref().map(serde_json::to_string).transpose()?, search.as_ref().map(serde_json::to_string).transpose()?,
                    run.execution_revision_id, run.node_scope.as_str(), serde_json::to_string(&run.input_assets)?,image_profile.as_ref().map(serde_json::to_string).transpose()?,run.image_input.as_ref().map(serde_json::to_string).transpose()?,allow_url_import,workspace_tools,run.model_route.as_ref().map(serde_json::to_string).transpose()?,serde_json::to_string(&run.image_attachments)?])?;
            tx.execute("INSERT INTO messages(workspace_id,run_id,role,content) VALUES (?1,?2,'user',?3)",
                params![run.workspace_id, run.id, prompt])?;
            tx.commit()?;
            Ok(Some(run))
        }).await
    }

    pub async fn history(&self, workspace_id: String) -> Result<Vec<Message>> {
        self.call(move |c| context::history(c, &workspace_id)).await
    }

    pub async fn run_context(&self, id: String) -> Result<Vec<ContextMessage>> {
        self.call(move |c| {
            Ok(c.query_row(
                "SELECT context_messages FROM runs WHERE id=?1",
                [id],
                |row| read_json(row, 0),
            )?)
        })
        .await
    }

    pub async fn mark_running(&self, id: String) -> Result<bool> {
        self.call(move |c| {
            Ok(c.execute(
                "UPDATE runs SET status='running',started_at_ms=?2 WHERE id=?1 AND status='queued'",
                params![id, now_ms()],
            )? == 1)
        })
        .await
    }

    pub(crate) async fn tool_round(&self, id: String, round: usize) -> Result<()> {
        self.call(move |c| {
            let run = c.query_row(&format!("SELECT {RUN_COLUMNS} FROM runs WHERE id=?1"),[&id],read_run)?;
            ensure!((1..=run.max_model_rounds()).contains(&round), "Model round budget exceeded.");
            ensure!(run.workspace_tools || run.search_enabled || run.import_enabled || run.node_scope != NodeScope::Off || run.image_profile.is_some(), "No model tools were granted.");
            ensure!(c.execute("UPDATE runs SET model_rounds=?2,output='' WHERE id=?1 AND status='running' AND model_rounds=?3", params![id, round as u32, (round-1) as u32])? == 1, "Tool run is no longer active.");
            Ok(())
        }).await
    }

    pub async fn progress(&self, id: String, completion: Completion) -> Result<()> {
        let stats = serde_json::to_string(&completion.stats)?;
        self.call(move |c| {
            c.execute(
                "UPDATE runs SET output=?2,stats=?3 WHERE id=?1 AND status='running'",
                params![id, completion.text, stats],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn files_at(
        &self,
        workspace_id: String,
        revision_id: Option<String>,
    ) -> Result<Vec<File>> {
        self.call(move |c| base_files(c, &workspace_id, revision_id.as_deref()))
            .await
    }

    pub async fn finish(
        &self,
        run: Run,
        content: String,
        artifact: Option<GeneratedArtifact>,
    ) -> Result<bool> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let changed = tx.execute("UPDATE runs SET status='succeeded',output='',finished_at_ms=?2 WHERE id=?1 AND status='running'", params![run.id, now_ms()])?;
            if changed == 0 { return Ok(false) }
            let unfinished: bool = tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM run_executions WHERE run_id=?1 AND result IS NULL) OR EXISTS(SELECT 1 FROM executions WHERE id IN (SELECT execution_id FROM run_executions WHERE run_id=?1) AND {}) OR EXISTS(SELECT 1 FROM image_jobs WHERE run_id=?1 AND {})", executions::ACTIVE,images::ACTIVE), [&run.id], |r| r.get(0))?;
            anyhow::ensure!(!unfinished, "Execution result or cleanup is still pending.");
            tx.execute("INSERT INTO messages(workspace_id,run_id,role,content) VALUES (?1,?2,'assistant',?3)",
                params![run.workspace_id, run.id, content])?;
            let changes = match artifact {
                Some(GeneratedArtifact::Html(content)) => Some((vec![FileChange::Write { path: "index.html".into(), content }],Vec::new())),
                Some(GeneratedArtifact::Changes(changes)) => Some((changes,Vec::new())),
                Some(GeneratedArtifact::Workspace {changes,asset_changes}) => {
                    ensure!(run.workspace_tools, "Workspace publication was not permitted.");
                    Some((changes,asset_changes))
                },
                None => None,
            };
            if let Some((changes,asset_changes)) = changes {
                if head(&tx, &run.workspace_id)?.as_ref().map(|r| r.id.as_str()) != run.base_revision_id.as_deref() {
                    return Err(ArtifactConflict::StaleBase.into());
                }
                let (files,bindings) = apply_project_changes(
                    &base_files(&tx, &run.workspace_id, run.base_revision_id.as_deref())?,
                    &project_assets::base_bindings(&tx,&run.workspace_id,run.base_revision_id.as_deref())?,
                    &changes,&asset_changes,
                )?;
                project_assets::resolve(&tx,&run.workspace_id,&files,&bindings)?;
                let proposal_id = Uuid::new_v4().to_string();
                tx.execute("INSERT INTO proposals(id,workspace_id,run_id,base_revision_id,status,changes,asset_changes) VALUES (?1,?2,?3,?4,'pending',?5,?6)",
                    params![proposal_id, run.workspace_id, run.id, run.base_revision_id, serde_json::to_string(&changes)?,serde_json::to_string(&asset_changes)?])?;
                if run.workspace_tools {
                    let proposal = tx.query_row(&format!("SELECT {PROPOSAL_COLUMNS} FROM proposals WHERE id=?1"),[&proposal_id],read_proposal)?;
                    let revision = insert_revision(&tx,&proposal,files,bindings)?;
                    tx.execute("UPDATE proposals SET status='applied',applied_revision_id=?2 WHERE id=?1",params![proposal_id,revision.id])?;
                }
            }
            tx.commit()?;
            Ok(true)
        }).await
    }

    pub async fn apply_proposal(&self, id: String) -> Result<Option<Revision>> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let Some(proposal) = tx.query_row(&format!("SELECT {PROPOSAL_COLUMNS} FROM proposals WHERE id=?1"), [&id], read_proposal).optional()? else { return Ok(None); };
            if let Some(revision_id) = &proposal.applied_revision_id {
                return Ok(Some(tx.query_row(&format!("SELECT {REVISION_COLUMNS} FROM revisions WHERE id=?1"), [revision_id], read_revision)?));
            }
            if proposal.status != "pending" { return Err(ArtifactConflict::Settled.into()); }
            let busy: bool = tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM runs WHERE workspace_id=?1 AND {ACTIVE_RUN_PREDICATE})"), [&proposal.workspace_id], |r| r.get(0))?;
            if busy { return Err(ArtifactConflict::ActiveRun.into()); }
            if head(&tx, &proposal.workspace_id)?.as_ref().map(|r| r.id.as_str()) != proposal.base_revision_id.as_deref() {
                return Err(ArtifactConflict::StaleBase.into());
            }
            let (files,bindings) = apply_project_changes(
                &base_files(&tx, &proposal.workspace_id, proposal.base_revision_id.as_deref())?,
                &project_assets::base_bindings(&tx,&proposal.workspace_id,proposal.base_revision_id.as_deref())?,
                &proposal.changes,&proposal.asset_changes,
            )?;
            project_assets::resolve(&tx,&proposal.workspace_id,&files,&bindings)?;
            let revision = insert_revision(&tx, &proposal, files,bindings)?;
            tx.execute("UPDATE proposals SET status='applied',applied_revision_id=?2 WHERE id=?1", params![id, revision.id])?;
            tx.commit()?;
            Ok(Some(revision))
        }).await
    }

    pub async fn reject_proposal(
        &self,
        id: String,
        reason: Option<String>,
    ) -> Result<Option<Proposal>> {
        let reason = reason
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if reason.as_ref().is_some_and(|value| value.len() > 4096) {
            anyhow::bail!("Rejection reason must fit within 4096 UTF-8 bytes.");
        }
        self.call(move |c| {
            let tx = c.transaction()?;
            let Some(proposal) = tx
                .query_row(
                    &format!("SELECT {PROPOSAL_COLUMNS} FROM proposals WHERE id=?1"),
                    [&id],
                    read_proposal,
                )
                .optional()?
            else {
                return Ok(None);
            };
            if proposal.status == "applied" {
                return Err(ArtifactConflict::Settled.into());
            }
            if proposal.status == "rejected" {
                if reason.is_some() && reason != proposal.rejection_reason {
                    return Err(ArtifactConflict::Settled.into());
                }
                return Ok(Some(proposal));
            }
            tx.execute(
                "UPDATE proposals SET status='rejected',rejection_reason=?2 WHERE id=?1 AND status='pending'",
                params![id, reason],
            )?;
            let proposal = tx.query_row(
                &format!("SELECT {PROPOSAL_COLUMNS} FROM proposals WHERE id=?1"),
                [id],
                read_proposal,
            )?;
            tx.commit()?;
            Ok(Some(proposal))
        })
        .await
    }

    pub async fn terminate(
        &self,
        id: String,
        status: &'static str,
        error: Option<String>,
    ) -> Result<()> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let changed = tx.execute(
                &format!("UPDATE runs SET status=?2,error=?3,finished_at_ms=?4 WHERE id=?1 AND {ACTIVE_RUN_PREDICATE}"),
                params![id, status, error, now_ms()],
            )?;
            if changed == 1 {
                searches::close_pending(&tx, &id, if status == "cancelled" { "cancelled" } else if status == "interrupted" { "interrupted" } else { "failed" })?;
                tool_executions::cancel_child(&tx, &id)?;
                images::close_pending(&tx, &id)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }
}
