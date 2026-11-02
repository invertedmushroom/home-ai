# Image generation contract

Status: implemented text-to-image and single-reference image editing through
WanGP or OpenAI. Native chat image input follows the [vision contract](vision.md).
Image uploads, project bindings and portable export follow the
[workspace asset contract](assets.md). Video and
other image runtimes remain outside the application slice. A separate
[video runtime probe](../operations/video-probe.md) measures WanGP text-to-video
without extending this contract's profiles, tools or asset types.
Environment launchers remain responsible
for starting WanGP. Deep never installs models or starts another inference process.

## OpenAI image presets

An image profile can declare `provider:"openai"`,
`endpoint:"https://api.openai.com/v1"` and optional `api_key_env`
(default `OPENAI_API_KEY`). Credentials resolve at startup and remain in memory.
Frozen configuration stores the environment reference, never the key.
The default resource group is `openai-images`, separate from `local-gpu`.
Existing profiles retain WanGP as their default provider, serialized configuration
and fingerprints. The remaining WanGP-specific sections apply to that provider.

OpenAI settings accept `model`, explicit `size`, optional `quality` and
`background`. Dimensions are divisible by 16, at most 2048 per side and have an
aspect ratio between 1:3 and 3:1. Rust fixes one PNG output and the submitted
prompt. LoRAs, seeds and WanGP execution settings do not apply. The
[example presets](../../deploy/images/profiles.openai.example.json) select
GPT Image 2.5 Flare and Sunburst at 1024x1024 with low quality for interface testing.
Availability and charges depend on the operator's OpenAI account.

