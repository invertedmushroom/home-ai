use super::{ImageProfile, Images, MAX_IMAGE_BYTES, ValidatedImage, decode_image};
use crate::store::images::ImageWork;
use anyhow::{Result, ensure};
use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) fn validate_profile(profile: &ImageProfile) -> Result<()> {
    let url = reqwest::Url::parse(&profile.endpoint)
        .map_err(|_| anyhow::anyhow!("OpenAI image presets need an HTTPS API endpoint."))?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && ((url.scheme() == "https"
                && url.host_str() == Some("api.openai.com")
                && url.port_or_known_default() == Some(443))
                || (url.scheme() == "http" && loopback))
            && url.path().trim_end_matches('/') == "/v1",
        "OpenAI image presets require https://api.openai.com/v1 or a loopback HTTP test endpoint."
    );
    if let Some(name) = &profile.api_key_env {
        ensure!(
            !name.is_empty()
                && name.len() <= 128
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "Image credentials must reference an environment variable."
        );
    }
    let settings = &profile.settings;
    ensure!(
        settings
            .keys()
            .all(|key| matches!(key.as_str(), "model" | "size" | "quality" | "background")),
        "Unsupported OpenAI image preset setting."
    );
    ensure!(
        settings
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))),
        "OpenAI image presets need a bounded model ID."
    );
    ensure!(
        settings
            .get("size")
            .and_then(Value::as_str)
            .is_some_and(|s| s.split_once('x').is_some_and(|(w, h)| {
                match (w.parse::<u32>(), h.parse::<u32>()) {
                    (Ok(w), Ok(h)) => {
                        (16..=2048).contains(&w)
                            && (16..=2048).contains(&h)
                            && w % 16 == 0
                            && h % 16 == 0
                            && w <= 3 * h
                            && h <= 3 * w
                    }
                    _ => false,
                }
            })),
        "OpenAI image presets need WIDTHxHEIGHT, dimensions divisible by 16 and at most 2048, aspect ratio from 1:3 to 3:1."
    );
    for (key, allowed) in [
        (
            "quality",
            &["low", "medium", "high", "xhigh", "max", "auto"][..],
        ),
        ("background", &["transparent", "opaque", "auto"][..]),
    ] {
        ensure!(
            settings
                .get(key)
                .is_none_or(|v| v.as_str().is_some_and(|s| allowed.contains(&s))),
            "Invalid OpenAI image preset {key}."
        );
    }
    Ok(())
}

// A fully read HTTP response establishes completion independently of whether
// its output validates. Lost or oversized responses retain unknown ownership.
async fn request(
    client: &reqwest::Client,
    endpoint: &str,
    key: &str,
    body: &Value,
    editing: bool,
) -> Result<Result<ValidatedImage, &'static str>> {
    let action = if editing { "edits" } else { "generations" };
    let mut response = client
        .post(format!(
            "{}/images/{action}",
            endpoint.trim_end_matches('/')
        ))
        .bearer_auth(key)
        .json(body)
        .send()
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "OpenAI image request completion is unconfirmed; the request will not be retried."
            )
        })?;
    if !response.status().is_success() {
        // Do not return provider response bodies: they may echo credentials or image data.
        return Ok(Err(
            "OpenAI rejected the image request. Check model access, account limits and the selected preset.",
        ));
    }
    let limit = MAX_IMAGE_BYTES.div_ceil(3) * 4 + 65_536;
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= limit as u64),
        "OpenAI image response exceeded the transfer limit; completion is unconfirmed."
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        anyhow::anyhow!("OpenAI image response was interrupted; completion is unconfirmed.")
    })? {
        ensure!(
            bytes.len() + chunk.len() <= limit,
            "OpenAI image response exceeded the transfer limit; completion is unconfirmed."
        );
        bytes.extend_from_slice(&chunk);
    }
    let parsed = (|| -> Result<ValidatedImage> {
        let body: Value = serde_json::from_slice(&bytes)?;
        ensure!(body["error"].is_null(), "Provider returned an error.");
        let data = body["data"]
            .as_array()
            .filter(|data| data.len() == 1)
            .ok_or_else(|| anyhow::anyhow!("Expected one image."))?;
        let encoded = data[0]["b64_json"]
            .as_str()
            .filter(|s| s.len() <= MAX_IMAGE_BYTES.div_ceil(3) * 4)
            .ok_or_else(|| anyhow::anyhow!("Expected bounded image bytes."))?;
        let content = base64::engine::general_purpose::STANDARD.decode(encoded)?;
        let digest = format!("{:x}", Sha256::digest(&content));
        decode_image(content, "image/png", &digest)
    })();
    Ok(parsed
        .map_err(|_| "OpenAI completed the request but returned an invalid or unsupported image."))
}

