# Local model / WanGP handoff measurement

`scripts/image-handoff.mjs` repeats the existing real-browser image smoke test
against a running local operator instance. It uses synthetic workspaces, calls
only the explicitly selected local text backend and image profile, and never
applies project changes. The optional cancellation case waits for the owned
image job to become terminal before asking the same backend to answer `19 + 23`.
It checks the saved assistant message, then rechecks the cancelled image receipt.

## Repeat

Use a separate Deep listener and database. In Windows PowerShell:

```powershell
$env:DEEP_BIND = '127.0.0.1:4321'
$env:DEEP_DATA_DIR = 'data/handoff-check'
$env:CARGO_TARGET_DIR = 'target/handoff-check'
$env:DEEP_PUBLIC_ORIGIN = ' '
$env:RUST_LOG = 'deep_server=info,tower_http=warn'
cargo run --locked -p deep-server
```

The space explicitly disables the public origin after trimming. Assigning an
empty string in Windows PowerShell removes the variable, allowing an origin in
`.env` to take effect again. The runner rejects guest mode. Keep the existing
LM Studio/model and image-profile configuration; both runtimes must already run.
Ensure no other client is generating during a controlled measurement.

In another terminal, create the report directory if needed and run:

```powershell
New-Item -ItemType Directory -Force test-results | Out-Null
node scripts/image-handoff.mjs --url http://127.0.0.1:4321 --backend lmstudio --profile flux-klein-4b --repeat 2 --cancel --report test-results/handoff.json
npm run test:scripts
```

Use a new report filename each time. `--repeat` accepts 1–10 image cases;
`--cancel` adds one cancellation/chat case. `--lmstudio-url` selects the native
LM Studio API origin (default `http://127.0.0.1:1234`). `--gpu-index` selects the
`nvidia-smi` device (default `0`); `none` disables GPU measurements.
Use `--backend compatible` for standalone llama-server. Compatible backends skip
LM Studio residency by default and record `not_probed`. Supply `--lmstudio-url`
only when that compatible endpoint actually belongs to LM Studio.
All server origins must be plain loopback HTTP origins.
The native LM Studio origin must identify the same server used by Deep's selected
backend. The image smoke currently expects a 512×512 profile.

When selected, the runner observes LM Studio residency before/after each case. It does not force
loads, unloads, residency or memory handoffs. An initially unloaded LLM is not
proof of a cold WanGP checkpoint or cold operating-system file cache.
For an initially unloaded LLM, unload only that selected instance in LM Studio
before starting; the report records whether any instances were loaded.

Failures are saved, remaining cases are skipped, and generation is never
automatically retried. Failure cleanup cancels only the accepted test run and
observes its receipt for up to 35 seconds. Unknown/pending remote cleanup stays
visible and blocks further cases. Workspaces and generated images remain in the
selected test database; WanGP can retain its own output copies.

## Standalone llama.cpp on Windows

The installed WinGet build is `b12000-68d20b884` with Vulkan. Its `Vulkan0`
device is the RTX 4060 Laptop GPU. Unload the selected Gemma instance in LM Studio
before loading the same model in standalone llama-server. LM Studio can keep
managing downloads. The GGUF stays in place and is not copied or converted.

Current demo command, with native recovery metrics enabled:

```powershell
llama-server `
  --model "$env:USERPROFILE\.lmstudio\models\lmstudio-community\gemma-4-E4B-it-GGUF\gemma-4-E4B-it-Q4_K_M.gguf" `
  --alias google/gemma-4-e4b `
  --host 127.0.0.1 --port 8080 `
  --device Vulkan0 --ctx-size 8192 --parallel 1 `
  --fit-target 3072 --jinja --metrics