The native `generate_image` tool and image-job/asset lifecycle remain the
interface. Without a reference, the adapter posts to `/images/generations`.
With a reference, it posts to `/images/edits`, using one inline image reference.
Only the frozen reference reaches OpenAI. Chat attachments do not implicitly
become editing inputs. The adapter follows the documented
[Image API](https://developers.openai.com/api/reference/resources/images/methods/edit).

The owned task commits dispatch intent before HTTP I/O, disables redirects and
retries, bounds response bytes, accepts one base64 PNG and applies existing
hash/decoder checks. Raw provider errors remain private. A complete response
with invalid output settles as a failed job. Lost, interrupted or oversized
responses retain an unknown outcome and block conflicting image jobs.

Hosted jobs publish no incremental progress or contact updates. While the
request is in flight, the result card shows hosted wording: elapsed time and an
indeterminate bar, never WanGP submission, progress or contact labels.

Parent cancellation prevents publication while the owned request drains.
This integration provides no recoverable OpenAI job receipt or cancellation
operation. Restart never replays generation or editing. After local communication
ends, a local operator can explicitly accept an unknown OpenAI outcome,
acknowledging possible completion/charges without recovering a result. This action
claims no verified remote cancellation. Guest browsers cannot resolve it.
The operator action is `POST /api/image-jobs/{id}/accept-unknown` with
`{accept_unknown_outcome:true}`. The legacy WanGP `confirm-stopped` operation
rejects OpenAI jobs, and the OpenAI operation rejects WanGP jobs.

## Configuration and permission

`DEEP_IMAGE_PROFILES_FILE` names a JSON array of operator-owned image profiles.
The [example](../../deploy/images/profiles.example.json) selects FLUX.2 Klein 4B
at 512 x 512 with four steps. Each profile supplies an ID, label, loopback HTTP
`/mcp` endpoint and bounded settings. Profile IDs must be unique.
Endpoints cannot contain credentials, query strings or fragments.

The permitted settings are model type, resolution, inference steps, local LoRA
filenames/multipliers, guidance scale/phases, sampler and flow shift.
Deep fixes one image, one repetition and no upscaling. Without a reference,
all media input modes remain empty. Rust supplies prompt, seed and any explicitly
attached reference. The image preflight requires an image model
and an available checkpoint. Ancillary downloads and runtime-global settings
remain under the operator's WanGP configuration.

A WanGP profile may also supply `lora_catalog`, at most 16 optional adapters.
Each entry has a unique portable `id`, `label` (160 bytes), `description` (1024
bytes), unique local `file` (256 bytes, no path), up to eight `trigger_words`
(128 bytes each), and numeric `strength` from -10 to 10. Text is nonblank and
has no controls. Membership asserts compatibility with this profile's exact
`model_type`; it is operator configuration, not an inference from filenames.
OpenAI profiles reject catalogs. Empty catalogs preserve legacy fingerprints.
The selected catalog freezes privately with the run and its public description
is included in profile metadata and the native tool definition.

Fixed `activated_loras` remain required preset settings (for example an
accelerator). Selected catalog adapters append in call order, never replace
those fixed adapters. Catalog files must be distinct from fixed files. There
are at most eight fixed and selected adapters together. Composition uses one
configured scalar strength per selected adapter; fixed strength expressions
remain supported but must have one whitespace-separated expression per fixed
adapter when composing. Empty fixed strengths resolve to 1 per fixed adapter.
The assistant includes applicable trigger words in its prompt; Deep does not
silently rewrite the prompt or tune strengths. Combining compatible adapters
does not establish visual quality; operators must test combinations.

`GET /api/status` publishes `image_profiles:[{id,label,model,fingerprint,loras,lora_strengths,lora_catalog?}]`.
The optional LoRA summary contains local filenames and their configured strength
string. Historical missing summaries remain unknown. The Models window calls these
choices image presets and exposes their base/LoRA summary in details. It does not
publish endpoints or other private settings. Configuration is not a health
probe. Fingerprints identify supplied configuration, not verified weights,
runtime-global configuration or GPU placement.

`POST /api/workspaces/{id}/runs` accepts optional `image_profile_id`.
Omission grants no image permission. Admission freezes the selected profile
privately with the run. Unknown profiles and demo backends reject before creating
a run. Search, imports, Node scopes, workspace files and execution attachments can
combine with image generation.

The composer makes the first configured image profile available by default on a
real backend. Independent capability choices preserve the selected profile.
The user can disable image generation or select another profile. Failed
submissions and implicit first workspace creation preserve the unsent choice.

The selected text backend receives the conversation and native tool instructions.
WanGP receives the generation prompt and fixed settings. The generation tool
returns metadata to chat. Visual inspection separately requires chat attachments.
Deep does not expose WanGP's Python, filesystem or queue-management tools to the
chat model.

## Reference image input

An image request may add `image_input:{role:"reference",asset_id:"<UUID>"}`.
The server accepts exactly one saved PNG/JPEG from the same workspace. Admission
resolves and freezes its immutable metadata in the run transaction before returning
the receipt. A missing profile, unknown role, unknown asset or foreign workspace
rejects the request before model work. Library presence and project bindings alone
grant no transfer permission.

The composer offers **Reference image** after a profile is selected. **None** retains
text-to-image behavior. Failed submissions preserve both selections. Accepted
submissions and explicit workspace changes clear the reference image while
preserving the selected profile. The selected asset ID never resolves through a name or “last image”.
Status advertises `image_input_available:true`. A browser connected to an older
server keeps the reference selector hidden until the server supports attachments.

Rust checks the configured model's advertised `reference_images` capability before
upload. It sends the exact stored bytes through a same-origin, bounded HTTP PUT
grant. The completed upload receipt must match size, media type and SHA-256.
Only a validated `@documents/uploads/` reference enters WanGP settings as one
`image_refs` item with `video_prompt_type:"KI"` (the reference is the main scene).
Upload capabilities and provider paths remain private. Upload progress stays in
memory. A failed upload never submits generation. WanGP owns transferred copies.

The native tool accepts the prompt and configured optional adapter IDs. The text model receives frozen input metadata,
not pixels or authority to change the asset, role, profile or runtime. Job receipts
add `input_image`; run receipts add `image_input`. Saved output links back through
its image job to that input. The Image window links the reference and shows its ID
and hash in details. Reload and restart preserve this provenance.

The existing owned task retains upload communication through cancellation.
Conditional dispatch prevents a completed late upload from starting generation.
Cancellation after dispatch retains the existing remote ownership and publication
rules. Restart never repeats uploads or generation for interrupted jobs.

## Native tool and budgets

`generate_image({prompt,loras?})` accepts 1–4096 UTF-8 bytes of nonblank prompt
text without NUL and an optional array of configured adapter IDs. Omission or
`[]` keeps only fixed preset adapters. With a nonempty catalog the advertised
strict schema requires `loras`; without one it retains the prompt-only schema.
The existing native batch validator rejects unknown/duplicate adapter IDs, duplicate
identities, extra fields, unauthorized tools and a second image invocation before
effects. Permission allows one invocation, not guaranteed invocation.

Job receipts and tool observations add `generation:{loras,lora_strengths,selected_lora_ids}`.
This describes exact submitted settings, including fixed adapters. It is read
from the existing persisted job settings and admitted catalog; no schema migration
or current configuration lookup is needed. The configured profile summary remains
separate. Result details display the actual filenames, ordered strengths and IDs
after reload/restart. These records identify settings, not adapter weight hashes.

Image-only legacy runs permit two model turns. Native workspace runs permit 16
model turns so the model can gather context and save outputs. Other combined
legacy requests use their search/import/Node turn budget. The final turn disables tool calls.
Each model HTTP call uses its configured backend timeout after slot acquisition.
The image execution clock starts after the owned task acquires its resource slot.
Its 300-second deadline includes MCP connection, preflight, generation and output
transfer. Initial, image-stage and continuation-stage queue waits have no deadline.
Image runs have no enclosing wall-clock deadline that consumes those waits. The
existing 128 KiB model-context budget applies before continuation.

An image profile may include an operator-owned `resource_group`. Omission uses
`local-gpu` and preserves the legacy fingerprint. Admission freezes this private
setting with the profile. The owned image task acquires its group once and retains
the slot through generation and cleanup. Model turns release their slots before
the image stage starts and acquire again afterward. Other groups can progress
while the image runs. A cancelled planned job leaves the group queue before
generation. Queue waiting consumes no image execution budget. Cancellation and the
16-parent admission bound still apply while waiting. Execution clocks stay in
memory and restart never replays a planned job.

Planned jobs from different workspaces may wait together. Image stages and local
model stages share the same FIFO queue when configured in the same resource group.
FIFO order follows stage arrival, not parent submission: an external model can
finish tool planning while a local model call owns the GPU. Image and text stages
have equal priority. The existing 16-parent admission bound limits queued work.
Admission rejects an unresolved image in the same frozen group or runtime.
Active or unresolved dispatched image ownership fences its frozen group/runtime.
WanGP dispatch checks the durable local model-call fence atomically before
committing submission intent. Unknown outcomes remain blocked after the task's
in-memory slot is released. See
[resource scheduling and ownership](runtime.md#local-inference-ownership).

## WanGP inventory

`GET /api/image-profiles/{id}/inventory` inspects only a configured WanGP endpoint
in local operator mode. Guest sessions cannot invoke it. The Models window exposes
**WanGP models and LoRAs** with a manual **Refresh WanGP** action; opening the
disclosure performs no I/O. Snapshots remain in memory. Failed or malformed
refreshes preserve the last valid snapshot; changing presets aborts the read.

The server uses fixed `wangp_models/search` pages filtered to image definitions,
`wangp_model/capabilities`, `wangp_model/loras` for the selected base, and CPU-only
`wangp_validate` for that preset's checkpoint availability. It creates no agent
workspace and never calls install, loading, generation or queue-management tools.
Other listed models' checkpoint availability and all load states remain unknown.
Only normalized IDs/labels, capability names, local adapter filenames and selected
checkpoint availability reach the browser. Raw settings, paths and errors stay private.
Discovery never registers profiles or authorizes adapters for chat selection.

Each collection permits 256 entries and 16 pages, with bounded unique cursors.
Connection, reads and session cleanup have 10-, 15- and 3-second limits; reads
share the four existing discovery slots. Existing MCP byte/framing limits apply.
An incomplete, oversized or incompatible inventory fails the refresh rather than
pretending its partial catalog is complete. No retries, polling or persistence.
Availability excludes ancillary downloads, memory fit and runtime health;
generation repeats its existing preflight before dispatch.

## Durable ownership and failure

An image job belongs to one workspace, run and native call. Its server-issued UUID
also serves as WanGP's request ID. Exact prompt, generated seed and supplied
settings are immutable before dispatch.

The lifecycle is `planned` -> `submitting` -> `running` -> `succeeded` or `failed`.
`planned` includes non-generating model discovery/preflight. Conditional dispatch
commits the context and submission intent only while the parent remains running.
Cancellation before that commit prevents generation submission.

An owned background task survives a dropped model/HTTP future. Submission binding,
observation, scoped cancellation and failure bookkeeping remain owned until the
task exits. WanGP's remote job ID and agent context bind to the saved local job.
Observations must identify that remote job. A successful result must contain
exactly one final image from its exact `mcp_<remote job ID>_1` task.

Success publishes image bytes/metadata and the terminal image status in one SQLite
transaction while the parent is running. A completed image survives a subsequent
text-model failure or cancellation. Parent termination prevents later image
publication. Image success and parent-response success are distinct facts.

Provider/preflight/output failures produce an explicit failed image receipt.
The model may complete an explanation of that failure. A transport failure after
submission intent is uncertain, not a successful empty result. It leaves an
`unresolved` job and fails the parent. There is no automatic model/generation
retry, profile fallback or queue replay.

Parent termination interrupts an unsubmitted planned job. A possibly submitted
job becomes `cancelling` when its remote identity is known, otherwise `unresolved`.
Cleanup reads/cancels only the owned remote job. It never clears the shared queue.
HTTP cancellation alone is not proof of GPU interruption.

Restart interrupts unfinished parents before image recovery. Recovery observes
known remote jobs without resubmitting generation. Late outputs are discarded.
Unbound receipts and WanGP's `unknown_after_restart` remain visibly unresolved.
Active/unresolved image jobs block deletion of their workspace. Unresolved image
ownership blocks new image work targeting the same group or runtime. Already
queued jobs repeat the ownership check before dispatch and fail without submission
if an earlier operation becomes unresolved.

### Reconciliation and operator resolution

`POST /api/image-jobs/{id}/reconcile` resumes read/cancel reconciliation of an
already-bound remote job. Concurrent communication returns `409`.
WanGP cannot look up a job using only its request ID without invoking generation,
so Deep does not use that operation to recover an unbound cancelled submission.

When remote reconciliation cannot establish an outcome, the operator must first
stop and check WanGP. The result card offers an explicit confirmation.
`POST /api/image-jobs/{id}/confirm-stopped` requires
`{confirm_remote_stopped:true}`, an unresolved job and no locally owned
communication. It records `interrupted`, retaining the unknown-outcome explanation.
This is an operator assertion, not remotely verified cancellation or success.
It publishes no image and removes that job's fence on admission/workspace deletion.

## Progress and diagnostics

The owned image task reads WanGP's latest progress through the existing scoped
`get_job` calls. SQLite stores the latest valid `progress:{phase,current_step,total_steps}`
and the server's `last_contact_at_ms`. Counts are nullable unsigned 32-bit integers.
A known total must be positive. A known current count cannot exceed a known total.
Only recognized phases enter saved progress. When a task ID is supplied, it must
identify the exact `mcp_<remote job ID>_1` task.

Progress is advisory. Missing, malformed or foreign-task progress leaves the last
valid snapshot intact. An identity-validated job response advances last contact
even when it supplies no valid progress. A failed contact leaves the timestamp
unchanged. Progress writes cannot change status, publish an image, replay work or
reopen a terminal job. A failed progress write does not end remote ownership.
Terminal result and binary validation remain the completion evidence.

The result card shows the phase, available counts, elapsed time since job creation
and time since the last job contact. After ten seconds without contact, the card
marks updates delayed. This is a freshness indication, not a diagnosis of stalled
GPU work. Counts and the bar describe only the current phase. Unknown counts use
an indeterminate bar. The card labels retained progress as last reported when
cleanup or outcome resolution is pending. After a successful remote result, Deep
shows retrieval/validation while it fetches the image. Terminal cards stop their
display timer. Workspace changes dispose that timer with the card.

Reload and restart preserve the snapshot and its original contact timestamp.
Recovery cannot turn that historical contact into a fresh observation.
Progress does not enter the text model's tool observation or subsequent context.
Raw status text, raw phase labels, console events and legacy percentages are not
saved as progress or exported in progress logs.

Application tracing records run admission/start/terminal status, model-call
duration and usage, model round, and image communication/progress/cancellation.
Run, local job and remote job IDs correlate background work. Progress logs carry
recognized phases and counts. They exclude prompts, image bytes and raw runtime
status text. Polls without a progress change do not emit progress events.

## Binary output

Image assets are immutable SQLite BLOBs, separate from UTF-8 computation assets.
Their bytes cannot enter Node inputs or text proposals. Reviewed project bindings
can reference their IDs without copying the BLOB. Workspace deletion removes
Deep-owned image data, not copies retained by WanGP or downloads.

Transfers use grants returned for the exact output media ID. Relative URLs resolve
against the frozen MCP endpoint. Downloads require that same origin, no redirects,
an exact declared byte length and matching SHA-256. Ambient proxies are disabled.
MCP requests have a 30-second transport timeout and 1 MiB response limit.
SSE framing has a 256 KiB line/event limit.

Only static PNG and JPEG are accepted. Rust decodes bytes under an 8 MiB encoded
limit, 2048 pixels per dimension and a 64 MiB decoder allocation budget.
MIME labels, filename extensions and provider-reported dimensions are not proof.
Animated PNG and invalid/mismatched image data fail without publishing an asset.

Workspace reads add `image_assets` metadata. Runs add nullable `image_profile`
and `image_job`. Binary bytes never enter ordinary workspace/run JSON.

| Route                                              | Result                                                                                                                     |
| -------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `GET /api/image-jobs/{id}`                         | Saved job, profile provenance, prompt/seed, status/error, nullable progress/contact timestamp and optional output metadata |
| `GET /api/image-assets/{id}`                       | Image metadata                                                                                                             |
| `GET /api/image-assets/{id}/content`               | Exact image bytes for display                                                                                              |
| `GET /api/image-assets/{id}/content?download=true` | Same bytes with attachment disposition                                                                                     |

Content responses use the validated image type, `nosniff`, `no-store` and a
server-generated filename. All routes retain the loopback/Origin policy.
The browser validates workspace/run/job/output binding before rendering a result.
Polling continues for known cleanup after the parent ends and restarts after
explicit reconciliation. Workspace changes dispose obsolete polling/actions.

Schema version 13 adds `runs.image_config`, `image_jobs` and `image_assets`.
Version 14 adds upload provenance and project bindings without changing image/job IDs.
Version 15 adds nullable progress and contact fields without inventing old observations.
Version 17 adds nullable `runs.image_input` for frozen input provenance. Existing
conversations, revisions, images and jobs remain unchanged and gain no attachment.
Version 20 removes the global active-image index so planned jobs can queue together.
It preserves job/image rows and leaves dispatch exclusion to group/runtime guards.
Migration preserves older IDs, text assets, revisions and receipts. Old runs gain
no image permission. Failed migration and newer schema rejection precede recovery.
