# Deep

A small local AI workbench: a **TypeScript/React UI**, a **Rust/Axum service**,
and separately managed model runtimes. Start without a GPU; move inference to
another PC or a Linux VM when the hardware is ready.

Conversations, project files and proposed changes live in SQLite. Each generation
has a server-issued run ID. Applied changes create immutable revisions; previous
files remain available. Generated HTML runs only after **Run preview**, inside a
sandboxed frame.

## Run locally

Use Node.js 24+ and Rust 1.89+ with a native C compiler
(SQLite is bundled). Run commands from this directory.

Linux and Windows use the same Rust/Node application. The
[Arch/Linux setup guide](docs/operations/linux.md) covers native prerequisites,
built-app launch, the local Docker worker and two GPU PCs with LM Studio/WanGP.
It requires no changes to those existing inference installations.

```powershell
npm ci
npm run dev
```

Open **http://127.0.0.1:5173**. The Rust API defaults to **127.0.0.1:4318**;
Vite proxies `/api`. The first Rust build takes longer than later starts.
Defaults use scripted **demo mode**, so no keys, model download, Python,
Pinokio, Conda, WSL or Docker are needed for this path.

To give someone private browser access from another device, follow the
[private guest setup guide](docs/operations/demo.md). It covers enabling guest
mode, Tailscale sharing, browser access and returning to operator mode.

Choose **Try a tiny counter app**, send, review the project diff, then **Apply
changes** and **Run preview**. The starter generates `index.html`, `style.css`
and `app.js` directly. The demo first creates the counter, then proposes changing
its increment from one to two. It verifies the workflow, not model quality.

**Change project files** is the default request mode. **Conversation** keeps a
request conversational. On a real backend, the default enables native workspace
tools: list/read files, write text directly and save existing text/image assets
into project files. Successful requests save their staged changes automatically
as one revision. Failed or cancelled requests preserve accepted files. Configured
computation and image tools can be used alongside file tools; web access stays
explicit. The current executor remains isolated Node, without a general shell.

Demo generation, legacy file-format requests and manual edits retain review. The file
list shows the whole candidate project and marks changed files. **Apply changes**
saves a revision; **Reject changes** preserves the proposal for inspection.
**View current** returns to accepted files. The Editor's version button opens the
floating **Revisions** tool with Current, Working and History groups. **Run preview** opens the selected HTML page or
the project's `index.html`, including while reviewing JavaScript or CSS.
Later proposals use the latest accepted revision, even when you are
viewing an older one. Another client's changes can make a proposal stale; refresh
and generate against the current revision.

Use **Edit** for the Monaco workspace. Edit existing text, add/delete/restore
files, inspect **Source** or **Diff**, then **Save for review** (Ctrl/Cmd+S) and
**Apply changes**. **Edit copy** on a pending proposal creates a separate manual proposal. Drafts survive
workspace switches in the same tab; save them before closing/reloading. They are
not included in model requests or Node execution until saved and applied.
The **Export** menu groups file, project-manifest and app-bundle downloads. A stale
draft offers **Export draft** and **Discard draft**. Monaco and language workers are
served locally; no editor CDN is required. If editor loading fails, a plain-text
fallback preserves editing access.

Chat, Editor and Preview share a three-pane workspace. Each pane's top-right button
expands it to fill the workspace and restores the split view. Drag a divider to
resize adjacent panes. Hiding Preview gives Editor its space; Show Preview brings
the running preview back. Chat uses one scroll area for replies and logs. Files & assets,
Runs, Tools & attachments and the workspace chooser open as floating tools.
Hidden views retain drafts and running requests. Preview starts explicitly with
Run preview and unloads with Stop preview. Its footer names the captured version
and offers **Update preview** when the accepted head changes. See the
[workspace UI contract](docs/contracts/workspace-ui.md) for controls and lifetimes.

Use the × beside a workspace to delete it. Confirmation lists the data being
removed; active model/Node/image jobs must finish or be reconciled first. Deletion is
permanent and includes that workspace's chat, files and execution history.