```
```
llama-server --model "$env:USERPROFILE\.lmstudio\models\lmstudio-community\gemma-4-E4B-it-GGUF\gemma-4-E4B-it-Q4_K_M.gguf" --alias google/gemma-4-e4b --host 127.0.0.1 --port 8080 --device Vulkan0 --ctx-size 8192 --parallel 1 --fit-target 3072 --jinja --metrics
```

The fit target leaves a conservative estimated margin for the shared image
runtime. It is not a VRAM reservation or a guarantee against larger workloads.
Text-only requests do not require the separate vision projector. Native chat
image attachments require a vision-enabled profile and its matching projector.

In `.env`, replace existing entries for these settings, then restart Deep:

```dotenv
DEEP_DEFAULT_BACKEND=compatible
DEEP_MODEL_BASE_URL=http://127.0.0.1:8080/v1
DEEP_MODEL_ID=google/gemma-4-e4b
DEEP_MODEL_RUNTIME=llamacpp
DEEP_MODEL_TIMEOUT_SECONDS=1800
```

Process environment variables override `.env`. Select **compatible** in Deep.
The LM Studio backend may remain configured for comparison, but keep its weights
unloaded while using standalone llama-server on this shared GPU.

The 2026-10-31 switch passed chat, native image tool calling, browser decoding,
download/reload checks, image cancellation and subsequent arithmetic chat. The
first arithmetic request took 21,920 ms. The later arithmetic request took
1,055 ms. Image generation with both model turns took 19,181 ms. These individual
requests include warmup/cache effects and do not rank Vulkan against CUDA.
The sampled peak during the handoff sequence was 6311 MiB of 8188 MiB.
Token usage remained unknown through the generic adapter and was not estimated.

Local evidence is in `test-results/llama-switch-20261031-handoff.json`, runtime
logs `test-results/llama-switch-20261031.{out,err}.log`, and the isolated database
`data/llama-switch-20261031/deep.sqlite3`. The runner's compatible-backend test
also checks that it sends no implicit requests to LM Studio's native API.

## Read the report

- Run queue/duration, aggregate usage and image creation/finish times come from
  saved receipts. `first_token_ms` is Deep's first visible text observation and
  can include preceding tool work; it is not a per-round model timing.
- The timeline records changed run/job states and progress through 200 ms polls.
  Intermediate phases may be missed. It does not measure exact phase durations.
- GPU samples target one second and measure whole-device memory/utilization,
  including unrelated processes. Sampled peaks are lower bounds. Unavailable
  memory is unknown, never zero; missing telemetry does not fail generation.
- Passing image cases require a validated native invocation, successful saved
  image, two model rounds, browser decoding, matching download bytes/SHA-256,
  reload recovery, no project revisions and a nonempty saved assistant reply.
  Image meaning and the truthfulness of the reply still require human review.
- A passing cancellation case requires a bound remote job, terminal cancellation,
  no published output and a correct saved arithmetic reply after cancellation.
  It does not prove that every cancellation timing/interleaving works.

Correlated server tracing supplies individual model-call durations when needed;
the JSON report does not infer those durations from polling or aggregate usage.

## Inspect local inference ownership

Schema 19 records ownership before local model dispatch. Stop cancels the parent
immediately while an owned task drains the response or performs native recovery.
Conflicting stages queue until completion or native idle confirmation. A timeout,
broken response or restart can leave ownership unresolved when confirmation is
unavailable; conflicting dispatch then fails explicitly.

If the runtime refuses the connection before Deep sends the model request, that
failed attempt settles as `resolved/request_not_sent` and releases its group.
Repeated attempts while the runtime is offline do not leave a persistent lock.
An explicit native llama-server may also reject a request with a complete JSON
HTTP 400 (`invalid_request_error` or `exceed_context_size_error`). Deep consumes
the bounded rejection and records terminal ownership while keeping the run
failed. That rejection no longer fences subsequent local work. The raw message
remains private. Generic HTTP errors and incomplete responses still require
completion evidence; old unresolved records still use the offline recovery below.
Once it is available, a fresh request can proceed; Deep never replays the failed
request. A connection lost after sending, or a timeout without proof of connection
setup failure, still needs confirmed completion or the recovery flow below.

The 2026-11-01 connection-failure checks passed repeated refusal, another backend
using the same group, and a fresh request after the runtime returned, for generic
compatible, native llama-server and LM Studio configurations. Connections broken
after request bytes arrived remained unresolved. All 162 Rust tests and strict
Clippy passed. The updated Tailscale demo answered `42` in 9,412 ms and retained
no pending model/image ownership. Evidence is in
`test-results/demo-request-not-sent-20261101.{json,log}`.

Compatible/llama-server, LM Studio and image profiles share `local-gpu` by default.
The scheduler acquires that group separately for each model turn and image job.
Hosted calls use their own provider groups. Keep aliases of the same runtime or
GPU in the same group; assign different groups only for independent hardware.
See [group configuration and queue timing](../contracts/runtime.md#local-inference-ownership).

Stop Deep and keep `DEEP_DATA_DIR` pointing at the affected database. Inspect
without startup recovery or model/API credentials:

```powershell
cargo run --locked -p deep-server -- model-calls
```

Check LM Studio/the compatible runtime directly. After confirming that the
runtime is idle, resolve the exact operation ID printed by inspection:

```powershell
cargo run --locked -p deep-server -- model-calls --resolve <operation-id> --confirm-idle
```

Resolution records an operator assertion. It preserves the unknown result and
does not publish an answer or replay inference. The command requires the exclusive
offline database lease. A running server cannot resolve its own live operations.
Completed/resolved records remain until their workspace is deleted. Pending
ownership blocks workspace deletion, branching, turn deletion and guest cleanup.
Before first upgrading an older database without ownership records, stop Deep
and verify that its runtimes are idle. Migration cannot reconstruct old requests
that were never recorded.

### When every browser loses local generation

A local model timeout without completion/idle confirmation can fence the shared
GPU even after its parent is failed.
Every browser then sees blocked local generation, including WanGP invoked by a
hosted model. Hosted text calls on their own provider groups can still work.
Opening another browser or restarting Deep does not resolve that ownership.

Inspect the first failed local run. `DEEP_MODEL_TIMEOUT_SECONDS` defaults to 180
seconds for the entire HTTP call, including prompt processing and reasoning.
Reasoning-only chunks do not produce visible answer text or reset that deadline.
A stream timeout names the deadline; server warnings identify the blocked
operation for recovery. The demo now uses 1800 seconds for long local generations.
Search/image orchestration retains its own parent budget. Native llama-server
recovery is described below; other compatible servers and historical ownership
still use the offline procedure above.

For standalone llama-server, inspect its slots directly before resolving:

```powershell
Invoke-RestMethod http://127.0.0.1:8080/slots | Select-Object id, is_processing
```

Check every slot reports `is_processing: false`, with no other client generating.
The [llama-server slots endpoint](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md#get-slots-returns-the-current-slots-processing-state)
reports processing state. A successful `/health` response or low GPU utilization
alone does not establish idleness. Then stop Deep and resolve only the exact
model-call operation. For WanGP-owned failures, use the image receipt's existing
reconciliation/operator-confirmation flow instead.

Group scheduling serializes execution; loaded model weights remain resident.
Keep one local text runtime loaded on this laptop. Loading Gemma in both
llama-server and LM Studio consumes additional memory even while one is idle.

The 2026-11-01 recovery traced the shared fence to a 180-second llama stream
timeout. The runtime log showed about 4.4 generated tokens/second and no saved
answer text. Both Gemma runtimes were loaded and GPU memory was 7691 MiB;
this indicates memory pressure may have contributed, without isolating its cost.
After verifying the llama slot idle, exact offline resolution preserved the
failed run and all workspace data. Two isolated browser sessions over Tailscale
then concurrently requested local chat: both answered `42`, one waited 26,062 ms,
and a foreign workspace read returned `404`.

The operator selected llama-server for the demo. Unloading only LM Studio's
Gemma instance reduced reported GPU memory to 3585 MiB immediately afterward.
The local `.env` now leaves `DEEP_LMSTUDIO_MODEL` commented out so a demo browser
cannot reload the duplicate through that backend. LM Studio's server and model
files remain available. After restarting with the diagnostic changes, the
Tailscale image workflow passed in 26,478 ms with two model rounds, matching
image bytes/hash, browser decoding and reload recovery. There was no pending
model/image ownership afterward. All 154 Rust tests and strict Clippy passed.

Evidence: `test-results/demo-ownership-20261031-{before,resolution}.json`,
`test-results/demo-two-browsers-20261031.json` and
`test-results/demo-recovered-20261031.log` (filenames use the UTC date), plus
`test-results/demo-image-recovery-20261101.json` and
`test-results/demo-llama-only-20261101.log`.

### Native timeout and Stop recovery

Start the standalone server with `--metrics` and set `DEEP_MODEL_RUNTIME=llamacpp`
in Deep's `.env`. Restart both after changing their configuration. Keep one direct
local text runtime loaded. Deep uses the same configured origin and credentials;
the read-only metrics endpoint remains on loopback.

After this attempt has supplied generated text, reasoning or tool arguments,
Deep can recover a timeout/Stop automatically. It retains the group slot while
the runtime confirms that both running and deferred requests are zero. The failed
or cancelled parent stays terminal, and its incomplete output remains a draft.
No generation is retried. A connection setup failure releases its exact intent
without probing. After possible transmission, a failure before generation or
unavailable/invalid idle evidence keeps the fence for manual recovery. Changing configuration never
reinterprets old unresolved ownership. See the
[native runtime contract](../contracts/runtime.md#native-llama-server-recovery)
for the precise eligibility, checked implementation and probe bounds.

The 2026-11-01 native check deliberately used a three-second timeout in an isolated
database. The long request failed at 3,023 ms with 103 bytes of incomplete text;
its exact ownership record became `resolved/runtime_confirmed_idle`. A queued
chat waited 2,276 ms, then answered `42` in 1,197 ms. A second started stream was
stopped, recovered automatically and followed by another correct chat in 897 ms.
Neither terminated run published a late assistant reply. WanGP's full two-model-
turn image workflow then passed in 12,120 ms with saved image/hash, browser decode
and reload checks. These probes establish recovery on this native runtime, not
idleness contracts for other compatible servers.

All 160 Rust tests and strict Clippy passed. Tests cover busy and deferred work,
non-generating failures, malformed/unavailable/oversized/redirected metrics,
operator-only configuration and unchanged generic transport recovery. The
adversarial review found that idle metrics cannot establish admission during HTTP
preprocessing; generated-stream eligibility preserves that distinction. No schema
migration was added.

The final native timeout/Stop and followup-chat check also passed with system
HTTP/HTTPS/ALL proxies set to an unreachable address and no loopback exclusion.
Native inference and reconciliation bypass those proxies. Queue gauges are
parsed as integer counts so malformed floating-point underflow cannot imply idle.

Evidence: `test-results/native-recovery-20261101.json`,
`test-results/native-recovery-short-20261101.log`,
`test-results/native-recovery-images-20261101.json`,
`test-results/native-recovery-proxy-20261101.{json,log}`,
`test-results/llama-metrics-20261101.{out,err}.log` and
`data/native-recovery-20261101/deep.sqlite3`. The demo's active settings are
`llamacpp` recovery and a 1800-second model timeout.

## LM Studio lifecycle and native cancellation

Chat now distinguishes a waiting model stage, active request, stopped parent with
live cleanup, and an unresolved blocked resource. The API exposes only a safe
per-run projection, not private connection configuration or another workspace's
operation. [The runtime contract](../contracts/runtime.md#public-local-model-activity)
owns the exact phases. Restart Deep and refresh the browser for the new display.
No database migration or model configuration change is required.

Server checks cover queued cancellation without dispatch, LM Studio/compatible
Stop before the first token, generation Stop, connection loss, native recovery,
restart and exact operation identity. Browser checks cover terminal-run polling,
settlement, blocked ownership without endless polling, workspace disposal,
invalid evidence and older APIs.

The 2026-11-02 isolated LM Studio/Gemma check observed browser cleanup after Stop.
The queued follow-up dispatched six milliseconds after the original request
settled, after an 18.616-second queue wait. Both journal
records completed, and the cancelled run published no assistant reply or revision.
Reload showed the original run as cancelled with settled upstream ownership.
The follow-up reported `stop` and 106 output tokens but no visible answer.
These observations verify lifecycle ordering, not arithmetic-answer quality.

The native cancellation probe remains deferred. The inspected official
`@lmstudio/sdk` 2.1.0 declares Zod `^3.22.4`. npm audit reported
[GHSA-mrhx-c7xc-m3w5](https://github.com/advisories/GHSA-mrhx-c7xc-m3w5),
which affects versions below 4.6.6. There was no patched current SDK release.
The trial dependency was removed, and the resulting dependency audit was clean.
No incompatible dependency override, historical SDK downgrade or undocumented
WebSocket client was substituted. Existing OpenAI-compatible LM Studio inference
and bounded draining remain unchanged. Before revisiting the native SDK, verify
its dependency advisory status and test scoped cancellation/terminal acknowledgement
before proposing a production transport change.

## Ownership slice verification: 2026-10-31

Schema 19 passed 144 Rust tests, six offline script tests, strict Clippy and
format checks. Focused tests cover stopped streams, timeouts, subscriber storage
failure, invalid terminal output, reciprocal model/image exclusion, offline CLI
access, exact operator resolution and migration rollback before recovery.

The live check used an isolated database, the configured Gemma 4 E4B/FLUX profile
and a 512-token output limit. Stop cancelled the parent immediately. A conflicting
local call failed before dispatch while the owned model stream drained. After
confirmed completion, a new chat correctly answered `42` in 1,889 ms. The cancelled
run gained no late assistant message.

The full image workflow passed in 12,472 ms. A second image job cancelled in
2,796 ms and its following chat passed in 954 ms. Offline inspection then found
six completed local-call records. The test server was stopped after all model
and image ownership was terminal.

Local evidence:

- `test-results/resource-ownership-20261031-stop.json`
- `test-results/resource-ownership-20261031-images.json`
- `test-results/resource-ownership-20261031-calls.json`
- `test-results/resource-ownership-20261031-server.log`
- `data/resource-ownership-20261031/deep.sqlite3`

These checks validate the ownership slice on the current setup. They do not
establish configurable-group concurrency or acknowledged upstream cancellation.
Stage scheduling was verified separately below. Runtime-specific acknowledged
cancellation remains conditional work in the scheduling plan.

## Stage scheduling verification: 2026-10-31

The stage-queue slice passed 153 Rust tests, seven offline script tests, strict
Clippy and format checks. Focused tests cover FIFO order, bounded queues,
cancelled model/image waiters, independent managed groups, hosted progress during
local work, a model slot released during web search, first-stage queue timing,
unchanged legacy profile fingerprints and unresolved runtime fences after group
renaming. Schema 19 remains unchanged.

The live check used standalone llama-server through `compatible`, the existing
Gemma/FLUX profile and a 512-token output limit. After Stop, the owned stream
drained while another workspace queued. Cancelling that queued request sent no
model call. A surviving request waited 8,707 ms, then answered `42` in 1,109 ms.
Neither cancelled parent gained a late assistant reply.

The real-browser model/image/model workflow passed in 9,004 ms, including saved
image decoding, byte/hash checks, download and reload recovery. A second image
job reached terminal cancellation after 3,017 ms; the following arithmetic chat
passed in 3,111 ms. The sampled peak during these image cases was 6538 MiB of
8188 MiB. These individual measurements do not establish larger-workload capacity.
No automatic residency hook was needed.

Offline inspection found six completed `local-gpu` model records and no pending
model/image ownership. The isolated listener was stopped after cleanup; the
existing demo and model/image runtimes remained running. Local evidence:

- `test-results/resource-queues-20261031-stop.json`
- `test-results/resource-queues-20261031-images.json`
- `test-results/resource-queues-20261031-calls.json`
- `test-results/resource-queues-20261031-server.log`
- `data/resource-queues-20261031/deep.sqlite3`

Independent hardware-group overlap is covered by controlled HTTP fixtures. The
live laptop check exercises one shared GPU and does not establish cross-process
coordination or prove that closing an HTTP connection stops upstream inference.

## Initial measured run: 2026-10-31

One fixed Deep executable, LM Studio `google/gemma-4-e4b` Q4_K_M, loaded context
8192, and WanGP 1.30.0 with `flux2_klein_4b`, 512×512, four inference steps.
GPU: NVIDIA GeForce RTX 4060 Laptop GPU, 8188 MiB reported device memory.
The profile fingerprint was
`47497bdcb5319165b43fdd4f79500d2098406c6954d089ec0374af05128a9527`.
The Deep executable SHA-256 was
`cfff26c38861e525b92bd3e098cb01ca2732c7691accb9b9b95c22b0653d5a48`.
Prompt edits made after that executable was built were not part of this run.

| Case                                       |             Parent active duration |        Image receipt duration | Sampled peak GPU MiB | Result                                |
| ------------------------------------------ | ---------------------------------: | ----------------------------: | -------------------: | ------------------------------------- |
| LLM initially unloaded; WanGP already warm |                          17,408 ms |                      5,665 ms |                7,400 | Passed                                |
| LLM already loaded; WanGP warm             |                          10,718 ms |                      5,655 ms |                7,399 | Passed                                |
| Cancel owned image, then chat              | 3,378 ms until parent cancellation | 3,086 ms until image terminal |                7,399 | Passed; chat active duration 1,301 ms |

The cold-LLM case followed an explicit unload of only the selected Gemma instance
using LM Studio's native API. No automatic unload hook was added. The sampled
peak across an earlier generation was 7549 MiB; it also generated successfully.
Two harness defects were corrected during setup: stale composer selectors and a
new assertion that incorrectly read the cleared draft instead of the saved reply.
Those exploratory reports remain failures, separate from the verified sequence.

Full local evidence is in `test-results/handoff-20261031-verified.json`, with
39 GPU samples, run/job IDs, observed phases, seeds, model configuration and output
hashes. The isolated database is `data/handoff-20261031/deep.sqlite3`.
Downloaded outputs are `test-results/handoff-20261031-1.jpg` and `-2.jpg`.
Visual inspection of the first output confirmed the requested orange robot,
seedling, wooden windowsill and watering action; this is one manual check.

This sequence establishes that this small workload completed and recovered from
one cancellation on this machine. It does not establish tail latency, safe
concurrency, memory capacity for larger images/contexts or a general model-quality
ranking. There is no demonstrated need for an automatic unload hook from these
cases alone; a larger workload should be measured before adding one.
