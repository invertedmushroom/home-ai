use crate::config::BackendConfig;
pub(crate) mod llamacpp;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    future::Future,
    time::{Duration, Instant},
};
use tracing::Instrument;

use crate::sse::SseDecoder;

const MAX_TEXT_BYTES: usize = 1_048_576;
const MAX_STREAM_BYTES: usize = 8 * MAX_TEXT_BYTES;
const MAX_SAFE_COUNT: i64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Chat,
    App,
    Files,
}
impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::App => "app",
            Self::Files => "files",
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelStats {
    pub reported_model: Option<String>,
    pub finish_reason: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub first_token_ms: Option<i64>,
}

#[derive(Clone, Default)]
pub struct Completion {
    pub workspace_artifact: Option<crate::artifacts::GeneratedArtifact>,
    pub text: String,
    pub stats: ModelStats,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone, Default, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
}

pub struct Tools {
    pub definitions: Vec<Value>,
    pub choice: ToolChoice,
}

impl Completion {
    fn apply(&mut self, body: &Value, streaming: bool, elapsed: Duration) -> Result<()> {
        if !body["error"].is_null() {
            bail!("Model stream reported an error.");
        }
        if !body["model"].is_null() {
            let model = body["model"]
                .as_str()
                .filter(|m| !m.is_empty() && m.len() <= 256)
                .ok_or_else(|| anyhow::anyhow!("Model returned invalid model identity."))?;
            if self
                .stats
                .reported_model
                .as_deref()
                .is_some_and(|previous| previous != model)
            {
                bail!("Model stream changed model identity.");
            }
            self.stats.reported_model = Some(model.into());
        }
        if !body["usage"].is_null() {
            let usage = body["usage"]
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("Model returned invalid token usage."))?;
            for (name, target) in [
                ("prompt_tokens", &mut self.stats.input_tokens),
                ("completion_tokens", &mut self.stats.output_tokens),
                ("total_tokens", &mut self.stats.total_tokens),
            ] {
                if let Some(value) = usage.get(name).filter(|value| !value.is_null()) {
                    *target = Some(
                        value
                            .as_i64()
                            .filter(|count| (0..=MAX_SAFE_COUNT).contains(count))
                            .ok_or_else(|| {
                                anyhow::anyhow!("Model returned invalid token usage.")
                            })?,
                    );
                }
            }
        }
        let choices = body["choices"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Model returned invalid choices."))?;
        if choices.len() > 1 {
            bail!("Model returned an unexpected completion choice.");
        }
        for choice in choices {
            if !choice["index"].is_null() && choice["index"].as_u64() != Some(0) {
                bail!("Model returned an unexpected completion choice.");
            }
            let message = &choice[if streaming { "delta" } else { "message" }];
            if let Some(calls) = message.get("tool_calls").filter(|v| !v.is_null()) {
                ensure_tool_deltas(self, calls, streaming)?;
            }
            if !message["content"].is_null() {
                let text = message["content"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Model returned invalid text content."))?;
                if !text.is_empty() && self.stats.finish_reason.is_some() {
                    bail!("Model returned text after completion.");
                }
                if self.text.len() + text.len() > MAX_TEXT_BYTES {
                    bail!("Model response exceeded 1 MiB.");
                }
                if streaming && !text.is_empty() && self.stats.first_token_ms.is_none() {
                    self.stats.first_token_ms =
                        Some(elapsed.as_millis().min(i64::MAX as u128) as i64);
                }
                self.text.push_str(text);
            }
            if !choice["finish_reason"].is_null() {
                let reason = choice["finish_reason"]
                    .as_str()
                    .filter(|r| !r.is_empty() && r.len() <= 64)
                    .ok_or_else(|| anyhow::anyhow!("Model returned invalid finish reason."))?;
                if self
                    .stats
                    .finish_reason
                    .as_deref()
                    .is_some_and(|previous| previous != reason)
                {
                    bail!("Model stream changed finish reason.");
                }
                self.stats.finish_reason = Some(reason.into());
            }
        }
        Ok(())
    }

    fn validate(&self, streaming: bool, tools: Option<&Tools>) -> Result<()> {
        match self.stats.finish_reason.as_deref() {
            Some("length") => bail!("Model output was truncated; request a smaller result."),
            Some("tool_calls")
                if tools.is_some_and(|tools| tools.choice == ToolChoice::Auto)
                    && !self.tool_calls.is_empty() =>
            {
                return Ok(());
            }
            Some("stop") => {}
            None if !streaming && tools.is_none() => {}
            _ => bail!("Model completion did not finish normally."),
        }
        if !self.tool_calls.is_empty() {
            bail!("Model returned an unauthorized native tool call.");
        }
        if self.text.trim().is_empty() {
            bail!("Model returned no text content.");
        }
        Ok(())
    }
}

fn ensure_tool_deltas(completion: &mut Completion, calls: &Value, streaming: bool) -> Result<()> {
    if completion.stats.finish_reason.is_some() {
        bail!("Model returned tool data after completion.");
    }
    let calls = calls
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid native tool calls."))?;
    if calls.len() > 2 {
        bail!("Model exceeded the tool-call batch limit.");
    }
    for (position, delta) in calls.iter().enumerate() {
        let index = if streaming {
            delta["index"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("Invalid tool-call index."))? as usize
        } else {
            position
        };
        if index >= 2 {
            bail!("Model exceeded the tool-call batch limit.");
        }
        if let Some(kind) = delta.get("type")
            && kind != "function"
        {
            bail!("Only native function calls are supported.");
        }
        completion.tool_calls.resize_with(
            completion.tool_calls.len().max(index + 1),
            ToolCall::default,
        );
        let call = &mut completion.tool_calls[index];
        if let Some(id) = delta.get("id") {
            let id = id
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 128)
                .ok_or_else(|| anyhow::anyhow!("Invalid tool-call identity."))?;
            if !call.id.is_empty() && call.id != id {
                bail!("Model changed tool-call identity.");
            }
            call.id = id.into();
        }
        if let Some(function) = delta.get("function") {
            if !function.is_object() {
                bail!("Invalid function call.");
            }
            for (key, value, limit) in [
                ("name", &mut call.name, 64),
                ("arguments", &mut call.arguments, MAX_TEXT_BYTES),
            ] {
                if let Some(part) = function.get(key) {
                    let part = part
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Invalid function-call data."))?;
                    if value.len() + part.len() > limit {
                        bail!("Tool-call data exceeded its limit.");
                    }
                    value.push_str(part);
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct Model {
    config: BackendConfig,
    client: reqwest::Client,
}

#[derive(Default)]
enum CompletionEvidence {
    #[default]
    Unknown,
    NotSubmitted,
    Generating,
    Terminal,
}

impl Model {
    pub(crate) fn provider(&self) -> &'static str {
        self.config.kind.as_str()
    }
    #[tracing::instrument(target = "deep_server::telemetry", name = "model_stage_wait", skip_all, fields(backend=%self.config.id, round=round))]
    pub(crate) async fn acquire(
        &self,
        store: &crate::store::Store,
        run: &str,
        round: u32,
    ) -> Result<tokio::sync::OwnedSemaphorePermit> {
        let managed = self.config.kind.local();
        let _stage = managed.then(|| store.model_stages.enter(run, round));
        store
            .resources
            .acquire(
                store,
                self.config.group().expect("HTTP backend group"),
                managed.then_some(self.config.base_url.as_str()),
                None,
            )
            .await
    }
    pub fn new(config: BackendConfig) -> Result<Self> {
        let mut client = reqwest::Client::builder()
            .timeout(config.timeout)
            .redirect(reqwest::redirect::Policy::none())
            // A later connection error must not hide an earlier transmitted attempt.
            .retry(reqwest::retry::never());
        if config.runtime.is_some() {
            client = client.no_proxy();
        }
        let client = client.build()?;
        Ok(Self { config, client })
    }

    pub async fn complete<F, Fut>(
        &self,
        messages: Vec<Value>,
        tools: Option<Tools>,
        mut publish: F,
    ) -> Result<Completion>
    where
        F: FnMut(Completion) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        self.receive(
            messages,
            tools,
            &mut publish,
            &mut CompletionEvidence::Unknown,
        )
        .await
    }

    #[tracing::instrument(target = "deep_server::telemetry", name = "model.operation", skip_all,
        fields(backend=%self.config.id, round=call.1, operation_id=tracing::field::Empty,
            outcome=tracing::field::Empty, ownership=tracing::field::Empty, otel.status_code=tracing::field::Empty))]
    pub(crate) async fn complete_owned<F, Fut>(
        &self,
        store: &crate::store::Store,
        call: (&str, u32),
        messages: Vec<Value>,
        tools: Option<Tools>,
        permit: tokio::sync::OwnedSemaphorePermit,
        mut publish: F,
    ) -> Result<Completion>
    where
        F: FnMut(Completion) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        if !self.config.kind.local() {
            let _permit = permit;
            let started = Instant::now();
            let result = self.complete(messages, tools, publish).await;
            let outcome = if result.is_ok() {
                "succeeded"
            } else {
                "failed"
            };
            crate::telemetry::model_call(&self.config.id, outcome, started.elapsed());
            tracing::Span::current().record("outcome", outcome);
            if result.is_err() {
                tracing::Span::current().record("otel.status_code", "ERROR");
            }
            return result;
        }
        let model = self.clone();
        let store = store.clone();
        let run_id = call.0.to_owned();
        let round = call.1;
        let (sender, mut progress) = tokio::sync::watch::channel::<Option<Completion>>(None);
        // Dropping the subscriber detaches this task. Only terminal evidence retires
        // its durable ownership. Progress delivery cannot abort inference cleanup.
        let mut task = tokio::spawn(async move {
            let _permit = permit;
            let group = model.config.group().expect("HTTP backend group").to_owned();
            let _stage = store.model_stages.enter(&run_id, round);
            let id = store.begin_model_call(run_id.clone(), round, model.config.base_url.clone(),group.clone()).await?;
            _stage.dispatched(id.clone());
            tracing::Span::current().record("operation_id", id.as_str());
            tracing::info!(operation_id=%id, resource_group=%group, "local model dispatch intent committed");
            let mut evidence = CompletionEvidence::Unknown;
            let started = Instant::now();
            let publish_sender = sender.clone();
            let result = {
                let receive = model.receive(messages, tools, move |part| {
                    publish_sender.send_replace(Some(part));
                    std::future::ready(Ok(()))
                }, &mut evidence);
                if model.config.runtime.is_some() {
                    tokio::select! {
                        result = receive => result,
                        _ = sender.closed() => Err(anyhow::anyhow!("Local inference subscriber stopped.")),
                    }
                } else { receive.await }
            };
            let observed_outcome = if result.is_ok() { "succeeded" } else {
                match evidence {
                    CompletionEvidence::NotSubmitted => "request_not_sent",
                    CompletionEvidence::Terminal => "failed",
                    _ => "unresolved",
                }
            };
            crate::telemetry::model_call(&model.config.id, observed_outcome, started.elapsed());
            tracing::Span::current().record("outcome", observed_outcome);
            if result.is_err() { tracing::Span::current().record("otel.status_code", "ERROR"); }
            let outcome = match evidence {
                CompletionEvidence::Terminal => crate::store::ModelCallOutcome::Completed,
                CompletionEvidence::NotSubmitted => crate::store::ModelCallOutcome::NotSubmitted,
                _ => crate::store::ModelCallOutcome::Unresolved,
            };
            let recovering = matches!(evidence, CompletionEvidence::Generating) && model.config.runtime.is_some();
            if recovering { _stage.recovering(); }
            store.finish_model_call(id.clone(), outcome).await?;
            tracing::Span::current().record("ownership", match evidence {
                CompletionEvidence::Terminal => "completed",
                CompletionEvidence::NotSubmitted => "request_not_sent",
                _ => "unresolved",
            });
            if matches!(evidence, CompletionEvidence::Unknown | CompletionEvidence::Generating) {
                tracing::warn!(operation_id=%id, resource_group=%group,
                    "local resource blocked: upstream completion unconfirmed; stop Deep, inspect model-calls, verify runtime idle, then resolve the exact operation");
            }
            tracing::info!(operation_id=%id,
                terminal_confirmed=matches!(evidence, CompletionEvidence::Terminal),
                request_not_sent=matches!(evidence, CompletionEvidence::NotSubmitted),
                "local model ownership settled");
            if recovering {
                // The foreground can fail now. Cleanup owns the same slot until
                // runtime evidence is committed or the bounded probe fails closed.
                let recovery_span = tracing::info_span!(target: crate::telemetry::TARGET, "model.recovery",
                    operation_id=%id, resource_group=%group, outcome=tracing::field::Empty);
                tokio::spawn(async move {
                    let _permit = _permit;
                    let _stage = _stage;
                    let started = Instant::now();
                    let outcome = if llamacpp::confirm_idle(&model.client,&model.config).await.is_ok() {
                        match store.confirm_model_idle(id.clone()).await {
                            Ok(()) => { tracing::info!(operation_id=%id,"llamacpp confirmed no running or deferred work; ownership resolved"); "confirmed_idle" },
                            Err(_) => { tracing::error!(operation_id=%id,"could not persist runtime idle confirmation; ownership remains blocked"); "persistence_failed" },
                        }
                    } else {
                        tracing::warn!(operation_id=%id,"llamacpp idle confirmation failed; ownership remains blocked for operator recovery");
                        "probe_failed"
                    };
                    crate::telemetry::recovery_finished(&group, outcome, started.elapsed());
                    tracing::Span::current().record("outcome", outcome);
                }.instrument(recovery_span));
            }
            result
        }.in_current_span());
        let mut subscribed = true;
        let completion = loop {
            tokio::select! {
                biased;
                result = &mut task => break result.map_err(|_| anyhow::anyhow!("Local inference task stopped; operator inspection is required."))??,
                changed = progress.changed(), if subscribed => {
                    subscribed = changed.is_ok();
                    let part = progress.borrow_and_update().clone();
                    if let Some(part) = part { publish(part).await?; }
                }
            }
        };
        publish(completion.clone()).await?;
        Ok(completion)
    }

