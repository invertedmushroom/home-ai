# Operator observability

Status: implemented opt-in local OTLP/HTTP traces and metrics. The
[operator guide](../operations/observability.md) owns setup and evidence commands.
This is a diagnostic view; SQLite and the existing runtime/image ownership
contracts remain authoritative. Optional operator-owned runtime logs use a
separate collector agent and the same local stack.

## Configuration and failure isolation

No `OTEL_EXPORTER_OTLP_ENDPOINT` means no SDK exporters or state sampler.
`OTEL_SDK_DISABLED=true` also disables them. Console tracing remains available.
The endpoint must be a plain HTTP loopback origin without credentials, path,
query or fragment. Invalid endpoint configuration fails startup without echoing
the supplied value. Application startup does not contact the collector.

Export uses OTLP/HTTP protobuf on `/v1/traces` and `/v1/metrics`. Separate SDK
threads perform network work. The trace queue holds at most 512 ended spans and
exports batches of at most 64, targeting one second between batches. HTTP
attempts have a one-second deadline, no proxy, no redirect and no retries.
Metrics target five-second exports. Unavailable collectors, full queues and
failed exports lose diagnostics rather than blocking model admission, inference
or resource release. No application operation awaits an export acknowledgement.
Graceful shutdown stops the sampler and requests bounded provider shutdown on a
blocking thread. Forced process termination can lose buffered telemetry.

The read-only sampler targets five-second observations, with one database read
in flight and missed ticks skipped. A delayed read makes its heartbeat stale;
it does not enqueue more reads. Queue/slot measurements are in-memory scheduler
observations. Durable unresolved ownership comes from SQLite. The sampler
changes no run, ownership, recovery or retention state. Telemetry adds no schema.

## Trace and privacy boundary

Only spans/events explicitly targeting `deep_server::telemetry` enter OTLP.
The general HTTP trace layer and arbitrary application/library errors are not
exported. Console log filters do not disable the independent OTLP allowlist.
Deep has no log exporter. The optional raw runtime feed below does not pass
through Deep's instrumentation or its redaction policy.

A run starts an independent root trace. Selected model waits, resource waits,
model operations, HTTP receive operations, owned image jobs and native recovery
are descendants. Image progress changes are bounded span events. Explicit image
parent capture preserves context across its owned task. Model tasks retain
context after their subscriber leaves. Startup image recovery can start a new
trace carrying the original run/job identity; it does not invent historical
trace context.

The trace can remain open through owned cleanup after the parent run is already
failed/cancelled. Its duration can therefore exceed the persisted parent active
duration. Queue waits dropped by cancellation may have no acquired outcome;
the parent outcome supplies cancellation context. Admission logs include the
trace ID when export is enabled. A logged trace ID proves creation, not delivery.

Exported span/event fields are operation names, opaque run/operation/job IDs,
configured backend/group/profile IDs, round, bounded outcome/ownership labels,
wait duration and validated image phase/step observations. Prompts, responses,
model labels, workspace titles, files, cookies, API credentials, HTTP headers,
runtime URLs and raw error messages are excluded. Automatic exception, location
and thread recording are disabled. Each span allows at most 16 attributes,
64 events and no links.

Resources contain only fixed `service.name=deep-server`, package version and a
random process `service.instance.id`. Ambient `OTEL_RESOURCE_ATTRIBUTES` and
`OTEL_SERVICE_NAME` do not add payload or override identity. The instance ID
separates concurrent Deep processes; it does not coordinate their GPUs.

## Metrics

The SDK emits these dot-separated names; the supplied Prometheus backend
translates dots to underscores, counters to `_total`, and seconds to `_seconds`.
Duration histograms expose `_bucket`, `_count` and `_sum`.