impl Images {
    pub(super) async fn process_openai(&self, id: String, work: &ImageWork) -> Result<()> {
        let key = self
            .credentials
            .get(&work.profile.info().fingerprint)
            .ok_or_else(|| {
                anyhow::anyhow!("OpenAI image credentials are unavailable for this preset.")
            })?;
        let client = reqwest::Client::builder()
            .timeout(self.deadline)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()?;
        let mut body = work.settings.clone();
        if let Some(input) = &work.job.input_image {
            let (asset, bytes) = self
                .store
                .image_content(input.asset.id.clone())
                .await?
                .ok_or_else(|| anyhow::anyhow!("Reference image is unavailable."))?;
            ensure!(
                asset == input.asset
                    && bytes.len() == asset.bytes as usize
                    && format!("{:x}", Sha256::digest(&bytes)) == asset.sha256,
                "Reference image no longer matches its admitted input."
            );
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
            body["images"] =
                json!([{"image_url":format!("data:{};base64,{encoded}",asset.media_type)}]);
        }
        // Persist dispatch intent with settings only. Pixels and keys stay in this task.
        if !self
            .store
            .image_dispatch(id.clone(), "openai-image-api".into(), work.settings.clone())
            .await?
        {
            return Ok(());
        }
        let outcome = request(
            &client,
            &work.profile.endpoint,
            key,
            &body,
            work.job.input_image.is_some(),
        )
        .await?;
        match outcome {
            Ok(image) => self.store.image_settle(id, Some(image), None).await?,
            Err(error) => {
                self.store
                    .image_settle(id, None, Some(error.into()))
                    .await?
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_presets_bound_endpoints_settings_and_credential_references() {
        let base = json!({"id":"hosted","label":"Hosted","provider":"openai",
            "endpoint":"https://api.openai.com/v1","api_key_env":"OPENAI_API_KEY",
            "settings":{"model":"gpt-image-2.5-flare","size":"1024x1024","quality":"low"}});
        let mut profile: ImageProfile = serde_json::from_value(base.clone()).unwrap();
        profile.validate().unwrap();
        let fingerprint = profile.info().fingerprint;
        profile.api_key = Some("private-fixture-key".into());
        assert_eq!(profile.info().fingerprint, fingerprint);
        assert!(
            !serde_json::to_string(&profile)
                .unwrap()
                .contains("private-fixture-key")
        );
        for endpoint in [
            "https://example.com/v1",
            "http://api.openai.com/v1",
            "https://user:pass@api.openai.com/v1",
            "https://api.openai.com/v1?key=secret",
        ] {
            let mut value = base.clone();
            value["endpoint"] = json!(endpoint);
            assert!(
                serde_json::from_value::<ImageProfile>(value)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        for (key, value) in [
            ("size", json!("4096x4096")),
            ("size", json!("1025x1024")),
            ("quality", json!("invalid")),
            ("activated_loras", json!([])),
            ("n", json!(4)),
        ] {
            let mut candidate = base.clone();
            candidate["settings"][key] = value;
            assert!(
                serde_json::from_value::<ImageProfile>(candidate)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        let mut candidate = base;
        candidate["api_key_env"] = json!("not an environment variable");
        assert!(
            serde_json::from_value::<ImageProfile>(candidate)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}