    #[tracing::instrument(target = "deep_server::telemetry", name = "model_call", skip_all, fields(backend=%self.config.id))]
    async fn receive<F, Fut>(
        &self,
        messages: Vec<Value>,
        tools: Option<Tools>,
        mut publish: F,
        evidence: &mut CompletionEvidence,
    ) -> Result<Completion>
    where
        F: FnMut(Completion) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let mut body = json!({"model":self.config.model,"messages":messages,"stream":true});
        if let Some(tools) = &tools {
            body["tools"] = json!(tools.definitions);
            body["tool_choice"] = json!(tools.choice);
            body["parallel_tool_calls"] = json!(false);
        }
        if matches!(
            self.config.kind,
            crate::config::BackendKind::Openai
                | crate::config::BackendKind::Groq
                | crate::config::BackendKind::Lmstudio
        ) {
            body["stream_options"] = json!({"include_usage":true});
        }
        let token_parameter = if matches!(
            self.config.kind,
            crate::config::BackendKind::Openai | crate::config::BackendKind::Groq
        ) {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        body[token_parameter] = json!(self.config.max_output_tokens);
        let mut request = self
            .client
            .post(format!("{}/chat/completions", self.config.base_url))
            .json(&body);
        if let Some(key) = &self.config.api_key {
            request = request.bearer_auth(key)
        }
        // Never propagate response bodies or request URLs into user errors: backends may echo secrets.
        let started = Instant::now();
        let mut response = request.send().await.map_err(|error| {
            // reqwest's connector failed before HTTP transmission. Check this
            // only on send(), never on a response-body or decoding failure.
            if error.is_connect() {
                *evidence = CompletionEvidence::NotSubmitted;
            }
            self.transport_error(error)
        })?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            if self.config.runtime == Some(crate::config::LocalRuntime::LlamaCpp)
                && llamacpp::terminal_rejection(&mut response)
                    .await
                    .map_err(|error| self.transport_error(error))?
            {
                *evidence = CompletionEvidence::Terminal;
            }
            bail!("Model backend returned HTTP {}.", status)
        }
        let streaming = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.split(';').next() == Some("text/event-stream"));
        let mut bytes = Vec::new();
        let mut decoder = SseDecoder::default();
        let mut completion = Completion::default();
        let mut wire_bytes = 0;
        let mut published_text = false;
        let mut last_publish = Instant::now();
        let mut done = false;
        let mut stream_error = None;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| self.transport_error(error))?
        {
            wire_bytes += chunk.len();
            if wire_bytes
                > if streaming {
                    MAX_STREAM_BYTES
                } else {
                    MAX_TEXT_BYTES
                }
            {
                bail!("Model response exceeded its transport size limit.")
            }
            if !streaming {
                bytes.extend_from_slice(&chunk);
                continue;
            }
            for event in decoder.push(&chunk)? {
                if event == "[DONE]" {
                    done = true;
                    *evidence = CompletionEvidence::Terminal;
                    break;
                }
                // Output errors do not prove inference has stopped. Keep draining
                // within the existing transport bounds to observe terminal evidence.
                if stream_error.is_some() {
                    continue;
                }
                let body: Value = match serde_json::from_str(&event) {
                    Ok(body) => body,
                    Err(_) => {
                        stream_error = Some(anyhow::anyhow!("Model stream returned invalid JSON."));
                        continue;
                    }
                };
                if let Err(error) = completion.apply(&body, true, started.elapsed()) {
                    stream_error = Some(error);
                    continue;
                }
                if generation_observed(&body) {
                    *evidence = CompletionEvidence::Generating;
                }
            }
            if !done
                && stream_error.is_none()
                && ((!published_text && !completion.text.is_empty())
                    || last_publish.elapsed() >= Duration::from_millis(50))
            {
                publish(completion.clone()).await?;
                published_text |= !completion.text.is_empty();
                last_publish = Instant::now();
            }
            if done {
                break;
            }
        }
        if !streaming {
            *evidence = CompletionEvidence::Terminal;
            let body: Value = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("Model returned invalid JSON."))?;
            completion.apply(&body, false, started.elapsed())?;
        }
        if let Some(error) = stream_error {
            return Err(error);
        }
        publish(completion.clone()).await?;
        if streaming && !done {
            bail!("Model stream ended before its completion marker.");
        }
        completion.validate(streaming, tools.as_ref())?;
        tracing::info!(duration_ms=started.elapsed().as_millis() as u64,
            input_tokens=?completion.stats.input_tokens, output_tokens=?completion.stats.output_tokens,
            "model call completed");
        Ok(completion)
    }

    fn transport_error(&self, error: reqwest::Error) -> anyhow::Error {
        // reqwest's Display includes the private URL. Classify without forwarding it.
        crate::telemetry::transport_failure(
            &self.config.id,
            if error.is_timeout() {
                "timeout"
            } else if error.is_connect() {
                "connection"
            } else {
                "transport"
            },
        );
        if error.is_timeout() {
            anyhow::anyhow!(
                "Model request exceeded its {}-second timeout; check the model runtime or DEEP_MODEL_TIMEOUT_SECONDS.",
                self.config.timeout.as_secs_f64()
            )
        } else {
            anyhow::anyhow!("Model backend connection failed; check its address and runtime.")
        }
    }
}

