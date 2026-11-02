use super::*;
use crate::artifacts::{AssetBinding, MAX_PROJECT_ASSET_BYTES, validate_project};
use std::io::{Cursor, Write};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectAsset {
    pub path: String,
    pub asset_id: String,
    pub media_type: String,
    pub bytes: u32,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
}

pub(super) fn base_bindings(
    c: &Connection,
    workspace: &str,
    revision: Option<&str>,
) -> Result<Vec<AssetBinding>> {
    match revision {
        None => Ok(Vec::new()),
        Some(revision) => c
            .query_row(
                "SELECT asset_bindings FROM revisions WHERE workspace_id=?1 AND id=?2",
                params![workspace, revision],
                |r| read_json(r, 0),
            )
            .optional()?
            .ok_or_else(|| ArtifactConflict::StaleBase.into()),
    }
}

pub(super) fn resolve(
    c: &Connection,
    workspace: &str,
    files: &[File],
    bindings: &[AssetBinding],
) -> Result<Vec<ProjectAsset>> {
    validate_project(files, bindings)?;
    let mut total = 0u64;
    let mut assets = Vec::new();
    for binding in bindings {
        let image: ImageAsset = c
            .query_row(
                &format!(
                    "SELECT {} FROM image_assets WHERE workspace_id=?1 AND id=?2",
                    super::images::ASSET_COLUMNS
                ),
                params![workspace, binding.asset_id],
                super::images::read_asset,
            )
            .optional()?
            .ok_or_else(|| InvalidEdit("A bound image was not found in this workspace.".into()))?;
        let path = binding.path.to_ascii_lowercase();
        ensure!(
            if image.media_type == "image/png" {
                path.ends_with(".png")
            } else {
                path.ends_with(".jpg") || path.ends_with(".jpeg")
            },
            "The project asset path extension must match its PNG/JPEG media type."
        );
        total += u64::from(image.bytes);
        ensure!(
            total <= MAX_PROJECT_ASSET_BYTES,
            "Bound project assets exceed the 32 MiB budget."
        );
        assets.push(ProjectAsset {
            path: binding.path.clone(),
            asset_id: image.id,
            media_type: image.media_type,
            bytes: image.bytes,
            width: image.width,
            height: image.height,
            sha256: image.sha256,
        });
    }
    Ok(assets)
}

impl Store {
    pub async fn project_assets_at(
        &self,
        workspace: String,
        revision: Option<String>,
    ) -> Result<Vec<ProjectAsset>> {
        self.call(move |c| {
            let files = base_files(c, &workspace, revision.as_deref())?;
            let bindings = base_bindings(c, &workspace, revision.as_deref())?;
            resolve(c, &workspace, &files, &bindings)
        })
        .await
    }

