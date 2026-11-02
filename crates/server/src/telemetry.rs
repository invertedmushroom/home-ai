//! Optional local OTLP export. Only explicitly selected spans/events cross this seam.
use crate::{
    config::Config,
    store::{Run, Store},
};
use anyhow::{Result, ensure};
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Gauge, Histogram, Meter, MeterProvider},
    trace::{TraceContextExt, TracerProvider},
};
use opentelemetry_otlp::{Protocol, RetryPolicy, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    Resource,
    metrics::{PeriodicReader, SdkMeterProvider},
    trace::{BatchConfigBuilder, BatchSpanProcessor, Sampler, SdkTracerProvider},
};
use std::{
    collections::HashSet,
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{
    EnvFilter, Layer, filter::filter_fn, layer::SubscriberExt, util::SubscriberInitExt,
};

pub(crate) const TARGET: &str = "deep_server::telemetry";
static METRICS: OnceLock<Metrics> = OnceLock::new();
const EXPORT_TIMEOUT: Duration = Duration::from_secs(1);
const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

fn endpoint(get: impl Fn(&str) -> Option<String>) -> Result<Option<String>> {
    if get("OTEL_SDK_DISABLED").is_some_and(|v| v.trim().eq_ignore_ascii_case("true")) {
        return Ok(None);
    }
    let Some(value) = get("OTEL_EXPORTER_OTLP_ENDPOINT").filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    let url = reqwest::Url::parse(value.trim())
        .map_err(|_| anyhow::anyhow!("OTLP endpoint must be a loopback HTTP origin."))?;
    ensure!(
        url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "OTLP endpoint must be a loopback HTTP origin."
    );
    Ok(Some(url.as_str().trim_end_matches('/').to_owned()))
}

/// Owns exporter threads and the optional read-only state sampler.
pub struct Telemetry {
    traces: Option<SdkTracerProvider>,
    metrics: Option<SdkMeterProvider>,
    sampler: Option<tokio::task::JoinHandle<()>>,
}

/// Own background export and sampling without blocking an async caller.
pub async fn init(config: &Config, store: Store) -> Result<Telemetry> {
    let config = config.clone();
    let mut telemetry = tokio::task::spawn_blocking(move || initialize(&config)).await??;
    telemetry.start_sampler(store);
    Ok(telemetry)
}

fn initialize(config: &Config) -> Result<Telemetry> {
    let endpoint = endpoint(|key| std::env::var(key).ok())?;
    let (traces, metrics) = if let Some(endpoint) = endpoint {
        let client = reqwest_otel::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest_otel::redirect::Policy::none())
            .retry(reqwest_otel::retry::never())
            .timeout(EXPORT_TIMEOUT)
            .build()?;
        // Empty resources avoid ambient OTEL_RESOURCE_ATTRIBUTES/user/host detectors.
        let resource = Resource::builder_empty()
            .with_attributes([
                KeyValue::new("service.name", "deep-server"),
                KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                KeyValue::new("service.instance.id", uuid::Uuid::new_v4().to_string()),
            ])
            .build();
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(format!("{endpoint}/v1/traces"))
            .with_protocol(Protocol::HttpBinary)
            .with_http_client(client.clone())
            .with_timeout(EXPORT_TIMEOUT)
            .with_retry_policy(RetryPolicy::disabled())
            .build()?;
        let batch = BatchSpanProcessor::builder(exporter)
            .with_batch_config(
                BatchConfigBuilder::default()
                    .with_max_queue_size(512)
                    .with_max_export_batch_size(64)
                    .with_scheduled_delay(Duration::from_secs(1))
                    .build(),
            )
            .build();
        let traces = SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_sampler(Sampler::AlwaysOn)
            .with_max_attributes_per_span(16)
            .with_max_events_per_span(64)
            .with_max_links_per_span(0)
            .with_span_processor(batch)
            .build();
        let exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .with_endpoint(format!("{endpoint}/v1/metrics"))
            .with_protocol(Protocol::HttpBinary)
            .with_http_client(client)
            .with_timeout(EXPORT_TIMEOUT)
            .with_retry_policy(RetryPolicy::disabled())
            .build()?;
        let reader = PeriodicReader::builder(exporter)
            .with_interval(SAMPLE_INTERVAL)
            .build();
        let metrics = SdkMeterProvider::builder()
            .with_resource(resource)
            .with_reader(reader)
            .build();
        let mut groups: HashSet<String> = config
            .backends
            .iter()
            .filter_map(|b| b.group().map(str::to_owned))
            .collect();
        groups.extend(config.image_profiles.iter().map(|p| p.group().to_owned()));
        ensure!(
            METRICS
                .set(Metrics::new(metrics.meter("deep-server"), groups))
                .is_ok(),
            "Telemetry is already initialized."
        );
        (Some(traces), Some(metrics))
    } else {
        (None, None)
    };
    let otel = traces.as_ref().map(|provider| {
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("deep-server"))
            .with_context_activation(false)
            .with_location(false)
            .with_threads(false)
            .with_tracked_inactivity(false)
            .with_error_events_to_exceptions(false)
            .with_error_fields_to_exceptions(false)
            .with_error_records_to_exceptions(false)
            .with_error_events_to_status(false)
            .with_filter(filter_fn(|metadata| metadata.target() == TARGET))
    });
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer().with_filter(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "deep_server=info,tower_http=info".into()),
            ),
        )
        .with(otel)
        .try_init()?;
    Ok(Telemetry {
        traces,
        metrics,
        sampler: None,
    })
}