Rejection accepts an optional reason. Chat shows the decision, while **Next
request context** explains what the selected model receives. Later requests
include proposal outcomes and reasons; settled file proposals are summarized,
pending contents remain available, and full source stays inspectable. Rejection preserves accepted files and chat
without starting another generation. Each request saves its context when
submitted, so later decisions do not change queued work.

**Download** exports the selected file; **Download project** exports a JSON file
manifest. **Download app bundle (.zip)** materializes source and bound images.
Preview supports relative classic JavaScript, styles and static image references.
Browser module builds, packages and dynamic relative resource loaders remain future
work. Native Node execution is a separate capability described below. The
[artifact contract](docs/contracts/artifacts.md) describes path/size limits and
preview behavior. Valid proposal JSON does not guarantee working code.

## Connect a model

Add settings to your existing `.env`. Use `.env.example` as a template only when
you do not already have that file. For named runtimes and model choices, set
`DEEP_MODEL_PROFILES_FILE` to a machine-local copy of the
[catalog example](deploy/models/profiles.example.json). It defines connections
and models together, with credentials referenced from `.env`. Empty task
preferences leave selection manual. See the [routing guide](docs/operations/model-routing.md)
for multiple PCs, read-only discovery and image presets.

The existing environment-only shortcuts remain supported when the catalog has
no connections. For LM Studio:

```dotenv
DEEP_DEFAULT_BACKEND=lmstudio
DEEP_LMSTUDIO_MODEL=google/gemma-4-e4b
# Optional; this is the default:
# DEEP_LMSTUDIO_BASE_URL=http://127.0.0.1:1234/v1
```

Use the exact model ID listed by your runtime. Enable hosted backends by naming
a model and providing its matching key:

```dotenv
DEEP_GROQ_MODEL=openai/gpt-oss-120b
GROQ_API_KEY=your-key
DEEP_OPENAI_MODEL=gpt-5.4-mini
OPENAI_API_KEY=your-key
```

Keys alone do not enable a backend. Groq and OpenAI use their official endpoints
and separate keys; global SDK endpoint overrides are ignored. `TYPESAFE_API_KEY`
and `JEV_MODEL` are reserved for later decision-model experiments, not chat routes.

Restart Deep and choose **Model backend** before sending. Each request sends the
completed conversation and current prompt to that backend; app/file modes also
send the accepted project files. A failure stays with
that backend; there is no automatic retry or fallback. Replies display their
persisted configured backend/model, including after reload. These labels do not
verify an upstream model revision or GPU placement.

To select several models through one runtime, configure a
[model profile catalog](deploy/models/profiles.example.json):

```dotenv
DEEP_MODEL_PROFILES_FILE=deploy/models/profiles.example.json
```

Restart Deep. **Model backend** then offers named profiles and **Auto · task
preferences**. Auto uses the selected **Task preference** and chooses the first
configured profile that supports the requested tools. Its selected profile and
reason stay with the run through tool calls, reload and branching. Existing
backend selections remain explicit overrides. See [model routing](docs/operations/model-routing.md)
for runtime requirements and switching checks.

The legacy single-backend configuration still works for llama-server and other
compatible servers:

```dotenv
DEEP_PROVIDER=compatible
DEEP_MODEL_BASE_URL=http://127.0.0.1:8080/v1
DEEP_MODEL_ID=your-loaded-model
```

Start your model runtime separately. `DEEP_MODEL_BASE_URL`
includes its API base path; the service appends `/chat/completions`. An optional
`DEEP_MODEL_API_KEY` stays on the Rust side. The adapter requests streaming text.
The browser displays saved draft snapshots while the run proceeds and can recover
them after reload. Success publishes a complete reply and any pending file proposal;
cancelled or failed drafts stay visibly incomplete. Backends that return ordinary
JSON in the same request are supported too.

Replies show provider-reported model identity, token usage, queue/execution time,
and time to first visible text when available. Missing usage stays unknown;
cost is not estimated. These values describe observed requests, not GPU utilization.

