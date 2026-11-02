//! Bounded, read-only snapshots of operator-configured text runtimes.
use crate::{
    config::{BackendConfig, BackendKind},
    routing::DiscoveryKind,
};
use anyhow::{Result, ensure};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

#[derive(Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub state: &'static str,
    pub tool_calls: Option<bool>,
    pub vision: Option<bool>,
    pub context_length: Option<u64>,
    pub max_context_length: Option<u64>,
}

#[derive(Serialize)]
pub struct Inventory {
    pub connection_id: String,
    pub status: &'static str,
    pub models: Vec<ModelInfo>,
    pub message: Option<&'static str>,
}

pub fn kind(backend: &BackendConfig, configured: Option<DiscoveryKind>) -> DiscoveryKind {
    configured.unwrap_or(match backend.kind {
        BackendKind::Lmstudio => DiscoveryKind::Lmstudio,
        BackendKind::Compatible if backend.runtime.is_some() => DiscoveryKind::Llamacpp,
        BackendKind::Compatible => DiscoveryKind::Compatible,
        _ => DiscoveryKind::None,
    })
}

fn name(value: &Value, key: &str) -> Result<String> {
    let text = value[key]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing model identity"))?;
    ensure!(
        !text.trim().is_empty() && text.len() <= 512 && !text.chars().any(char::is_control),
        "Invalid model identity"
    );
    Ok(text.into())
}

fn positive(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .filter(|n| *n > 0 && *n <= 9_007_199_254_740_991)
}

fn parse(value: Value, source: DiscoveryKind) -> Result<Vec<ModelInfo>> {
    if source == DiscoveryKind::Llamacpp {
        let id = name(&value, "model_alias")?;
        return Ok(vec![ModelInfo {
            label: id.clone(),
            id,
            state: match value["is_sleeping"].as_bool() {
                Some(true) => "sleeping",
                Some(false) => "loaded",
                None => "unknown",
            },
            tool_calls: None,
            vision: value["modalities"]["vision"].as_bool(),
            context_length: positive(&value["default_generation_settings"]["n_ctx"]),
            max_context_length: None,
        }]);
    }
    let entries = value[if source == DiscoveryKind::Lmstudio {
        "models"
    } else {
        "data"
    }]
    .as_array()
    .ok_or_else(|| anyhow::anyhow!("Missing model inventory"))?;
    ensure!(entries.len() <= 256, "Model inventory exceeds its limit");
    let mut models = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for entry in entries {
        if source == DiscoveryKind::Lmstudio && entry["type"] != "llm" {
            continue;
        }
        let id = name(
            entry,
            if source == DiscoveryKind::Lmstudio {
                "key"
            } else {
                "id"
            },
        )?;
        ensure!(ids.insert(id.clone()), "Duplicate model identity");
        let loaded = entry["loaded_instances"].as_array();
        let instance = loaded.and_then(|items| items.first());
        let state = match source {
            DiscoveryKind::Lmstudio => {
                let instances =
                    loaded.ok_or_else(|| anyhow::anyhow!("Missing loaded-instance information"))?;
                if instances.is_empty() {
                    "unloaded"
                } else {
                    "loaded"
                }
            }
            DiscoveryKind::Llamacpp => "loaded",
            DiscoveryKind::LlamacppRouter => match entry["status"]["value"].as_str() {
                Some("loaded") => "loaded",
                Some("unloaded") => "unloaded",
                Some("sleeping") => "sleeping",
                _ => "unknown",
            },
            _ => "unknown",
        };
        models.push(ModelInfo {
            label: entry["display_name"]
                .as_str()
                .filter(|s| {
                    !s.trim().is_empty() && s.len() <= 512 && !s.chars().any(char::is_control)
                })
                .unwrap_or(&id)
                .into(),
            id,
            state,
            tool_calls: entry["capabilities"]["trained_for_tool_use"].as_bool(),
            vision: entry["capabilities"]["vision"].as_bool().or_else(|| {
                entry["architecture"]["input_modalities"]
                    .as_array()
                    .map(|a| a.iter().any(|m| m == "image"))
            }),
            context_length: instance
                .and_then(|i| positive(&i["config"]["context_length"]))
                .or_else(|| positive(&entry["meta"]["n_ctx"])),
            max_context_length: positive(&entry["max_context_length"])
                .or_else(|| positive(&entry["meta"]["n_ctx_train"])),
        });
    }
    Ok(models)
}