impl Telemetry {
    fn start_sampler(&mut self, store: Store) {
        let Some(metrics) = METRICS.get() else { return };
        self.sampler = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(SAMPLE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                // Keep one read in flight. Timing out Store::call would detach its
                // blocking work and allow later samples to accumulate behind it.
                match store.telemetry_snapshot().await {
                    Ok(snapshot) => {
                        metrics.run_queued.record(snapshot.queued, &[]);
                        metrics.run_active.record(snapshot.running, &[]);
                        let slots = store.resources.snapshot();
                        for group in metrics
                            .groups
                            .iter()
                            .map(String::as_str)
                            .chain(std::iter::once("unconfigured"))
                        {
                            let (mut active, mut queued, mut unresolved, mut age) =
                                (0, 0, 0, 0.0_f64);
                            for (name, used, waiting) in &slots {
                                if metrics.group(name) == group {
                                    active += used;
                                    queued += waiting;
                                }
                            }
                            for (name, count, seconds) in &snapshot.unresolved {
                                if metrics.group(name) == group {
                                    unresolved += count;
                                    age = age.max(*seconds);
                                }
                            }
                            let attributes = [KeyValue::new("resource_group", group.to_owned())];
                            metrics.resource_active.record(active, &attributes);
                            metrics.resource_queued.record(queued, &attributes);
                            metrics.unresolved.record(unresolved, &attributes);
                            metrics.unresolved_age.record(age, &attributes);
                        }
                        if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
                            metrics.snapshot_time.record(now.as_secs_f64(), &[]);
                        }
                    }
                    _ => metrics.snapshot_failures.add(1, &[]),
                }
            }
        }));
    }

    pub async fn shutdown(mut self) {
        if let Some(sampler) = self.sampler.take() {
            sampler.abort();
            let _ = sampler.await;
        }
        let _ = tokio::task::spawn_blocking(move || {
            if let Some(provider) = self.traces {
                let _ = provider.shutdown_with_timeout(Duration::from_secs(2));
            }
            if let Some(provider) = self.metrics {
                let _ = provider.shutdown_with_timeout(Duration::from_secs(2));
            }
        })
        .await;
    }
}

struct Metrics {
    groups: HashSet<String>,
    runs: Counter<u64>,
    run_duration: Histogram<f64>,
    queue_duration: Histogram<f64>,
    model_calls: Counter<u64>,
    model_duration: Histogram<f64>,
    transport_failures: Counter<u64>,
    acquisitions: Counter<u64>,
    resource_wait: Histogram<f64>,
    image_attempts: Counter<u64>,
    image_duration: Histogram<f64>,
    recoveries: Counter<u64>,
    recovery_duration: Histogram<f64>,
    run_queued: Gauge<u64>,
    run_active: Gauge<u64>,
    resource_active: Gauge<u64>,
    resource_queued: Gauge<u64>,
    unresolved: Gauge<u64>,
    unresolved_age: Gauge<f64>,
    snapshot_time: Gauge<f64>,
    snapshot_failures: Counter<u64>,
}

impl Metrics {
    fn new(meter: Meter, groups: HashSet<String>) -> Self {
        let histogram = |name| {
            meter
                .f64_histogram(name)
                .with_unit("s")
                .with_boundaries(vec![
                    0.001, 0.01, 0.05, 0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0,
                    900.0, 1800.0,
                ])
                .build()
        };
        Self {
            groups,
            runs: meter.u64_counter("deep.run.finished").build(),
            run_duration: histogram("deep.run.duration"),
            queue_duration: histogram("deep.run.queue.duration"),
            model_calls: meter.u64_counter("deep.model.calls").build(),
            model_duration: histogram("deep.model.duration"),
            transport_failures: meter.u64_counter("deep.model.transport.failures").build(),
            acquisitions: meter.u64_counter("deep.resource.acquisitions").build(),
            resource_wait: histogram("deep.resource.wait.duration"),
            image_attempts: meter.u64_counter("deep.image.attempts").build(),
            image_duration: histogram("deep.image.duration"),
            recoveries: meter.u64_counter("deep.model.recoveries").build(),
            recovery_duration: histogram("deep.model.recovery.duration"),
            run_queued: meter.u64_gauge("deep.run.queued").build(),
            run_active: meter.u64_gauge("deep.run.active").build(),
            resource_active: meter.u64_gauge("deep.resource.active").build(),
            resource_queued: meter.u64_gauge("deep.resource.queued").build(),
            unresolved: meter.u64_gauge("deep.ownership.unresolved").build(),
            unresolved_age: meter
                .f64_gauge("deep.ownership.unresolved.age")
                .with_unit("s")
                .build(),
            snapshot_time: meter
                .f64_gauge("deep.observability.snapshot.timestamp")
                .with_unit("s")
                .build(),
            snapshot_failures: meter
                .u64_counter("deep.observability.snapshot.failures")
                .build(),
        }
    }

