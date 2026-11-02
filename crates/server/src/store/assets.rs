use super::*;
use crate::artifacts::{MAX_FILES, MAX_PROJECT_BYTES, valid_path};
use axum::http::StatusCode;
use serde::Deserialize;
use std::collections::HashSet;

pub(super) const ASSET_COLUMNS: &str =
    "id,workspace_id,name,media_type,bytes,origin,execution_id,created_at_ms,source";

#[derive(Debug)]
pub struct InvalidAsset(pub String);
impl std::fmt::Display for InvalidAsset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidAsset {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub media_type: String,
    pub bytes: usize,
    pub origin: String,
    pub execution_id: Option<String>,
    pub created_at_ms: i64,
    #[serde(default)]
    pub source: Option<ImportSource>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportSource {
    pub url: String,
    pub final_url: String,
    pub sha256: String,
    pub run_id: String,
    pub call_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetDetail {
    #[serde(flatten)]
    pub asset: Asset,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRef {
    pub id: String,
    pub name: String,
    pub media_type: String,
    pub bytes: usize,
    pub path: String,
}

const ALLOWED_MEDIA_TYPES: &[&str] = &[
    "text/csv",
    "text/plain",
    "text/tab-separated-values",
    "application/json",
    "image/svg+xml",
    "text/html",
    "text/css",
    "text/javascript",
    "text/markdown",
    "application/xml",
    "text/xml",
];

pub(super) fn infer_media_type(
    name: &str,
    requested: Option<&str>,
) -> std::result::Result<String, InvalidAsset> {
    if let Some(raw) = requested {
        let base = raw
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if !base.is_empty() {
            if ALLOWED_MEDIA_TYPES.contains(&base.as_str()) {
                return Ok(base);
            }
            return Err(InvalidAsset(
                "Unsupported asset media type; upload UTF-8 text, CSV, JSON or SVG.".into(),
            ));
        }
    }
    let lower = name.to_ascii_lowercase();
    let inferred = if lower.ends_with(".csv") {
        "text/csv"
    } else if lower.ends_with(".tsv") {
        "text/tab-separated-values"
    } else if lower.ends_with(".json") {
        "application/json"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else if lower.ends_with(".html") || lower.ends_with(".htm") {
        "text/html"
    } else if lower.ends_with(".css") {
        "text/css"
    } else if lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".cjs") {
        "text/javascript"
    } else if lower.ends_with(".md") {
        "text/markdown"
    } else if lower.ends_with(".xml") {
        "application/xml"
    } else {
        "text/plain"
    };
    Ok(inferred.into())
}

pub(super) fn read_asset(row: &rusqlite::Row<'_>) -> rusqlite::Result<Asset> {
    Ok(Asset {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        name: row.get(2)?,
        media_type: row.get(3)?,
        bytes: row.get::<_, i64>(4)?.max(0) as usize,
        origin: row.get(5)?,
        execution_id: row.get(6)?,
        created_at_ms: row.get(7)?,
        source: read_optional_json(row, 8)?,
    })
}

pub(super) fn workspace_assets(c: &Connection, workspace: &str) -> Result<Vec<Asset>> {
    Ok(c.prepare(&format!(
        "SELECT {ASSET_COLUMNS} FROM assets WHERE workspace_id=?1 ORDER BY rowid"
    ))?
    .query_map([workspace], read_asset)?
    .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(super) fn execution_assets(c: &Connection, execution_id: &str) -> Result<Vec<Asset>> {
    Ok(c.prepare(&format!(
        "SELECT {ASSET_COLUMNS} FROM assets WHERE execution_id=?1 ORDER BY rowid"
    ))?
    .query_map([execution_id], read_asset)?
    .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(super) fn imports_for_run(c: &Connection, run_id: &str) -> Result<Vec<Asset>> {
    Ok(c.prepare(&format!("SELECT {ASSET_COLUMNS} FROM assets WHERE origin='import' AND json_extract(source,'$.run_id')=?1 ORDER BY rowid"))?
        .query_map([run_id], read_asset)?.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn reserved_worker_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower == "__compute.cjs"
        || lower == "output"
        || lower.starts_with("output/")
        || lower == "inputs"
        || lower.starts_with("inputs/")
}

pub(super) fn resolve_input_assets(
    c: &Connection,
    workspace: &str,
    ids: &[String],
) -> Result<Vec<(AssetRef, String)>> {
    if ids.len() >= MAX_FILES {
        return Err(
            InvalidAsset("At most 63 input assets can be selected per request.".into()).into(),
        );
    }
    let mut seen_ids = HashSet::new();
    let mut raw = Vec::with_capacity(ids.len());
    for id in ids {
        Uuid::parse_str(id).map_err(|_| InvalidAsset("Asset ID must be a UUID.".into()))?;
        if !seen_ids.insert(id.to_ascii_lowercase()) {
            return Err(InvalidAsset("Duplicate input asset IDs are not permitted.".into()).into());
        }
        let row: Option<(Asset, String)> = c
            .query_row(
                &format!(
                    "SELECT {ASSET_COLUMNS},content FROM assets WHERE id=?1 AND workspace_id=?2"
                ),
                params![id, workspace],
                |r| Ok((read_asset(r)?, r.get(9)?)),
            )
            .optional()?;
        let (asset, content) = row.ok_or(ExecutionError(
            StatusCode::NOT_FOUND,
            "Selected computation asset was not found in this workspace.",
        ))?;
        raw.push((asset, content));
    }

    let mut used_paths = HashSet::new();
    used_paths.insert("__compute.cjs".to_string());
    let mut resolved = Vec::with_capacity(raw.len());
    for (index, (asset, content)) in raw.into_iter().enumerate() {
        let lower = asset.name.to_ascii_lowercase();
        let conflict = reserved_worker_path(&asset.name)
            || !used_paths.insert(lower.clone())
            || used_paths.iter().any(|existing| {
                existing != &lower
                    && (existing.starts_with(&format!("{lower}/"))
                        || lower.starts_with(&format!("{existing}/")))
            });
        let worker_path = if conflict {
            let disambiguated = format!("inputs/{}-{}", index + 1, asset.name);
            used_paths.insert(disambiguated.to_ascii_lowercase());
            disambiguated
        } else {
            asset.name.clone()
        };
        if !valid_path(&worker_path) {
            return Err(
                InvalidAsset("Input asset worker path exceeds portable limits.".into()).into(),
            );
        }
        resolved.push((
            AssetRef {
                id: asset.id,
                name: asset.name,
                media_type: asset.media_type,
                bytes: asset.bytes,
                path: worker_path,
            },
            content,
        ));
    }
    let total_bytes: usize = resolved
        .iter()
        .map(|(item, content)| item.path.len() + content.len())
        .sum();
    if total_bytes >= MAX_PROJECT_BYTES {
        return Err(
            InvalidAsset("Selected input assets exceed the 1 MiB worker budget.".into()).into(),
        );
    }
    Ok(resolved)
}

impl Store {
    pub(crate) async fn save_import(
        &self,
        run_id: String,
        call_id: String,
        name: String,
        download: crate::url_import::Download,
        deadline: std::time::Instant,
    ) -> Result<Asset> {
        ensure!(
            crate::url_import::valid_name(&name),
            "Choose a portable file basename for import."
        );
        self.call(move |c| {
            let tx = c.transaction()?;
            ensure!(std::time::Instant::now() < deadline, "URL import deadline expired before saving.");
            let workspace: String = tx.query_row("SELECT workspace_id FROM runs WHERE id=?1 AND status='running' AND import_enabled=1", [&run_id], |r| r.get(0))
                .optional()?.ok_or_else(|| anyhow::anyhow!("URL import permission is no longer active."))?;
            let existing = imports_for_run(&tx, &run_id)?;
            ensure!(existing.len() < 2 && !existing.iter().any(|a| a.source.as_ref().is_some_and(|s| s.call_id == call_id)), "URL import budget or call identity is unavailable.");
            let id = Uuid::new_v4().to_string();
            let names = workspace_assets(&tx, &workspace)?;
            let mut name = name;
            if names.iter().any(|a| a.name.eq_ignore_ascii_case(&name) || a.name.to_ascii_lowercase().starts_with(&format!("{}/",name.to_ascii_lowercase()))) {
                let (stem, ext) = name.rsplit_once('.').map_or((name.as_str(), String::new()), |(s,e)| (s,format!(".{e}")));
                name = format!("{stem}-{id}{ext}");
            }
            ensure!(name.len() + download.content.len() < crate::artifacts::MAX_PROJECT_BYTES, "Imported file and name must fit within the 1 MiB worker budget.");
            let source = ImportSource { url: download.url, final_url: download.final_url, sha256: download.sha256, run_id, call_id };
            let asset = Asset {id,workspace_id: workspace,name,media_type: download.media_type, bytes:download.content.len(),origin:"import".into(),execution_id:None,created_at_ms:now_ms(),source:Some(source)};
            tx.execute("INSERT INTO assets(id,workspace_id,name,media_type,bytes,content,origin,created_at_ms,source) VALUES (?1,?2,?3,?4,?5,?6,'import',?7,?8)", params![asset.id,asset.workspace_id,asset.name,asset.media_type,asset.bytes as i64,download.content,asset.created_at_ms,serde_json::to_string(&asset.source)?])?;
            ensure!(std::time::Instant::now() < deadline, "URL import deadline expired before saving.");
            tx.commit()?;
            Ok(asset)
        }).await
    }

    pub async fn create_asset(
        &self,
        workspace: String,
        name: String,
        media_type: Option<String>,
        content: String,
    ) -> Result<Option<Asset>> {
        if !valid_path(&name) {
            return Err(InvalidAsset(
                "Asset name must be a portable relative path (1–240 ASCII characters).".into(),
            )
            .into());
        }
        if content.len() > MAX_PROJECT_BYTES || name.len() + content.len() > MAX_PROJECT_BYTES {
            return Err(
                InvalidAsset("Asset must fit within the 1 MiB UTF-8 text budget.".into()).into(),
            );
        }
        let media_type = infer_media_type(&name, media_type.as_deref())?;
        self.call(move |c| {
            let exists: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=?1)",
                [&workspace],
                |r| r.get(0),
            )?;
            if !exists {
                return Ok(None);
            }
            let asset = Asset {
                id: Uuid::new_v4().to_string(),
                workspace_id: workspace,
                name,
                media_type,
                bytes: content.len(),
                origin: "upload".into(),
                execution_id: None,
                created_at_ms: now_ms(),
                source: None,
            };
            c.execute(
                "INSERT INTO assets(id,workspace_id,name,media_type,bytes,content,origin,execution_id,created_at_ms)
                 VALUES (?1,?2,?3,?4,?5,?6,'upload',NULL,?7)",
                params![
                    asset.id,
                    asset.workspace_id,
                    asset.name,
                    asset.media_type,
                    asset.bytes as i64,
                    content,
                    asset.created_at_ms
                ],
            )?;
            Ok(Some(asset))
        })
        .await
    }

    pub async fn asset(&self, id: String) -> Result<Option<AssetDetail>> {
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {ASSET_COLUMNS},content FROM assets WHERE id=?1"),
                [id],
                |r| {
                    Ok(AssetDetail {
                        asset: read_asset(r)?,
                        content: r.get(9)?,
                    })
                },
            )
            .optional()?)
        })
        .await
    }

    pub async fn propose_assets(
        &self,
        workspace: String,
        base: Option<String>,
        asset_ids: Vec<String>,
    ) -> Result<Option<Proposal>> {
        if asset_ids.is_empty() || asset_ids.len() > MAX_FILES {
            return Err(InvalidAsset(
                "Select 1–64 workspace assets to review for the project.".into(),
            )
            .into());
        }
        let mut seen = HashSet::new();
        for id in &asset_ids {
            Uuid::parse_str(id).map_err(|_| InvalidAsset("Asset ID must be a UUID.".into()))?;
            if !seen.insert(id.to_ascii_lowercase()) {
                return Err(InvalidAsset(
                    "Duplicate asset IDs in selection are not permitted.".into(),
                )
                .into());
            }
        }
        let encoded_ids = serde_json::to_string(&asset_ids)?;
        self.call(move |c| {
            let tx = c.transaction()?;
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=?1)",
                [&workspace],
                |r| r.get(0),
            )?;
            if !exists {
                return Ok(None);
            }
            if let Some(existing) = tx
                .query_row(
                    &format!(
                        "SELECT {PROPOSAL_COLUMNS} FROM proposals WHERE workspace_id=?1 AND asset_ids=?2 AND base_revision_id IS ?3 ORDER BY rowid DESC LIMIT 1"
                    ),
                    params![workspace, encoded_ids, base],
                    read_proposal,
                )
                .optional()?
            {
                return Ok(Some(existing));
            }
            if head(&tx, &workspace)?.as_ref().map(|r| &r.id) != base.as_ref() {
                return Err(ArtifactConflict::StaleBase.into());
            }
            let base_snapshot = base_files(&tx, &workspace, base.as_deref())?;
            let mut target_paths = HashSet::new();
            let mut changes = Vec::new();
            for id in &asset_ids {
                let row: Option<(String, String)> = tx
                    .query_row(
                        "SELECT name,content FROM assets WHERE id=?1 AND workspace_id=?2",
                        params![id, workspace],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                let (name, content) = row.ok_or({
                    ExecutionError(
                        StatusCode::NOT_FOUND,
                        "Selected asset was not found in this workspace.",
                    )
                })?;
                if !target_paths.insert(name.to_ascii_lowercase()) {
                    return Err(InvalidAsset(
                        "Selected assets contain colliding project file paths.".into(),
                    )
                    .into());
                }
                if !base_snapshot
                    .iter()
                    .any(|file| file.path == name && file.content == content)
                {
                    changes.push(FileChange::Write { path: name, content });
                }
            }
            if changes.is_empty() {
                return Err(InvalidAsset(
                    "Selected assets match the current project files.".into(),
                )
                .into());
            }
            let (files,bindings)=apply_project_changes(&base_snapshot,
                &project_assets::base_bindings(&tx,&workspace,base.as_deref())?,&changes,&[])
                .map_err(|error| InvalidAsset(error.to_string()))?;
            project_assets::resolve(&tx,&workspace,&files,&bindings)
                .map_err(|error|InvalidAsset(error.to_string()))?;
            let proposal_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO proposals(id,workspace_id,asset_ids,base_revision_id,status,changes)
                 VALUES (?1,?2,?3,?4,'pending',?5)",
                params![
                    proposal_id,
                    workspace,
                    encoded_ids,
                    base,
                    serde_json::to_string(&changes)?
                ],
            )?;
            let proposal = tx.query_row(
                &format!("SELECT {PROPOSAL_COLUMNS} FROM proposals WHERE id=?1"),
                [proposal_id],
                read_proposal,
            )?;
            tx.commit()?;
            Ok(Some(proposal))
        })
        .await
    }
}

