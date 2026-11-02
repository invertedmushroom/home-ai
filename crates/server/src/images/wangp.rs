use super::{ImageProgress, MAX_IMAGE_BYTES, ValidatedImage, decode_image};
use crate::sse::SseDecoder;
use crate::store::ImageAsset;
use anyhow::{Result, ensure};
use reqwest::{Client, Url, header};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;

const MAX_RPC_BYTES: usize = 1_048_576;
const PROTOCOL: &str = "2025-03-26";

pub(super) struct Wangp {
    client: Client,
    endpoint: Url,
    session: Option<String>,
    next_id: u64,
}

#[derive(Deserialize)]
pub(super) struct RemoteJob {
    pub job_id: String,
    pub done: bool,
    pub result: Option<RemoteResult>,
    pub status: Option<String>,
    agent_context: Option<String>,
    request_id: Option<String>,
    #[serde(default)]
    progress: Value,
}

impl RemoteJob {
    pub fn progress(&self) -> Option<ImageProgress> {
        if let Some(task) = self.progress.get("task_id").filter(|task| !task.is_null())
            && task.as_str() != Some(format!("mcp_{}_1", self.job_id).as_str())
        {
            return None;
        }
        let progress: ImageProgress = serde_json::from_value(self.progress.clone()).ok()?;
        progress.valid().then_some(progress)
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    fn job(progress: Value) -> RemoteJob {
        serde_json::from_value(json!({"job_id":"owned", "done":false, "progress":progress}))
            .unwrap()
    }

    #[test]
    fn progress_keeps_only_bounded_phase_counts_from_the_owned_task() {
        let progress = job(json!({
            "phase":"inference_stage_1", "current_step":2, "total_steps":4,
            "task_id":"mcp_owned_1", "status":"private prompt or path", "raw_phase":"private"
        }))
        .progress()
        .unwrap();
        assert_eq!(
            serde_json::to_value(progress).unwrap(),
            json!({
                "phase":"inference_stage_1", "current_step":2, "total_steps":4
            })
        );
        assert!(job(json!({"phase":"loading_model"})).progress().is_some());
        for invalid in [
            json!(null),
            json!("bad"),
            json!({"phase":"unknown"}),
            json!({"phase":"inference", "current_step":-1}),
            json!({"phase":"inference", "total_steps":0}),
            json!({"phase":"inference", "current_step":5, "total_steps":4}),
            json!({"phase":"inference", "current_step":4_294_967_296_u64}),
            json!({"phase":"inference", "task_id":"mcp_foreign_1"}),
            json!({"phase":"inference", "task_id":42}),
        ] {
            assert!(job(invalid.clone()).progress().is_none(), "{invalid}");
        }
    }
}

#[derive(Deserialize)]
pub(super) struct RemoteResult {
    pub success: bool,
    pub cancelled: bool,
    #[serde(default)]
    gallery_items: Vec<Output>,
}

#[derive(Deserialize)]
pub(super) struct Output {
    pub media_id: String,
    media_type: String,
    role: Option<String>,
    task_id: Option<String>,
}

impl RemoteResult {
    pub fn single_image(&self, remote: &str) -> Result<&Output> {
        ensure!(
            self.gallery_items.len() == 1,
            "WanGP must return exactly one image output."
        );
        let output = &self.gallery_items[0];
        ensure!(
            output.media_type == "image"
                && output.role.as_deref() == Some("final")
                && output.task_id.as_deref() == Some(format!("mcp_{remote}_1").as_str())
                && valid_remote_id(&output.media_id),
            "WanGP image output is not bound to the exact final task."
        );
        Ok(output)
    }
}

fn valid_remote_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_:-".contains(&c))
}

fn result(body: Value, id: u64) -> Result<Value> {
    ensure!(
        body.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
            && body.get("id").and_then(Value::as_u64) == Some(id),
        "MCP response identity did not match the request."
    );
    ensure!(body.get("error").is_none(), "WanGP MCP request failed.");
    body.get("result")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("MCP response has no result."))
}