    fn group<'a>(&self, group: &'a str) -> &'a str {
        if self.groups.contains(group) {
            group
        } else {
            "unconfigured"
        }
    }
}

fn backend(value: &str) -> &str {
    if matches!(
        value,
        "demo" | "compatible" | "lmstudio" | "openai" | "groq"
    ) {
        value
    } else {
        "unknown"
    }
}

pub(crate) fn enabled() -> bool {
    METRICS.get().is_some()
}

pub(crate) fn trace_id(span: &tracing::Span) -> Option<String> {
    enabled().then(|| span.context().span().span_context().trace_id().to_string())
}

pub(crate) fn run_finished(run: &Run) {
    if !matches!(
        run.status.as_str(),
        "succeeded" | "failed" | "cancelled" | "interrupted"
    ) {
        return;
    }
    if let Some(metrics) = METRICS.get() {
        let attributes = [
            KeyValue::new("backend", backend(&run.provider).to_owned()),
            KeyValue::new("outcome", run.status.clone()),
        ];
        metrics.runs.add(1, &attributes);
        if let Some(ms) = run.duration_ms {
            metrics.run_duration.record(ms as f64 / 1000.0, &attributes);
        }
        if let Some(ms) = run.queue_ms {
            metrics
                .queue_duration
                .record(ms as f64 / 1000.0, &attributes);
        }
    }
}

pub(crate) fn model_call(provider: &str, outcome: &'static str, duration: Duration) {
    if let Some(metrics) = METRICS.get() {
        let attributes = [
            KeyValue::new("backend", backend(provider).to_owned()),
            KeyValue::new("outcome", outcome),
        ];
        metrics.model_calls.add(1, &attributes);
        metrics
            .model_duration
            .record(duration.as_secs_f64(), &attributes);
    }
}

pub(crate) fn transport_failure(provider: &str, kind: &'static str) {
    if let Some(metrics) = METRICS.get() {
        metrics.transport_failures.add(
            1,
            &[
                KeyValue::new("backend", backend(provider).to_owned()),
                KeyValue::new("kind", kind),
            ],
        );
    }
}

pub(crate) fn resource_acquired(group: &str, duration: Duration) {
    if let Some(metrics) = METRICS.get() {
        let attributes = [KeyValue::new(
            "resource_group",
            metrics.group(group).to_owned(),
        )];
        metrics
            .resource_wait
            .record(duration.as_secs_f64(), &attributes);
        metrics.acquisitions.add(
            1,
            &[attributes[0].clone(), KeyValue::new("outcome", "acquired")],
        );
    }
}

pub(crate) fn resource_rejected(group: &str, outcome: &'static str) {
    if let Some(metrics) = METRICS.get() {
        metrics.acquisitions.add(
            1,
            &[
                KeyValue::new("resource_group", metrics.group(group).to_owned()),
                KeyValue::new("outcome", outcome),
            ],
        );
    }
}

pub(crate) fn image_finished(outcome: &str, duration: Duration) {
    if let Some(metrics) = METRICS.get() {
        let outcome = match outcome {
            "succeeded" | "failed" | "cancelled" | "interrupted" | "unresolved" => outcome,
            _ => "unknown",
        };
        let attributes = [KeyValue::new("outcome", outcome.to_owned())];
        metrics.image_attempts.add(1, &attributes);
        metrics
            .image_duration
            .record(duration.as_secs_f64(), &attributes);
    }
}

pub(crate) fn recovery_finished(group: &str, outcome: &'static str, duration: Duration) {
    if let Some(metrics) = METRICS.get() {
        let attributes = [
            KeyValue::new("resource_group", metrics.group(group).to_owned()),
            KeyValue::new("outcome", outcome),
        ];
        metrics.recoveries.add(1, &attributes);
        metrics
            .recovery_duration
            .record(duration.as_secs_f64(), &attributes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_is_explicit_and_only_accepts_a_local_origin() {
        let parse = |disabled: Option<&str>, value: Option<&str>| {
            endpoint(|key| match key {
                "OTEL_SDK_DISABLED" => disabled.map(str::to_owned),
                _ => value.map(str::to_owned),
            })
        };
        assert_eq!(parse(None, None).unwrap(), None);
        assert_eq!(
            parse(Some("true"), Some("https://private.example/key")).unwrap(),
            None
        );
        assert_eq!(
            parse(None, Some("http://127.0.0.1:14318/"))
                .unwrap()
                .as_deref(),
            Some("http://127.0.0.1:14318")
        );
        for bad in [
            "https://localhost:4318",
            "http://private.example",
            "http://user:secret@127.0.0.1",
            "http://localhost/path",
            "http://localhost/?secret",
            "http://localhost/#secret",
        ] {
            let error = parse(None, Some(bad)).unwrap_err().to_string();
            assert!(!error.contains(bad));
        }
    }
}