fn generation_observed(body: &Value) -> bool {
    body["choices"].as_array().is_some_and(|choices| {
        choices.iter().any(|choice| {
            let delta = &choice["delta"];
            ["content", "reasoning_content"]
                .iter()
                .any(|field| delta[field].as_str().is_some_and(|text| !text.is_empty()))
                || delta["tool_calls"].as_array().is_some_and(|calls| {
                    calls.iter().any(|call| {
                        call["function"]["arguments"]
                            .as_str()
                            .is_some_and(|text| !text.is_empty())
                    })
                })
        })
    })
}

pub fn extract_html(content: &str) -> Option<String> {
    let start = ["```html\n", "```html\r\n"]
        .into_iter()
        .filter_map(|fence| content.find(fence).map(|pos| pos + fence.len()))
        .min()?;
    let tail = &content[start..];
    // Use the last standalone closing fence, not literal backticks inside the document.
    let end = tail.rmatch_indices("\n```").find_map(|(pos, _)| {
        let rest = tail[pos + 4..].split('\n').next()?;
        rest.trim().is_empty().then_some(pos)
    })?;
    let html = tail[..end].trim();
    (!html.is_empty()).then(|| html.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_code_arguments_above_two_kib_decode_in_json_and_streaming() {
        let arguments = json!({
            "code": format!("// {}\nconsole.log(42);", "x".repeat(4096)),
            "input_asset_ids": []
        })
        .to_string();
        for streaming in [false, true] {
            let mut completion = Completion::default();
            let fragments: Vec<_> = if streaming {
                arguments
                    .as_bytes()
                    .chunks(257)
                    .map(|part| std::str::from_utf8(part).unwrap())
                    .collect()
            } else {
                vec![arguments.as_str()]
            };
            for (index, fragment) in fragments.iter().enumerate() {
                let message = json!({"tool_calls":[{
                    "index":0, "id":"call_compute", "type":"function",
                    "function":{
                        "name": if index == 0 { "compute_node" } else { "" },
                        "arguments":fragment
                    }
                }]});
                let field = if streaming { "delta" } else { "message" };
                completion
                    .apply(
                        &json!({"choices":[{"index":0, (field):message}]}),
                        streaming,
                        Duration::ZERO,
                    )
                    .unwrap();
            }
            assert_eq!(completion.tool_calls.len(), 1);
            assert_eq!(completion.tool_calls[0].name, "compute_node");
            assert_eq!(completion.tool_calls[0].arguments, arguments);
        }
    }

    #[test]
    fn native_arguments_enforce_the_exact_utf8_byte_limit_in_json_and_streaming() {
        for streaming in [false, true] {
            for size in [MAX_TEXT_BYTES - 1, MAX_TEXT_BYTES, MAX_TEXT_BYTES + 1] {
                let arguments = "界".repeat(size / 3) + &"x".repeat(size % 3);
                let mut completion = Completion::default();
                let calls = |part: &str| json!([{"index":0,"function":{"arguments":part}}]);
                let result = if streaming {
                    ensure_tool_deltas(&mut completion, &calls(&arguments[..3000]), true).unwrap();
                    ensure_tool_deltas(&mut completion, &calls(&arguments[3000..]), true)
                } else {
                    ensure_tool_deltas(&mut completion, &calls(&arguments), false)
                };
                assert_eq!(result.is_ok(), size <= MAX_TEXT_BYTES, "{result:?}");
                if result.is_ok() {
                    assert_eq!(completion.tool_calls[0].arguments, arguments);
                }
            }
        }
    }

    #[test]
    fn invalid_observations_and_provider_errors_are_rejected_without_echoing_content() {
        for body in [
            json!({"error":{"message":"private-test-value"}}),
            json!({"choices":[],"usage":{"prompt_tokens":i64::MAX}}),
            json!({"choices":[{"index":"private-test-value","delta":{"content":"Answer"}}]}),
            json!({"choices":[{"index":0,"delta":{"content":"One"}},{"index":0,"delta":{"content":"Two"}}]}),
        ] {
            let error = Completion::default()
                .apply(&body, true, Duration::ZERO)
                .err()
                .unwrap();
            assert!(!error.to_string().contains("private-test-value"));
        }
    }
}