impl Wangp {
    pub async fn connect(endpoint: &str) -> Result<Self> {
        let mut client = Self {
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(30))
                .build()?,
            endpoint: Url::parse(endpoint)?,
            session: None,
            next_id: 0,
        };
        let initialized = client
            .rpc(
                "initialize",
                json!({
                    "protocolVersion":PROTOCOL,"capabilities":{},
                    "clientInfo":{"name":"deep-image-runtime","version":env!("CARGO_PKG_VERSION")}
                }),
                false,
            )
            .await?;
        ensure!(
            initialized["protocolVersion"] == PROTOCOL
                && initialized["serverInfo"]["name"] == "WanGP",
            "Expected WanGP MCP protocol 2025-03-26."
        );
        client
            .rpc("notifications/initialized", json!({}), true)
            .await?;
        Ok(client)
    }

    async fn rpc(&mut self, method: &str, params: Value, notification: bool) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let mut request = self
            .client
            .post(self.endpoint.clone())
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL);
        if let Some(session) = &self.session {
            request = request.header("Mcp-Session-Id", session);
        }
        let mut body = json!({"jsonrpc":"2.0","method":method,"params":params});
        if !notification {
            body["id"] = json!(id);
        }
        let mut response = request
            .json(&body)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Cannot reach WanGP MCP; check its availability."))?;
        ensure!(
            response.status().is_success(),
            "WanGP MCP returned HTTP {}.",
            response.status().as_u16()
        );
        if let Some(session) = response.headers().get("Mcp-Session-Id") {
            let session = session
                .to_str()
                .map_err(|_| anyhow::anyhow!("Invalid MCP session."))?;
            ensure!(
                !session.is_empty()
                    && session.len() <= 256
                    && self.session.as_deref().is_none_or(|old| old == session),
                "MCP session identity changed."
            );
            self.session = Some(session.into());
        }
        if notification {
            ensure!(
                matches!(response.status().as_u16(), 202 | 204),
                "MCP notification was not acknowledged."
            );
            return Ok(Value::Null);
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .to_owned();
        ensure!(
            matches!(
                content_type.as_str(),
                "application/json" | "text/event-stream"
            ),
            "Unsupported MCP content type."
        );
        let mut bytes = Vec::new();
        let mut count = 0;
        let mut decoder = SseDecoder::default();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("MCP response transport failed."))?
        {
            count += chunk.len();
            ensure!(count <= MAX_RPC_BYTES, "MCP response exceeded 1 MiB.");
            if content_type == "text/event-stream" {
                for event in decoder.push(&chunk)? {
                    let body: Value = serde_json::from_str(&event)
                        .map_err(|_| anyhow::anyhow!("MCP returned invalid JSON."))?;
                    if body.get("id").is_some() {
                        return result(body, id);
                    }
                }
            } else {
                bytes.extend_from_slice(&chunk);
            }
        }
        ensure!(
            content_type == "application/json",
            "MCP stream ended without a response."
        );
        result(
            serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("MCP returned invalid JSON."))?,
            id,
        )
    }

    pub(super) async fn tool(&mut self, name: &str, args: Value) -> Result<Value> {
        #[derive(Deserialize)]
        struct Content {
            #[serde(rename = "type")]
            kind: String,
            text: Option<String>,
        }
        #[derive(Deserialize)]
        struct Envelope {
            #[serde(rename = "isError", default)]
            is_error: bool,
            #[serde(rename = "structuredContent")]
            structured: Option<Value>,
            #[serde(default)]
            content: Vec<Content>,
        }
        let envelope: Envelope = serde_json::from_value(
            self.rpc("tools/call", json!({"name":name,"arguments":args}), false)
                .await?,
        )
        .map_err(|_| anyhow::anyhow!("Invalid WanGP tool envelope."))?;
        ensure!(!envelope.is_error, "WanGP tool {name} failed.");
        let value = if let Some(value) = envelope.structured {
            value
        } else {
            ensure!(
                envelope.content.len() == 1 && envelope.content[0].kind == "text",
                "WanGP tool returned no single structured result."
            );
            serde_json::from_str(
                envelope.content[0]
                    .text
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("WanGP tool has no text."))?,
            )
            .map_err(|_| anyhow::anyhow!("Invalid WanGP tool JSON."))?
        };
        ensure!(
            value.is_object() && value["status"] != "error",
            "WanGP tool {name} rejected the request."
        );
        Ok(value)
    }

    pub async fn context(&mut self) -> Result<String> {
        let value = self.tool("wangp_workspace", json!({})).await?;
        let context = value["agent_context"]
            .as_str()
            .filter(|v| valid_remote_id(v))
            .ok_or_else(|| anyhow::anyhow!("WanGP returned no valid agent context."))?;
        Ok(context.into())
    }

    pub async fn preflight(&mut self, settings: &Value, context: &str) -> Result<()> {
        let models = self.tool("wangp_models",json!({
            "action":"search","arguments":{"filters":{"model_type":settings["model_type"],"main_output":"image"},"limit":2}
        })).await?;
        ensure!(
            models["models"].as_array().is_some_and(
                |models| models.len() == 1 && models[0]["model_type"] == settings["model_type"]
            ) && models["has_more"] == false,
            "The configured image profile must select an image-generation model."
        );
        let value = self
            .tool(
                "wangp_validate",
                json!({"settings":settings,"agent_context":context}),
            )
            .await?;
        ensure!(
            value["valid"] == true
                && value["submitted"] == false
                && value["tasks"].as_array().is_some_and(
                    |tasks| tasks.len() == 1 && tasks[0]["availability"]["available"] == true
                ),
            "WanGP image preflight failed or the selected model is not locally available."
        );
        Ok(())
    }

    fn transfer_url(&self, path: &str) -> Result<Url> {
        let url = self
            .endpoint
            .join(path)
            .map_err(|_| anyhow::anyhow!("Invalid image transfer URL."))?;
        ensure!(
            url.origin() == self.endpoint.origin()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none(),
            "Image transfer must remain on the configured MCP origin."
        );
        Ok(url)
    }

    pub async fn reference_image(
        &mut self,
        settings: &Value,
        asset: &ImageAsset,
        bytes: Vec<u8>,
        context: &str,
    ) -> Result<String> {
        ensure!(
            !bytes.is_empty()
                && bytes.len() <= MAX_IMAGE_BYTES
                && bytes.len() == asset.bytes as usize
                && format!("{:x}", Sha256::digest(&bytes)) == asset.sha256,
            "Reference image bytes do not match the admitted asset."
        );
        let capabilities = self
            .tool(
                "wangp_model",
                json!({
                    "model_type":settings["model_type"],"action":"capabilities","arguments":{}
                }),
            )
            .await?;
        ensure!(
            capabilities["metadata"]["model_type"] == settings["model_type"]
                && capabilities["metadata"]["capabilities"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item == "reference_images")),
            "The selected model does not support reference images."
        );
        #[derive(Deserialize)]
        struct Upload {
            upload_url: String,
            method: String,
            offset: usize,
            size_bytes: usize,
        }
        let extension = if asset.media_type == "image/png" {
            "png"
        } else {
            "jpg"
        };
        let descriptor: Upload = serde_json::from_value(
            self.tool(
                "wangp_session",
                json!({
                    "action":"create_file_upload", "arguments":{
                        "filename":format!("deep-{}.{}",asset.id,extension),
                        "size_bytes":asset.bytes,"sha256":asset.sha256
                    },"agent_context":context
                }),
            )
            .await?,
        )
        .map_err(|_| anyhow::anyhow!("Invalid WanGP upload descriptor."))?;
        ensure!(
            descriptor.method == "PUT"
                && descriptor.offset == 0
                && descriptor.size_bytes == bytes.len(),
            "WanGP upload does not match the reference image."
        );
        let url = self.transfer_url(&descriptor.upload_url)?;
        let mut response = self
            .client
            .put(url)
            .header("Upload-Offset", "0")
            .header(header::CONTENT_TYPE, &asset.media_type)
            .body(bytes)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Reference image upload failed."))?;
        ensure!(
            response.status().as_u16() == 200,
            "Reference image upload was not completed."
        );
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("Reference image upload response failed."))?
        {
            ensure!(
                body.len() + chunk.len() <= MAX_RPC_BYTES,
                "Reference image upload receipt exceeded 1 MiB."
            );
            body.extend_from_slice(&chunk);
        }
        let receipt: Value = serde_json::from_slice(&body)
            .map_err(|_| anyhow::anyhow!("Invalid reference image upload receipt."))?;
        ensure!(
            receipt["status"] == "done"
                && receipt["size_bytes"] == asset.bytes
                && receipt["offset"] == asset.bytes
                && receipt["sha256"] == asset.sha256
                && receipt["mime_type"] == asset.media_type,
            "Reference image upload was not verified."
        );
        let path = receipt["path"]
            .as_str()
            .filter(|path| {
                path.len() <= 1024
                    && path.starts_with("@documents/uploads/")
                    && path
                        .split('/')
                        .all(|part| !part.is_empty() && part != "." && part != "..")
                    && path
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"@/._-".contains(&c))
            })
            .ok_or_else(|| {
                anyhow::anyhow!("WanGP returned an invalid uploaded image reference.")
            })?;
        Ok(path.into())
    }

    pub async fn generate(
        &mut self,
        settings: &Value,
        request: &str,
        context: &str,
    ) -> Result<RemoteJob> {
        let receipt = self
            .tool(
                "wangp_generate",
                json!({
                    "settings":settings,"request_id":request,"wait":false,"agent_context":context
                }),
            )
            .await?;
        let receipt = Self::receipt(receipt, context)?;
        ensure!(
            receipt.request_id.as_deref() == Some(request),
            "WanGP did not bind the submitted request ID."
        );
        Ok(receipt)
    }

    fn receipt(value: Value, context: &str) -> Result<RemoteJob> {
        let receipt: RemoteJob = serde_json::from_value(value)
            .map_err(|_| anyhow::anyhow!("Invalid WanGP job receipt."))?;
        ensure!(
            valid_remote_id(&receipt.job_id)
                && receipt
                    .agent_context
                    .as_deref()
                    .is_none_or(|v| v == context),
            "WanGP returned a foreign job context."
        );
        Ok(receipt)
    }

    pub async fn job(&mut self, id: &str, context: &str) -> Result<RemoteJob> {
        let value = self.tool("wangp_session",json!({"action":"get_job","arguments":{"job_id":id,"wait_s":1},"agent_context":context})).await?;
        Self::receipt(value, context)
    }

    pub async fn cancel(&mut self, id: &str, context: &str) -> Result<()> {
        let value = self
            .tool(
                "wangp_session",
                json!({"action":"cancel_job","arguments":{"job_id":id},"agent_context":context}),
            )
            .await?;
        let receipt = Self::receipt(value, context)?;
        ensure!(
            receipt.job_id == id,
            "WanGP cancelled another job's receipt."
        );
        Ok(())
    }

    pub async fn image(&mut self, source: &str, context: &str) -> Result<ValidatedImage> {
        #[derive(Deserialize)]
        struct Download {
            download_url: String,
            method: String,
            size_bytes: usize,
            mime_type: String,
            sha256: String,
        }
        let descriptor: Download = serde_json::from_value(self.tool("wangp_session",json!({
            "action":"create_file_download","arguments":{"source":source,"include_sha256":true},"agent_context":context
        })).await?).map_err(|_|anyhow::anyhow!("Invalid WanGP image transfer descriptor."))?;
        ensure!(
            descriptor.method == "GET"
                && (1..=MAX_IMAGE_BYTES).contains(&descriptor.size_bytes)
                && matches!(descriptor.mime_type.as_str(), "image/png" | "image/jpeg")
                && descriptor.sha256.len() == 64
                && descriptor.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "WanGP returned an unsupported image transfer."
        );
        let url = self.transfer_url(&descriptor.download_url)?;
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Image download failed."))?;
        ensure!(
            response.status().as_u16() == 200,
            "Image transfer returned HTTP {}.",
            response.status().as_u16()
        );
        ensure!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(';').next())
                == Some(descriptor.mime_type.as_str()),
            "Image transfer media type did not match its receipt."
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("Image download transport failed."))?
        {
            ensure!(
                bytes.len() + chunk.len() <= descriptor.size_bytes,
                "Image download exceeded its declared byte length."
            );
            bytes.extend_from_slice(&chunk);
        }
        ensure!(
            bytes.len() == descriptor.size_bytes,
            "Image download was incomplete."
        );
        tokio::task::spawn_blocking(move || {
            decode_image(bytes, &descriptor.mime_type, &descriptor.sha256)
        })
        .await?
    }

    pub async fn close(&mut self) -> Result<()> {
        if let Some(session) = self.session.take() {
            let response = self
                .client
                .delete(self.endpoint.clone())
                .header("Mcp-Session-Id", session)
                .header("MCP-Protocol-Version", PROTOCOL)
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("Could not close the WanGP MCP session."))?;
            ensure!(
                response.status().is_success(),
                "WanGP MCP session close failed."
            );
        }
        Ok(())
    }
}
