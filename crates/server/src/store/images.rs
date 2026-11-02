use super::*;
use crate::images::{ImageProfile, ImageProfileInfo, ImageProgress, ValidatedImage};

pub(super) const ACTIVE: &str =
    "status IN ('planned','submitting','running','cancelling','unresolved')";
const JOB_COLUMNS: &str = "j.id,j.workspace_id,j.run_id,j.call_id,j.prompt,j.seed,j.status,j.error,j.created_at_ms,j.finished_at_ms,r.image_config,j.remote_job_id,j.progress,j.last_contact_at_ms,r.image_input,j.settings";
pub(super) const ASSET_COLUMNS: &str =
    "id,workspace_id,image_job_id,media_type,bytes,width,height,sha256,created_at_ms,name,origin";

pub(super) fn check_admission(c: &Connection, profile: &ImageProfile) -> Result<()> {
    let unresolved: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM image_jobs j JOIN runs r ON r.id=j.run_id
         WHERE j.status='unresolved'
           AND (COALESCE(json_extract(r.image_config,'$.resource_group'),CASE WHEN json_extract(r.image_config,'$.provider')='openai' THEN 'openai-images' ELSE 'local-gpu' END)=?1
                OR json_extract(r.image_config,'$.endpoint')=?2))",
        params![profile.group(), profile.endpoint],
        |row| row.get(0),
    )?;
    if unresolved {
        return Err(ExecutionError(
            axum::http::StatusCode::CONFLICT,
            "Image ownership is unresolved; ask the operator to inspect the previous operation.",
        )
        .into());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageAsset {
    pub id: String,
    pub workspace_id: String,
    pub image_job_id: Option<String>,
    pub name: String,
    pub origin: String,
    pub media_type: String,
    pub bytes: u32,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageInput {
    pub role: crate::images::ImageInputRole,
    pub asset: ImageAsset,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageJob {
    pub id: String,
    pub workspace_id: String,
    pub run_id: String,
    pub call_id: String,
    pub prompt: String,
    pub seed: u32,
    pub status: String,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub profile: ImageProfileInfo,
    pub output_image: Option<ImageAsset>,
    pub input_image: Option<ImageInput>,
    pub remote_job_id: Option<String>,
    pub progress: Option<ImageProgress>,
    pub last_contact_at_ms: Option<i64>,
    pub generation: ImageGenerationInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageGenerationInfo {
    pub loras: Vec<String>,
    pub lora_strengths: String,
    pub selected_lora_ids: Vec<String>,
}

impl ImageJob {
    pub fn is_active(&self) -> bool {
        matches!(
            self.status.as_str(),
            "planned" | "submitting" | "running" | "cancelling" | "unresolved"
        )
    }

    pub(crate) fn observation(&self) -> serde_json::Value {
        serde_json::json!({
            "image_job_id":self.id,
            "status":self.status,
            "output_image":self.output_image,
            "input_image":self.input_image,
            "error":self.error,
            "generation":self.generation,
            "note":"This image tool returns metadata only. Visual inspection requires an explicit chat image attachment."
        })
    }
}

pub(crate) struct ImageWork {
    pub job: ImageJob,
    pub profile: ImageProfile,
    pub settings: serde_json::Value,
    pub remote_job_id: Option<String>,
    pub agent_context: Option<String>,
}

pub(super) fn read_asset(row: &rusqlite::Row<'_>) -> rusqlite::Result<ImageAsset> {
    Ok(ImageAsset {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        image_job_id: row.get(2)?,
        media_type: row.get(3)?,
        bytes: row.get(4)?,
        width: row.get(5)?,
        height: row.get(6)?,
        sha256: row.get(7)?,
        created_at_ms: row.get(8)?,
        name: row.get(9)?,
        origin: row.get(10)?,
    })
}

pub(super) fn workspace_assets(c: &Connection, workspace: &str) -> Result<Vec<ImageAsset>> {
    Ok(c.prepare(&format!(
        "SELECT {ASSET_COLUMNS} FROM image_assets WHERE workspace_id=?1 ORDER BY rowid"
    ))?
    .query_map([workspace], read_asset)?
    .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(super) fn resolve_attachments(
    c: &Connection,
    workspace: &str,
    ids: &[String],
) -> Result<Vec<ImageAsset>> {
    if ids.len() > crate::images::MAX_CHAT_IMAGES {
        return Err(InvalidAsset("Attach at most four images to a message.".into()).into());
    }
    let mut seen = std::collections::HashSet::new();
    let mut total = 0;
    ids.iter()
        .map(|id| {
            if !seen.insert(id) {
                return Err(InvalidAsset("Chat image attachments must be unique.".into()).into());
            }
            let asset = c
                .query_row(
                    &format!(
                        "SELECT {ASSET_COLUMNS} FROM image_assets WHERE id=?1 AND workspace_id=?2"
                    ),
                    params![id, workspace],
                    read_asset,
                )
                .optional()?
                .ok_or_else(|| {
                    InvalidAsset("Choose saved chat images in this workspace.".into())
                })?;
            total += asset.bytes as usize;
            if total > crate::images::MAX_CHAT_IMAGE_BYTES {
                return Err(InvalidAsset(
                    "Chat image attachments must total at most 16 MiB.".into(),
                )
                .into());
            }
            Ok(asset)
        })
        .collect()
}

fn read_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<ImageJob> {
    let profile: ImageProfile = read_json(row, 10)?;
    let settings: serde_json::Value = read_json(row, 15)?;
    let loras: Vec<String> = settings["activated_loras"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect();
    let selected_lora_ids = loras
        .iter()
        .filter_map(|file| {
            profile
                .lora_catalog
                .iter()
                .find(|lora| lora.file == *file)
                .map(|lora| lora.id.clone())
        })
        .collect();
    Ok(ImageJob {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        run_id: row.get(2)?,
        call_id: row.get(3)?,
        prompt: row.get(4)?,
        seed: row.get(5)?,
        status: row.get(6)?,
        error: row.get(7)?,
        created_at_ms: row.get(8)?,
        finished_at_ms: row.get(9)?,
        profile: profile.info(),
        output_image: None,
        remote_job_id: row.get(11)?,
        progress: row
            .get::<_, Option<String>>(12)?
            .map(|_| read_json(row, 12))
            .transpose()?,
        last_contact_at_ms: row.get(13)?,
        input_image: read_optional_json(row, 14)?,
        generation: ImageGenerationInfo {
            loras,
            lora_strengths: settings["loras_multipliers"]
                .as_str()
                .unwrap_or("")
                .to_owned(),
            selected_lora_ids,
        },
    })
}

pub(super) fn for_run(c: &Connection, run: &str) -> Result<Option<ImageJob>> {
    let mut job = c.query_row(
        &format!("SELECT {JOB_COLUMNS} FROM image_jobs j JOIN runs r ON r.id=j.run_id WHERE j.run_id=?1"),
        [run], read_job,
    ).optional()?;
    if let Some(job) = &mut job {
        job.output_image = c
            .query_row(
                &format!("SELECT {ASSET_COLUMNS} FROM image_assets WHERE image_job_id=?1"),
                [&job.id],
                read_asset,
            )
            .optional()?;
    }
    Ok(job)
}

pub(super) fn close_pending(c: &Connection, run: &str) -> Result<()> {
    c.execute(
        "UPDATE image_jobs SET status='interrupted',error='Parent ended before image submission.',finished_at_ms=?2 WHERE run_id=?1 AND status='planned'",
        params![run, now_ms()],
    )?;
    c.execute(
        "UPDATE image_jobs SET status=CASE WHEN remote_job_id IS NULL THEN 'unresolved' ELSE 'cancelling' END,error='Parent ended; remote image completion is not confirmed.' WHERE run_id=?1 AND status IN ('submitting','running','cancelling','unresolved')",
        [run],
    )?;
    Ok(())
}

impl Store {
    pub async fn upload_image(
        &self,
        workspace: String,
        id: String,
        name: String,
        content: Vec<u8>,
        media_type: String,
    ) -> Result<Option<ImageAsset>> {
        use sha2::{Digest, Sha256};
        if content.is_empty() || content.len() > crate::images::MAX_IMAGE_BYTES {
            return Err(InvalidAsset("Images must contain 1–8388608 bytes.".into()).into());
        }
        let id = Uuid::parse_str(&id)
            .map_err(|_| InvalidAsset("Image upload ID must be a UUID.".into()))?
            .to_string();
        if name.trim().is_empty()
            || name.len() > 160
            || name.chars().any(char::is_control)
            || name.contains(['/', '\\'])
        {
            return Err(InvalidAsset(
                "Image names must contain 1–160 UTF-8 bytes without controls or slashes.".into(),
            )
            .into());
        }
        let digest = format!("{:x}", Sha256::digest(&content));
        let image = tokio::task::spawn_blocking(move || {
            crate::images::decode_image(content, &media_type, &digest)
        })
        .await?
        .map_err(|error| InvalidAsset(error.to_string()))?;
        self.call(move |c| {
            let tx=c.transaction()?;
            if let Some(existing)=tx.query_row(
                &format!("SELECT {ASSET_COLUMNS} FROM image_assets WHERE id=?1"),[&id],read_asset,
            ).optional()? {
                if existing.workspace_id != workspace || existing.origin != "upload" || existing.name != name || existing.sha256 != image.sha256 || existing.media_type != image.media_type {
                    return Err(ExecutionError(axum::http::StatusCode::CONFLICT,"This image upload ID already names different input.").into());
                }
                return Ok(Some(existing));
            }
            let text_asset_exists: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE id=?1)",[&id],|r|r.get(0))?;
            if text_asset_exists {
                return Err(ExecutionError(axum::http::StatusCode::CONFLICT,
                    "This image upload ID already names a text asset.").into());
            }
            let exists: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=?1)",[&workspace],|r|r.get(0))?;
            if !exists {return Ok(None);}
            tx.execute(
                "INSERT INTO image_assets(id,workspace_id,name,origin,media_type,bytes,width,height,sha256,content,created_at_ms) VALUES(?1,?2,?3,'upload',?4,?5,?6,?7,?8,?9,?10)",
                params![id,workspace,name,image.media_type,u32::try_from(image.content.len())?,image.width,image.height,image.sha256,image.content,now_ms()],
            )?;
            let asset=tx.query_row(&format!("SELECT {ASSET_COLUMNS} FROM image_assets WHERE id=?1"),[id],read_asset)?;
            tx.commit()?;
            Ok(Some(asset))
        }).await
    }

    pub async fn image_job(&self, id: String) -> Result<Option<ImageJob>> {
        self.call(move |c| {
            let run: Option<String> = c
                .query_row("SELECT run_id FROM image_jobs WHERE id=?1", [id], |r| {
                    r.get(0)
                })
                .optional()?;
            run.map(|run| for_run(c, &run))
                .transpose()
                .map(Option::flatten)
        })
        .await
    }

    pub async fn image_asset(&self, id: String) -> Result<Option<ImageAsset>> {
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {ASSET_COLUMNS} FROM image_assets WHERE id=?1"),
                [id],
                read_asset,
            )
            .optional()?)
        })
        .await
    }

    pub async fn image_content(&self, id: String) -> Result<Option<(ImageAsset, Vec<u8>)>> {
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {ASSET_COLUMNS},content FROM image_assets WHERE id=?1"),
                [id],
                |row| Ok((read_asset(row)?, row.get(11)?)),
            )
            .optional()?)
        })
        .await
    }

    pub(crate) async fn plan_image_job(
        &self,
        run: String,
        call: String,
        prompt: String,
        loras: Vec<String>,
    ) -> Result<ImageJob> {
        ensure!(
            crate::images::valid_prompt(&prompt),
            "Image prompt must contain 1–4096 UTF-8 bytes."
        );
        self.call(move |c| {
            let tx = c.transaction()?;
            let profile: ImageProfile = tx.query_row(
                "SELECT image_config FROM runs WHERE id=?1 AND status='running' AND image_config IS NOT NULL",
                [&run], |r| read_json(r, 0),
            )?;
            check_admission(&tx, &profile)?;
            let workspace: String = tx.query_row("SELECT workspace_id FROM runs WHERE id=?1", [&run], |r| r.get(0))?;
            let id = Uuid::new_v4().to_string();
            let random = Uuid::new_v4();
            let seed = u32::from_le_bytes(random.as_bytes()[..4].try_into()?);
            let settings = profile.settings_for(&prompt, seed, &loras)?;
            tx.execute(
                "INSERT INTO image_jobs(id,workspace_id,run_id,call_id,prompt,seed,settings,status,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,'planned',?8)",
                params![id,workspace,run,call,prompt,seed,serde_json::to_string(&settings)?,now_ms()],
            )?;
            let job = for_run(&tx, &run)?.expect("inserted image job");
            tx.commit()?;
            Ok(job)
        }).await
    }

    pub(crate) async fn image_work(&self, id: String) -> Result<ImageWork> {
        self.call(move |c| {
            let (run, profile, settings, remote_job_id, agent_context) = c.query_row(
                "SELECT j.run_id,r.image_config,j.settings,j.remote_job_id,j.agent_context FROM image_jobs j JOIN runs r ON r.id=j.run_id WHERE j.id=?1",
                [id], |r| Ok((r.get::<_, String>(0)?, read_json(r,1)?,read_json(r,2)?,r.get(3)?,r.get(4)?)),
            )?;
            Ok(ImageWork { job:for_run(c,&run)?.expect("existing image job"), profile,settings,remote_job_id,agent_context })
        }).await
    }

    pub(crate) async fn image_dispatch(
        &self,
        id: String,
        context: String,
        settings: serde_json::Value,
    ) -> Result<bool> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let profile: ImageProfile = tx.query_row(
                "SELECT r.image_config FROM runs r JOIN image_jobs j ON j.run_id=r.id WHERE j.id=?1",
                [&id], |r| read_json(r,0),
            )?;
            let blocked = super::model_calls::resource_busy(&tx,profile.group(),Some(&profile.endpoint),Some(&id))?;
            ensure!(!blocked, "Local GPU ownership is active or unresolved; image generation was not submitted.");
            let changed = tx.execute(
            "UPDATE image_jobs SET status=?4,agent_context=?2,settings=?3 WHERE id=?1 AND status='planned' AND EXISTS(SELECT 1 FROM runs WHERE runs.id=image_jobs.run_id AND runs.status='running')",
            params![id,context,serde_json::to_string(&settings)?,if profile.provider == crate::images::ImageProvider::Openai {"running"} else {"submitting"}],
            )? == 1;
            tx.commit()?;
            Ok(changed)
        }).await
    }

    pub(crate) async fn image_bind(&self, id: String, remote: String) -> Result<()> {
        self.call(move |c| {
            ensure!(c.execute(
                "UPDATE image_jobs SET remote_job_id=?2,status=CASE WHEN EXISTS(SELECT 1 FROM runs WHERE runs.id=image_jobs.run_id AND runs.status='running') THEN 'running' ELSE 'cancelling' END WHERE id=?1 AND status IN ('submitting','unresolved') AND remote_job_id IS NULL",
                params![id,remote],
            )? == 1, "Image receipt could not bind to its submitted job.");
            Ok(())
        }).await
    }

    pub(crate) async fn image_observed(
        &self,
        id: String,
        remote: String,
        progress: Option<ImageProgress>,
    ) -> Result<()> {
        ensure!(
            progress.as_ref().is_none_or(ImageProgress::valid),
            "Invalid image progress."
        );
        let progress = progress
            .map(|progress| serde_json::to_string(&progress))
            .transpose()?;
        self.call(move |c| {
            c.execute(
                "UPDATE image_jobs SET progress=COALESCE(?3,progress),last_contact_at_ms=MAX(COALESCE(last_contact_at_ms,0),?4) WHERE id=?1 AND remote_job_id=?2 AND status IN ('running','cancelling','unresolved')",
                params![id,remote,progress,now_ms()],
            )?;
            Ok(())
        }).await
    }

    pub(crate) async fn image_problem(
        &self,
        id: String,
        error: String,
        uncertain: bool,
    ) -> Result<()> {
        let error = crate::images::bounded_error(&error);
        self.call(move |c| {
            c.execute(
                &format!("UPDATE image_jobs SET status=?2,error=?3,finished_at_ms=?4 WHERE id=?1 AND {ACTIVE}"),
                params![id,if uncertain {"unresolved"}else{"failed"},error,if uncertain {None}else{Some(now_ms())}],
            )?;
            Ok(())
        }).await
    }

    pub(crate) async fn image_settle(
        &self,
        id: String,
        image: Option<ValidatedImage>,
        error: Option<String>,
    ) -> Result<()> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let (workspace, status, parent): (String,String,String) = tx.query_row(
                "SELECT j.workspace_id,j.status,r.status FROM image_jobs j JOIN runs r ON r.id=j.run_id WHERE j.id=?1",
                [&id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            )?;
            if !matches!(status.as_str(),"running"|"cancelling"|"unresolved") { return Ok(()); }
            let publish = parent == "running" && status == "running";
            let success = publish && image.is_some();
            if let Some(image) = image.filter(|_| publish) {
                let asset_id=Uuid::new_v4().to_string();
                let extension=if image.media_type == "image/png" {"png"} else {"jpg"};
                tx.execute(
                    "INSERT INTO image_assets(id,workspace_id,image_job_id,media_type,bytes,width,height,sha256,content,created_at_ms,name,origin) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'generation')",
                    params![asset_id,workspace,id,image.media_type,u32::try_from(image.content.len())?,image.width,image.height,image.sha256,image.content,now_ms(),format!("image-{asset_id}.{extension}")],
                )?;
            }
            let status = if success {"succeeded"} else if parent == "interrupted" {"interrupted"} else if !publish {"cancelled"} else {"failed"};
            let error = if success {None} else {Some(crate::images::bounded_error(error.as_deref().unwrap_or(if publish {"WanGP image generation failed."}else{"Parent ended; remote job is terminal and no late image was published."})))};
            tx.execute("UPDATE image_jobs SET status=?2,error=?3,finished_at_ms=?4 WHERE id=?1",params![id,status,error,now_ms()])?;
            tx.commit()?;
            Ok(())
        }).await
    }

    pub(crate) async fn pending_image_ids(&self) -> Result<Vec<String>> {
        self.call(move |c| {
            Ok(
                c.prepare(&format!("SELECT id FROM image_jobs WHERE {ACTIVE}"))?
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            )
        })
        .await
    }

    pub(crate) async fn reconcile_image(&self, id: String) -> Result<()> {
        self.call(move |c| {
            let (status,remote): (String,Option<String>) = c.query_row(
                "SELECT status,remote_job_id FROM image_jobs WHERE id=?1", [&id], |r|Ok((r.get(0)?,r.get(1)?)),
            )?;
            if !matches!(status.as_str(),"unresolved"|"cancelling") {
                return Err(ExecutionError(axum::http::StatusCode::CONFLICT,
                    "Only an unresolved or cancelling image job needs reconciliation.").into());
            }
            if remote.is_none() {
                return Err(ExecutionError(axum::http::StatusCode::CONFLICT,
                    "This job has no recoverable remote receipt. Ask the operator to inspect the provider and resolve the unknown outcome; Deep will not replay it.").into());
            }
            c.execute("UPDATE image_jobs SET status='cancelling' WHERE id=?1",[id])?;
            Ok(())
        }).await
    }

    pub(crate) async fn resolve_image_unknown(&self, id: String) -> Result<()> {
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE image_jobs SET status='interrupted',error=CASE WHEN EXISTS(SELECT 1 FROM runs r WHERE r.id=image_jobs.run_id AND json_extract(r.image_config,'$.provider')='openai') THEN 'Operator accepted the unknown OpenAI image outcome. The request may have completed and incurred charges; no result was recovered.' ELSE 'Operator confirmed the WanGP runtime was stopped. The previous generation outcome remains unknown.' END,finished_at_ms=?2 WHERE id=?1 AND status='unresolved'",
                params![id,now_ms()],
            )?;
            if changed != 1 {
                return Err(ExecutionError(axum::http::StatusCode::CONFLICT,
                    "The image job is no longer unresolved; refresh its receipt.").into());
            }
            Ok(())
        }).await
    }
}
