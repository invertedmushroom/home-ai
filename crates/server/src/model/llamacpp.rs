use super::BackendConfig;
use anyhow::{Result, ensure};
use reqwest::{Client, Url};
use std::time::Duration;

// The direct native chat route returns a complete JSON HTTP 400 for rejected
// validation/context requests, including an error before the first SSE event.
// This is terminal evidence for that request, not a successful completion.
// See llama.cpp 68d20b884 tools/server/server-context.cpp::handle_completions_impl.
// Generic compatible servers have no such declared contract.
pub(super) async fn terminal_rejection(
    response: &mut reqwest::Response,
) -> std::result::Result<bool, reqwest::Error> {
    if response.status() != reqwest::StatusCode::BAD_REQUEST
        || !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
            })
    {
        return Ok(false);
    }
    let read = async {
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 8192 {
                return Ok(false);
            }
            bytes.extend_from_slice(&chunk);
        }
        let Ok(body) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return Ok(false);
        };
        let error = &body["error"];
        Ok(error["code"].as_u64() == Some(400)
            && matches!(
                error["type"].as_str(),
                Some("invalid_request_error" | "exceed_context_size_error")
            )
            && error["message"].as_str().is_some_and(|s| !s.is_empty()))
    };
    // A stalled or partial error body cannot hold the slot for a long model
    // timeout and cannot retire ownership. It retains the ordinary HTTP error.
    match tokio::time::timeout(Duration::from_secs(2), read).await {
        Ok(result) => result,
        Err(_) => Ok(false),
    }
}

pub(crate) fn metrics_url(base: &str) -> Result<Url> {
    let mut url = Url::parse(base)?;
    ensure!(
        url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))
            && url.path() == "/v1"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "llamacpp recovery requires a loopback HTTP /v1 URL"
    );
    url.set_path("/metrics");
    Ok(url)
}

fn idle_metrics(text: &str) -> Result<bool> {
    let mut processing = None;
    let mut deferred = None;
    for line in text.lines() {
        let mut fields = line.split_ascii_whitespace();
        let target = match fields.next() {
            Some("llamacpp:requests_processing") => &mut processing,
            Some("llamacpp:requests_deferred") => &mut deferred,
            _ => continue,
        };
        let value: u64 = fields
            .next()
            .ok_or_else(|| anyhow::anyhow!("Missing runtime gauge"))?
            .parse()?;
        ensure!(
            target.is_none() && fields.next().is_none(),
            "Invalid runtime gauge"
        );
        *target = Some(value);
    }
    ensure!(
        processing.is_some() && deferred.is_some(),
        "Runtime gauges missing"
    );
    Ok(processing == Some(0) && deferred == Some(0))
}

async fn idle(client: &Client, config: &BackendConfig) -> Result<bool> {
    let mut request = client
        .get(metrics_url(&config.base_url)?)
        .timeout(Duration::from_secs(2));
    if let Some(key) = &config.api_key {
        request = request.bearer_auth(key);
    }
    let mut response = request.send().await?;
    ensure!(
        response.status() == reqwest::StatusCode::OK,
        "Runtime metrics unavailable"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 64 * 1024,
            "Runtime metrics too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    idle_metrics(std::str::from_utf8(&bytes)?)
}

// Completion recovery is scoped to this live attempt and its captured runtime.
// Slots alone miss deferred requests. Both queue gauges must report idle twice.
pub(super) async fn confirm_idle(client: &Client, config: &BackendConfig) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut previous_idle = false;
        loop {
            let current_idle = idle(client, config).await?;
            if previous_idle && current_idle {
                return Ok(());
            }
            previous_idle = current_idle;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_both_valid_unique_idle_gauges() {
        let gauges = "llamacpp:requests_processing 0\nllamacpp:requests_deferred 0\n";
        assert!(idle_metrics(gauges).unwrap());
        assert!(!idle_metrics(&gauges.replace("deferred 0", "deferred 1")).unwrap());
        assert!(!idle_metrics(&gauges.replace("processing 0", "processing 1")).unwrap());
        for invalid in [
            "",
            "llamacpp:requests_processing 0",
            "<html>ok</html>",
            &format!("{gauges}llamacpp:requests_deferred 0\n"),
            &gauges.replace("processing 0", "processing NaN"),
            &gauges.replace("processing 0", "processing -1"),
            &gauges.replace("processing 0", "processing 0.5"),
            &gauges.replace("processing 0", "processing 1e-400"),
            &gauges.replace("processing 0", "processing 0 extra"),
        ] {
            assert!(idle_metrics(invalid).is_err(), "{invalid}");
        }
    }
}
