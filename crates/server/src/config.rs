use anyhow::{Result, bail};
use std::{env, net::SocketAddr, path::PathBuf, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    Demo,
    Compatible,
    Lmstudio,
    Groq,
    Openai,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Demo => "demo",
            Self::Compatible => "compatible",
            Self::Lmstudio => "lmstudio",
            Self::Groq => "groq",
            Self::Openai => "openai",
        }
    }
    pub fn local(self) -> bool {
        matches!(self, Self::Compatible | Self::Lmstudio)
    }
    fn legacy(id: &str) -> Option<Self> {
        match id {
            "demo" => Some(Self::Demo),
            "compatible" => Some(Self::Compatible),
            "lmstudio" => Some(Self::Lmstudio),
            "groq" => Some(Self::Groq),
            "openai" => Some(Self::Openai),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct BackendConfig {
    pub kind: BackendKind,
    pub runtime: Option<LocalRuntime>,
    pub resource_group: Option<String>,
    pub id: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub timeout: Duration,
    pub max_output_tokens: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalRuntime {
    #[serde(rename = "llamacpp")]
    LlamaCpp,
}

impl BackendConfig {
    pub(crate) fn group(&self) -> Option<&str> {
        (self.kind != BackendKind::Demo).then(|| {
            self.resource_group.as_deref().unwrap_or_else(|| {
                if !self.kind.local() {
                    &self.id
                } else {
                    crate::resources::LOCAL_GROUP
                }
            })
        })
    }
}

#[derive(Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub public_origin: Option<String>,
    pub data_dir: PathBuf,
    pub web_dir: PathBuf,
    pub default_backend: String,
    pub backends: Vec<BackendConfig>,
    pub worker_image: Option<String>,
    pub search: Option<crate::search::SearchConfig>,
    pub image_profiles: Vec<crate::images::ImageProfile>,
    pub model_catalog: crate::routing::ModelCatalog,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_values(|name| env::var(name).ok())
    }

    fn from_values(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let legacy = get("DEEP_PROVIDER").unwrap_or_else(|| "demo".into());
        if !matches!(legacy.as_str(), "demo" | "compatible") {
            bail!(
                "DEEP_PROVIDER must be demo or compatible; use DEEP_DEFAULT_BACKEND for named backends"
            );
        }
        let default_backend = get("DEEP_DEFAULT_BACKEND").unwrap_or_else(|| legacy.clone());
        let runtime = match get("DEEP_MODEL_RUNTIME").as_deref().map(str::trim) {
            None | Some("") => None,
            Some("llamacpp") => Some(LocalRuntime::LlamaCpp),
            _ => bail!("DEEP_MODEL_RUNTIME must be empty or llamacpp"),
        };
        let seconds: u64 = get("DEEP_MODEL_TIMEOUT_SECONDS")
            .unwrap_or_else(|| "180".into())
            .parse()
            .map_err(|_| {
                anyhow::anyhow!("DEEP_MODEL_TIMEOUT_SECONDS must be a positive integer")
            })?;
        let max_output_tokens: u32 = get("DEEP_MODEL_MAX_OUTPUT_TOKENS")
            .unwrap_or_else(|| "4096".into())
            .parse()
            .map_err(|_| {
                anyhow::anyhow!("DEEP_MODEL_MAX_OUTPUT_TOKENS must be a positive integer")
            })?;
        let mut backends = vec![BackendConfig {
            kind: BackendKind::Demo,
            runtime: None,
            resource_group: None,
            id: "demo".into(),
            base_url: String::new(),
            model: "demo-model".into(),
            api_key: None,
            timeout: Duration::from_secs(seconds),
            max_output_tokens,
        }];
        let mut register = |id: &str, model: String, base_url: String, api_key: Option<String>| {
            backends.push(BackendConfig {
                kind: BackendKind::legacy(id).expect("legacy backend kind"),
                runtime: (id == "compatible").then_some(runtime).flatten(),
                resource_group: get(match id {
                    "compatible" => "DEEP_MODEL_RESOURCE_GROUP",
                    "lmstudio" => "DEEP_LMSTUDIO_RESOURCE_GROUP",
                    "groq" => "DEEP_GROQ_RESOURCE_GROUP",
                    "openai" => "DEEP_OPENAI_RESOURCE_GROUP",
                    _ => unreachable!(),
                })
                .filter(|group| !group.trim().is_empty()),
                id: id.into(),
                model,
                base_url: base_url.trim_end_matches('/').into(),
                api_key: api_key.filter(|key| !key.trim().is_empty()),
                timeout: Duration::from_secs(seconds),
                max_output_tokens,
            });
        };
        if legacy == "compatible"
            || get("DEEP_MODEL_ID").is_some_and(|model| !model.trim().is_empty())
        {
            register(
                "compatible",
                get("DEEP_MODEL_ID").unwrap_or_default(),
                get("DEEP_MODEL_BASE_URL").unwrap_or_else(|| "http://127.0.0.1:8080/v1".into()),
                get("DEEP_MODEL_API_KEY"),
            );
        }
        if let Some(model) = get("DEEP_LMSTUDIO_MODEL").filter(|model| !model.trim().is_empty()) {
            register(
                "lmstudio",
                model,
                get("DEEP_LMSTUDIO_BASE_URL").unwrap_or_else(|| "http://127.0.0.1:1234/v1".into()),
                get("DEEP_LMSTUDIO_API_KEY"),
            );
        }
        for (id, model_var, key_var, base_url) in [
            (
                "groq",
                "DEEP_GROQ_MODEL",
                "GROQ_API_KEY",
                "https://api.groq.com/openai/v1",
            ),
            (
                "openai",
                "DEEP_OPENAI_MODEL",
                "OPENAI_API_KEY",
                "https://api.openai.com/v1",
            ),
        ] {
            if let Some(model) = get(model_var).filter(|model| !model.trim().is_empty()) {
                register(id, model, base_url.into(), get(key_var));
            }
        }
        let mut config = Self {
            model_catalog: get("DEEP_MODEL_PROFILES_FILE")
                .filter(|path| !path.trim().is_empty())
                .map(|path| {
                    let text = std::fs::read_to_string(path)
                        .map_err(|_| anyhow::anyhow!("Cannot read DEEP_MODEL_PROFILES_FILE."))?;
                    serde_json::from_str(&text).map_err(|_| {
                        anyhow::anyhow!(
                            "DEEP_MODEL_PROFILES_FILE must contain a model profile catalog."
                        )
                    })
                })
                .transpose()?
                .unwrap_or_default(),
            public_origin: get("DEEP_PUBLIC_ORIGIN").filter(|value| !value.trim().is_empty()),
            bind: get("DEEP_BIND")
                .unwrap_or_else(|| "127.0.0.1:4318".into())
                .parse()?,
            data_dir: get("DEEP_DATA_DIR").unwrap_or_else(|| "data".into()).into(),
            web_dir: get("DEEP_WEB_DIR")
                .unwrap_or_else(|| "apps/web/dist".into())
                .into(),
            default_backend,
            backends,
            worker_image: get("DEEP_WORKER_IMAGE").filter(|image| !image.trim().is_empty()),
            search: get("DEEP_SEARXNG_URL")
                .filter(|url| !url.trim().is_empty())
                .map(|base_url| crate::search::SearchConfig {
                    base_url,
                    engine: get("DEEP_SEARXNG_ENGINE").unwrap_or_else(|| "brave".into()),
                }),
            image_profiles: get("DEEP_IMAGE_PROFILES_FILE")
                .filter(|path| !path.trim().is_empty())
                .map(|path| {
                    let text = std::fs::read_to_string(path)
                        .map_err(|_| anyhow::anyhow!("Cannot read DEEP_IMAGE_PROFILES_FILE."))?;
                    serde_json::from_str(&text).map_err(|_| {
                        anyhow::anyhow!(
                            "DEEP_IMAGE_PROFILES_FILE must contain a JSON array of image profiles."
                        )
                    })
                })
                .transpose()?
                .unwrap_or_default(),
        };
        if config.model_catalog.connections.is_empty()
            && runtime.is_some()
            && !config
                .backends
                .iter()
                .any(|backend| backend.id == "compatible")
        {
            bail!("DEEP_MODEL_RUNTIME requires a configured compatible backend");
        }
        if !config.model_catalog.connections.is_empty() {
            // A connection catalog is authoritative; legacy model env vars are
            // used only by the legacy format, never merged into this catalog.
            config
                .backends
                .retain(|backend| backend.kind == BackendKind::Demo);
            for connection in &config.model_catalog.connections {
                config.backends.push(connection.backend(
                    &config.model_catalog,
                    &get,
                    Duration::from_secs(seconds),
                    max_output_tokens,
                )?);
            }
            config.default_backend = "demo".into();
        }
        if let Some(id) = &config.model_catalog.default_profile {
            let profile = config
                .model_catalog
                .profiles
                .iter()
                .find(|p| &p.id == id)
                .ok_or_else(|| {
                    anyhow::anyhow!("Default model profile must name a configured profile.")
                })?;
            let backend = config
                .backends
                .iter_mut()
                .find(|b| b.id == profile.backend)
                .ok_or_else(|| {
                    anyhow::anyhow!("Default model profile must name a configured connection.")
                })?;
            backend.model.clone_from(&profile.model);
            config.default_backend.clone_from(&backend.id);
        }
        for profile in &mut config.image_profiles {
            if profile.provider == crate::images::ImageProvider::Openai {
                profile.api_key = get(profile.api_key_env.as_deref().unwrap_or("OPENAI_API_KEY"))
                    .filter(|key| !key.trim().is_empty());
            }
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if let Some(origin) = &self.public_origin {
            let valid = reqwest::Url::parse(origin).is_ok_and(|url| {
                url.scheme() == "https"
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.path() == "/"
                    && url.query().is_none()
                    && url.fragment().is_none()
            });
            let exact = origin.parse::<axum::http::Uri>().is_ok_and(|uri| {
                uri.authority()
                    .is_some_and(|authority| !authority.as_str().contains('@'))
                    && uri.path_and_query().is_none_or(|path| path.as_str() == "/")
            });
            if !valid || !exact || !self.bind.ip().is_loopback() {
                bail!("DEEP_PUBLIC_ORIGIN requires an exact HTTPS origin and a loopback DEEP_BIND");
            }
        }
        let mut profile_ids = std::collections::HashSet::new();
        for profile in &self.image_profiles {
            profile.validate()?;
            if profile.provider == crate::images::ImageProvider::Openai
                && profile
                    .api_key
                    .as_deref()
                    .is_none_or(|key| key.trim().is_empty())
            {
                bail!("Set the API key for OpenAI image presets.");
            }
            if !profile_ids.insert(&profile.id) {
                bail!("image profile IDs must be unique");
            }
        }
        if let Some(search) = &self.search {
            search.validate()?;
        }
        let mut ids = std::collections::HashSet::new();
        for backend in &self.backends {
            if backend.runtime.is_some()
                && (backend.kind != BackendKind::Compatible
                    || crate::model::llamacpp::metrics_url(&backend.base_url).is_err())
            {
                bail!("llamacpp recovery requires a compatible backend at a loopback HTTP /v1 URL");
            }
            if backend
                .group()
                .is_some_and(|group| !crate::resources::valid_group(group))
            {
                bail!(
                    "Backend resource groups require 1–64 ASCII letters, digits, dots, underscores or hyphens."
                );
            }
            if !crate::resources::valid_group(&backend.id)
                || matches!(backend.id.as_str(), "." | "..")
                || BackendKind::legacy(&backend.id).is_some_and(|kind| kind != backend.kind)
                || !ids.insert(&backend.id)
            {
                bail!(
                    "Connection IDs must be unique portable identifiers; reserved backend names must match their kind."
                );
            }
            if backend.timeout.is_zero() || backend.max_output_tokens == 0 {
                bail!("backend timeout and output-token limit must be positive");
            }
            if backend.kind == BackendKind::Demo {
                if backend.id != "demo" {
                    bail!("The scripted demo uses the reserved demo connection.");
                }
                continue;
            }
            if (backend.model.trim().is_empty()
                && !self
                    .model_catalog
                    .connections
                    .iter()
                    .any(|c| c.id == backend.id))
                || backend.model == "demo-model"
            {
                bail!("set a model ID for backend {}", backend.id);
            }
            let valid = reqwest::Url::parse(&backend.base_url).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none()
            });
            if !valid {
                bail!(
                    "backend {} needs an HTTP(S) API base URL without credentials, query or fragment",
                    backend.id
                );
            }
            if !backend.kind.local()
                && backend
                    .api_key
                    .as_ref()
                    .is_none_or(|key| key.trim().is_empty())
            {
                bail!("set the API key for backend {}", backend.id);
            }
        }
        if !ids.contains(&self.default_backend) {
            bail!("DEEP_DEFAULT_BACKEND must name a configured backend");
        }
        if self
            .backends
            .iter()
            .find(|b| b.id == self.default_backend)
            .is_some_and(|b| b.model.is_empty())
        {
            bail!("The default connection needs a configured model profile.");
        }
        self.model_catalog.validate(&self.backends)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_pc_examples_pair_each_text_and_image_runtime_on_its_own_gpu_group() {
        let directory = tempfile::tempdir().unwrap();
        let models = directory.path().join("models.json");
        let images = directory.path().join("images.json");
        std::fs::write(
            &models,
            include_str!("../../../deploy/models/profiles.multi-pc.example.json"),
        )
        .unwrap();
        std::fs::write(
            &images,
            include_str!("../../../deploy/images/profiles.multi-pc.example.json"),
        )
        .unwrap();
        let cfg = config(&[
            ("DEEP_MODEL_PROFILES_FILE", models.to_str().unwrap()),
            ("DEEP_IMAGE_PROFILES_FILE", images.to_str().unwrap()),
        ])
        .unwrap();
        assert_eq!(cfg.default_backend, "linux-text");
        for (backend, image, port) in [
            ("linux-text", "flux-local", 8000),
            ("second-text", "flux-second", 8001),
        ] {
            let text = cfg.backends.iter().find(|b| b.id == backend).unwrap();
            let media = cfg.image_profiles.iter().find(|p| p.id == image).unwrap();
            assert_eq!(text.group(), Some(media.group()));
            assert_eq!(media.endpoint, format!("http://127.0.0.1:{port}/mcp"));
        }
        assert_ne!(cfg.image_profiles[0].group(), cfg.image_profiles[1].group());
    }

    #[test]
    fn connection_catalog_owns_models_and_resolves_credentials_without_legacy_model_env_vars() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        std::fs::write(&path, r#"{
            "connections":[
                {"id":"desk-text","label":"Desk PC","kind":"lmstudio","api_key_env":"DESK_KEY"},
                {"id":"linux-text","label":"Linux PC","kind":"lmstudio","base_url":"http://192.168.1.20:1234/v1","resource_group":"linux-gpu"},
                {"id":"inventory-only","label":"Another runtime","kind":"compatible"}
            ],
            "default_profile":"saluki",
            "profiles":[
                {"id":"saluki","label":"Saluki","backend":"desk-text","model":"saluki-model","tool_calls":true},
                {"id":"qwen","label":"Qwen","backend":"linux-text","model":"qwen-model","tool_calls":true}
            ]
        }"#).unwrap();
        let path = path.to_str().unwrap();
        let cfg = config(&[
            ("DEEP_MODEL_PROFILES_FILE", path),
            ("DESK_KEY", "private-fixture"),
            ("DEEP_MODEL_ID", "stale-legacy-model"),
            ("DEEP_DEFAULT_BACKEND", "compatible"),
        ])
        .unwrap();
        assert_eq!(cfg.backends.len(), 4);
        assert_eq!(cfg.default_backend, "desk-text");
        assert_eq!(cfg.backends[1].model, "saluki-model");
        assert_eq!(cfg.backends[1].api_key.as_deref(), Some("private-fixture"));
        assert_eq!(cfg.backends[2].kind, BackendKind::Lmstudio);
        assert!(cfg.backends[3].model.is_empty());
        let public = serde_json::to_string(&cfg.model_catalog).unwrap();
        assert!(!public.contains("DESK_KEY"));
        assert!(!public.contains("192.168"));
        assert!(!cfg.backends.iter().any(|b| b.model == "stale-legacy-model"));
    }

    #[test]
    fn private_demo_requires_exact_https_and_loopback_binding() {
        for (origin, bind, expected) in [
            ("https://demo.friend.ts.net", "127.0.0.1:4320", true),
            ("http://demo.friend.ts.net", "127.0.0.1:4320", false),
            ("https://demo.friend.ts.net/path", "127.0.0.1:4320", false),
            ("https://demo.friend.ts.net/a/../", "127.0.0.1:4320", false),
            ("https://@demo.friend.ts.net", "127.0.0.1:4320", false),
            (
                "https://demo.friend.ts.net?token=private",
                "127.0.0.1:4320",
                false,
            ),
            (
                "https://user:secret@demo.friend.ts.net",
                "127.0.0.1:4320",
                false,
            ),
            ("https://demo.friend.ts.net", "0.0.0.0:4320", false),
        ] {
            let result = Config::from_values(|name| match name {
                "DEEP_PUBLIC_ORIGIN" => Some(origin.into()),
                "DEEP_BIND" => Some(bind.into()),
                _ => None,
            });
            assert_eq!(result.is_ok(), expected);
            if let Err(error) = result {
                assert!(
                    !error.to_string().contains("private") && !error.to_string().contains("secret")
                );
            }
        }
    }

    fn config(values: &[(&str, &str)]) -> Result<Config> {
        Config::from_values(|name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, v)| (*v).into())
        })
    }

    #[test]
    fn keys_alone_do_not_enable_remote_backends() {
        let cfg = config(&[
            ("OPENAI_API_KEY", "test-key"),
            ("GROQ_API_KEY", "test-key"),
            ("DEEP_MODEL_ID", ""),
            ("DEEP_LMSTUDIO_MODEL", ""),
            ("DEEP_OPENAI_MODEL", ""),
            ("DEEP_GROQ_MODEL", ""),
        ])
        .unwrap();
        assert_eq!(cfg.default_backend, "demo");
        assert_eq!(cfg.backends.len(), 1);
    }

    #[test]
    fn named_backends_keep_endpoints_and_credentials_separate() {
        let cfg = config(&[
            ("DEEP_DEFAULT_BACKEND", "lmstudio"),
            ("DEEP_LMSTUDIO_MODEL", "local-model"),
            ("DEEP_GROQ_MODEL", "groq-model"),
            ("GROQ_API_KEY", "groq-key"),
            ("DEEP_OPENAI_MODEL", "openai-model"),
            ("OPENAI_API_KEY", "openai-key"),
            ("OPENAI_BASE_URL", "http://unrelated.invalid"),
        ])
        .unwrap();
        assert_eq!(cfg.backends[1].base_url, "http://127.0.0.1:1234/v1");
        assert!(cfg.backends[1].api_key.is_none());
        assert_eq!(cfg.backends[2].base_url, "https://api.groq.com/openai/v1");
        assert_eq!(cfg.backends[2].api_key.as_deref(), Some("groq-key"));
        assert_eq!(cfg.backends[3].base_url, "https://api.openai.com/v1");
        assert_eq!(cfg.backends[3].api_key.as_deref(), Some("openai-key"));
    }

    #[test]
    fn invalid_configuration_fails_without_echoing_values() {
        for values in [
            vec![("DEEP_DEFAULT_BACKEND", "missing")],
            vec![("DEEP_GROQ_MODEL", "model")],
            vec![
                ("DEEP_DEFAULT_BACKEND", "lmstudio"),
                ("DEEP_LMSTUDIO_MODEL", ""),
            ],
            vec![("DEEP_MODEL_TIMEOUT_SECONDS", "0")],
            vec![("DEEP_MODEL_MAX_OUTPUT_TOKENS", "0")],
            vec![("DEEP_PROVIDER", "compatible")],
            vec![
                ("DEEP_LMSTUDIO_MODEL", "model"),
                ("DEEP_LMSTUDIO_BASE_URL", "http://user:secret@localhost/v1"),
            ],
        ] {
            let result = config(&values);
            assert!(result.is_err());
            assert!(!result.err().unwrap().to_string().contains("secret"));
        }
    }

    #[test]
    fn resource_groups_default_by_backend_and_accept_only_operator_identifiers() {
        let cfg = config(&[
            ("DEEP_MODEL_ID", "llama-model"),
            ("DEEP_LMSTUDIO_MODEL", "lm-model"),
            ("DEEP_OPENAI_MODEL", "cloud-model"),
            ("OPENAI_API_KEY", "fixture"),
        ])
        .unwrap();
        assert_eq!(cfg.backends[0].group(), None);
        assert_eq!(cfg.backends[1].group(), Some("local-gpu"));
        assert_eq!(cfg.backends[2].group(), Some("local-gpu"));
        assert_eq!(cfg.backends[3].group(), Some("openai"));
        let cfg = config(&[
            ("DEEP_MODEL_ID", "llama-model"),
            ("DEEP_MODEL_RESOURCE_GROUP", "rtx-a"),
            ("DEEP_LMSTUDIO_MODEL", "lm-model"),
            ("DEEP_LMSTUDIO_RESOURCE_GROUP", "rtx-b"),
        ])
        .unwrap();
        assert_eq!(cfg.backends[1].group(), Some("rtx-a"));
        assert_eq!(cfg.backends[2].group(), Some("rtx-b"));
        for bad in ["a/b", "private secret", "🚀", &"a".repeat(65)] {
            let error = config(&[
                ("DEEP_MODEL_ID", "llama-model"),
                ("DEEP_MODEL_RESOURCE_GROUP", bad),
            ])
            .err()
            .unwrap();
            assert!(!error.to_string().contains(bad));
        }
    }

    #[test]
    fn llamacpp_recovery_is_explicit_and_requires_a_direct_loopback_runtime() {
        let cfg = config(&[("DEEP_MODEL_ID", "fixture")]).unwrap();
        assert!(cfg.backends[1].runtime.is_none());
        let cfg = config(&[
            ("DEEP_MODEL_ID", "fixture"),
            ("DEEP_MODEL_RUNTIME", "llamacpp"),
        ])
        .unwrap();
        assert!(cfg.backends[1].runtime == Some(LocalRuntime::LlamaCpp));
        for url in [
            "http://127.0.0.1:8080/prefix/v1",
            "http://192.168.1.2:8080/v1",
            "https://127.0.0.1:8080/v1",
        ] {
            assert!(
                config(&[
                    ("DEEP_MODEL_ID", "fixture"),
                    ("DEEP_MODEL_RUNTIME", "llamacpp"),
                    ("DEEP_MODEL_BASE_URL", url)
                ])
                .is_err()
            );
        }
        assert!(config(&[("DEEP_MODEL_RUNTIME", "llamacpp")]).is_err());
        assert!(config(&[("DEEP_MODEL_RUNTIME", "infer-from-host")]).is_err());
    }
}
