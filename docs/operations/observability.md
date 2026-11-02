# Observe Deep runs

The optional local stack uses the pinned Collector/Grafana/Tempo/Prometheus/Loki image
already used by ForgeOps. It is a disposable development dependency. The
[observability contract](../contracts/observability.md) owns signals, privacy,
sampling and failure behavior.

## Start and connect

```powershell
npm run observability:up
```

| Interface          | Loopback address                         |
| ------------------ | ---------------------------------------- |
| Grafana dashboard  | <http://127.0.0.1:13000/d/deep-overview> |
| OTLP/HTTP receiver | `http://127.0.0.1:14318`                 |
| Prometheus         | <http://127.0.0.1:19090>                 |
| Tempo query API    | `http://127.0.0.1:13200`                 |

Add this to `.env`, then restart Deep:

```dotenv
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:14318
```

The ports avoid Deep's default 4318 listener and the ForgeOps local stack. The
dashboard has anonymous local Editor access so Tempo Explore works; its
provisioned dashboard is read-only. Grafana requires Editor or Admin for
[Explore access](https://grafana.com/docs/grafana/latest/administration/roles-and-permissions/).
Tailscale Serve should keep
forwarding the application listener only. Telemetry views span all application
workspaces; they are operator views, not guest-authorized application routes.

Normal runs need no collector when the endpoint is unset. To disable an already
configured exporter, set `OTEL_SDK_DISABLED=true` and restart Deep. Stop the
receiver with `npm run observability:stop`. Stopping the receiver alone leaves
generation working, but diagnostic batches can be lost. There is no replay.

No named data volume is created. Telemetry stays in the container's writable
layer, survives a stop/start and is discarded when that container is removed.
Prometheus retains at most 24 hours. Application SQLite data is separate.

## Read the dashboard

- **Live status:** sampler age shows the oldest selected process observation.
  Its failure counter helps distinguish failed observations from a stopped
  process. Missing observations are unknown. Parent counts, resource slots and
  waiters distinguish admission from GPU stage occupancy.
- **Recent runs:** click a trace to follow the run, its model/image stages and
  owned cleanup. Results follow the time range and **Deep process** selector.
- **Latency** (collapsed): initial queue, parent active duration, resource wait,
  model HTTP, image attempt and native recovery P50/P95. Parent duration excludes
  initial queueing. Percentiles need multiple observations; inspect a trace for
  one slow run.
- **Outcomes** (collapsed): run/model/image attempts, successful/rejected stage
  acquisitions, native recovery and transport failures. Success ratios and
  counts use selected process totals; throughput uses a rolling rate per minute.
  These reset at process restart. No outcomes means unknown, not 100% success.
- **Unresolved ownership:** a receipt can fence a group while no slot is active.
  Its oldest age starts at receipt creation. Use the existing
  [ownership recovery guide](gpu-handoff.md#inspect-local-inference-ownership).
- **Runtime logs:** the optional raw feed has separate **Runtime** and
  **Log contains** filters. The Deep process selector does not filter independent
  runtimes. Lines display `runtime@host`; expand a line for structured metadata.

Use the **Deep process** selector to separate listeners/restarts. Dashboard links
open Tempo and Loki Explore. To find a specific run in Tempo:

```text
{ resource.service.name = "deep-server" && span.run_id = "<run UUID>" }
```

The admission log also includes `trace_id`. Selected traces follow waits, model
calls, WanGP progress and owned cleanup. The run receipt remains authoritative;
a trace can continue after parent cancellation while cleanup still owns work.
Five-second sampling can miss short transitions. This dashboard does not infer
available VRAM or account for independent runtime/UI clients.

Dashboard JSON is provisioned read-only. Grafana reloads changes to the mounted
file automatically. Collector pipeline changes require restarting `lgtm`:

```powershell
docker compose -f deploy/observability/compose.yaml restart lgtm
```

Restarting keeps container history. Recreating/removing `lgtm` discards history.

## Add local runtime logs

Deep does not start llama-server or WanGP and cannot capture their stdout/stderr.
This opt-in feed tails files on the runtime machine. It is separate from Deep's
redacted telemetry. Raw logs can include prompts, responses, paths and errors.
The [raw-log contract](../contracts/observability.md#operator-owned-raw-runtime-logs)
owns privacy, replay and delivery behavior.

The pinned image's Collector 0.156.0 contains `file_log` and `otlp_http`, but no
Loki exporter. We use Loki's
[native OTLP endpoint](https://grafana.com/docs/loki/latest/send-data/otel/).
The `runtime-logs` profile runs only another collector process from the same
pinned image, with read-only source mounts and no published ports. Loki remains
inside the local stack.

Add directory/glob settings to the repository-root `.env`. Paths are absolute,
or relative to `deploy/observability/`. Default directories are
`../../logs/llama-server` and `../../logs/wangp`, each limited to `*.log`.
For the existing launchers on this machine, these were configured:

```dotenv
DEEP_LLAMA_LOG_DIR=../../test-results
DEEP_LLAMA_LOG_GLOB=llama-metrics-20261101.*.log
DEEP_WANGP_LOG_DIR=C:/pinokio/api/wan.git/logs/api/start.js
DEEP_WANGP_LOG_GLOB=latest
DEEP_LOG_HOST=helios
```

The llama pattern selects only that runtime's redirected output/error files.
Pinokio already maintains the WanGP `latest` log. No runtime restart or launcher
edit is needed. When changing the llama launch filename, update its glob too.
Then run:

```powershell
npm run observability:logs
```

This command explicitly reads the root `.env`, enables the profile and starts
the agent. The ordinary `observability:up` command starts the core stack without
enabling file collection. Run `npm run observability:logs:stop` to stop only
file collection. `npm run observability:stop` stops the whole stack, including
the optional agent. Neither command stops the runtimes or Deep.

For a future manual launch, preserve your runtime arguments and append to stable
UTF-8 files. For example, in **PowerShell 7**, after setting `$llamaArgs` to your
existing arguments:

```powershell
New-Item -ItemType Directory -Force logs/llama-server | Out-Null
llama-server @llamaArgs 2>&1 | Tee-Object -FilePath logs/llama-server/server.log -Append
```

PowerShell 7's [`Tee-Object`](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.utility/tee-object)
writes UTF-8. Windows PowerShell 5.1 writes UTF-16 by
default, which does not match this agent's UTF-8 input. Keep using Pinokio's
existing log for launcher-managed WanGP. For a manually launched Python runtime,
use unbuffered output (`python -u`) with the same append pattern.

The agent backfills selected existing files once, then tails. Ingestion times
are collection times, including backfill; native timestamps remain in the body.
The [file receiver](https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/v0.156.0/receiver/filelogreceiver/README.md)
keeps offsets through a normal agent stop/start. Recreating it discards offsets
and can replay logs. Source files stay on the host and remain operator-owned;
removing a container does not clean them. An empty panel can mean no matching
file or no new output, not an unhealthy runtime. Inspect agent diagnostics with
`docker compose -f deploy/observability/compose.yaml --profile runtime-logs logs --tail 50 logs-agent`.

## When a runtime moves to another machine

SSH plus `tail -F` (or PowerShell `Get-Content -Wait`) remains sufficient when
central logs are unnecessary. Deep traces still describe its own remote calls.
To keep the central Grafana view, run the same `runtime-logs.yaml` collector
configuration **on the worker**, where the files live. Deep does not fetch them.

One private forwarding option is to open this tunnel from the worker to the
machine running the observability stack:

```text
ssh -N -L 14318:127.0.0.1:14318 OPERATOR@SERVER_TAILSCALE_NAME
```

Run a native Collector Contrib 0.156.0 on that worker with the copied
`deploy/observability/runtime-logs.yaml` and these environment variables:

| Variable                   | Worker value                                |
| -------------------------- | ------------------------------------------- |
| `DEEP_LLAMA_LOG_INCLUDE`   | Absolute worker-side llama file glob        |
| `DEEP_WANGP_LOG_INCLUDE`   | Absolute worker-side WanGP file glob        |
| `DEEP_LOG_HOST`            | Stable worker name, e.g. `worker-a`         |
| `DEEP_LOG_EXPORT_ENDPOINT` | `http://127.0.0.1:14318` through the tunnel |
| `DEEP_LOG_CHECKPOINT_DIR`  | Writable local directory for file offsets   |

Start it with `otelcol-contrib --config=runtime-logs.yaml`. If only one runtime
lives there, give the other receiver a nonmatching glob. Keep native source paths
on the worker, rather than trying to mount them from the server. The loopback
receiver, Loki and Grafana need no public/tailnet port exposure. The tunnel and
worker agent are operator-managed; delivery is best effort during disconnects.

## Prove application telemetry

Check all dashboard queries, including collapsed panels, without generation:

```powershell
npm run observability:check
npm run observability:check -- --require-runtime-logs --report test-results/observability-queries.json
```

The optional flag requires a recent line from **each** runtime, checked separately
so a busy feed cannot hide the other in a combined panel's line limit. Queries
with no data still pass the syntax/transport check. Report files use exclusive
creation. The checker does not generate work or change application state.

Use a separate local operator listener/database, preserving existing demo data
and model/profile settings. In Windows PowerShell:

```powershell
$env:DEEP_BIND = '127.0.0.1:4321'
$env:DEEP_DATA_DIR = 'data/observability-check'
$env:CARGO_TARGET_DIR = 'target/observability-check'
$env:DEEP_PUBLIC_ORIGIN = ' '
$env:OTEL_EXPORTER_OTLP_ENDPOINT = 'http://127.0.0.1:14318'
cargo run --locked -p deep-server
```

The space disables the public origin after trimming; an empty assignment in
Windows PowerShell removes the override and can restore `.env` guest mode.
Ensure no other client is generating during a controlled GPU measurement.

In another terminal:

```powershell
New-Item -ItemType Directory -Force test-results | Out-Null
npm run observability:evidence -- --url http://127.0.0.1:4321 --backend compatible --report test-results/observability-chat.json
npm run observability:evidence -- --url http://127.0.0.1:4321 --backend compatible --profile flux-klein-4b --report test-results/observability-image.json
```

Select a configured local backend explicitly. An image case requires the
selected profile and runtime. Use a new report filename each time; an existing
report prevents all network requests/generation. The harness rejects guest mode,
uses synthetic workspaces, never applies project changes and never retries
generation. It saves receipt timing, run/trace/process IDs, exported stage names
and the observed outcome-counter increment. A failed telemetry check can coexist
with successful generation; inspect both fields in its report. Failure cleanup
cancels only its own active accepted parent and reports the observed cleanup
state without claiming remote idleness. Created workspaces/artifacts remain.
Trace searches use the run's time window and allow up to 60 seconds for visibility;
the measured lookup delay is saved separately from generation duration. A local
check observed about 31 seconds before a delivered trace became searchable. The
window includes two minutes before admission to avoid Tempo's recent-data boundary
error for very narrow ranges.

## Verification: 2026-11-01

The Rust process tests decoded real OTLP payloads, checked shared run trace IDs
and parentage, and rejected private prompts, replies, model labels, runtime
paths, credentials, cookies and ambient resource overrides. Refused connection
and unresolved ownership cases reached the expected counters/gauges. A stalled
collector did not delay subsequent synthetic generation. Offline script checks
cover report preservation and metric-label/retry behavior.

The configured llama-server chat and WanGP two-turn workflow both reached Tempo
and Prometheus through an isolated Deep listener. Native recovery retained the
original trace after parent failure and emitted its confirmed-idle result.
All 167 Rust tests, nine script tests and strict Clippy passed. The browser loaded
the original provisioned dashboard without query errors; its 17 PromQL targets
parsed successfully. The loopback Editor role was checked against the Tempo Explore
link. The shared Tailscale demo was restarted with export enabled and its local
chat trace arrived. Local evidence:

- `test-results/observability-20261101-chat.json`
- `test-results/observability-20261101-image.json`
- `test-results/observability-20261101-server-2.log`
- `test-results/observability-20261101-dashboard.{json,png}`
- `test-results/observability-20261101-{queries,explorer}.json`
- `test-results/demo-observability-20261101.{json,log}`
- `test-results/demo-observability-window-20261101.json`
- `test-results/observability-20261101-windowed-2.json`
- `data/observability-20261101/deep.sqlite3`

These checks establish delivery/correlation and basic failure isolation on this
setup. They do not establish lossless telemetry, production retention, alert
routing, long-term SLOs or model/image quality.

### Expanded dashboard and runtime feeds

The updated dashboard has 34 targets across Prometheus, Tempo and Loki, including
collapsed latency/outcome panels. The query checker passed all 34 plus two
separate runtime-feed probes. Real llama-server and Pinokio/WanGP files reached
Loki with `host_name=helios`. Agent restart retained offsets: unchanged source
files produced no replay. The browser verified recent-trace navigation, runtime
and text filters, and expanded panels with no query/page errors. A fresh isolated
demo run reached its matching Tempo trace and Prometheus counter. No runtime was
restarted, and no GPU generation was needed for these additional checks.

Nine script tests and formatting checks passed. No Rust code or application
schema changed. Additional local evidence:

- `test-results/observability-20261101-expanded-queries-final.json`
- `test-results/observability-20261101-expanded-trace.json`
- `test-results/observability-20261101-log-restart.json`
- `test-results/observability-20261101-browser-final.json`
- `test-results/observability-20261101-{dashboard-final,logs-filter,latency}.png`
