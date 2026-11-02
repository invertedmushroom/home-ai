pub mod artifacts;
pub mod config;
mod demo;
mod discovery;
mod guests;
pub mod images;
pub mod model;
mod prompt;
mod request_policy;
mod resources;
pub mod routing;
mod run;
pub mod search;
mod sse;
pub mod store;
pub mod telemetry;
mod url_import;
pub mod worker;

use artifacts::{GeneratedArtifact, parse_changes};
use axum::{
    Extension, Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use config::Config;
use model::{Mode, Model, extract_html};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use store::{Run, Store};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tower_http::services::{ServeDir, ServeFile};
use tracing::Instrument;

const MAX_PENDING_AND_ACTIVE: usize = 16;

#[derive(Clone)]
pub struct App {
    pub store: Store,
    pub worker: worker::Worker,
    pub images: images::Images,
    config: Config,
    backends: HashMap<String, run::Backend>,
    model_profiles: HashMap<String, run::Backend>,
    admission: Arc<Semaphore>,
    cancellation: Arc<Mutex<HashMap<String, CancellationToken>>>,
    discovery_slots: Arc<Semaphore>,
}

impl App {
    pub fn new(config: Config, store: Store) -> anyhow::Result<Self> {
        config.validate()?;
        let backends = config
            .backends
            .iter()
            .filter(|backend| !backend.model.is_empty())
            .map(|backend| {
                let adapter = if backend.id == "demo" {
                    run::Backend::Demo
                } else {
                    run::Backend::Http(Model::new(backend.clone())?)
                };
                Ok((backend.id.clone(), adapter))
            })
            .collect::<anyhow::Result<_>>()?;
        let model_profiles = config
            .model_catalog
            .profiles
            .iter()
            .map(|profile| {
                let (backend, _) = config.model_catalog.resolve(
                    &routing::Selection::Profile {
                        profile_id: profile.id.clone(),
                    },
                    false,
                    false,
                    &config.backends,
                )?;
                Ok((profile.id.clone(), run::Backend::Http(Model::new(backend)?)))
            })
            .collect::<anyhow::Result<_>>()?;
        Ok(Self {
            images: images::Images::new(store.clone(), &config.image_profiles),
            worker: worker::Worker::new(store.clone(), config.worker_image.clone()),
            backends,
            model_profiles,
            store,
            config,
            admission: Arc::new(Semaphore::new(MAX_PENDING_AND_ACTIVE)),
            cancellation: Arc::default(),
            discovery_slots: Arc::new(Semaphore::new(4)),
        })
    }

    async fn execute(
        &self,
        backend: run::Backend,
        run: Run,
        prompt: String,
        mode: Mode,
    ) -> anyhow::Result<()> {
        let completion = run::complete(
            &backend,
            &self.store,
            &self.worker,
            &self.images,
            &run,
            prompt,
            mode,
        )
        .await?;
        let content = completion.text;
        let artifact = match mode {
            Mode::App => Some(GeneratedArtifact::Html(extract_html(&content).ok_or_else(|| {
                anyhow::anyhow!(
                    "No complete HTML code fence was returned; the previous revision is unchanged."
                )
            })?)),
            Mode::Files => Some(GeneratedArtifact::Changes(parse_changes(&content)?)),
            Mode::Chat => completion.workspace_artifact,
        };
        self.store.finish(run, content, artifact).await?;
        Ok(())
    }
}

struct ApiError(StatusCode, String);
impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        if let Some(error) = error.downcast_ref::<store::InvalidEdit>() {
            return Self(StatusCode::BAD_REQUEST, error.to_string());
        }
        if let Some(error) = error.downcast_ref::<store::InvalidAsset>() {
            return Self(StatusCode::BAD_REQUEST, error.to_string());
        }
        if let Some(error) = error.downcast_ref::<store::ExecutionError>() {
            return Self(error.0, error.1.into());
        }
        if let Some(conflict) = error.downcast_ref::<store::ArtifactConflict>() {
            return Self(StatusCode::CONFLICT, conflict.to_string());
        }
        tracing::error!(%error, "request failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal error; inspect the server log.".into(),
        )
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
type ApiResult<T> = Result<T, ApiError>;

pub fn router(app: App) -> Router {
    Router::new()
        .route("/api", any(api_not_found))
        .route("/api/", any(api_not_found))
        .route("/api/status", get(status))
        .route("/api/connections/{id}/models", get(model_inventory))
        .route("/api/image-profiles/{id}/inventory", get(image_inventory))
        .route("/api/session", post(session))
        .route("/api/worker", get(worker_status))
        .route("/api/workspaces", get(workspaces).post(create_workspace))
        .route(
            "/api/workspaces/{id}",
            get(workspace).delete(delete_workspace),
        )
        .route("/api/workspaces/{id}/branch", post(branch_workspace))
        .route(
            "/api/workspaces/{id}/messages/{run_id}",
            axum::routing::delete(delete_turn),
        )
        .route("/api/workspaces/{id}/runs", post(submit))
        .route(
            "/api/workspaces/{id}/assets",
            post(create_asset).layer(DefaultBodyLimit::max(8_388_608)),
        )
        .route("/api/workspaces/{id}/asset-proposals", post(propose_assets))
        .route(
            "/api/workspaces/{id}/proposals",
            post(propose_edits).layer(DefaultBodyLimit::max(8_388_608)),
        )
        .route("/api/assets/{id}", get(get_asset))
        .route(
            "/api/workspaces/{id}/image-assets",
            post(upload_image).layer(DefaultBodyLimit::max(8_388_608)),
        )
        .route(
            "/api/workspaces/{id}/project-export",
            post(export_project).layer(DefaultBodyLimit::max(8_388_608)),
        )
        .route("/api/image-assets/{id}", get(get_image_asset))
        .route("/api/image-assets/{id}/content", get(get_image_content))
        .route("/api/image-jobs/{id}", get(get_image_job))
        .route("/api/image-jobs/{id}/reconcile", post(reconcile_image_job))
        .route(
            "/api/image-jobs/{id}/accept-unknown",
            post(accept_unknown_image_outcome),
        )
        .route(
            "/api/image-jobs/{id}/confirm-stopped",
            post(confirm_image_stopped),
        )
        .route("/api/runs/{id}", get(run).delete(cancel))
        .route(
            "/api/workspaces/{id}/executions",
            get(executions).post(execute_revision),
        )
        .route(
            "/api/executions/{id}",
            get(execution).delete(cancel_execution),
        )
        .route(
            "/api/executions/{id}/proposal",
            post(propose_execution_outputs),
        )
        .route("/api/proposals/{id}/apply", post(apply_proposal))
        .route(
            "/api/proposals/{id}/reject",
            post(reject_proposal_with_reason),
        )
        .route(
            "/api/proposals/{id}",
            axum::routing::delete(reject_proposal),
        )
        .route("/api/{*path}", any(api_not_found))
        .fallback_service(
            ServeDir::new(&app.config.web_dir)
                .not_found_service(ServeFile::new(app.config.web_dir.join("index.html"))),
        )
        .layer(DefaultBodyLimit::max(65_536))
        .layer(middleware::from_fn_with_state(
            app.clone(),
            guests::require_guest,
        ))
        .layer(middleware::from_fn_with_state(
            app.clone(),
            request_policy::require_origin,
        ))
        .with_state(app)
}

async fn api_not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error":"API route not found."})),
    )
}