`DEEP_MODEL_TIMEOUT_SECONDS` defaults to 180 and
`DEEP_MODEL_MAX_OUTPUT_TOKENS` to 4096, applying to all configured text backends.
Stages sharing a resource group execute one at a time; independent groups can overlap.
Backend configuration is not a health probe.

Pinokio and
Conda can keep owning experimental model environments. Deep connects over HTTP
and does not install packages into them. Keep Wan2GP as a sibling checkout.

## Observe runs

Optional local traces and metrics show resource waits, model/image work, failures
and native recovery. Start `npm run observability:up`, set
`OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:14318` in `.env`, then restart Deep.
Open [the Grafana dashboard](http://127.0.0.1:13000/d/deep-overview).
It includes recent run traces and grouped latency/outcome charts. To add raw
llama-server/WanGP output, select their log files in `.env` and run
`npm run observability:logs`. Run `npm run observability:check` to validate every
dashboard query without generating work.
See [setup and real-run evidence](docs/operations/observability.md) for the
bounded export, private-field policy and repeatable verification commands.

## Generate an image with WanGP

Start WanGP separately and enable its **MCP with Web UI** option on localhost.
Use the [profile example](deploy/images/profiles.example.json), then add:

```dotenv
DEEP_IMAGE_PROFILES_FILE='deploy\images\profiles.example.json'
```

Restart Deep. Select a real **Model backend**, then choose **Tools → Image
generation → FLUX.2 Klein 4B**. This switches to Conversation. Send a request such
as “Use the image tool to draw a small robot watering a plant.”
The saved result card shows the actual image, status and **Download image**.
While generating, it shows WanGP's reported phase and available step counts,
elapsed time and last job contact. Delayed contact is marked after ten seconds.
Progress describes the current phase and does not estimate overall completion or ETA.
The model can decline to invoke the tool. Permission alone does not generate.

Failed submissions preserve the selected profile. WanGP must already have
the selected model and compatible settings. Deep starts no inference process and
changes no Python environment.

In local operator mode, **WanGP models and LoRAs → Refresh WanGP** reads model
definitions, the selected checkpoint's availability and compatible local adapter
filenames without loading weights. Add a described `lora_catalog` to an image
preset to let the assistant choose optional adapters per call. The
[catalog example](deploy/images/profiles.loras.example.json) uses placeholder
filenames; replace them with compatible installed adapters. Fixed preset LoRAs
remain in place. Saved image details show the exact submitted combination.

The [image contract](docs/contracts/images.md) defines limits, privacy,
cancellation and unresolved-job recovery. Video generation is not integrated.

## Inspect images in chat

Upload a PNG/JPEG in **Assets**, or generate one. Select a model profile marked
**Vision**, then choose images under **Images for chat** and send your message.
Up to four images totaling 16 MiB can be attached. The selected model connection
receives their pixels. Later turns keep metadata; reattach images for another
visual question. Failed sends preserve your selection.

Profiles enable this with `vision:true`. Local models need their matching vision
projector. The [OpenAI chat examples](deploy/models/profiles.openai.example.json)
include GPT-5.4 Mini and GPT-5.4. Image generation/editing uses a separate
**Image preset** selection. An OpenAI image preset sends its prompt and optional
**Reference image** to OpenAI directly. The [OpenAI image examples](deploy/images/profiles.openai.example.json)
can be combined with existing local presets in `DEEP_IMAGE_PROFILES_FILE`.
Credentials stay in `.env`; machine-local catalogs stay out of Git.

The [vision contract](docs/contracts/vision.md) defines attachment scope and provenance.

## Use images in an app

Generated images automatically appear in the workspace's **Assets** panel.
**Upload image** adds your own PNG/JPEG to the same image library.

1. Choose an image's project path, such as `assets/hero.jpg`, and click **Use in
   project**.
2. Review and **Apply changes**. The project now pins that exact image ID at the
   chosen path. Uploading/generating alone never changes project files.
3. Use **Change project files** and ask the model to include
   `<img src="./assets/hero.jpg">`. It receives paths and metadata, not image bytes.
4. Review/apply the source changes, then **Run preview**.
5. **Download app bundle (.zip)** exports real source files and images. A JSON
   project manifest contains asset IDs only, not portable image data.

Replacing a binding preserves the older image in historical revisions.
**Remove binding** does not delete the library image. Text editor drafts and
historical exports retain their exact references.

The [asset contract](docs/contracts/assets.md) defines ownership, budgets,
supported static preview references and export behavior. Source edits and Node
jobs do not receive binary pixels. Image editing requires an explicit reference
image. Chat vision separately requires explicit chat attachments.

## Check or run JavaScript

The optional Node worker runs saved project files in disposable Linux Docker
containers. Build its image from the small dedicated context:

```powershell
docker build -t deep-node-worker:local -f workers/node/Dockerfile workers/node
```

Add `DEEP_WORKER_IMAGE=deep-node-worker:local` to `.env` and restart Deep. The
image was built during development; rebuilding picks up later runner changes.
Select a saved `.js`, `.mjs` or `.cjs` file. **Run script** and **Check syntax**
appear beside its path. Selecting a file starts no execution. The script receives
the saved project text files inside an isolated Node container. Binary image
bindings are not mounted. Results survive reload;
**Stop execution** removes the entire container. Browser scripts can be
syntax-checked, but Node has no browser DOM. HTML apps use **Run preview**.

While viewing other files, **Run JavaScript** opens an optional script picker.
One saved result card shows the entry, input revision, status, logs and generated
files. It stays visible when you switch files or collapse the controls. Prior
executions remain available in the card's history.

**Ask AI to fix** attaches a terminal result to a draft request, preserves your
text/backend and selects **Change project files**. Inspect or remove the attachment,
then **Send**. The selected model receives the result identity and up to the last
8 KiB of each log stream. Captured files are not attached. Older results identify
their input revision while fixes target current accepted files. Only explicitly
included results appear in the conversation and later successful-turn context.

When a script produces files, **Review output changes** creates their proposal.
Review and apply through the existing project workflow, then rerun explicitly.
Pending changes must be applied before execution. **Current files** returns to
accepted files without discarding a draft. Scripts are optional project files;
generating an HTML app does not require a Node generator.

For model-driven execution, select a real backend and enable **Allow one
saved-script action for this request**. The checkbox names the permitted saved
revision. Choose **Conversation** to report results or **Change project files**
to propose a fix, then ask, for example, “Run main.cjs once and explain the
failure.” The model receives the saved project and the worker's bounded logs.

Permission defaults off and resets after acceptance, workspace changes or a
change to the saved revision. One syntax check or run is allowed per request.
The model can use search too when separately enabled. Newly proposed code needs
review/application before a later request can run it. Generated output still uses
**Review output changes**. The [execution contract](docs/contracts/execution.md#model-driven-execution)
and [live native-tool check](docs/reviews/node-tools-2026-10-30.md) describe the flow.

Scripts can write regular UTF-8 text files under `/output` (also available as
`process.env.DEEP_OUTPUT_DIR`). Successful jobs capture at most 64 files and
1 MiB of paths/content. Unchanged outputs are omitted from proposals; missing
outputs never delete existing files. Review compares against the exact input
revision, and applying stale output is rejected. Other files in `/work` remain temporary.

Scripts may also write to `./output` (`/work/output`). It shares capture limits
with `/output`; duplicate mapped paths fail capture. **Check syntax** does not
execute a generator. After **Run script**, use **Review output changes** and
apply the proposal to add the files to the editor. Old successful jobs cannot
recover discarded files; run the script again after rebuilding an older image.

Limits are 15 seconds, one CPU, 256 MiB, 64 processes, 16 MiB per temporary directory
storage and 64 KiB saved output per stream. Generated code gets no application
keys, host mounts, Docker socket or external network. Captured files require
explicit proposal review/application; other writes are discarded. No packages are
installed. The container has its own watchdog so an app crash does
not leave generated code running indefinitely. See the
[execution contract](docs/contracts/execution.md) for controls and limitations.

The first enabled use binds the worker to its local Docker daemon and canonical
database path. Changing Docker context does not move that binding. A copied/moved
bound database cannot execute or clean up the original's jobs; restore its original
path or leave execution disabled. Automatic rebinding is not implemented. The
application container has no Docker socket and this worker is currently intended
for the native application. Chat, proposals and browser previews work without it.

## Optional web search

Start the [local SearXNG service](deploy/search/README.md), add these settings to
the root `.env`, and restart Deep:

```dotenv
DEEP_SEARXNG_URL=http://127.0.0.1:8888
DEEP_SEARXNG_ENGINE=brave
```

Select a model backend and check **Search the web for this request** before
sending. Search defaults off again for the next request. The selected model can
call the configured search engine, then answer or propose file changes. Activity
and saved source links appear beside the response and survive reload. Incompatible
models and search failures show an error without switching providers.

Search-only requests allow up to two searches and three model turns within 90
seconds of execution. Script permission permits four model turns in a combined
request. Model-generated queries can contain conversation or project details.
SearXNG forwards queries to its configured upstream, and saved snippets return to
your selected model. This is not offline search. See the
[search contract](docs/contracts/search.md) and
[live application check](docs/reviews/search-run-smoke-2026-10-30.md).

### Provider checks

The standalone probe supports self-hosted SearXNG and paid OpenAI web search,
independently of your chat backend. Both passed the
[live provider checks](docs/reviews/search-providers-2026-10-30.md).
The probe is independent of application search. Hosted OpenAI search is available
only through this explicit developer probe for now.

```powershell
npm run test:search
# Self-hosted: first follow deploy/search/README.md to start the service.
node scripts/search-smoke.mjs --provider searxng --searxng-url http://127.0.0.1:8888 --engine brave --query "Rust programming language official documentation"
# Hosted: reads OPENAI_API_KEY from the process environment or optional .env.
node scripts/search-smoke.mjs --provider openai --model gpt-5.4-mini --query "Rust programming language official documentation"
```

OpenAI requires a separate explicit search model (`--model` or
`DEEP_OPENAI_SEARCH_MODEL`) and can incur API charges. SearXNG forwards queries to
its configured keyless engine. Neither option sends your conversation or project
files. The probe never retries or switches providers. It does not open source URLs;
OpenAI's hosted tool retrieves sources remotely. Existing chat and execution
flows do not invoke the probe. See the [probe contract](docs/contracts/search-probe.md)
for controls and response meanings, and the [deployment guide](deploy/search/README.md)
for the isolated SearXNG lifecycle. DuckDuckGo remains an experimental adapter
after its recorded HTTP 403; it is not a verified working provider.

## Build and verify

```powershell
npm run build:web
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The opt-in image smoke test uses a running Deep instance, its configured WanGP
profile and an explicitly selected text backend. It creates a synthetic workspace
and can incur that backend's API charges:

```powershell
node scripts\image-smoke.mjs --url http://127.0.0.1:4318 --backend openai --profile flux-klein-4b --output generated-image.jpg
node scripts\image-smoke.mjs --url http://127.0.0.1:4318 --backend openai --profile flux-klein-4b --cancel
```

The first command verifies actual generation, image decoding, downloaded bytes,
SHA-256 and reload without changing project files. The second verifies scoped
cancellation and absence of late image publication. Both retain their workspace
receipts for inspection. Use a new output filename to avoid overwriting a file.

The separate [video runtime probe](docs/operations/video-probe.md) verifies one
WanGP text-to-video job, MP4 integrity/playback and scoped cancellation. It defaults
to CPU-only preflight and requires `--generate` for inference. Video and 3D are
not yet application tools or workspace asset types.

For repeated local model/WanGP measurements, use a separate local Deep instance
without guest mode and an unused report filename:

```powershell
node scripts/image-handoff.mjs --url http://127.0.0.1:4321 --backend lmstudio --profile flux-klein-4b --repeat 2 --cancel --report test-results/handoff.json
```

This reuses the browser smoke checks, records model residency and sampled GPU
memory, and checks an arithmetic chat after confirmed image cancellation.
It stops on the first failure and retains the failed receipt. It never unloads
models automatically. See [the measured run and repeat instructions](docs/operations/gpu-handoff.md).
Use `--backend compatible` for llama-server; LM Studio residency is then left unprobed.
`npm run test:scripts` checks the reporting and failure paths offline.

Inference is serialized per resource group. Compatible/llama-server, LM Studio
and WanGP share `local-gpu` by default. Hosted backends use separate provider
groups. Operator overrides are documented in
[resource scheduling](docs/contracts/runtime.md#local-inference-ownership).
Queued cancellation sends no inference request, and uncertain completion keeps
the affected resource fenced until reconciliation or operator resolution.
Chat reports waiting, active requests, live cleanup and blocked ownership
separately from the parent run status. A cancelled run can still be draining.
Settlement concerns that request, not model unloading or free VRAM. See
[local-model activity](docs/contracts/runtime.md#public-local-model-activity).
Standalone llama-server can opt into native timeout/Stop recovery with
`DEEP_MODEL_RUNTIME=llamacpp` and its `--metrics` launch flag. Deep retains the slot
until native running/deferred work is confirmed idle. See
[native recovery](docs/contracts/runtime.md#native-llama-server-recovery) for its
eligibility and limits. Longer local generations can use
`DEEP_MODEL_TIMEOUT_SECONDS=1800`; the default remains 180 seconds.

The asset-reuse check creates a fresh synthetic workspace, uploads a local image,
reviews/applies its project binding, asks the selected backend to create and edit
an app, and verifies captured preview and ZIP bytes:

```powershell
node scripts\asset-smoke.mjs --url http://127.0.0.1:4318 --backend openai --image your-image.jpg --output app-bundle.zip
```

Only source and image metadata go to that text backend. The image stays local to
Deep's upload/preview/export path. This check can incur text-model API charges.

CI also checks the locked workspace with Rust 1.89 and runs `cargo audit --deny warnings`
on changes and weekly. Install `cargo-audit` to run the advisory check locally.
Dependabot monitors Cargo, npm and GitHub Actions updates.

The Docker integration test is opt-in and uses disposable fixtures, no model API:

```powershell
$env:DEEP_TEST_WORKER_IMAGE = 'deep-node-worker:local'
cargo test --locked --test executions -- --ignored
```

After the web build, `cargo run --locked -p deep-server` also serves the UI on
http://127.0.0.1:4318 by default. Browser regression checks use a separate demo data
directory and ports 14318/14319:

```powershell
npx playwright install chromium
npm run test:browser
```

For fresh data on alternate ports 14328/14329, with browser output kept separate
from retained evidence, use `npx playwright test --config playwright.workspace.config.ts`.

To use an existing Chrome/Edge executable instead of downloading Chromium,
set `DEEP_BROWSER_PATH` to its absolute path before the browser check.

## Project map

```text
apps/web/          TypeScript UI and explicit HTML previews
crates/server/     Rust HTTP API, run ownership, model adapter, SQLite
tests/browser/     Browser checks for revisions and preview isolation
deploy/            Optional container recipe
docs/              Architecture, contracts, invariants and home-lab plan
scripts/smoke.py   Opt-in synthetic chat/app/file checks against a running server
data/              Local SQLite files (ignored by Git)
```

The default setup is for one trusted local user. For separate browser guests
behind private HTTPS sharing, see the [private guest setup guide](docs/operations/demo.md).
Guest mode has browser ownership without application accounts.
Keep its listener and published container port on loopback. Generated previews
have no host shell, filesystem or tool interface; browser sandboxing is not a
general native-code execution environment.
In local operator mode, API and static routes accept only loopback Host values
and matching browser Origins. Exact `localhost` and literal loopback IPs work,
including Vite's local frontend port. Guest mode additionally accepts its one
configured HTTPS origin. Direct LAN browser access remains unsupported.
Host/Origin checks mitigate DNS rebinding; they do not authenticate local programs
or make a publicly exposed listener safe.

Start with [architecture](docs/architecture.md), [runtime contract](docs/contracts/runtime.md)
and the [delivery plan](docs/plan/plan.md). The [home-lab notes](docs/home-lab.md) cover
optional runtime hosts. Native workspace tools include bounded literal source
search, exact targeted edits and text-file deletion. General Linux execution and context
compaction remain follow-up work.

SQLite schema versions 1–20 upgrade automatically to version 21 on startup, preserving
conversations, observations and revisions; old HTML becomes `index.html`. Stop the service and copy the data directory before
upgrading if you need a rollback copy. Older binaries cannot open the upgraded
database. Version 5 preserves existing IDs/content/links while adding captured
outputs and model-run/execution provenance for proposals and revisions.
Version 6 adds rejection reasons and saved model-context snapshots. See the
[runtime contract](docs/contracts/runtime.md#drafts-and-observations).
Version 7 adds manual-edit proposal/revision provenance without inventing model
runs. After updating, run `npm ci` for Monaco dependencies and restart the server
after making the backup described above.
Version 8 retains only execution cleanup IDs when workspace history is deleted,
so stopped worker containers remain discoverable. Those IDs cannot be reused.
Version 9 adds explicit result attachments to model requests. Existing runs retain
null attachments; prior execution logs are not shared retroactively.
Version 10 adds per-request search permission and durable search activity. Existing
runs keep search disabled, with no search receipts.
Version 11 adds revision-bound model execution permission and native intent/result
records. Existing runs receive no script-execution permission.
Version 21 adds the frozen workspace file-tool grant. Existing runs and pending
proposals retain their original behavior. Rebuild/restart Deep to use native
workspace tools and automatic revisions.

## Live smoke checks

These calls use configured models and can incur API charges. Start a separate
server in a terminal with an isolated data directory; your existing `.env`
continues to supply the backend models and keys:

```powershell
$env:DEEP_BIND = '127.0.0.1:4320'
$env:DEEP_DATA_DIR = 'data/live-smoke'
$env:DEEP_MODEL_MAX_OUTPUT_TOKENS = '2048'
cargo run --locked -p deep-server
```

In another terminal, with Python 3:

```powershell
python scripts/smoke.py --url http://127.0.0.1:4320 --backend lmstudio --backend openai
# Select one backend/mode for a smaller check:
python scripts/smoke.py --url http://127.0.0.1:4320 --backend groq --mode chat
# Require observing a draft before app completion:
python scripts/smoke.py --url http://127.0.0.1:4320 --backend lmstudio --mode app --require-draft
# Validate a pending three-file proposal without applying it:
python scripts/smoke.py --url http://127.0.0.1:4320 --backend openai --mode files
```

The script submits synthetic prompts, checks arithmetic output or pending
HTML/JSON proposal binding, and prints observed drafts, status,
usage and timing. Each case creates a workspace. File checks never autoapply.
It does not execute generated HTML or establish code correctness. Stop the
separate server afterward. Offline Rust/browser checks never need live API keys.
The [file verification record](docs/reviews/files-smoke-2026-10-29.md) includes
both successful previews and model formatting/source failures.

Repeatable local-model file tasks are in [evals/cases](evals/cases/README.md).
Against a running operator installation with the updated server:

```powershell
node scripts/eval-workspace.mjs --url http://127.0.0.1:4318 --profile <local-profile-id> --report logs/workspace-evals.json
```

The runner creates retained synthetic workspaces and checks published files,
unchanged content, historical revisions and applied-proposal receipts. A model run
that completes with incorrect files fails the evaluation. The report records the
exact selected model/profile, model rounds and timing. Use an existing report
directory and a new filename. See the case guide for individual tasks and limits.
Keep retained reports outside Playwright's `test-results` cleanup directory.

If startup reports an occupied address, another process already owns that port.
Set `DEEP_BIND` in `.env` or stop that process. Docker publishes 4317; native
development defaults to 4318. Vite follows the configured API address. A bind
failure occurs before the application opens or migrates its database.

After Rust changes, stop and restart `npm run dev`: Vite updates the UI while
`cargo run` keeps the original API process until restarted. A missing-action API
error usually means the running backend is older than the current UI. An action
failure preserves the editor draft; retry it after restarting the API.
