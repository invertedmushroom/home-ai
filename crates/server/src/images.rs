use crate::store::{ImageJob, Store};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, io::Cursor, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tracing::Instrument;

mod inventory;
mod openai;
mod wangp;
pub use inventory::{ImageInventory, inspect as inspect_inventory};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageProvider {
    #[default]
    Wangp,
    Openai,
}

impl ImageProvider {
    fn is_wangp(&self) -> bool {
        *self == Self::Wangp
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageInputRole {
    Reference,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageInputRequest {
    pub role: ImageInputRole,
    pub asset_id: uuid::Uuid,
}

pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_CHAT_IMAGES: usize = 4;
pub const MAX_CHAT_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const IMAGE_DEADLINE: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImagePhase {
    LoadingModel,
    EncodingText,
    Inference,
    #[serde(rename = "inference_stage_1")]
    InferenceStage1,
    #[serde(rename = "inference_stage_2")]
    InferenceStage2,
    #[serde(rename = "inference_stage_3")]
    InferenceStage3,
    Decoding,
    DownloadingOutput,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageProgress {
    pub phase: ImagePhase,
    #[serde(default)]
    pub current_step: Option<u32>,
    #[serde(default)]
    pub total_steps: Option<u32>,
}

impl ImageProgress {
    pub(crate) fn valid(&self) -> bool {
        self.total_steps != Some(0)
            && !matches!((self.current_step, self.total_steps), (Some(current), Some(total)) if current > total)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageProfile {
    #[serde(default, skip_serializing_if = "ImageProvider::is_wangp")]
    pub provider: ImageProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(skip)]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_group: Option<String>,
    pub id: String,
    pub label: String,
    pub endpoint: String,
    pub settings: serde_json::Map<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lora_catalog: Vec<ImageLora>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ImageLora {
    pub id: String,
    pub label: String,
    pub description: String,
    pub file: String,
    #[serde(default)]
    pub trigger_words: Vec<String>,
    pub strength: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ImageProfileInfo {
    #[serde(default, skip_serializing_if = "ImageProvider::is_wangp")]
    pub provider: ImageProvider,
    pub id: String,
    pub label: String,
    pub model: String,
    pub fingerprint: String,
    #[serde(default)]
    pub loras: Option<Vec<String>>,
    #[serde(default)]
    pub lora_strengths: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lora_catalog: Vec<ImageLora>,
}

impl ImageProfileInfo {
    pub(crate) fn select_loras(&self, ids: &[String]) -> Result<Vec<&ImageLora>> {
        ensure!(
            ids.len() + self.loras.as_ref().map_or(0, Vec::len) <= 8,
            "An image call permits at most eight fixed and selected LoRAs together."
        );
        let mut seen = std::collections::HashSet::new();
        ids.iter()
            .map(|id| {
                ensure!(seen.insert(id), "Duplicate LoRA ID in image call.");
                self.lora_catalog
                    .iter()
                    .find(|lora| lora.id == *id)
                    .ok_or_else(|| {
                        anyhow::anyhow!("Choose LoRA IDs from the selected image preset's catalog.")
                    })
            })
            .collect()
    }
}

impl ImageProfile {
    pub(crate) fn group(&self) -> &str {
        self.resource_group
            .as_deref()
            .unwrap_or(if self.provider == ImageProvider::Openai {
                "openai-images"
            } else {
                crate::resources::LOCAL_GROUP
            })
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            crate::resources::valid_group(self.group()),
            "Image resource groups require 1–64 ASCII letters, digits, dots, underscores or hyphens."
        );
        ensure!(
            valid_id(&self.id),
            "Image profile IDs must contain 1–64 ASCII letters, digits, hyphens or underscores."
        );
        ensure!(
            !self.label.trim().is_empty()
                && self.label.len() <= 160
                && !self.label.chars().any(char::is_control),
            "Image profile labels must contain 1–160 UTF-8 bytes without controls."
        );
        if self.provider == ImageProvider::Openai {
            ensure!(
                self.lora_catalog.is_empty(),
                "OpenAI image presets cannot select LoRAs."
            );
            return openai::validate_profile(self);
        }
        ensure!(
            self.api_key_env.is_none() && self.api_key.is_none(),
            "WanGP presets do not use API credentials."
        );
        let url = reqwest::Url::parse(&self.endpoint)
            .map_err(|_| anyhow::anyhow!("Image profile needs a loopback HTTP MCP endpoint."))?;
        ensure!(
            url.scheme() == "http"
                && url.host_str().is_some_and(|host| host == "localhost"
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
                    || host == "[::1]")
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path().trim_end_matches('/') == "/mcp",
            "Image profiles require a loopback HTTP /mcp endpoint without credentials, query or fragment."
        );
        ensure!(
            self.settings
                .get("model_type")
                .and_then(Value::as_str)
                .is_some_and(valid_id),
            "Image profile requires a bounded model_type."
        );
        ensure!(
            self.settings
                .get("resolution")
                .and_then(Value::as_str)
                .is_some_and(|s| {
                    s.split_once('x').is_some_and(|(w, h)| {
                        w.parse::<u32>().is_ok_and(|n| (1..=2048).contains(&n))
                            && h.parse::<u32>().is_ok_and(|n| (1..=2048).contains(&n))
                    })
                }),
            "Image profile resolution must be WIDTHxHEIGHT with each dimension at most 2048."
        );
        ensure!(
            self.settings
                .get("num_inference_steps")
                .and_then(Value::as_u64)
                .is_some_and(|n| (1..=100).contains(&n)),
            "Image profile requires 1–100 inference steps."
        );
        for key in self.settings.keys() {
            ensure!(
                [
                    "model_type",
                    "resolution",
                    "num_inference_steps",
                    "activated_loras",
                    "loras_multipliers",
                    "guidance_scale",
                    "guidance_phases",
                    "sample_solver",
                    "flow_shift"
                ]
                .contains(&key.as_str()),
                "Unsupported image profile setting: {key}."
            );
        }
        if let Some(loras) = self.settings.get("activated_loras") {
            ensure!(
                loras.as_array().is_some_and(|items| items.len() <= 8
                    && items
                        .iter()
                        .all(|v| v.as_str().is_some_and(|name| !name.is_empty()
                            && name.len() <= 256
                            && !name.chars().any(char::is_control)
                            && !name.contains(['/', '\\', ':'])))),
                "Image profile LoRAs must be a bounded list of local filenames."
            );
        }
        for key in ["guidance_scale", "flow_shift"] {
            if let Some(value) = self.settings.get(key) {
                ensure!(
                    value
                        .as_f64()
                        .is_some_and(|n| n.is_finite() && (0.0..=100.0).contains(&n)),
                    "Image profile {key} must be a number from 0 to 100."
                );
            }
        }
        if let Some(value) = self.settings.get("guidance_phases") {
            ensure!(
                value.as_u64().is_some_and(|n| (1..=4).contains(&n)),
                "Image profile guidance_phases must be 1–4."
            );
        }
        for key in ["sample_solver", "loras_multipliers"] {
            if let Some(value) = self.settings.get(key) {
                ensure!(
                    value
                        .as_str()
                        .is_some_and(|s| s.len() <= 256 && !s.chars().any(char::is_control)),
                    "Image profile {key} must be a string of at most 256 bytes without controls."
                );
            }
        }
        ensure!(
            serde_json::to_vec(&self.settings)?.len() <= 8192,
            "Image profile settings exceed 8 KiB."
        );
        ensure!(
            self.lora_catalog.len() <= 16,
            "An image preset permits at most 16 catalog LoRAs."
        );
        let mut ids = std::collections::HashSet::new();
        let mut files = self
            .settings
            .get("activated_loras")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<std::collections::HashSet<_>>();
        if !self.lora_catalog.is_empty() {
            let strengths = self
                .settings
                .get("loras_multipliers")
                .and_then(Value::as_str)
                .unwrap_or("");
            let fixed_count = self
                .settings
                .get("activated_loras")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            ensure!(
                strengths.trim().is_empty() || strengths.split_whitespace().count() == fixed_count,
                "Fixed LoRA strengths must contain one expression per adapter before composition."
            );
        }
        for lora in &self.lora_catalog {
            ensure!(
                valid_id(&lora.id) && ids.insert(&lora.id),
                "LoRA IDs must be unique portable IDs."
            );
            ensure!(
                valid_lora_file(&lora.file) && files.insert(&lora.file),
                "LoRA catalog filenames must be local, unique and distinct from fixed adapters."
            );
            ensure!(
                valid_description(&lora.label, 160) && valid_description(&lora.description, 1024),
                "LoRA labels/descriptions require bounded text without controls."
            );
            ensure!(
                lora.trigger_words.len() <= 8
                    && lora.trigger_words.iter().all(|s| valid_description(s, 128)),
                "LoRA trigger words require at most eight bounded phrases."
            );
            ensure!(
                lora.strength.is_finite() && (-10.0..=10.0).contains(&lora.strength),
                "LoRA strength must be a finite number from -10 to 10."
            );
        }
        Ok(())
    }

    pub fn info(&self) -> ImageProfileInfo {
        ImageProfileInfo {
            lora_catalog: self.lora_catalog.clone(),
            provider: self.provider,
            loras: Some(
                self.settings
                    .get("activated_loras")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            lora_strengths: self
                .settings
                .get("loras_multipliers")
                .and_then(Value::as_str)
                .map(str::to_owned),
            id: self.id.clone(),
            label: self.label.clone(),
            model: self
                .settings
                .get(if self.provider == ImageProvider::Openai {
                    "model"
                } else {
                    "model_type"
                })
                .and_then(Value::as_str)
                .expect("validated image model")
                .into(),
            fingerprint: format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(self).expect("serializable image profile"))
            ),
        }
    }

    pub(crate) fn settings_for(&self, prompt: &str, seed: u32, loras: &[String]) -> Result<Value> {
        self.validate()?;
        let info = self.info();
        let selected = info.select_loras(loras)?;
        ensure!(
            valid_prompt(prompt),
            "Image prompt must contain 1–4096 UTF-8 bytes."
        );
        let mut settings = self.settings.clone();
        if self.provider == ImageProvider::Openai {
            settings.insert("prompt".into(), json!(prompt));
            settings.insert("n".into(), json!(1));
            settings.insert("output_format".into(), json!("png"));
            return Ok(Value::Object(settings));
        }
        for (key, value) in [
            ("prompt", json!(prompt)),
            ("seed", json!(seed)),
            ("batch_size", json!(1)),
            ("repeat_generation", json!(1)),
            ("multi_prompts_gen_type", json!("FG")),
            ("image_prompt_type", json!("")),
            ("video_prompt_type", json!("")),
            ("audio_prompt_type", json!("")),
            ("spatial_upsampling", json!("")),
            ("temporal_upsampling", json!("")),
            ("output_filename", json!("")),
        ] {
            settings.insert(key.into(), value);
        }
        settings.entry("activated_loras").or_insert(json!([]));
        settings.entry("loras_multipliers").or_insert(json!(""));
        if !selected.is_empty() {
            let fixed = settings["activated_loras"]
                .as_array()
                .expect("validated adapters")
                .len();
            let mut strengths = settings["loras_multipliers"]
                .as_str()
                .expect("validated strengths")
                .to_owned();
            if strengths.trim().is_empty() {
                strengths = vec!["1"; fixed].join(" ");
            }
            ensure!(
                strengths.split_whitespace().count() == fixed,
                "Fixed LoRA strengths must contain one expression per adapter before composition."
            );
            for lora in selected {
                settings["activated_loras"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(lora.file));
                if !strengths.is_empty() {
                    strengths.push(' ');
                }
                strengths.push_str(&lora.strength.to_string());
            }
            settings.insert("loras_multipliers".into(), json!(strengths));
        }
        Ok(Value::Object(settings))
    }
}

fn valid_description(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn valid_lora_file(value: &str) -> bool {
    valid_description(value, 256)
        && !matches!(value, "." | "..")
        && !value.contains(['/', '\\', ':'])
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

pub(crate) fn valid_prompt(prompt: &str) -> bool {
    !prompt.trim().is_empty() && prompt.len() <= 4096 && !prompt.contains('\0')
}

pub(crate) fn bounded_error(error: &str) -> String {
    let mut end = error.len().min(1024);
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    error[..end].to_owned()
}

pub(crate) struct ValidatedImage {
    pub content: Vec<u8>,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
}

pub(crate) fn decode_image(
    content: Vec<u8>,
    media_type: &str,
    sha256: &str,
) -> Result<ValidatedImage> {
    ensure!(
        !content.is_empty() && content.len() <= MAX_IMAGE_BYTES,
        "Image exceeds the 8 MiB byte limit."
    );
    ensure!(
        format!("{:x}", Sha256::digest(&content)) == sha256,
        "Image SHA-256 did not match its transfer receipt."
    );
    let format = image::guess_format(&content)
        .map_err(|_| anyhow::anyhow!("Image bytes have an invalid format."))?;
    ensure!(
        (format == image::ImageFormat::Png && media_type == "image/png")
            || (format == image::ImageFormat::Jpeg && media_type == "image/jpeg"),
        "Image format does not match the permitted PNG/JPEG media type."
    );
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(64 * 1024 * 1024);
    if format == image::ImageFormat::Png {
        let decoder =
            image::codecs::png::PngDecoder::with_limits(Cursor::new(&content), limits.clone())
                .map_err(|_| anyhow::anyhow!("PNG headers exceed limits or are invalid."))?;
        ensure!(
            !decoder
                .is_apng()
                .map_err(|_| anyhow::anyhow!("Invalid PNG animation metadata."))?,
            "Animated PNG images are not supported."
        );
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(&content), format);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| {
        anyhow::anyhow!("Image decoding failed or exceeded its dimension/allocation limits.")
    })?;
    let (width, height) = (decoded.width(), decoded.height());
    ensure!(
        width > 0 && height > 0,
        "Image dimensions must be positive."
    );
    Ok(ValidatedImage {
        content,
        media_type: media_type.into(),
        width,
        height,
        sha256: sha256.into(),
    })
}

#[derive(Clone)]
pub struct Images {
    store: Store,
    credentials: Arc<HashMap<String, String>>,
    // None while queued; the execution clock starts only after slot acquisition.
    live: Arc<Mutex<HashMap<String, Option<tokio::time::Instant>>>>,
    deadline: Duration,
}

impl Images {
    pub fn new(store: Store, profiles: &[ImageProfile]) -> Self {
        Self {
            store,
            credentials: Arc::new(
                profiles
                    .iter()
                    .filter_map(|profile| {
                        profile
                            .api_key
                            .as_ref()
                            .map(|key| (profile.info().fingerprint, key.clone()))
                    })
                    .collect(),
            ),
            live: Arc::default(),
            deadline: IMAGE_DEADLINE,
        }
    }

    fn start(&self, id: String) {
        let images = self.clone();
        let parent = tracing::Span::current();
        tokio::spawn(async move {
            let mut live = images.live.lock().await;
            if live.contains_key(&id) {
                return;
            }
            live.insert(id.clone(), None);
            drop(live);
            let started = tokio::time::Instant::now();
            let span = tracing::info_span!(target: crate::telemetry::TARGET, parent: &parent, "image_job", job_id=%id,
                run_id=tracing::field::Empty, profile=tracing::field::Empty,
                remote_job_id=tracing::field::Empty, outcome=tracing::field::Empty);
            let result = images.process(id.clone()).instrument(span.clone()).await;
            let mut live = images.live.lock().await;
            if let Err(error) = result {
                tracing::warn!(job=%id,%error,"image job did not settle normally");
                match images.store.image_work(id.clone()).await {
                    Ok(work) if work.job.is_active() => {
                        if let Err(error) = images
                            .store
                            .image_problem(
                                id.clone(),
                                error.to_string(),
                                work.job.status != "planned",
                            )
                            .await
                        {
                            tracing::error!(job=%id,%error,"could not save image job failure");
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::error!(job=%id,%error,"could not read image job after failure")
                    }
                }
            }
            live.remove(&id);
            drop(live);
            let duration = started.elapsed();
            if crate::telemetry::enabled() {
                let outcome =
                    match tokio::time::timeout(Duration::from_secs(1), images.store.image_job(id))
                        .await
                    {
                        Ok(Ok(Some(job))) => job.status,
                        _ => "unknown".to_owned(),
                    };
                span.record("outcome", outcome.as_str());
                crate::telemetry::image_finished(&outcome, duration);
            }
        });
    }

    pub async fn recover(&self) -> Result<()> {
        for id in self.store.pending_image_ids().await? {
            let work = self.store.image_work(id.clone()).await?;
            if work.remote_job_id.is_some() {
                self.start(id);
            } else if work.job.status != "planned" {
                tracing::warn!(job=%id,"image submission receipt is unresolved; work will not be replayed");
            }
        }
        Ok(())
    }

    pub async fn reconcile(&self, id: String) -> Result<()> {
        let live = self.live.lock().await;
        if live.contains_key(&id) {
            return Err(crate::store::ExecutionError(
                axum::http::StatusCode::CONFLICT,
                "Image communication is still active; wait and retry reconciliation.",
            )
            .into());
        }
        self.store.reconcile_image(id.clone()).await?;
        self.start(id);
        Ok(())
    }

    pub async fn resolve_unknown(&self, id: String, provider: ImageProvider) -> Result<()> {
        let live = self.live.lock().await;
        if live.contains_key(&id) {
            return Err(crate::store::ExecutionError(
                axum::http::StatusCode::CONFLICT,
                "Wait for image communication to finish before resolving the unknown outcome.",
            )
            .into());
        }
        let job = self.store.image_job(id.clone()).await?.ok_or({
            crate::store::ExecutionError(axum::http::StatusCode::NOT_FOUND, "Image job not found.")
        })?;
        if job.profile.provider != provider {
            return Err(crate::store::ExecutionError(
                axum::http::StatusCode::BAD_REQUEST,
                "This resolution does not match the image provider.",
            )
            .into());
        }
        self.store.resolve_image_unknown(id.clone()).await?;
        tracing::warn!(job=%id,"operator resolved an unknown image outcome");
        Ok(())
    }

    pub(crate) async fn cancel_run(&self, run: String) -> Result<()> {
        if let Some(job) = self.store.run(run).await?.and_then(|r| r.image_job)
            && job.is_active()
        {
            self.start(job.id);
        }
        Ok(())
    }

    pub(crate) async fn execute(&self, job: ImageJob) -> Result<Value> {
        self.start(job.id.clone());
        loop {
            let current = self
                .store
                .image_job(job.id.clone())
                .await?
                .ok_or_else(|| anyhow::anyhow!("Image job disappeared."))?;
            match current.status.as_str() {
                "succeeded" | "failed" => return Ok(current.observation()),
                "unresolved" => bail!(
                    "Image submission or completion is unresolved; inspect its image job receipt."
                ),
                "cancelled" | "interrupted" => {
                    bail!("Image work ended without a publishable result.")
                }
                _ => {}
            }
            let started = self.live.lock().await.get(&job.id).copied().flatten();
            if started.is_some_and(|started| started.elapsed() >= self.deadline) {
                let error = format!(
                    "Image job exceeded its {}-second execution deadline.",
                    self.deadline.as_secs()
                );
                self.store
                    .terminate(job.run_id, "failed", Some(error.clone()))
                    .await?;
                bail!(error);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn process(&self, id: String) -> Result<()> {
        let work = self.store.image_work(id.clone()).await?;
        if !work.job.is_active() {
            return Ok(());
        }
        if work.remote_job_id.is_none() && work.job.status != "planned" {
            bail!("Submission receipt unknown; cancelled work will not be replayed.");
        }
        tracing::Span::current().record("run_id", work.job.run_id.as_str());
        tracing::Span::current().record("profile", work.job.profile.id.as_str());
        let acquire = self.store.resources.acquire(
            &self.store,
            work.profile.group(),
            Some(&work.profile.endpoint),
            Some(&id),
        );
        tokio::pin!(acquire);
        let _permit = loop {
            tokio::select! {
                permit = &mut acquire => break permit?,
                _ = tokio::time::sleep(Duration::from_millis(100)), if work.job.status == "planned" => {
                    let current = self.store.image_job(id.clone()).await?
                        .ok_or_else(|| anyhow::anyhow!("Image job disappeared."))?;
                    if current.status != "planned" { return Ok(()); }
                }
            }
        };
        if !self
            .store
            .image_job(id.clone())
            .await?
            .is_some_and(|job| job.is_active())
        {
            return Ok(());
        }
        if let Some(started) = self.live.lock().await.get_mut(&id) {
            *started = Some(tokio::time::Instant::now());
        }
        tracing::info!(status=%work.job.status, "image communication started");
        if work.profile.provider == ImageProvider::Openai {
            return self.process_openai(id.clone(), &work).await;
        }
        let started = tokio::time::Instant::now();
        let mut client = wangp::Wangp::connect(&work.profile.endpoint).await?;
        let result = self.process_connected(id.clone(), &mut client).await;
        if let Err(error) = client.close().await {
            tracing::warn!(%error,"could not close image MCP session");
        }
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            completed = result.is_ok(),
            "image communication finished"
        );
        result
    }

    async fn process_connected(&self, id: String, client: &mut wangp::Wangp) -> Result<()> {
        let work = self.store.image_work(id.clone()).await?;
        if work.job.status == "planned" {
            let context = client.context().await?;
            let mut settings = work.settings;
            if let Some(input) = &work.job.input_image {
                let (asset, bytes) = self
                    .store
                    .image_content(input.asset.id.clone())
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Reference image is unavailable."))?;
                ensure!(
                    asset == input.asset,
                    "Reference image no longer matches the admitted input."
                );
                let reference = client
                    .reference_image(&settings, &asset, bytes, &context)
                    .await?;
                settings["image_refs"] = json!([reference]);
                settings["video_prompt_type"] = json!("KI");
            }
            client.preflight(&settings, &context).await?;
            if !self
                .store
                .image_dispatch(id.clone(), context.clone(), settings.clone())
                .await?
            {
                return Ok(());
            }
            let receipt = client.generate(&settings, &id, &context).await?;
            tracing::info!(remote_job_id=%receipt.job_id, "image submission bound");
            self.store.image_bind(id.clone(), receipt.job_id).await?;
        }
        let work = self.store.image_work(id.clone()).await?;
        let remote = work
            .remote_job_id
            .ok_or_else(|| anyhow::anyhow!("Image submission has no bound remote job."))?;
        let context = work
            .agent_context
            .ok_or_else(|| anyhow::anyhow!("Image submission has no bound context."))?;
        tracing::Span::current().record("remote_job_id", remote.as_str());
        let mut cancelled_at = None;
        let mut previous_progress = work.job.progress.clone();
        loop {
            let job = self
                .store
                .image_job(id.clone())
                .await?
                .ok_or_else(|| anyhow::anyhow!("Image job disappeared."))?;
            if !job.is_active() {
                return Ok(());
            }
            let parent_running = self
                .store
                .run(job.run_id.clone())
                .await?
                .is_some_and(|r| r.status == "running");
            let observed = client.job(&remote, &context).await?;
            ensure!(
                observed.job_id == remote,
                "WanGP returned another job's receipt."
            );
            let progress = observed.progress();
            if let Some(progress) = &progress
                && previous_progress.as_ref() != Some(progress)
            {
                tracing::info!(target: crate::telemetry::TARGET, remote_job_id=%remote, phase=?progress.phase,
                    current_step=?progress.current_step, total_steps=?progress.total_steps,
                    "image progress changed");
                previous_progress = Some(progress.clone());
            }
            // Progress is advisory. A telemetry write must not interrupt remote ownership.
            let progress = if observed.done
                && parent_running
                && job.status != "cancelling"
                && observed
                    .result
                    .as_ref()
                    .is_some_and(|result| result.success && !result.cancelled)
            {
                Some(ImageProgress {
                    phase: ImagePhase::DownloadingOutput,
                    current_step: None,
                    total_steps: None,
                })
            } else {
                progress
            };
            if let Err(error) = self
                .store
                .image_observed(id.clone(), remote.clone(), progress)
                .await
            {
                tracing::warn!(%error, "could not save image progress");
            }
            if observed.status.as_deref() == Some("unknown_after_restart") {
                bail!("WanGP restarted without a known image outcome; reconciliation is required.");
            }
            if observed.done {
                if !parent_running || job.status == "cancelling" {
                    self.store.image_settle(id, None, None).await?;
                    return Ok(());
                }
                let result = observed
                    .result
                    .ok_or_else(|| anyhow::anyhow!("Terminal image job has no result."))?;
                if !result.success || result.cancelled {
                    self.store
                        .image_settle(
                            id,
                            None,
                            Some("WanGP image generation failed or was cancelled.".into()),
                        )
                        .await?;
                    return Ok(());
                }
                let image = match result.single_image(&remote) {
                    Ok(output) => client.image(&output.media_id, &context).await,
                    Err(error) => Err(error),
                };
                match image {
                    Ok(image) => self.store.image_settle(id, Some(image), None).await?,
                    Err(error) => {
                        self.store
                            .image_problem(id, error.to_string(), false)
                            .await?
                    }
                }
                return Ok(());
            }
            if !parent_running || job.status == "cancelling" {
                if cancelled_at.is_none() {
                    tracing::info!(remote_job_id=%remote, "requesting image cancellation");
                    client.cancel(&remote, &context).await?;
                    cancelled_at = Some(tokio::time::Instant::now());
                }
                if cancelled_at.is_some_and(|time| time.elapsed() > Duration::from_secs(30)) {
                    bail!("WanGP cancellation is not confirmed; reconciliation is required.");
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::Mode, store::RunOptions};

    fn profile() -> ImageProfile {
        ImageProfile { lora_catalog: Vec::new(), provider: Default::default(), api_key_env: None, api_key: None, resource_group: None, id:"klein".into(),label:"Klein".into(),endpoint:"http://127.0.0.1:8000/mcp".into(),
                        settings:json!({"model_type":"flux2_klein_4b","resolution":"512x512","num_inference_steps":4})
                            .as_object().unwrap().clone(),
                    }
    }

    fn encoded(width: u32, format: image::ImageFormat) -> Vec<u8> {
        let mut target = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(width, 1)
            .write_to(&mut target, format)
            .unwrap();
        target.into_inner()
    }

    #[test]
    fn image_profiles_and_prompts_reject_authority_and_byte_budget_expansion() {
        let base = profile();
        base.validate().unwrap();
        for endpoint in [
            "https://example.com/mcp",
            "file:///private",
            "http://user:pass@127.0.0.1/mcp",
            "http://127.0.0.1/mcp?token=x",
        ] {
            let mut p = base.clone();
            p.endpoint = endpoint.into();
            assert!(p.validate().is_err());
        }
        for (key, value) in [
            ("prompt", json!("injected")),
            ("seed", json!(1)),
            ("image_refs", json!(["secret"])),
            ("resolution", json!("2049x512")),
            ("num_inference_steps", json!(0)),
            ("activated_loras", json!(["https://example.com/weights"])),
            ("guidance_scale", json!("invalid")),
        ] {
            let mut p = base.clone();
            p.settings.insert(key.into(), value);
            assert!(p.validate().is_err(), "{key}");
        }
        assert!(valid_prompt(&"x".repeat(4096)));
        assert!(!valid_prompt(&"x".repeat(4097)));
        assert!(!valid_prompt(&"界".repeat(1366)));
        assert!(!valid_prompt(" \n "));
        let settings = base.settings_for("picture", 123, &[]).unwrap();
        assert_eq!(settings["seed"], 123);
        assert_eq!(settings["repeat_generation"], 1);
        assert_eq!(settings["batch_size"], 1);
        assert_eq!(settings["activated_loras"], json!([]));
        assert!(bounded_error(&"界".repeat(500)).len() <= 1024);
    }

    #[test]
    fn catalog_composition_preserves_fixed_adapters_and_rejects_ambiguous_configuration() {
        let mut base = profile();
        base.settings
            .insert("activated_loras".into(), json!(["accelerator.safetensors"]));
        base.lora_catalog = serde_json::from_value(json!([
            {"id":"selfie","label":"Selfie","description":"Selfie perspective","file":"selfie.safetensors","trigger_words":["selfie"],"strength":0.7},
            {"id":"neon","label":"Neon","description":"Neon style","file":"neon.safetensors","trigger_words":["neon"],"strength":0.4}
        ])).unwrap();
        let settings = base
            .settings_for("selfie in neon", 123, &["neon".into(), "selfie".into()])
            .unwrap();
        assert_eq!(
            settings["activated_loras"],
            json!([
                "accelerator.safetensors",
                "neon.safetensors",
                "selfie.safetensors"
            ])
        );
        assert_eq!(settings["loras_multipliers"], "1 0.4 0.7");
        assert_eq!(
            base.settings_for("tree", 123, &[]).unwrap()["activated_loras"],
            json!(["accelerator.safetensors"])
        );
        assert!(
            base.settings_for("tree", 123, &["other-base".into()])
                .is_err()
        );
        assert!(
            base.settings_for("tree", 123, &["neon".into(), "neon".into()])
                .is_err()
        );
        let mut ambiguous = base.clone();
        ambiguous
            .settings
            .insert("loras_multipliers".into(), json!("0.5 0.8"));
        assert!(ambiguous.validate().is_err());
        for mutation in 0..6 {
            let mut invalid = base.clone();
            match mutation {
                0 => invalid.lora_catalog[1].id = "selfie".into(),
                1 => invalid.lora_catalog[0].file = "../selfie.safetensors".into(),
                2 => invalid.lora_catalog[0].file = "accelerator.safetensors".into(),
                3 => invalid.lora_catalog[0].strength = f64::INFINITY,
                4 => invalid.lora_catalog[0].description = "".into(),
                _ => invalid.lora_catalog[1].file = "selfie.safetensors".into(),
            }
            assert!(invalid.validate().is_err());
        }
        base.settings.insert(
            "activated_loras".into(),
            json!(["1", "2", "3", "4", "5", "6", "7", "8"]),
        );
        assert!(
            base.settings_for("selfie", 123, &["selfie".into()])
                .is_err()
        );
    }

    #[test]
    fn image_decoder_enforces_actual_format_dimensions_hash_and_exact_byte_limit() {
        for (format, mime) in [
            (image::ImageFormat::Png, "image/png"),
            (image::ImageFormat::Jpeg, "image/jpeg"),
        ] {
            let content = encoded(2048, format);
            let digest = format!("{:x}", Sha256::digest(&content));
            let image = decode_image(content.clone(), mime, &digest).unwrap();
            assert_eq!(image.width, 2048);
            assert_eq!(image.height, 1);
            assert!(decode_image(content.clone(), mime, &"0".repeat(64)).is_err());
            assert!(decode_image(content, "text/html", &digest).is_err());
            let large = encoded(2049, format);
            let digest = format!("{:x}", Sha256::digest(&large));
            assert!(decode_image(large, mime, &digest).is_err());
        }
        let mut exact = encoded(1, image::ImageFormat::Png);
        exact.resize(MAX_IMAGE_BYTES, 0);
        let digest = format!("{:x}", Sha256::digest(&exact));
        assert!(decode_image(exact.clone(), "image/png", &digest).is_ok());
        exact.push(0);
        assert!(decode_image(exact, "image/png", &digest).is_err());
    }

    #[test]
    fn animated_png_is_rejected_instead_of_silently_using_its_first_frame() {
        fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
            let mut crc = u32::MAX;
            for byte in kind.iter().chain(data) {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb8_8320u32 & 0u32.wrapping_sub(crc & 1));
                }
            }
            [
                u32::try_from(data.len()).unwrap().to_be_bytes().as_slice(),
                kind,
                data,
                (!crc).to_be_bytes().as_slice(),
            ]
            .concat()
        }
        let content = encoded(1, image::ImageFormat::Png);
        let control = [1u32.to_be_bytes(), 0u32.to_be_bytes()].concat();
        let frame = [
            0u32.to_be_bytes().as_slice(),
            1u32.to_be_bytes().as_slice(),
            1u32.to_be_bytes().as_slice(),
            0u32.to_be_bytes().as_slice(),
            0u32.to_be_bytes().as_slice(),
            1u16.to_be_bytes().as_slice(),
            10u16.to_be_bytes().as_slice(),
            &[0, 0],
        ]
        .concat();
        let animated = [
            content[..33].to_vec(),
            chunk(b"acTL", &control),
            chunk(b"fcTL", &frame),
            content[33..].to_vec(),
        ]
        .concat();
        let digest = format!("{:x}", Sha256::digest(&animated));
        let error = decode_image(animated, "image/png", &digest).err().unwrap();
        assert!(error.to_string().contains("Animated PNG"), "{error}");
    }

    async fn job(store: &Store) -> ImageJob {
        job_with_profile(store, profile()).await
    }

    async fn job_with_profile(store: &Store, profile: ImageProfile) -> ImageJob {
        let workspace = store.create_workspace("Images".into()).await.unwrap();
        let run = store
            .begin(
                workspace.id,
                "Create image".into(),
                "compatible".into(),
                "fixture".into(),
                RunOptions {
                    mode: Mode::Chat,
                    image_profile: Some(profile),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
        store.mark_running(run.id.clone()).await.unwrap();
        store
            .plan_image_job(run.id, "image_call".into(), "A green tree.".into(), vec![])
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn cancelled_planned_image_leaves_the_resource_queue_without_connecting() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let held = store
            .resources
            .acquire(&store, "local-gpu", None, None)
            .await
            .unwrap();
        let job = job(&store).await;
        let images = Images::new(store.clone(), &[]);
        let id = job.id.clone();
        let waiting = tokio::spawn(async move { images.process(id).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        store
            .terminate(job.run_id, "cancelled", None)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let saved = store.image_job(job.id).await.unwrap().unwrap();
        assert_eq!(saved.status, "interrupted");
        assert!(saved.remote_job_id.is_none());
        // The slot remains held throughout the test: no MCP connection was possible.
        drop(held);
    }

    #[test]
    fn legacy_profile_group_default_preserves_its_original_fingerprint() {
        let mut profile: ImageProfile = serde_json::from_value(json!({
            "id":"flux-klein-4b","label":"FLUX.2 Klein 4B","endpoint":"http://127.0.0.1:8000/mcp",
            "settings":{"model_type":"flux2_klein_4b","resolution":"512x512","num_inference_steps":4,"activated_loras":[],"loras_multipliers":""}
        })).unwrap();
        let fingerprint = profile.info().fingerprint;
        assert_eq!(
            fingerprint,
            "47497bdcb5319165b43fdd4f79500d2098406c6954d089ec0374af05128a9527"
        );
        assert_eq!(profile.group(), "local-gpu");
        profile.resource_group = Some("other-gpu".into());
        assert_ne!(profile.info().fingerprint, fingerprint);
    }

    #[tokio::test]
    async fn active_image_deadline_terminates_parent_and_preserves_cleanup_ownership() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let job = job(&store).await;
        store
            .image_dispatch(job.id.clone(), "context".into(), serde_json::json!({}))
            .await
            .unwrap();
        store
            .image_bind(job.id.clone(), "remote".into())
            .await
            .unwrap();
        let images = Images {
            store: store.clone(),
            credentials: Arc::default(),
            live: Arc::new(Mutex::new(HashMap::from([(
                job.id.clone(),
                Some(tokio::time::Instant::now()),
            )]))),
            deadline: Duration::from_millis(10),
        };
        assert_eq!(
            Images::new(store.clone(), &[]).deadline,
            Duration::from_secs(300)
        );
        assert!(
            images
                .execute(job.clone())
                .await
                .unwrap_err()
                .to_string()
                .contains("deadline")
        );
        let parent = store.run(job.run_id).await.unwrap().unwrap();
        assert_eq!(parent.status, "failed");
        assert_eq!(parent.image_job.unwrap().status, "cancelling");
    }

    #[tokio::test]
    async fn preflight_deadline_does_not_claim_remote_cleanup_for_an_unsubmitted_image() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        // The runtime accepts a connection but stalls MCP initialization. This
        // exercises the owned task's real slot acquisition and execution clock.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut profile = profile();
        profile.endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let job = job_with_profile(&store, profile).await;
        let remote = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let mut images = Images::new(store.clone(), &[]);
        images.deadline = Duration::from_millis(10);
        let error = tokio::time::timeout(Duration::from_secs(2), images.execute(job.clone()))
            .await
            .unwrap()
            .unwrap_err()
            .to_string();
        remote.abort();
        assert!(error.contains("execution deadline"));
        assert!(!error.contains("cleanup is pending"));
        let parent = store.run(job.run_id).await.unwrap().unwrap();
        assert_eq!(parent.status, "failed");
        assert_eq!(parent.error.as_deref(), Some(error.as_str()));
        let image = parent.image_job.unwrap();
        assert_eq!(image.status, "interrupted");
        assert!(image.remote_job_id.is_none());
        tokio::time::timeout(Duration::from_secs(2), async {
            while !images.live.lock().await.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn queued_images_cannot_dispatch_across_active_or_unresolved_ownership() {
        let store = Store::open(std::path::Path::new(":memory:")).unwrap();
        let first = job(&store).await;
        let second = job(&store).await;
        store
            .image_dispatch(first.id.clone(), "first".into(), serde_json::json!({}))
            .await
            .unwrap();
        assert!(
            store
                .image_dispatch(second.id.clone(), "second".into(), serde_json::json!({}))
                .await
                .is_err()
        );
        store
            .image_problem(first.id, "Unknown submission".into(), true)
            .await
            .unwrap();
        assert!(
            store
                .image_dispatch(second.id.clone(), "second".into(), serde_json::json!({}))
                .await
                .is_err()
        );
        let queued = store.image_work(second.id).await.unwrap();
        assert_eq!(queued.job.status, "planned");
        assert!(queued.agent_context.is_none());
        assert!(queued.remote_job_id.is_none());
    }

    #[tokio::test]
    async fn version_nineteen_upgrade_preserves_images_and_unresolved_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep.sqlite3");
        let store = Store::open(&path).unwrap();
        let completed = job(&store).await;
        store
            .image_dispatch(
                completed.id.clone(),
                "complete".into(),
                serde_json::json!({}),
            )
            .await
            .unwrap();
        store
            .image_bind(completed.id.clone(), "remote_complete".into())
            .await
            .unwrap();
        let bytes = encoded(1, image::ImageFormat::Png);
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let image = decode_image(bytes.clone(), "image/png", &digest).unwrap();
        store
            .image_settle(completed.id.clone(), Some(image), None)
            .await
            .unwrap();
        store
            .terminate(completed.run_id, "succeeded", None)
            .await
            .unwrap();
        let saved_image = store
            .image_job(completed.id.clone())
            .await
            .unwrap()
            .unwrap()
            .output_image
            .unwrap();
        let unresolved = job(&store).await;
        store
            .image_dispatch(
                unresolved.id.clone(),
                "uncertain".into(),
                serde_json::json!({"frozen":true}),
            )
            .await
            .unwrap();
        store
            .image_problem(unresolved.id.clone(), "Unknown submission".into(), true)
            .await
            .unwrap();
        let before = store.image_work(unresolved.id.clone()).await.unwrap();
        drop(store);
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch("ALTER TABLE runs DROP COLUMN image_attachments; ALTER TABLE runs DROP COLUMN model_route; ALTER TABLE runs DROP COLUMN workspace_tools; CREATE UNIQUE INDEX one_active_image_job ON image_jobs((1)) WHERE status IN ('planned','submitting','running','cancelling','unresolved'); PRAGMA user_version=19;").unwrap();
        drop(c);

        let store = Store::open(&path).unwrap();
        let after = store.image_work(unresolved.id).await.unwrap();
        assert_eq!(after.job.status, "unresolved");
        assert_eq!(after.job.prompt, before.job.prompt);
        assert_eq!(after.job.seed, before.job.seed);
        assert_eq!(after.job.profile, before.job.profile);
        assert_eq!(after.settings, before.settings);
        assert_eq!(after.agent_context, before.agent_context);
        assert_eq!(
            store
                .image_content(saved_image.id)
                .await
                .unwrap()
                .unwrap()
                .1,
            bytes
        );
        assert_eq!(
            store.image_job(completed.id).await.unwrap().unwrap().status,
            "succeeded"
        );
        assert!(
            store
                .check_resource("local-gpu".into(), None, None)
                .await
                .is_err()
        );
        let workspace = store
            .create_workspace("Blocked image".into())
            .await
            .unwrap();
        assert!(
            store
                .begin(
                    workspace.id,
                    "Image".into(),
                    "compatible".into(),
                    "fixture".into(),
                    RunOptions {
                        mode: Mode::Chat,
                        image_profile: Some(profile()),
                        ..Default::default()
                    }
                )
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("unresolved")
        );
        let c = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            23
        );
        assert!(
            !c.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='one_active_image_job')",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
        );
    }

    #[tokio::test]
    async fn restart_preserves_frozen_inputs_and_never_resets_uncertain_jobs_for_replay() {
        for state in ["planned", "submitting", "running"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("deep.sqlite3");
            let store = Store::open(&path).unwrap();
            let job = job(&store).await;
            if state != "planned" {
                store
                    .image_dispatch(job.id.clone(), "context".into(), serde_json::json!({}))
                    .await
                    .unwrap();
            }
            if state == "running" {
                store
                    .image_bind(job.id.clone(), "remote".into())
                    .await
                    .unwrap();
            }
            let before = store.image_work(job.id.clone()).await.unwrap();
            drop(store);
            let store = Store::open(&path).unwrap();
            let after = store.image_work(job.id.clone()).await.unwrap();
            assert_eq!(after.settings, before.settings);
            assert_eq!(after.job.seed, before.job.seed);
            assert_eq!(after.job.profile, before.job.profile);
            assert_eq!(
                after.job.status,
                match state {
                    "planned" => "interrupted",
                    "running" => "cancelling",
                    _ => "unresolved",
                }
            );
            assert_eq!(
                store.run(job.run_id).await.unwrap().unwrap().status,
                "interrupted"
            );
            if state != "planned" {
                assert!(store.delete_workspace(job.workspace_id).await.is_err());
            }
        }
    }

    #[tokio::test]
    async fn progress_survives_restart_and_cannot_change_foreign_or_terminal_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep.sqlite3");
        let store = Store::open(&path).unwrap();
        let job = job(&store).await;
        store
            .image_dispatch(job.id.clone(), "context".into(), serde_json::json!({}))
            .await
            .unwrap();
        store
            .image_bind(job.id.clone(), "remote".into())
            .await
            .unwrap();
        let progress = ImageProgress {
            phase: ImagePhase::Inference,
            current_step: Some(2),
            total_steps: Some(4),
        };
        store
            .image_observed(job.id.clone(), "foreign".into(), Some(progress.clone()))
            .await
            .unwrap();
        assert!(
            store
                .image_job(job.id.clone())
                .await
                .unwrap()
                .unwrap()
                .last_contact_at_ms
                .is_none()
        );
        store
            .image_observed(job.id.clone(), "remote".into(), Some(progress.clone()))
            .await
            .unwrap();
        store
            .image_observed(job.id.clone(), "remote".into(), None)
            .await
            .unwrap();
        let before = store.image_job(job.id.clone()).await.unwrap().unwrap();
        assert_eq!(before.progress, Some(progress.clone()));
        assert!(before.last_contact_at_ms.is_some());
        drop(store);
        let store = Store::open(&path).unwrap();
        let after = store.image_job(job.id.clone()).await.unwrap().unwrap();
        assert_eq!(after.status, "cancelling");
        assert_eq!(after.progress, before.progress);
        assert_eq!(after.last_contact_at_ms, before.last_contact_at_ms);
        store
            .image_settle(job.id.clone(), None, None)
            .await
            .unwrap();
        let terminal = store.image_job(job.id.clone()).await.unwrap().unwrap();
        assert_eq!(terminal.status, "interrupted");
        let later = ImageProgress {
            current_step: Some(4),
            ..progress
        };
        store
            .image_observed(job.id.clone(), "remote".into(), Some(later))
            .await
            .unwrap();
        let unchanged = store.image_job(job.id).await.unwrap().unwrap();
        assert_eq!(unchanged.status, terminal.status);
        assert_eq!(unchanged.progress, terminal.progress);
        assert_eq!(unchanged.last_contact_at_ms, terminal.last_contact_at_ms);
        assert!(unchanged.output_image.is_none());
    }

    #[tokio::test]
    async fn version_fourteen_progress_upgrade_is_atomic_and_preserves_job_identity() {
        for conflict in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("deep.sqlite3");
            let store = Store::open(&path).unwrap();
            let job = job(&store).await;
            drop(store);
            let c = rusqlite::Connection::open(&path).unwrap();
            c.execute_batch("ALTER TABLE runs DROP COLUMN image_attachments; ALTER TABLE runs DROP COLUMN model_route; ALTER TABLE runs DROP COLUMN workspace_tools; ALTER TABLE runs DROP COLUMN import_enabled; ALTER TABLE runs DROP COLUMN image_input; ALTER TABLE image_jobs DROP COLUMN progress; ALTER TABLE image_jobs DROP COLUMN last_contact_at_ms; DROP INDEX workspaces_guest; ALTER TABLE workspaces DROP COLUMN guest_id; DROP TABLE guests; DROP TABLE model_calls; PRAGMA user_version=14;").unwrap();
            if conflict {
                c.execute_batch("ALTER TABLE image_jobs ADD COLUMN last_contact_at_ms INTEGER;")
                    .unwrap();
            }
            drop(c);
            if conflict {
                assert!(Store::open(&path).is_err());
                let c = rusqlite::Connection::open(&path).unwrap();
                assert_eq!(
                    c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                        .unwrap(),
                    14
                );
                assert_eq!(
                    c.query_row("SELECT status FROM runs WHERE id=?1", [job.run_id], |r| {
                        r.get::<_, String>(0)
                    })
                    .unwrap(),
                    "running"
                );
                assert!(c.prepare("SELECT progress FROM image_jobs").is_err());
            } else {
                let store = Store::open(&path).unwrap();
                let saved = store.image_job(job.id.clone()).await.unwrap().unwrap();
                assert_eq!(saved.id, job.id);
                assert_eq!(saved.seed, job.seed);
                assert_eq!(saved.prompt, job.prompt);
                assert_eq!(saved.status, "interrupted");
                assert!(saved.progress.is_none() && saved.last_contact_at_ms.is_none());
            }
        }
    }

    #[tokio::test]
    async fn schema_twelve_upgrade_preserves_existing_text_assets_and_grants_no_image_permission() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep.sqlite3");
        let store = Store::open(&path).unwrap();
        let workspace = store
            .create_workspace("Before image support".into())
            .await
            .unwrap();
        let asset = store
            .create_asset(
                workspace.id.clone(),
                "input.csv".into(),
                None,
                "a,b\n1,2\n".into(),
            )
            .await
            .unwrap()
            .unwrap();
        drop(store);
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch("ALTER TABLE runs DROP COLUMN image_attachments; ALTER TABLE runs DROP COLUMN model_route; ALTER TABLE runs DROP COLUMN workspace_tools; DROP TABLE model_calls; DROP TABLE image_assets; DROP TABLE image_jobs; ALTER TABLE runs DROP COLUMN image_config; ALTER TABLE runs DROP COLUMN import_enabled; ALTER TABLE runs DROP COLUMN image_input; ALTER TABLE revisions DROP COLUMN asset_bindings; ALTER TABLE proposals DROP COLUMN asset_changes; DROP INDEX workspaces_guest; ALTER TABLE workspaces DROP COLUMN guest_id; DROP TABLE guests; PRAGMA user_version=12;").unwrap();
        drop(c);
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.asset(asset.id).await.unwrap().unwrap().content,
            "a,b\n1,2\n"
        );
        let detail = store.workspace(workspace.id).await.unwrap().unwrap();
        assert!(detail.image_assets.is_empty());
        assert!(detail.runs.iter().all(|run| run.image_profile.is_none()));
    }
}