async fn fetch(backend: &BackendConfig, source: DiscoveryKind) -> Result<Vec<ModelInfo>> {
    let url = match source {
        DiscoveryKind::Llamacpp => format!("{}/props", backend.base_url.trim_end_matches("/v1")),
        DiscoveryKind::Lmstudio => {
            format!("{}/api/v1/models", backend.base_url.trim_end_matches("/v1"))
        }
        DiscoveryKind::LlamacppRouter => {
            format!("{}/models", backend.base_url.trim_end_matches("/v1"))
        }
        _ => format!("{}/models", backend.base_url),
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()?;
    let mut request = client.get(url);
    if let Some(key) = &backend.api_key {
        request = request.bearer_auth(key);
    }
    let mut response = request.send().await?;
    ensure!(
        response.status().is_success(),
        "Runtime refused model inventory"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 2_097_152,
            "Model inventory exceeds its byte limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    parse(serde_json::from_slice(&bytes)?, source)
}

pub async fn inspect(backend: &BackendConfig, source: DiscoveryKind) -> Inventory {
    if source == DiscoveryKind::None {
        return Inventory {
            connection_id: backend.id.clone(),
            status: "unsupported",
            models: Vec::new(),
            message: Some("Model discovery is not enabled for this connection."),
        };
    }
    match fetch(backend, source).await {
        Ok(models) => Inventory {
            connection_id: backend.id.clone(),
            status: "available",
            models,
            message: None,
        },
        // Runtime bodies, credential values, URLs, paths and transport errors stay private.
        Err(_) => Inventory {
            connection_id: backend.id.clone(),
            status: "unavailable",
            models: Vec::new(),
            message: Some(
                "The runtime inventory could not be read. Check the runtime and its discovery configuration.",
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn native_discovery_uses_idle_safe_endpoints_and_errors_do_not_echo_runtime_details() {
        let upstream = axum::Router::new()
            .route("/props",axum::routing::get(|| async {axum::Json(json!({"model_alias":"native-model","model_path":"PRIVATE_PATH","is_sleeping":true,"modalities":{"vision":true},"default_generation_settings":{"n_ctx":8192}}))}))
            .route("/models",axum::routing::get(|uri:axum::http::Uri|async move {
                assert!(uri.query().is_none());
                axum::Json(json!({"data":[{"id":"router-model","status":{"value":"unloaded"}}]}))
            }))
            .route("/v1/models",axum::routing::get(||async {(axum::http::StatusCode::INTERNAL_SERVER_ERROR,"PRIVATE_CREDENTIAL_AND_URL")}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let backend = BackendConfig {
            kind: BackendKind::Compatible,
            id: "native-test".into(),
            runtime: None,
            resource_group: None,
            base_url: format!("http://{address}/v1"),
            model: "native-model".into(),
            api_key: None,
            timeout: Duration::from_secs(3),
            max_output_tokens: 512,
        };
        let native = inspect(&backend, DiscoveryKind::Llamacpp).await;
        assert_eq!(native.status, "available");
        assert_eq!(native.models[0].state, "sleeping");
        assert!(
            !serde_json::to_string(&native)
                .unwrap()
                .contains("PRIVATE_PATH")
        );
        let router = inspect(&backend, DiscoveryKind::LlamacppRouter).await;
        assert_eq!(router.models[0].state, "unloaded");
        let unavailable = inspect(&backend, DiscoveryKind::Compatible).await;
        assert_eq!(unavailable.status, "unavailable");
        assert!(
            !serde_json::to_string(&unavailable)
                .unwrap()
                .contains("PRIVATE_CREDENTIAL")
        );
        server.abort();
    }
    #[test]
    fn inventories_distinguish_installed_loaded_and_unknown_without_inventing_capabilities() {
        let models = parse(json!({"models":[
            {"type":"llm","key":"a","display_name":"Model A","loaded_instances":[],"max_context_length":1000,"capabilities":{"vision":true,"trained_for_tool_use":true}},
            {"type":"llm","key":"b","loaded_instances":[{"config":{"context_length":512}}]},
            {"type":"embedding","key":"unused"}
        ]}),DiscoveryKind::Lmstudio).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].state, "unloaded");
        assert_eq!(models[1].state, "loaded");
        assert_eq!(models[1].context_length, Some(512));
        assert!(models[1].tool_calls.is_none());
        let models = parse(json!({"data":[{"id":"a"}]}), DiscoveryKind::Compatible).unwrap();
        assert_eq!(models[0].state, "unknown");
        assert!(
            parse(
                json!({"models":[{"type":"llm","key":"a"}]}),
                DiscoveryKind::Lmstudio
            )
            .is_err()
        );
        assert!(
            parse(
                json!({"data":[{"id":"a"},{"id":"a"}]}),
                DiscoveryKind::Compatible
            )
            .is_err()
        );
    }
}