    pub async fn export_project(
        &self,
        workspace: String,
        files: Vec<File>,
        bindings: Vec<AssetBinding>,
    ) -> Result<Vec<u8>> {
        let entries = self
            .call(move |c| {
                let exists: bool = c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=?1)",
                    [&workspace],
                    |r| r.get(0),
                )?;
                if !exists {
                    return Err(ExecutionError(
                        axum::http::StatusCode::NOT_FOUND,
                        "Workspace not found.",
                    )
                    .into());
                }
                resolve(c, &workspace, &files, &bindings)
                    .map_err(|error| InvalidEdit(error.to_string()))?;
                let mut entries: Vec<(String, Vec<u8>)> = files
                    .into_iter()
                    .map(|file| (file.path, file.content.into_bytes()))
                    .collect();
                for binding in bindings {
                    let bytes = c.query_row(
                        "SELECT content FROM image_assets WHERE id=?1 AND workspace_id=?2",
                        params![binding.asset_id, workspace],
                        |r| r.get::<_, Vec<u8>>(0),
                    )?;
                    entries.push((binding.path, bytes));
                }
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                Ok(entries)
            })
            .await?;
        tokio::task::spawn_blocking(move || {
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (path, bytes) in entries {
                zip.start_file(path, options)?;
                zip.write_all(&bytes)?;
            }
            Ok(zip.finish()?.into_inner())
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::{ImageProfile, decode_image};
    use sha2::{Digest, Sha256};

    fn downgrade_to_thirteen(path: &std::path::Path, conflict: bool) {
        let c = Connection::open(path).unwrap();
        c.execute_batch(
            "ALTER TABLE runs DROP COLUMN image_attachments; ALTER TABLE runs DROP COLUMN model_route; ALTER TABLE runs DROP COLUMN workspace_tools;
             ALTER TABLE revisions DROP COLUMN asset_bindings;
             ALTER TABLE proposals DROP COLUMN asset_changes;
             ALTER TABLE image_assets RENAME TO image_assets_new;
             CREATE TABLE image_assets (
                id TEXT PRIMARY KEY,workspace_id TEXT NOT NULL REFERENCES workspaces(id),
                image_job_id TEXT NOT NULL UNIQUE REFERENCES image_jobs(id),
                media_type TEXT NOT NULL,bytes INTEGER NOT NULL,width INTEGER NOT NULL,height INTEGER NOT NULL,
                sha256 TEXT NOT NULL,content BLOB NOT NULL,created_at_ms INTEGER NOT NULL
             );
             INSERT INTO image_assets(rowid,id,workspace_id,image_job_id,media_type,bytes,width,height,sha256,content,created_at_ms)
                SELECT rowid,id,workspace_id,image_job_id,media_type,bytes,width,height,sha256,content,created_at_ms FROM image_assets_new;
             DROP TABLE image_assets_new;
             ALTER TABLE runs DROP COLUMN import_enabled; ALTER TABLE runs DROP COLUMN image_input; ALTER TABLE image_jobs DROP COLUMN progress;
             ALTER TABLE image_jobs DROP COLUMN last_contact_at_ms;
             DROP INDEX workspaces_guest; ALTER TABLE workspaces DROP COLUMN guest_id; DROP TABLE guests; DROP TABLE model_calls; PRAGMA user_version=13;",
        ).unwrap();
        if conflict {
            c.execute_batch("ALTER TABLE revisions ADD COLUMN asset_bindings TEXT;")
                .unwrap();
        }
    }

    #[tokio::test]
    async fn version_thirteen_upgrade_preserves_generated_image_identity_and_rolls_back_before_recovery()
     {
        for conflict in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("assets.sqlite3");
            let store = Store::open(&path).unwrap();
            let w = store
                .create_workspace("Before bindings".into())
                .await
                .unwrap();
            let profile=ImageProfile { lora_catalog: Vec::new(), provider: Default::default(), api_key_env: None, api_key: None, resource_group: None, id:"klein".into(),label:"Klein".into(),endpoint:"http://127.0.0.1:8000/mcp".into(),
                settings:serde_json::json!({"model_type":"flux2_klein_4b","resolution":"512x512","num_inference_steps":4}).as_object().unwrap().clone(),
            };
            let run = store
                .begin(
                    w.id.clone(),
                    "image".into(),
                    "compatible".into(),
                    "fixture".into(),
                    RunOptions {
                        image_profile: Some(profile),
                        ..Default::default()
                    },
                )
                .await
                .unwrap()
                .unwrap();
            store.mark_running(run.id.clone()).await.unwrap();
            let job = store
                .plan_image_job(
                    run.id.clone(),
                    "image_call".into(),
                    "A tree.".into(),
                    vec![],
                )
                .await
                .unwrap();
            store
                .image_dispatch(job.id.clone(), "context".into(), serde_json::json!({}))
                .await
                .unwrap();
            store
                .image_bind(job.id.clone(), "remote".into())
                .await
                .unwrap();
            let mut cursor = Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(2, 2)
                .write_to(&mut cursor, image::ImageFormat::Png)
                .unwrap();
            let bytes = cursor.into_inner();
            let digest = format!("{:x}", Sha256::digest(&bytes));
            store
                .image_settle(
                    job.id.clone(),
                    Some(decode_image(bytes.clone(), "image/png", &digest).unwrap()),
                    None,
                )
                .await
                .unwrap();
            let image = store
                .image_job(job.id)
                .await
                .unwrap()
                .unwrap()
                .output_image
                .unwrap();
            drop(store);
            downgrade_to_thirteen(&path, conflict);
            if conflict {
                assert!(Store::open(&path).is_err());
                let c = Connection::open(&path).unwrap();
                assert_eq!(
                    c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                        .unwrap(),
                    13
                );
                assert_eq!(
                    c.query_row("SELECT status FROM runs WHERE id=?1", [&run.id], |r| r
                        .get::<_, String>(
                        0
                    ))
                    .unwrap(),
                    "running"
                );
                assert_eq!(c.query_row("SELECT count(*) FROM pragma_table_info('image_assets') WHERE name='origin'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
                c.execute_batch("ALTER TABLE revisions DROP COLUMN asset_bindings;")
                    .unwrap();
            }
            let store = Store::open(&path).unwrap();
            let (after, content) = store
                .image_content(image.id.clone())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(after, image);
            assert_eq!(content, bytes);
            assert_eq!(
                store.run(run.id).await.unwrap().unwrap().status,
                "interrupted"
            );
            assert!(
                store
                    .workspace(w.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .revisions
                    .is_empty()
            );
        }
    }
}
