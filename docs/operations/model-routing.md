# Runtime connections and model profiles

Deep selects a model before admitting a run. Runtimes own loading and placement;
the existing resource groups own stage serialization. This slice adds no model
downloads, runtime processes, automatic unload hooks or hardware probes.

Set `DEEP_MODEL_PROFILES_FILE` to an operator-owned JSON catalog and restart Deep.
The [local example](../../deploy/models/profiles.example.json) connects to an
existing single-model llama-server and LM Studio. It offers Gemma, Saluki and the
Qwen distill as manual choices. Replace model IDs with the runtime's exact IDs.
The [multi-PC example](../../deploy/models/profiles.multi-pc.example.json) shows
two independently named LM Studio connections. Use a machine-local copy with your
own addresses. Checked-in examples contain no credentials.
The multi-PC example uses a local LM Studio endpoint and a loopback SSH forward
for the second PC. The [Linux setup](linux.md#4-use-both-gpu-machines) pairs these
connections with image presets and shared groups for each physical GPU.

With a nonempty `connections` array, the catalog owns connection and model
definitions. Legacy model/backend environment variables are ignored for this
format. `.env` supplies secrets, process settings, global timeout/output limits
and the catalog path. Each connection declares `id`, `label`, `kind` and optional
`base_url`, `api_key_env`, `resource_group`, `runtime` and `discovery`. Credentials
are read from the named environment variable and never returned to the browser.
Different connections can use the same kind (`lmstudio`, `compatible`, `groq` or
`openai`). `default_profile` selects the initial model and omitted API selection.

Profiles reference their connection through `backend` and supply the model ID
once. They inherit transport, credentials, group, timeout and output limit.
Catalog connections are selected through model profiles, not a second default
model alias. Connections without profiles may still be inspected through discovery.
Without a `connections` block, existing `.env` backends and old profile catalogs
retain their original behavior.

Each profile has `id`, `label`, `backend`, `model`, `tool_calls` and optional
`vision` (default false). Vision enables explicit chat image attachments and
requires the matching runtime projector. The tool flag
is an operator declaration of protocol support, not evidence of reliable tool use.
Optional `preferences.conversation` and `preferences.coding` are ordered profile
IDs. Empty/omitted preferences leave manual selection as the only model choice.
Auto has no prompt classifier and makes no additional model request. Task quality
and hardware fit are not inferred from model names or advertised capabilities.

In **Model backend**, select a profile directly or choose **Auto · task preferences**
and a task. Auto picks the first listed profile supporting the requested tools and image input;
it does not try later entries after a runtime or model failure. Empty task lists
or no eligible profile reject before a run or user message is saved. Turning off
project, web, computation and image tools permits profiles without tool support.
Existing direct backend choices keep their original behavior.

Automatic preferences accept connections of kind `lmstudio` or `compatible` only. The
operator's endpoint determines where data goes: a compatible endpoint may be on
another machine. Selecting Auto never introduces a hosted-provider fallback.
Explicit profiles can use an already configured hosted backend.

## Runtime requirements

The runtime must honor the request's `model` field. LM Studio can load installed
models on demand with JIT enabled. Its TTL/eviction settings remain operator-owned.
A standalone llama-server started with one model ignores requested model names;
use router mode for multiple models. Native `llamacpp` recovery currently targets
one server/model, so profiles on that backend must use its configured model.
A router can use a generic compatible backend, retaining generic completion and
offline unresolved-ownership recovery.

`/api/status` reports configured profiles/connections without probing runtimes.
Neither file size nor profile order proves a model fits available memory. Keep one
preferred runtime for ordinary use; running the same model in LM Studio and a
standalone server creates separate runtime memory and caches.

## Read-only discovery

Open **Runtime models**, select a connection and press **Refresh models**. This
reads a bounded snapshot without generation, model load/unload, downloads or
automatic profile registration. Failed or malformed refreshes preserve the last
successful snapshot. Changing the connection or leaving the page cancels the
owned read; stale results cannot replace the selected connection's view.

Default discovery uses LM Studio's `/api/v1/models`, a native single-model
llama-server's `/props`, or a compatible server's `/models` under its configured
API base. `discovery:"llamacpp_router"` uses the router's `/models` without a
reload parameter. A connection can explicitly set `discovery:"none"`. Hosted
provider inventory is disabled; configure hosted models directly.

Inventory distinguishes loaded, unloaded, sleeping and unknown states. LM Studio
reports installed chat models and loaded instances; generic compatible lists
do not establish residency. Advertised model context limits and a loaded
instance's context are separate observations. Tool/vision flags are protocol
metadata, not quality evaluations. Explicit chat attachments require a profile
with `vision:true`, independently of inventory observations. See [chat image input](../contracts/vision.md).

Discovery is available when `DEEP_PUBLIC_ORIGIN` is unset or whitespace-only,
through `GET /api/connections/{id}/models`. Guest mode hides discovery for every
browser, including the operator's browser on the host PC. Configured model
selection remains available. See the [private guest guide](demo.md#use-operator-access-to-inspect-guest-data)
to switch to operator access. Up to four reads run concurrently, each with a
five-second limit and 2 MiB response limit. URLs, raw runtime errors, filesystem
paths and credentials are excluded from responses. Inventory remains in browser
memory without polling.

## Image presets

Image profiles select WanGP presets. Give each compatible base
model/LoRA combination a meaningful label. **Image preset details** shows the base
model, fixed LoRA filenames, strengths, and the optional adapter catalog.
`lora_catalog` lets the assistant compose described adapters on this preset's
base at configured strengths; fixed adapters remain required. See the
[catalog example](../../deploy/images/profiles.loras.example.json) and
[image contract](../contracts/images.md#configuration-and-permission).
**WanGP models and LoRAs** provides manual read-only inventory refresh in local
operator mode; discovery never authorizes an adapter or installs weights.
Several presets can share one base;
the runtime owns weight reuse. Historical metadata without LoRA details stays
unknown. The submitted profile and settings remain frozen for the run.

Image configuration uses `DEEP_IMAGE_PROFILES_FILE`. An array may include local
WanGP and hosted OpenAI presets together. The [OpenAI image examples](../../deploy/images/profiles.openai.example.json)
use Flare and Sunburst for generation or editing with `OPENAI_API_KEY`.
The [OpenAI chat examples](../../deploy/models/profiles.openai.example.json)
offer GPT-5.4 Mini and GPT-5.4 with vision. Local vision declarations require the
matching projector; this change does not load it. Merge required entries into the active
machine-local catalogs, then restart. Selecting a hosted image preset transfers
the generation prompt and any explicit reference to OpenAI. Chat attachment
selection independently controls pixels sent to the chat model.

Remote WanGP needs a
loopback tunnel under the current image transport contract. WanGP discovery
uses image presets' existing MCP endpoints, separately from text connections.

## Switching checks

Use an isolated Deep listener/data directory with no public origin. Leave existing
conversations and runtime instances intact. Select two installed models with
known load settings; account for memory already occupied by other runtimes.

1. Configure two profiles and place a different one first for each task.
2. With request tools off, send a short Conversation request using Auto.
3. Switch Task preference to Coding and send another short request.
4. Switch back to Conversation and send again.
5. Check each run's configured and reported model, saved profile, selection reason,
   status and duration. Reload and verify the same provenance.

These checks establish dispatch and publication through different models. They
do not rank model quality or isolate cold-load, token-cache and generation costs.
Only unload instances created for the check. The resource scheduler coordinates
Deep's requests; other clients of the same runtimes can bypass it.

Hardware-aware preference ranking, measured context/memory eligibility and scoped
residency changes remain later work. Image presets now preserve fixed adapters
and may authorize optional per-call adapter IDs through their frozen catalog.