async fn session(State(app): State<App>) -> Json<serde_json::Value> {
    Json(json!({"guest_mode":app.config.public_origin.is_some()}))
}

async fn status(State(app): State<App>) -> Json<serde_json::Value> {
    let backends: Vec<_> = app
        .config
        .backends
        .iter()
        .filter(|backend| !backend.model.is_empty())
        .map(|backend| json!({"id":backend.id,"model":backend.model,"kind":backend.kind,"profile_required":app.config.model_catalog.connections.iter().any(|c| c.id == backend.id)}))
        .collect();
    let default = app
        .config
        .backends
        .iter()
        .find(|backend| backend.id == app.config.default_backend)
        .expect("validated default backend");
    Json(
        json!({"service":"deep","guest_mode":app.config.public_origin.is_some(),"provider":default.id,"model":default.model,"backends":backends,
        "model_catalog":app.config.model_catalog,
        "connections":app.config.backends.iter().filter(|b| b.id != "demo").map(|b| {
            let connection = app.config.model_catalog.connections.iter().find(|c| c.id == b.id);
            json!({"id":b.id,"label":connection.map(|c|c.label.as_str()).unwrap_or(&b.id),"kind":b.kind,"discovery":discovery::kind(b,connection.and_then(|c|c.discovery)) != routing::DiscoveryKind::None})
        }).collect::<Vec<_>>(),
        "model_discovery_available":app.config.public_origin.is_none(),
        "model_health":"not_probed","search_available":app.config.search.is_some(),"execution_available":app.config.worker_image.is_some(),"asset_library_available":true,"url_import_available":true,"image_input_available":true,"chat_image_input_available":true,"image_profiles":app.config.image_profiles.iter().map(|p|p.info()).collect::<Vec<_>>(),"max_active":MAX_PENDING_AND_ACTIVE,"max_pending_and_active":MAX_PENDING_AND_ACTIVE}),
    )
}