| Instrument                                              | Meaning                                                                                                  | Attributes                  |
| ------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- | --------------------------- |
| `deep.run.finished`                                     | Settled admitted parents observed by the background task                                                 | `backend`, `outcome`        |
| `deep.run.duration`, `deep.run.queue.duration`          | Persisted active/initial queue durations; absent timings are skipped                                     | `backend`, `outcome`        |
| `deep.model.calls`, `deep.model.duration`               | Returned model transport outcomes/duration; owned calls can outlive parent Stop                          | `backend`, `outcome`        |
| `deep.model.transport.failures`                         | Redacted transport error classification                                                                  | `backend`, `kind`           |
| `deep.resource.acquisitions`                            | Successful acquisition, full queue or blocked ownership                                                  | `resource_group`, `outcome` |
| `deep.resource.wait.duration`                           | Wait until successful acquisition; cancelled/rejected waits are excluded                                 | `resource_group`            |
| `deep.image.attempts`, `deep.image.duration`            | Owned image communication attempts through processing/cleanup; reconciliation can create another attempt | `outcome`                   |
| `deep.model.recoveries`, `deep.model.recovery.duration` | Native confirmation attempts and persistence outcome                                                     | `resource_group`, `outcome` |
| `deep.run.queued`, `deep.run.active`                    | Sampled persisted parent counts                                                                          | None                        |
| `deep.resource.queued`, `deep.resource.active`          | Sampled wait-admission occupancy and leased stage slots                                                  | `resource_group`            |
| `deep.ownership.unresolved`                             | Model/image unresolved receipts, by frozen group                                                         | `resource_group`            |
| `deep.ownership.unresolved.age`                         | Age of the oldest unresolved receipt since creation; not time since it became unresolved                 | `resource_group`            |
| `deep.observability.snapshot.timestamp`                 | Wall-clock timestamp of the last successful state observation                                            | None                        |
| `deep.observability.snapshot.failures`                  | Failed state observations                                                                                | None                        |

Metric backend names retain the legacy provider IDs plus `unknown`; named
connections currently aggregate into `unknown`. Traces retain configured
connection IDs. Group names
are restricted to the startup configuration plus an aggregate `unconfigured`
bucket for old/unknown frozen groups. Outcomes/kinds are bounded code-selected
values. Run, workspace, operation and remote job IDs are never metric dimensions.
The backend retains the process instance resource attribute for series identity.

Counters and histograms reset per process. Hosted transport dropped by parent
cancellation may have no returned-call measurement; the parent cancellation and
trace remain visible. Short occupancy/recovery transitions can fall between
samples. Missing series, stale heartbeats, absent usage and insufficient samples
for a percentile are unknown, not zero or proof of healthy/idle hardware.
Scheduler occupancy does not measure whole-device GPU utilization or residency.

## Evidence boundary

The harness requires a real deployed run trace in Tempo, its root outcome and
expected stage spans, a matching process's outcome-counter increment in
Prometheus, and a fresh sampler heartbeat. An image case also requires the
owned image span and successful two-turn receipt. Concurrent traffic can
contribute to aggregate counter increments; use a controlled workload for
measurements. The backend being reachable alone is not tracing evidence.

Offline process tests decode actual OTLP payloads, verify trace parentage and
private-field exclusion, exercise refused/uncertain transport failures and
durable ownership gauges, and show that stalled export does not delay synthetic
generation. Script tests preserve existing reports, reject identity labels and
prevent generation retries. Operational results are recorded in the guide.

## Operator-owned raw runtime logs

The `runtime-logs` Compose profile enables a separate collector agent beside the
runtimes. Its two `file_log` receivers read only operator-selected llama-server
and WanGP file patterns through read-only directory mounts. This agent does not
launch, restart or supervise either runtime. An absent file does not prove that
a runtime is stopped. Deep's HTTP calls do not capture runtime stdout/stderr.

The pinned collector has no Loki exporter. The agent sends OTLP/HTTP logs to the
stack receiver, which forwards them to Loki's native `/otlp/v1/logs` endpoint.
Resources identify `service.namespace=deep-runtime`, `service.name=llama-server`
or `wangp`, and operator-configured `host.name`. Loki indexes the service fields
and retains `host_name` as structured metadata. Run/job IDs are not inferred from
raw text. The Deep process selector does not filter these independent runtimes.

These are unredacted operator logs. Bodies may include prompts, replies, paths,
model labels, ports, commands and error payloads. Deep's private-field exclusion
applies only to Deep's own OTLP spans/metrics. The agent adds no file-path/name
attributes. Operators select the files and keep Grafana/ingestion loopback-only.

The agent reads existing selected files on first discovery, then tails them.
It caps a record at 64 KiB and tracks offsets in its container writable layer.
A normal agent stop/start retains offsets. Recreating the agent discards offsets
and can replay source files. File timestamps are collection times, including
initial backfill; native timestamps remain in the body when supplied. The
dashboard removes ANSI colors for display without changing stored bodies.

Raw-log export is bounded and best effort, with no durable delivery queue or
retry. Stopping the agent or losing delivery cannot block runtime execution.
Removing the LGTM container discards its ingested logs; removing that container
does not remove host source files. Operators own source-file rotation and
cleanup. A worker on another machine can run the same agent configuration and
forward through a private SSH tunnel without changing Deep or exposing Loki.
