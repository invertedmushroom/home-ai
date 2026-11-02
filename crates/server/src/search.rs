use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, time::Duration};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchConfig {
    pub base_url: String,
    pub engine: String,
}

impl SearchConfig {
    pub fn validate(&self) -> Result<()> {
        let url = reqwest::Url::parse(&self.base_url)
            .map_err(|_| anyhow::anyhow!("Invalid SearXNG configuration."))?;
        ensure!(
            url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none(),
            "SearXNG must use a literal loopback HTTP origin."
        );
        ensure!(
            !self.engine.is_empty()
                && self.engine.len() <= 64
                && self.engine.as_bytes()[0].is_ascii_alphanumeric()
                && self
                    .engine
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_ -".contains(&c))
                && self.engine.trim() == self.engine,
            "Invalid SearXNG engine."
        );
        Ok(())
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Source>> {
        self.validate()?;
        ensure!(valid_query(query), "Invalid search query.");
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(8))
            .build()?;
        let mut url = reqwest::Url::parse(&self.base_url)?;
        url.set_path("/search");
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("format", "json")
            .append_pair("engines", &self.engine);
        let mut response = client
            .get(url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Search service unavailable or timed out."))?;
        ensure!(
            response.status() == reqwest::StatusCode::OK,
            "Search service returned HTTP {}.",
            response.status().as_u16()
        );
        ensure!(
            response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v
                    .split(';')
                    .next()
                    .is_some_and(|v| v.trim() == "application/json")),
            "Search returned an incompatible content type."
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("Search response transport failed."))?
        {
            ensure!(
                bytes.len() + chunk.len() <= 262_144,
                "Search response exceeded 256 KiB."
            );
            bytes.extend_from_slice(&chunk);
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| anyhow::anyhow!("Search returned invalid UTF-8."))?;
        let body: Value = serde_json::from_str(text)
            .map_err(|_| anyhow::anyhow!("Search returned invalid JSON."))?;
        decode(&body, &self.engine)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchReceipt {
    pub id: String,
    pub call_id: String,
    pub query: String,
    pub engine: String,
    pub status: String,
    pub elapsed_ms: Option<i64>,
    pub error: Option<String>,
    pub sources: Vec<Source>,
}

pub fn valid_query(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 512
        && !value.bytes().any(|c| c < 32 || c == 127)
        && !value
            .split_whitespace()
            .any(|word| word.starts_with(['!', ':', '\\']))
}

pub fn valid_source_url(value: &str) -> bool {
    if value.len() > 2048 || value.bytes().any(|c| c <= 32 || c == 127) {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    if url.as_str().len() > 2048
        || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    host.contains('.')
        && host.len() <= 253
        && !host.ends_with('.')
        && host.parse::<std::net::IpAddr>().is_err()
        && !host.starts_with('[')
        && ![
            "localhost",
            "local",
            "internal",
            "test",
            "invalid",
            "example",
        ]
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

fn text(value: Option<&str>, limit: usize) -> String {
    value
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}

fn decode(body: &Value, engine: &str) -> Result<Vec<Source>> {
    ensure!(
        body["unresponsive_engines"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "Search reported an upstream engine failure or incompatible status."
    );
    let items = body["results"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Search returned invalid results."))?;
    if items.is_empty() {
        for key in ["answers", "infoboxes"] {
            if body
                .get(key)
                .is_some_and(|v| v.as_array().is_none_or(|v| !v.is_empty()))
            {
                bail!("Search returned unsupported answer-only results.");
            }
        }
    }
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for item in items {
        ensure!(
            item["engine"].as_str() == Some(engine)
                && item["title"].as_str().is_some_and(|v| !v.trim().is_empty())
                && item["url"].as_str().is_some_and(valid_source_url)
                && item["content"].is_string(),
            "Search returned invalid results or unexpected engine provenance."
        );
        if let Some(engines) = item.get("engines") {
            ensure!(
                engines
                    .as_array()
                    .is_some_and(|v| !v.is_empty() && v.iter().all(|e| e.as_str() == Some(engine))),
                "Search returned unexpected engine provenance."
            );
        }
    }
    for item in items.iter().take(25) {
        let Some(url) = item["url"].as_str().filter(|url| valid_source_url(url)) else {
            continue;
        };
        let mut url = reqwest::Url::parse(url)?;
        url.set_fragment(None);
        let title = text(item["title"].as_str(), 200);
        if title.is_empty() || !seen.insert(url.to_string()) {
            continue;
        }
        result.push(Source {
            id: String::new(),
            url: url.to_string(),
            title,
            snippet: text(item["content"].as_str(), 700),
        });
        if result.len() == 5 {
            break;
        }
    }
    ensure!(
        items.is_empty() || !result.is_empty(),
        "Search returned no usable source links."
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_configuration_queries_and_source_links_are_bounded() {
        for base in [
            "https://127.0.0.1:8888",
            "http://localhost:8888",
            "http://127.0.0.1:8888/path",
            "http://user:secret@127.0.0.1:8888",
        ] {
            assert!(
                SearchConfig {
                    base_url: base.into(),
                    engine: "brave".into()
                }
                .validate()
                .is_err()
            );
        }
        assert!(valid_query("site:rust-lang.org Unicode 界"));
        assert!(valid_query("\u{feff}"));
        assert!(valid_query("x\u{feff}!literal"));
        for query in [
            "",
            "!brave query",
            "query :engine",
            "query\nheader",
            "\u{85}",
            "x\u{85}!engine",
        ] {
            assert!(!valid_query(query));
        }
        assert!(!valid_query(&"界".repeat(171)));
        for source in [
            "javascript:alert(1)",
            "http://127.0.0.1/",
            "http://2130706433/",
            "http://localhost/",
            "https://user:secret@www.rust-lang.org/",
            "https://docs.internal/",
            "https://www.rust-lang.org:8443/",
        ] {
            assert!(!valid_source_url(source), "{source}");
        }
        assert!(valid_source_url("https://doc.rust-lang.org/book/"));
    }
    #[test]
    fn partial_unknown_and_answer_only_results_are_not_successful_empty_searches() {
        for body in [
            serde_json::json!({"results":[],"unresponsive_engines":[["brave","timeout"]]}),
            serde_json::json!({"results":[],"answers":["An answer"],"unresponsive_engines":[]}),
            serde_json::json!({"results":[],"unresponsive_engines":"unknown"}),
            serde_json::json!({"results":[{"title":"Docs","url":"https://www.rust-lang.org/","content":"text","engine":"other"}],"unresponsive_engines":[]}),
        ] {
            assert!(decode(&body, "brave").is_err());
        }
        assert!(
            decode(
                &serde_json::json!({"results":[],"unresponsive_engines":[]}),
                "brave"
            )
            .unwrap()
            .is_empty()
        );
    }
}