async fn model_inventory(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<Json<discovery::Inventory>> {
    let backend = app
        .config
        .backends
        .iter()
        .find(|b| b.id == id && b.kind != config::BackendKind::Demo)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Connection not found.".into()))?;
    let _slot = app
        .discovery_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Model discovery is busy. Try again shortly.".into(),
            )
        })?;
    let configured = app
        .config
        .model_catalog
        .connections
        .iter()
        .find(|c| c.id == id)
        .and_then(|c| c.discovery);
    Ok(Json(
        discovery::inspect(backend, discovery::kind(backend, configured)).await,
    ))
}

async fn image_inventory(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<Json<images::ImageInventory>> {
    let profile = app
        .config
        .image_profiles
        .iter()
        .find(|p| p.id == id && p.provider == images::ImageProvider::Wangp)
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                "WanGP image preset not found.".into(),
            )
        })?;
    let _slot = app
        .discovery_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Runtime discovery is busy. Try again shortly.".into(),
            )
        })?;
    let inventory = images::inspect_inventory(profile).await.map_err(|_| {
        ApiError(
            StatusCode::BAD_GATEWAY,
            "WanGP inventory is unavailable or incompatible. Check the runtime and retry.".into(),
        )
    })?;
    Ok(Json(inventory))
}
async fn workspaces(
    State(app): State<App>,
    Extension(guests::Guest(guest)): Extension<guests::Guest>,
) -> ApiResult<Json<Vec<store::Workspace>>> {
    Ok(Json(match guest {
        Some(guest) => app.store.guest_workspaces(guest).await?,
        None => app.store.workspaces().await?,
    }))
}
#[derive(Deserialize)]
struct NewWorkspace {
    title: String,
}
async fn create_workspace(
    State(app): State<App>,
    Extension(guests::Guest(guest)): Extension<guests::Guest>,
    Json(input): Json<NewWorkspace>,
) -> ApiResult<(StatusCode, Json<store::Workspace>)> {
    let title = input.title.trim();
    if title.is_empty() || title.chars().count() > 80 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Title must contain 1–80 characters.".into(),
        ));
    }
    Ok((
        StatusCode::CREATED,
        Json(match guest {
            Some(guest) => {
                app.store
                    .create_guest_workspace(title.into(), guest)
                    .await?
            }
            None => app.store.create_workspace(title.into()).await?,
        }),
    ))
}
async fn workspace(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<Json<store::WorkspaceDetail>> {
    Ok(Json(app.store.workspace(id).await?.ok_or_else(|| {
        ApiError(StatusCode::NOT_FOUND, "Workspace not found.".into())
    })?))
}
async fn run(State(app): State<App>, Path(id): Path<String>) -> ApiResult<Json<Run>> {
    Ok(Json(app.store.run(id).await?.ok_or_else(|| {
        ApiError(StatusCode::NOT_FOUND, "Run not found.".into())
    })?))
}
async fn delete_workspace(State(app): State<App>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    app.store.delete_workspace(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct BranchSubmission {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    up_to_run_id: Option<String>,
}

async fn branch_workspace(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(input): Json<BranchSubmission>,
) -> ApiResult<(StatusCode, Json<store::Workspace>)> {
    let branched = app
        .store
        .branch_workspace(id, input.title, input.up_to_run_id)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Workspace not found.".into()))?;
    Ok((StatusCode::CREATED, Json(branched)))
}

async fn delete_turn(
    State(app): State<App>,
    Path((workspace_id, run_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    app.store
        .delete_turn(workspace_id, run_id)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Conversation turn not found.".into()))?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevisionBase {
    #[serde(deserialize_with = "required_base_id")]
    id: Option<String>,
}

// Prevents serde from implicitly treating Option<String> as #[serde(default)],
// so {"id": null} is accepted while {} (missing "id") is rejected.
fn required_base_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}
#[derive(Deserialize)]
struct Submission {
    #[serde(default)]
    workspace_tools: bool,
    prompt: String,
    mode: Mode,
    backend: Option<String>,
    model_selection: Option<routing::Selection>,
    base_revision: Option<RevisionBase>,
    execution_result_id: Option<uuid::Uuid>,
    #[serde(default)]
    search: bool,
    #[serde(default)]
    allow_url_import: bool,
    execution_revision_id: Option<uuid::Uuid>,
    #[serde(default)]
    node_scope: store::NodeScope,
    #[serde(default)]
    input_asset_ids: Vec<uuid::Uuid>,
    image_profile_id: Option<String>,
    image_input: Option<images::ImageInputRequest>,
    #[serde(default)]
    image_attachment_ids: Vec<uuid::Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewAsset {
    name: String,
    #[serde(default)]
    media_type: Option<String>,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetProposalSubmission {
    base_revision: RevisionBase,
    asset_ids: Vec<uuid::Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditSubmission {
    id: String,
    base_revision: RevisionBase,
    changes: Vec<artifacts::FileChange>,
    #[serde(default)]
    asset_changes: Vec<artifacts::AssetChange>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageUpload {
    id: uuid::Uuid,
    name: String,
}

async fn upload_image(
    State(app): State<App>,
    Path(workspace): Path<String>,
    Query(input): Query<ImageUpload>,
    headers: axum::http::HeaderMap,
    content: Bytes,
) -> ApiResult<(StatusCode, Json<store::ImageAsset>)> {
    let media_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .unwrap_or_default();
    if !matches!(media_type, "image/png" | "image/jpeg") {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Upload a PNG or JPEG with the matching Content-Type.".into(),
        ));
    }
    let image = app
        .store
        .upload_image(
            workspace,
            input.id.to_string(),
            input.name,
            content.to_vec(),
            media_type.into(),
        )
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Workspace not found.".into()))?;
    Ok((StatusCode::CREATED, Json(image)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectExport {
    files: Vec<artifacts::File>,
    asset_bindings: Vec<artifacts::AssetBinding>,
}

async fn export_project(
    State(app): State<App>,
    Path(workspace): Path<String>,
    Json(input): Json<ProjectExport>,
) -> ApiResult<Response> {
    let bytes = app
        .store
        .export_project(workspace, input.files, input.asset_bindings)
        .await?;
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, "application/zip"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"deep-project.zip\"",
            ),
            (axum::http::header::CACHE_CONTROL, "no-store"),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response())
}

async fn create_asset(
    State(app): State<App>,
    Path(workspace): Path<String>,
    Json(input): Json<NewAsset>,
) -> ApiResult<(StatusCode, Json<store::Asset>)> {
    Ok((
        StatusCode::CREATED,
        Json(
            app.store
                .create_asset(workspace, input.name, input.media_type, input.content)
                .await?
                .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Workspace not found.".into()))?,
        ),
    ))
}

async fn get_asset(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<Json<store::AssetDetail>> {
    Ok(Json(app.store.asset(id).await?.ok_or_else(|| {
        ApiError(StatusCode::NOT_FOUND, "Asset not found.".into())
    })?))
}

async fn get_image_asset(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<Json<store::ImageAsset>> {
    Ok(Json(app.store.image_asset(id).await?.ok_or_else(|| {
        ApiError(StatusCode::NOT_FOUND, "Image not found.".into())
    })?))
}

#[derive(Deserialize)]
struct ImageDownload {
    download: Option<bool>,
}

async fn get_image_content(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(input): Query<ImageDownload>,
) -> ApiResult<Response> {
    let (asset, content) = app
        .store
        .image_content(id)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Image not found.".into()))?;
    let extension = if asset.media_type == "image/png" {
        "png"
    } else {
        "jpg"
    };
    let mut response = content.into_response();
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        asset.media_type.parse().map_err(anyhow::Error::from)?,
    );
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    let disposition = if input.download.unwrap_or(false) {
        "attachment"
    } else {
        "inline"
    };
    headers.insert(
        axum::http::header::CONTENT_DISPOSITION,
        format!("{disposition}; filename=\"image-{}.{extension}\"", asset.id)
            .parse()
            .map_err(anyhow::Error::from)?,
    );
    Ok(response)
}

async fn get_image_job(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<Json<store::ImageJob>> {
    Ok(Json(app.store.image_job(id).await?.ok_or_else(|| {
        ApiError(StatusCode::NOT_FOUND, "Image job not found.".into())
    })?))
}

async fn reconcile_image_job(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<Json<store::ImageJob>> {
    if app.store.image_job(id.clone()).await?.is_none() {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "Image job not found.".into(),
        ));
    }
    app.images.reconcile(id.clone()).await?;
    get_image_job(State(app), Path(id)).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmImageStopped {
    confirm_remote_stopped: bool,
}

async fn confirm_image_stopped(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(input): Json<ConfirmImageStopped>,
) -> ApiResult<Json<store::ImageJob>> {
    if !input.confirm_remote_stopped {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Explicit confirmation that the WanGP runtime was stopped is required.".into(),
        ));
    }
    let job = app
        .store
        .image_job(id.clone())
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Image job not found.".into()))?;
    if job.status != "unresolved" {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only an unresolved image job can be resolved this way.".into(),
        ));
    }
    app.images
        .resolve_unknown(id.clone(), images::ImageProvider::Wangp)
        .await?;
    get_image_job(State(app), Path(id)).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptUnknownImageOutcome {
    accept_unknown_outcome: bool,
}

async fn accept_unknown_image_outcome(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(input): Json<AcceptUnknownImageOutcome>,
) -> ApiResult<Json<store::ImageJob>> {
    if !input.accept_unknown_outcome {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Explicit acceptance of the unknown OpenAI image outcome is required.".into(),
        ));
    }
    app.images
        .resolve_unknown(id.clone(), images::ImageProvider::Openai)
        .await?;
    get_image_job(State(app), Path(id)).await
}

async fn propose_assets(
    State(app): State<App>,
    Path(workspace): Path<String>,
    Json(input): Json<AssetProposalSubmission>,
) -> ApiResult<Json<store::Proposal>> {
    Ok(Json(
        app.store
            .propose_assets(
                workspace,
                input.base_revision.id,
                input
                    .asset_ids
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
            )
            .await?
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Workspace not found.".into()))?,
    ))
}

async fn propose_edits(
    State(app): State<App>,
    Path(workspace): Path<String>,
    Json(input): Json<EditSubmission>,
) -> ApiResult<Json<store::Proposal>> {
    Ok(Json(
        app.store
            .propose_edits(
                workspace,
                input.id,
                input.base_revision.id,
                input.changes,
                input.asset_changes,
            )
            .await?
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Workspace not found.".into()))?,
    ))
}

async fn submit(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(input): Json<Submission>,
) -> ApiResult<(StatusCode, Json<Run>)> {
    if input.prompt.trim().is_empty() || input.prompt.len() > 32_768 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Prompt must contain 1–32768 UTF-8 bytes.".into(),
        ));
    }
    if (input.mode == Mode::Files) != input.base_revision.is_some() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "File-change mode requires an explicit base_revision object; other modes omit it."
                .into(),
        ));
    }
    if input.backend.is_some() && input.model_selection.is_some() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Choose either a backend or a model selection.".into(),
        ));
    }
    let needs_tools = input.workspace_tools
        || input.search
        || input.allow_url_import
        || input.node_scope != store::NodeScope::Off
        || input.execution_revision_id.is_some()
        || input.image_profile_id.is_some();
    let selection = input.model_selection.or_else(|| {
        input
            .backend
            .is_none()
            .then(|| {
                app.config.model_catalog.default_profile.as_ref().map(|id| {
                    routing::Selection::Profile {
                        profile_id: id.clone(),
                    }
                })
            })
            .flatten()
    });
    let resolved = selection
        .as_ref()
        .map(|selection| {
            app.config
                .model_catalog
                .resolve(
                    selection,
                    needs_tools,
                    !input.image_attachment_ids.is_empty(),
                    &app.config.backends,
                )
                .map_err(|error| ApiError(StatusCode::BAD_REQUEST, error.to_string()))
        })
        .transpose()?;
    let backend_id = resolved
        .as_ref()
        .map(|(backend, _)| backend.id.as_str())
        .unwrap_or_else(|| {
            input
                .backend
                .as_deref()
                .unwrap_or(&app.config.default_backend)
        });
    let backend = resolved
        .as_ref()
        .map(|(backend, _)| backend)
        .or_else(|| {
            app.config
                .backends
                .iter()
                .find(|backend| backend.id == backend_id)
        })
        .ok_or_else(|| {
            ApiError(
                StatusCode::BAD_REQUEST,
                "Select a configured backend.".into(),
            )
        })?;
    if backend.model.is_empty()
        || (resolved.is_none()
            && app
                .config
                .model_catalog
                .connections
                .iter()
                .any(|c| c.id == backend.id))
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Select a configured model profile for this connection.".into(),
        ));
    }
    if !input.image_attachment_ids.is_empty() && resolved.is_none() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Select a model profile with vision enabled before attaching images to chat.".into(),
        ));
    }
    let adapter = if let Some((_, route)) = &resolved {
        app.model_profiles
            .get(&route.profile_id)
            .expect("configured model profile adapter")
            .clone()
    } else {
        app.backends
            .get(backend_id)
            .expect("configured backend adapter")
            .clone()
    };
    let image_profile = input
        .image_profile_id
        .as_ref()
        .map(|id| {
            app.config
                .image_profiles
                .iter()
                .find(|p| &p.id == id)
                .cloned()
                .ok_or_else(|| {
                    ApiError(
                        StatusCode::BAD_REQUEST,
                        "Select a configured image profile.".into(),
                    )
                })
        })
        .transpose()?;
    if (image_profile.is_some() || input.workspace_tools) && backend_id == "demo" {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Native tools require a real model backend.".into(),
        ));
    }
    if input.workspace_tools && input.mode != Mode::Chat {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Workspace tools require Conversation mode.".into(),
        ));
    }
    if input.allow_url_import && backend_id == "demo" {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "URL import requires a real model backend.".into(),
        ));
    }
    let search = if input.search {
        if backend_id == "demo" {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "Select a model backend for native search tools.".into(),
            ));
        }
        Some(app.config.search.clone().ok_or_else(|| {
            ApiError(
                StatusCode::BAD_REQUEST,
                "Search is not configured on this server.".into(),
            )
        })?)
    } else {
        None
    };
    let wants_node =
        input.node_scope != store::NodeScope::Off || input.execution_revision_id.is_some();
    if wants_node && (backend_id == "demo" || app.config.worker_image.is_none()) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Script execution requires a real model backend and a configured Node worker.".into(),
        ));
    }
    if app.store.workspace(id.clone()).await?.is_none() {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "Workspace not found.".into(),
        ));
    }
    let admission = app.admission.clone().try_acquire_owned().map_err(|_| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Local queue is full.".into(),
        )
    })?;
    let run = app
        .store
        .begin(
            id,
            input.prompt.clone(),
            backend.id.clone(),
            backend.model.clone(),
            store::RunOptions {
                model_route: resolved.as_ref().map(|(_, route)| route.clone()),
                workspace_tools: input.workspace_tools,
                mode: input.mode,
                base_revision_id: input.base_revision.and_then(|base| base.id),
                execution_result_id: input.execution_result_id.map(|id| id.to_string()),
                search,
                allow_url_import: input.allow_url_import,
                execution_revision_id: input.execution_revision_id.map(|id| id.to_string()),
                node_scope: input.node_scope,
                input_asset_ids: input
                    .input_asset_ids
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                image_profile,
                image_input: input.image_input,
                image_attachment_ids: input
                    .image_attachment_ids
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
            },
        )
        .await?
        .ok_or_else(|| {
            ApiError(
                StatusCode::CONFLICT,
                "This workspace already has an active run.".into(),
            )
        })?;
    let token = CancellationToken::new();
    app.cancellation
        .lock()
        .expect("cancellation lock")
        .insert(run.id.clone(), token.clone());
    let background_run = run.clone();
    let span = tracing::info_span!(target: telemetry::TARGET, parent: None, "run",
        run_id=%run.id, backend=%run.provider, outcome=tracing::field::Empty,
        otel.status_code=tracing::field::Empty);
    if let Some(trace_id) = telemetry::trace_id(&span) {
        tracing::info!(parent: &span, %trace_id, "run admitted");
    } else {
        tracing::info!(parent: &span, "run admitted");
    }
    tokio::spawn(async move {
        let _admission = admission;
        let future = async {
            app.execute(adapter, background_run.clone(), input.prompt, input.mode)
                .await
        };
        tokio::select! {
            biased;
            _ = token.cancelled() => { tracing::info!("run cancellation received"); },
            result = future => {
                if let Err(error) = result {
                    tracing::warn!(%error, "run failed");
                    if let Err(db_error) = app.store.terminate(background_run.id.clone(), "failed", Some(error.to_string())).await {
                        tracing::error!(%db_error, "could not record run failure");
                    }
                }
            }
        }
        if background_run.node_scope != store::NodeScope::Off
            && let Err(error) = app.worker.cancel_run(background_run.id.clone()).await
        {
            tracing::error!(%error, "linked execution cleanup remains pending");
        }
        if background_run.image_profile.is_some()
            && let Err(error) = app.images.cancel_run(background_run.id.clone()).await
        {
            tracing::error!(%error,"linked image reconciliation remains pending");
        }
        if let Ok(Some(settled)) = app.store.run(background_run.id.clone()).await {
            telemetry::run_finished(&settled);
            tracing::Span::current().record("outcome", settled.status.as_str());
            if matches!(settled.status.as_str(), "failed" | "interrupted") {
                tracing::Span::current().record("otel.status_code", "ERROR");
            }
            tracing::info!(status=%settled.status, "run task finished");
        }
        app.cancellation
            .lock()
            .expect("cancellation lock")
            .remove(&background_run.id);
    }.instrument(span));
    Ok((StatusCode::ACCEPTED, Json(run)))
}