#[cfg(test)]
mod import_tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn download() -> crate::url_import::Download {
        crate::url_import::Download {
            url: "https://example.com/age.csv".into(),
            final_url: "https://cdn.example.com/age.csv".into(),
            media_type: "text/csv".into(),
            content: "Age\n42\n".into(),
            sha256: "a".repeat(64),
        }
    }
    async fn begin(store: &Store, workspace: &str, permitted: bool) -> Run {
        let run = store
            .begin(
                workspace.into(),
                "Analyze URL".into(),
                "compatible".into(),
                "fixture".into(),
                RunOptions {
                    allow_url_import: permitted,
                    node_scope: NodeScope::Compute,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
        store.mark_running(run.id.clone()).await.unwrap();
        run
    }
    async fn save(store: &Store, run: &Run, call: &str, deadline: Instant) -> Result<Asset> {
        store
            .save_import(
                run.id.clone(),
                call.into(),
                "age.csv".into(),
                download(),
                deadline,
            )
            .await
    }

    #[tokio::test]
    async fn imported_inputs_are_bound_to_the_creating_run_without_changing_frozen_selection() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let workspace = store.create_workspace("Imports".into()).await.unwrap();
        let foreign = store.create_workspace("Other".into()).await.unwrap();
        let hidden = store
            .create_asset(
                workspace.id.clone(),
                "age.csv".into(),
                None,
                "Age\n999\n".into(),
            )
            .await
            .unwrap()
            .unwrap();
        let foreign_asset = store
            .create_asset(foreign.id, "foreign.csv".into(), None, "Age\n99\n".into())
            .await
            .unwrap()
            .unwrap();
        let run = begin(&store, &workspace.id, true).await;
        let asset = save(
            &store,
            &run,
            "import_1",
            Instant::now() + Duration::from_secs(15),
        )
        .await
        .unwrap();
        assert_ne!(asset.name, hidden.name);
        assert!(asset.name.ends_with(".csv"));
        let saved = store.run(run.id.clone()).await.unwrap().unwrap();
        assert!(saved.input_assets.is_empty());
        assert_eq!(saved.imports, vec![asset.clone()]);
        assert_eq!(
            store
                .asset(asset.id.clone())
                .await
                .unwrap()
                .unwrap()
                .content,
            "Age\n42\n"
        );
        for denied in [&hidden.id, &foreign_asset.id] {
            assert!(
                store
                    .plan_compute_execution(
                        run.id.clone(),
                        "denied".into(),
                        "console.log(1)".into(),
                        vec![denied.clone()]
                    )
                    .await
                    .is_err()
            );
        }
        let intent = store
            .plan_compute_execution(
                run.id.clone(),
                "compute_1".into(),
                "console.log(1)".into(),
                vec![asset.id.clone()],
            )
            .await
            .unwrap();
        assert_eq!(intent.input_assets[0].path, asset.name);
        assert!(
            save(
                &store,
                &run,
                "import_1",
                Instant::now() + Duration::from_secs(15)
            )
            .await
            .is_err()
        );
        save(
            &store,
            &run,
            "import_2",
            Instant::now() + Duration::from_secs(15),
        )
        .await
        .unwrap();
        assert!(
            save(
                &store,
                &run,
                "import_3",
                Instant::now() + Duration::from_secs(15)
            )
            .await
            .is_err()
        );
        store.terminate(run.id, "cancelled", None).await.unwrap();
        let later = begin(&store, &workspace.id, true).await;
        assert!(
            store
                .plan_compute_execution(
                    later.id,
                    "cross_run".into(),
                    "console.log(1)".into(),
                    vec![asset.id]
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn unpermitted_cancelled_and_expired_imports_publish_no_asset() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let workspace = store.create_workspace("Imports".into()).await.unwrap();
        let run = begin(&store, &workspace.id, false).await;
        assert!(
            save(
                &store,
                &run,
                "off",
                Instant::now() + Duration::from_secs(15)
            )
            .await
            .is_err()
        );
        store
            .terminate(run.id.clone(), "cancelled", None)
            .await
            .unwrap();
        let run = begin(&store, &workspace.id, true).await;
        assert!(
            save(
                &store,
                &run,
                "expired",
                Instant::now() - Duration::from_secs(1)
            )
            .await
            .is_err()
        );
        store
            .terminate(run.id.clone(), "cancelled", None)
            .await
            .unwrap();
        assert!(
            save(
                &store,
                &run,
                "late",
                Instant::now() + Duration::from_secs(15)
            )
            .await
            .is_err()
        );
        assert!(
            store
                .workspace(workspace.id)
                .await
                .unwrap()
                .unwrap()
                .assets
                .is_empty()
        );
    }

    #[tokio::test]
    async fn detached_database_work_checks_the_deadline_after_waiting_for_the_lock() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let workspace = store.create_workspace("Imports".into()).await.unwrap();
        let run = begin(&store, &workspace.id, true).await;
        let (locked, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let blocked = store.clone();
        let hold = tokio::spawn(async move {
            blocked
                .call(move |_| {
                    locked.send(()).unwrap();
                    wait.recv().unwrap();
                    Ok(())
                })
                .await
                .unwrap()
        });
        ready.await.unwrap();
        let saver = store.clone();
        let future = tokio::spawn(async move {
            save(
                &saver,
                &run,
                "delayed",
                Instant::now() + Duration::from_millis(30),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(60)).await;
        // Dropping an async caller leaves spawn_blocking work queued.
        future.abort();
        release.send(()).unwrap();
        hold.await.unwrap();
        assert!(
            store
                .workspace(workspace.id)
                .await
                .unwrap()
                .unwrap()
                .assets
                .is_empty()
        );
    }

    #[tokio::test]
    async fn version_seventeen_upgrade_preserves_assets_and_rolls_back_before_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("imports.sqlite3");
        let store = Store::open(&path).unwrap();
        let workspace = store.create_workspace("Preserve".into()).await.unwrap();
        let asset = store
            .create_asset(
                workspace.id.clone(),
                "age.csv".into(),
                None,
                "Age\r\n42\r\n".into(),
            )
            .await
            .unwrap()
            .unwrap();
        let run = begin(&store, &workspace.id, false).await;
        drop(store);
        let c = Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE assets_v17(id TEXT PRIMARY KEY,workspace_id TEXT NOT NULL REFERENCES workspaces(id),name TEXT NOT NULL,media_type TEXT NOT NULL,bytes INTEGER NOT NULL CHECK(bytes>=0),content TEXT NOT NULL,origin TEXT NOT NULL CHECK(origin IN ('upload','execution')),execution_id TEXT REFERENCES executions(id),created_at_ms INTEGER NOT NULL,CHECK((origin='execution' AND execution_id IS NOT NULL) OR (origin='upload' AND execution_id IS NULL)));
            INSERT INTO assets_v17 SELECT id,workspace_id,name,media_type,bytes,content,origin,execution_id,created_at_ms FROM assets;
            DROP TABLE assets; ALTER TABLE assets_v17 RENAME TO assets;
            ALTER TABLE runs DROP COLUMN image_attachments; ALTER TABLE runs DROP COLUMN model_route; ALTER TABLE runs DROP COLUMN workspace_tools; ALTER TABLE runs DROP COLUMN import_enabled; DROP TABLE model_calls; PRAGMA user_version=17; CREATE TABLE assets_v18(sentinel TEXT); INSERT INTO assets_v18 VALUES('keep');").unwrap();
        drop(c);
        assert!(Store::open(&path).is_err());
        let c = Connection::open(&path).unwrap();
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            17
        );
        assert_eq!(
            c.query_row("SELECT status FROM runs WHERE id=?1", [&run.id], |r| r
                .get::<_, String>(
                0
            ))
            .unwrap(),
            "running"
        );
        assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('runs') WHERE name='import_enabled')",[],|r|r.get::<_,bool>(0)).unwrap());
        c.execute_batch("DROP TABLE assets_v18;").unwrap();
        drop(c);
        let store = Store::open(&path).unwrap();
        let detail = store.workspace(workspace.id).await.unwrap().unwrap();
        assert_eq!(detail.assets, vec![asset]);
        assert!(!detail.runs[0].import_enabled);
        assert!(detail.runs[0].imports.is_empty());
        assert_eq!(detail.runs[0].status, "interrupted");
        assert_eq!(
            store
                .asset(detail.assets[0].id.clone())
                .await
                .unwrap()
                .unwrap()
                .content,
            "Age\r\n42\r\n"
        );
    }
}
