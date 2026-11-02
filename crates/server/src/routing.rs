//! Operator-owned model profiles. Selection happens once, before run admission.
use crate::config::{BackendConfig, BackendKind, LocalRuntime};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    Conversation,
    Coding,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProfile {
    pub id: String,
    pub label: String,
    pub backend: String,
    pub model: String,
    pub tool_calls: bool,
    #[serde(default)]
    pub vision: bool,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    #[serde(default)]
    pub conversation: Vec<String>,
    #[serde(default)]
    pub coding: Vec<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCatalog {
    #[serde(default, skip_serializing)]
    pub connections: Vec<Connection>,
    #[serde(default)]
    pub default_profile: Option<String>,
    pub profiles: Vec<ModelProfile>,
    #[serde(default)]
    pub preferences: Preferences,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryKind {
    #[default]
    None,
    Compatible,
    Lmstudio,
    Llamacpp,
    LlamacppRouter,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub id: String,
    pub label: String,
    pub kind: BackendKind,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub resource_group: Option<String>,
    pub runtime: Option<LocalRuntime>,
    pub discovery: Option<DiscoveryKind>,
}

impl Connection {
    pub fn backend(
        &self,
        catalog: &ModelCatalog,
        get: &impl Fn(&str) -> Option<String>,
        timeout: std::time::Duration,
        max_output_tokens: u32,
    ) -> Result<BackendConfig> {
        ensure!(
            self.kind != BackendKind::Demo,
            "Catalog connections must use a real runtime/provider."
        );
        ensure!(
            !self.label.trim().is_empty() && self.label.len() <= 128,
            "Connection labels require 1–128 UTF-8 bytes."
        );
        if let Some(name) = &self.api_key_env {
            ensure!(
                !name.is_empty()
                    && name.len() <= 128
                    && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
                "Credential references must name environment variables."
            );
        }
        let default_url = match self.kind {
            BackendKind::Groq => "https://api.groq.com/openai/v1",
            BackendKind::Openai => "https://api.openai.com/v1",
            BackendKind::Lmstudio => "http://127.0.0.1:1234/v1",
            _ => "http://127.0.0.1:8080/v1",
        };
        ensure!(
            self.discovery
                .is_none_or(|d| d == DiscoveryKind::None || self.kind.local()),
            "Hosted provider discovery is not enabled; configure hosted models explicitly."
        );
        ensure!(
            self.runtime.is_none()
                || self
                    .discovery
                    .is_none_or(|d| matches!(d, DiscoveryKind::None | DiscoveryKind::Llamacpp)),
            "Native llama-server recovery requires idle-safe single-model discovery or no discovery."
        );
        Ok(BackendConfig {
            kind: self.kind,
            id: self.id.clone(),
            runtime: self.runtime,
            resource_group: self.resource_group.clone(),
            base_url: self
                .base_url
                .as_deref()
                .unwrap_or(default_url)
                .trim_end_matches('/')
                .into(),
            model: catalog
                .profiles
                .iter()
                .find(|p| p.backend == self.id)
                .map(|p| p.model.clone())
                .unwrap_or_default(),
            api_key: self
                .api_key_env
                .as_deref()
                .and_then(get)
                .filter(|key| !key.trim().is_empty()),
            timeout,
            max_output_tokens,
        })
    }
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selection {
    Auto { task: Task },
    Profile { profile_id: String },
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRoute {
    pub profile_id: String,
    pub label: String,
    pub backend: String,
    pub model: String,
    #[serde(default)]
    pub vision: bool,
    pub task: Option<Task>,
    pub automatic: bool,
    pub reason: String,
}

impl ModelCatalog {
    pub fn validate(&self, backends: &[BackendConfig]) -> Result<()> {
        ensure!(
            self.profiles.len() <= 64,
            "Configure at most 64 model profiles."
        );
        let mut ids = std::collections::HashSet::new();
        ensure!(
            self.connections.len() <= 16,
            "Configure at most 16 runtime connections."
        );
        let mut connections = std::collections::HashSet::new();
        for connection in &self.connections {
            ensure!(
                connections.insert(&connection.id),
                "Connection IDs must be unique."
            );
        }
        for p in &self.profiles {
            ensure!(
                crate::resources::valid_group(&p.id) && ids.insert(&p.id),
                "Model profile IDs must be unique portable identifiers."
            );
            ensure!(
                !p.label.trim().is_empty() && p.label.len() <= 128,
                "Model profile labels require 1–128 UTF-8 bytes."
            );
            ensure!(
                !p.model.trim().is_empty()
                    && p.model.len() <= 512
                    && !p.model.chars().any(char::is_control),
                "Model profiles require a bounded model ID."
            );
            ensure!(
                backends.iter().any(|b| b.id == p.backend && b.id != "demo"),
                "Model profiles must name a configured real backend."
            );
            ensure!(
                backends
                    .iter()
                    .filter(|b| b.id == p.backend && b.runtime.is_some())
                    .all(|b| b.model == p.model),
                "Native llama-server recovery profiles must use the backend's single configured model. Use a separate router backend for multiple models."
            );
        }
        for preferences in [&self.preferences.conversation, &self.preferences.coding] {
            let mut seen = std::collections::HashSet::new();
            ensure!(
                preferences.len() <= 64
                    && preferences
                        .iter()
                        .all(|id| ids.contains(id) && seen.insert(id)),
                "Task preferences must contain unique configured model profile IDs."
            );
            ensure!(
                preferences.iter().all(|id| {
                    let p = self
                        .profiles
                        .iter()
                        .find(|p| &p.id == id)
                        .expect("validated profile");
                    backends.iter().any(|b| b.id == p.backend && b.kind.local())
                }),
                "Automatic task preferences must use local-runtime backends."
            );
        }
        ensure!(
            self.default_profile
                .as_ref()
                .is_none_or(|id| ids.contains(id)),
            "Default model profile must name a configured profile."
        );
        Ok(())
    }

    pub fn resolve(
        &self,
        selection: &Selection,
        needs_tools: bool,
        needs_vision: bool,
        backends: &[BackendConfig],
    ) -> Result<(BackendConfig, ModelRoute)> {
        let (profile, task) = match selection {
            Selection::Profile { profile_id } => (
                self.profiles
                    .iter()
                    .find(|p| &p.id == profile_id)
                    .ok_or_else(|| anyhow::anyhow!("Select a configured model profile."))?,
                None,
            ),
            Selection::Auto { task } => {
                let preferences = match task {
                    Task::Conversation => &self.preferences.conversation,
                    Task::Coding => &self.preferences.coding,
                };
                let profile = preferences.iter().filter_map(|id| self.profiles.iter().find(|p| &p.id == id))
                    .find(|p| (!needs_tools || p.tool_calls) && (!needs_vision || p.vision))
                    .ok_or_else(|| anyhow::anyhow!("No configured model profile meets this task's tool and image-input requirements. Choose a model explicitly or update task preferences."))?;
                (profile, Some(*task))
            }
        };
        ensure!(
            !needs_tools || profile.tool_calls,
            "This model profile does not support the requested tools."
        );
        ensure!(
            !needs_vision || profile.vision,
            "This model profile does not support chat image attachments."
        );
        let mut backend = backends
            .iter()
            .find(|b| b.id == profile.backend)
            .expect("validated profile backend")
            .clone();
        backend.model.clone_from(&profile.model);
        let route = ModelRoute {
            profile_id: profile.id.clone(),
            label: profile.label.clone(),
            backend: backend.id.clone(),
            model: backend.model.clone(),
            vision: profile.vision,
            task,
            automatic: task.is_some(),
            reason: if task.is_some() {
                "First task preference supporting the requested capabilities."
            } else {
                "Explicit model profile selection."
            }
            .into(),
        };
        Ok((backend, route))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LocalRuntime;
    use std::time::Duration;

    fn backend() -> BackendConfig {
        BackendConfig {
            kind: crate::config::BackendKind::Lmstudio,
            runtime: None,
            resource_group: None,
            id: "lmstudio".into(),
            base_url: "http://127.0.0.1:1234/v1".into(),
            model: "default".into(),
            api_key: None,
            timeout: Duration::from_secs(30),
            max_output_tokens: 4096,
        }
    }

    fn catalog() -> ModelCatalog {
        serde_json::from_value(serde_json::json!({
            "profiles":[
                {"id":"small","label":"Small","backend":"lmstudio","model":"small-model","tool_calls":false},
                {"id":"agent","label":"Agent","backend":"lmstudio","model":"agent-model","tool_calls":true}
            ],
            "preferences":{"conversation":["small","agent"],"coding":["agent"]}
        })).unwrap()
    }

    #[test]
    fn task_preferences_filter_tools_without_changing_manual_selection() {
        let catalog = catalog();
        let backends = [backend()];
        catalog.validate(&backends).unwrap();
        for (tools, expected) in [(false, "small-model"), (true, "agent-model")] {
            let (backend, route) = catalog
                .resolve(
                    &Selection::Auto {
                        task: Task::Conversation,
                    },
                    tools,
                    false,
                    &backends,
                )
                .unwrap();
            assert_eq!(backend.model, expected);
            assert!(route.automatic);
            assert_eq!(backend.base_url, backends[0].base_url);
        }
        assert!(
            catalog
                .resolve(
                    &Selection::Profile {
                        profile_id: "small".into()
                    },
                    true,
                    false,
                    &backends
                )
                .is_err()
        );
        assert!(
            catalog
                .resolve(
                    &Selection::Profile {
                        profile_id: "unknown".into()
                    },
                    false,
                    false,
                    &backends
                )
                .is_err()
        );
        let mut no_match = catalog.clone();
        no_match.preferences.coding = vec!["small".into()];
        assert!(
            no_match
                .resolve(
                    &Selection::Auto { task: Task::Coding },
                    true,
                    false,
                    &backends
                )
                .is_err()
        );
    }

    #[test]
    fn vision_is_explicit_and_auto_requires_both_vision_and_requested_tools() {
        let mut catalog = catalog();
        let backends = [backend()];
        assert!(catalog.profiles.iter().all(|profile| !profile.vision));
        let selection = Selection::Auto {
            task: Task::Conversation,
        };
        assert!(catalog.resolve(&selection, false, true, &backends).is_err());
        catalog.profiles[0].vision = true;
        assert_eq!(
            catalog
                .resolve(&selection, false, true, &backends)
                .unwrap()
                .1
                .profile_id,
            "small"
        );
        assert!(catalog.resolve(&selection, true, true, &backends).is_err());
        catalog.profiles[1].vision = true;
        let route = catalog
            .resolve(&selection, true, true, &backends)
            .unwrap()
            .1;
        assert_eq!(route.profile_id, "agent");
        assert!(route.vision);
        catalog.profiles[0].vision = false;
        assert!(
            catalog
                .resolve(
                    &Selection::Profile {
                        profile_id: "small".into()
                    },
                    false,
                    true,
                    &backends
                )
                .is_err()
        );
    }

    #[test]
    fn catalog_rejects_ambiguous_or_unsupported_routes() {
        let backends = [backend()];
        let base = catalog();
        let mut bad = base.clone();
        bad.profiles[1].id = "small".into();
        assert!(bad.validate(&backends).is_err());
        let mut bad = base.clone();
        bad.preferences.coding = vec!["missing".into()];
        assert!(bad.validate(&backends).is_err());
        let mut bad = base.clone();
        bad.preferences.coding = vec!["agent".into(), "agent".into()];
        assert!(bad.validate(&backends).is_err());
        let mut bad = base.clone();
        bad.profiles[0].backend = "unknown".into();
        assert!(bad.validate(&backends).is_err());
        let mut native = backend();
        native.id = "compatible".into();
        native.kind = crate::config::BackendKind::Compatible;
        native.runtime = Some(LocalRuntime::LlamaCpp);
        let mut bad = base.clone();
        bad.profiles
            .iter_mut()
            .for_each(|p| p.backend = "compatible".into());
        assert!(bad.validate(&[native]).is_err());
        let mut hosted = backend();
        hosted.id = "openai".into();
        hosted.kind = crate::config::BackendKind::Openai;
        let mut bad = base;
        bad.profiles
            .iter_mut()
            .for_each(|p| p.backend = "openai".into());
        assert!(bad.validate(&[hosted]).is_err());
        assert!(
            serde_json::from_str::<Selection>(
                r#"{"mode":"auto","task":"conversation","model":"invented"}"#
            )
            .is_err()
        );
    }
}