async fn cancel(State(app): State<App>, Path(id): Path<String>) -> ApiResult<Json<Run>> {
    if app.store.run(id.clone()).await?.is_none() {
        return Err(ApiError(StatusCode::NOT_FOUND, "Run not found.".into()));
    }
    // Commit the terminal state before signalling: a late completion cannot publish a revision.
    app.store
        .terminate(
            id.clone(),
            "cancelled",
            Some("Local request cancelled; upstream GPU interruption is not confirmed.".into()),
        )
        .await?;
    if let Some(token) = app.cancellation.lock().expect("cancellation lock").get(&id) {
        token.cancel()
    }
    Ok(Json(app.store.run(id).await?.expect("run retained")))
}

async fn apply_proposal(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<Json<store::Revision>> {
    Ok(Json(app.store.apply_proposal(id).await?.ok_or_else(
        || ApiError(StatusCode::NOT_FOUND, "Proposal not found.".into()),
    )?))
}

async fn reject_proposal(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<Json<store::Proposal>> {
    Ok(Json(
        app.store
            .reject_proposal(id, None)
            .await?
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Proposal not found.".into()))?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RejectionInput {
    reason: Option<String>,
}

async fn reject_proposal_with_reason(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(input): Json<RejectionInput>,
) -> ApiResult<Json<store::Proposal>> {
    if input
        .reason
        .as_ref()
        .is_some_and(|reason| reason.trim().len() > 4096)
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Rejection reason must fit within 4096 UTF-8 bytes.".into(),
        ));
    }
    Ok(Json(
        app.store
            .reject_proposal(id, input.reason)
            .await?
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Proposal not found.".into()))?,
    ))
}

async fn worker_status(State(app): State<App>) -> Json<worker::WorkerStatus> {
    Json(app.worker.status().await)
}

async fn executions(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<store::Execution>>> {
    if app.store.workspace(id.clone()).await?.is_none() {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "Workspace not found.".into(),
        ));
    }
    Ok(Json(app.store.executions(id).await?))
}

async fn execute_revision(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(input): Json<store::ExecutionInput>,
) -> ApiResult<(StatusCode, Json<store::Execution>)> {
    Ok((
        StatusCode::ACCEPTED,
        Json(app.worker.submit(id, input).await?),
    ))
}

async fn execution(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<Json<store::Execution>> {
    Ok(Json(app.store.execution(id).await?.ok_or_else(|| {
        ApiError(StatusCode::NOT_FOUND, "Execution not found.".into())
    })?))
}

async fn cancel_execution(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<Json<store::Execution>> {
    Ok(Json(app.worker.cancel(id).await?.ok_or_else(|| {
        ApiError(StatusCode::NOT_FOUND, "Execution not found.".into())
    })?))
}

async fn propose_execution_outputs(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<Json<store::Proposal>> {
    Ok(Json(
        app.store
            .propose_execution_outputs(id)
            .await?
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Execution not found.".into()))?,
    ))
}
