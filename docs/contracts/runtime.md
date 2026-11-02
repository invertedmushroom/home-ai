# Runtime contract

Status: implemented local operator mode and opt-in private browser guests. HTTP JSON except static UI.
Optional native Node jobs have their own [execution contract](execution.md),
identities, admission and cancellation semantics, separate from model runs below.
Optional native model search follows the [search contract](search.md).
Optional fixed-profile image generation follows the [image contract](images.md).
Workspace uploads, pinned project assets and ZIP export follow the
[asset contract](assets.md).
Optional operator tracing/metrics follow the [observability contract](observability.md).

By default, all routes, including static UI and fallback, require an exact `localhost` or
literal loopback IP authority. The server performs no DNS lookup to extend that
allowlist. When a request supplies Origin, the server requires an HTTP(S) origin
whose host and effective port match the request authority. Foreign and opaque
(`null`) origins are rejected. Application-policy rejections return JSON `403`.
Forwarding headers cannot grant trust. Requests without Origin remain supported
for direct navigation and local clients.

Local Vite proxying preserves the frontend authority and works on its own port.
Binding to a wildcard address for a container does not permit arbitrary hosts.
Custom hostnames require the explicit private-demo configuration below. This policy
mitigates DNS rebinding; it is not account authorization or protection against a
local process that can forge headers. Keep published listeners on loopback.

## Private browser guests

`DEEP_PUBLIC_ORIGIN` enables guest ownership and accepts that exact HTTPS origin
in addition to local authorities. Configuration requires a loopback listener.
The explicit external origin also works when a local reverse proxy rewrites Host.
Forwarding and identity headers never grant authority. Tailscale Serve or an
equivalent trusted proxy supplies HTTPS and network admission. See the
[operator guide](../operations/demo.md) for deployment and cleanup commands.

`POST /api/session` initializes an anonymous browser guest. In guest mode it
requires a matching Origin. It reuses an existing valid cookie or issues a random
opaque `__Host-deep_guest` cookie with `Secure`, `HttpOnly`, `SameSite=Lax`,
`Path=/` and a 30-day lifetime. SQLite stores only its SHA-256 digest and activity
timestamps. The browser serializes initialization across tabs with Web Locks
before loading workspaces. Guest mode requires that browser capability.
Guest initialization does not create a workspace. Missing or expired credentials
on other API routes return `401` and require browser reload.

Each guest can list, create and operate only on its own workspaces. A closed route
inventory checks ownership before handlers run for workspace, run, proposal,
execution, text asset, image asset and image job IDs. Nested references retain
their existing same-workspace checks. Foreign and absent resources both return
`404`. Delete retries treat that absence as acknowledged removal in the browser.
Pre-existing ownerless data remains local-operator data. Local operator mode has
no guest restriction and must remain private on loopback.

Guest API requests with cross-site or same-site Fetch Metadata are rejected.
Unsafe methods additionally require a matching Origin. Worker readiness requests
require the application's request marker because readiness can perform recovery.
Static navigation remains accessible from an invitation link. Guest API responses
disable caching. Guests can reconcile owned image jobs but cannot assert that
WanGP was stopped. That assertion remains a local operator action.

SQLite persists guest ownership and last activity. Activity writes are throttled
to once per minute. Cleanup is an explicit offline operator command, previewing
inactive guests by default. It holds the exclusive data-directory lease and opens
only the current schema without migrations or startup recovery. An active run or
pending Node/image cleanup blocks deletion of the entire guest. Applied cleanup
removes eligible guests and their workspace content in one transaction. It keeps
ownerless local workspaces and the execution cleanup ledger. No automatic
retention task runs. Clearing cookies loses access without deleting server data.

| Method and path                                 | Input                                                                                                                                                                                                            | Success                                                                                                                                                                                     |
| ----------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET /api/status`                               | None                                                                                                                                                                                                             | Default provider/model, configured `backends: [{id,model}]`, `search_available`, `execution_available`, safe `image_profiles` metadata and fixed concurrency limits; health is `not_probed` |
| `GET /api/connections/{id}/models`                | Configured connection ID; local operator installation only                                                                                                                                                        | Bounded normalized model inventory; never generation or residency commands |
| `GET /api/workspaces`                           | None                                                                                                                                                                                                             | Workspace summaries, newest first                                                                                                                                                           |
| `POST /api/workspaces`                          | `{title}`                                                                                                                                                                                                        | `201 {id,title}`                                                                                                                                                                            |
| `GET /api/workspaces/{id}`                      | Workspace ID                                                                                                                                                                                                     | `{workspace,messages,revisions,assets,runs,proposals}`                                                                                                                                      |
| `POST /api/workspaces/{id}/branch`              | `{title?,up_to_run_id?}`                                                                                                                                                                                         | `201 {id,title}`; clones workspace files, assets, proposals (including pending reviews) and chat history up to `up_to_run_id`; `409` while jobs are active                                  |
| `DELETE /api/workspaces/{id}/messages/{run_id}` | Workspace ID and settled run ID                                                                                                                                                                                  | `204`; idempotently removes that turn's chat messages while preserving workspace revisions, assets and proposals; `409` while jobs are active                                               |
| `DELETE /api/workspaces/{id}`                   | Workspace ID                                                                                                                                                                                                     | `204`; permanently removes workspace/history, idempotent for absent IDs; `409` while jobs are active                                                                                        |
| `POST /api/workspaces/{id}/assets`              | `{name,content,media_type?}`                                                                                                                                                                                     | `201` immutable UTF-8 workspace asset (`origin:"upload"`)                                                                                                                                   |
| `GET /api/assets/{id}`                          | Asset ID                                                                                                                                                                                                         | `200` asset metadata plus exact UTF-8 `content`                                                                                                                                             |
| `POST /api/workspaces/{id}/asset-proposals`     | `{base_revision:{id:UUID\|null},asset_ids:UUID[]}`                                                                                                                                                               | `200` pending proposal adding selected workspace text assets against current head                                                                                                           |
| `POST /api/workspaces/{id}/proposals`           | `{id,base_revision:{id:UUID\|null},changes,asset_changes?}`                                                                                                                                                      | `200` manual-edit proposal; matching submission UUID and complete input are idempotent                                                                                                      |
| `POST /api/workspaces/{id}/runs`                | `{prompt,mode:"chat"\|"app"\|"files",backend?,model_selection?,image_attachment_ids?,workspace_tools?:boolean,base_revision?,execution_result_id?,search?,allow_url_import?,node_scope?:"off"\|"revision"\|"compute",execution_revision_id?,input_asset_ids?,image_profile_id?}` | `202` persisted run receipt                                                                                                                                                                 |
| `GET /api/runs/{id}`                            | Authoritative run ID                                                                                                                                                                                             | Persisted run state                                                                                                                                                                         |
| `DELETE /api/runs/{id}`                         | Authoritative run ID                                                                                                                                                                                             | Resulting terminal state, including an already completed result                                                                                                                             |
| `POST /api/proposals/{id}/apply`                | Proposal ID                                                                                                                                                                                                      | `200` immutable applied revision (idempotent)                                                                                                                                               |
| `POST /api/proposals/{id}/reject`               | `{reason?:string\|null}`                                                                                                                                                                                         | `200` rejected proposal with immutable optional reason                                                                                                                                      |
| `DELETE /api/proposals/{id}`                    | Proposal ID                                                                                                                                                                                                      | `200` rejected proposal (idempotent)                                                                                                                                                        |

Unknown API paths, including bare `/api` and `/api/`, return a JSON `404` error,
never the static UI shell.

Workspace deletion removes messages, model runs, proposals, revisions, execution
history and image data in one transaction. Queued/running model jobs,
queued/running/cancelling Node jobs and active/unresolved image jobs block deletion.
Foreign keys remain enabled; circular artifact references
are deferred until commit. Failure preserves the complete workspace; other
workspaces and the process-wide worker binding are unaffected. Deleted files are
not retained as a restore point. The UI requires confirmation, includes unsaved
draft loss in that confirmation, and removes tab state only after acknowledged
deletion. An uncertain response can be retried with the same workspace ID.
Only execution UUIDs remain in a private cleanup ledger: no workspace association,
prompts, files, outputs or logs. They preserve discovery of late stopped containers
and cannot be reused for another execution. The ledger is not conversation history.

The browser explains missing-action routes/unsupported methods as an API version
mismatch and preserves draft data. `npm run dev` starts Rust with `cargo run`;
it does not hot-reload Rust changes. Restart it after backend changes. Vite can
otherwise show the new interface against an older running API. Rejection errors
remain in the proposal review beside its actions.

Run fields: `id`, `workspace_id`, `status`, `error`, `provider`, `model`.
Runs also include `output`, `stats`, `queue_ms` and `duration_ms`.
`execution_result` is a nullable frozen, bounded execution observation. It is set
only by explicit `execution_result_id` submission; see the
[result attachment contract](execution.md#conversation-results-and-repair-requests).
`mode` records `chat`, `app` or `files`; `base_revision_id` is nullable. File mode
requires an explicit `base_revision:{id:UUID|null}` matching the current accepted
head. See the [artifact contract](artifacts.md) for validation and `409` conflicts.
Provider and model are configured labels, not verified artifact revisions.
`provider` stores the selected connection ID. Legacy IDs remain `demo`,
`compatible`, `lmstudio`, `groq` and `openai`; catalog connections use portable
operator-owned names. Connection kind separately selects transport behavior,
usage fields and provider prompt fragments. A submission's optional `backend`
selects a legacy configured backend; catalog connections require a model profile.
Omitted selection uses `default_profile` when configured, otherwise the legacy
default backend. An unknown ID returns `400`
before inserting a run/message or invoking a model. The accepted receipt freezes
the backend/model selection; a later selection in Models cannot change that run.
Existing SQLite provider/model fields retain this provenance without a migration.

Optional `model_selection` replaces `backend` on a request; supplying both is
invalid. `{mode:"profile",profile_id}` selects an operator-configured model profile.
`{mode:"auto",task:"conversation"|"coding"}` selects the first configured task
preference supporting the requested native tools. An empty task or no eligible
profile rejects before admission; failure after selection never tries another
profile. Configuration, not a prompt classifier, supplies the ordering.

`DEEP_MODEL_PROFILES_FILE` loads a strict catalog of at most 64 profiles and
optional ordered task preferences. A nonempty `connections` array (at most 16)
makes that catalog authoritative for connections/models, replacing legacy model
environment variables. Connection fields are `id`, `label`, `kind` and optional
`base_url`, `api_key_env`, `resource_group`, `runtime` and `discovery`. Secrets
resolve through environment-variable references. Without connections, legacy
`.env` configuration remains supported. Each profile names an existing real connection and a
model ID, with a label and declared `tool_calls` support. It inherits transport,
credentials, limits and group from that backend. Auto preferences use only
connections of kind `compatible`/`lmstudio`; endpoints remain operator-owned and may be on
another machine. Explicit profiles may use configured hosted backends.

Admission freezes the resolved backend/model and optional `model_route` (profile
ID/label, backend/model, task, automatic flag and reason). Schema 22 stores that
receipt and branching preserves it. Historical runs have no route. Browser
validation checks route/identity agreement and receipt selection before publishing
state. Status exposes the catalog without endpoints or credentials, and does not
claim models are installed, loaded, healthy or suitable for a hardware budget.
The runtime must honor requested model IDs. Native single-model llama-server
recovery permits only profiles matching that backend's configured model.

`GET /api/connections/{id}/models` inspects only configured text connections on
local operator installations. The closed guest resource inventory does not expose
it. It never generates, loads/unloads, downloads or registers models. Four
process-local slots bound concurrent reads; each request has a five-second timeout,
2 MiB response bound and 256-entry limit. Redirects/retries are disabled. Only
normalized IDs, names, load states, context observations and optional capabilities
are public. Errors are fixed diagnostics without URLs, secrets or upstream bodies.
Unsupported providers perform no I/O. Snapshots are transient and do not alter
configured choices. Status advertises availability without making a probe.

Each backend receives a model-context projection frozen at run admission plus
the current prompt, including turns previously served by another backend. Earlier
applied/rejected file proposals are summarized by identity, paths and outcome;
pending proposal contents remain available for follow-up revision requests.
Project state includes the accepted revision and rejection reasons. Full source
remains available in the stored transcript/proposals. Decisions after admission
affect subsequent requests, never an accepted/queued request. App/file modes also receive
the pinned accepted file snapshot. Selection is explicit in
the UI, including an explicit Auto task option when configured. There are no automatic retries or cross-backend fallbacks. Resource groups
serialize stages that share hardware, as described under stage scheduling.
`/api/status` exposes no endpoint URLs, keys, or other credential configuration.

```text
queued → running → succeeded | failed
queued | running → cancelled
queued | running → interrupted  (next process startup)
```

Terminal states never change. Cancellation records its local terminal state
before signalling the task. If success commits first, cancellation returns
`succeeded`. Dropping an HTTP request to the model is not proof of GPU interruption.
An interrupted run is not automatically retried. The UI can recover state by ID
after transport loss; submission retries have no idempotency-key guarantee yet.
The browser tracks an accepted receipt before reading workspace details, so a
failed detail read does not lose polling or the Stop action. It retries reads,
not submissions. A refresh for an obsolete workspace cannot cancel the selected
workspace's read or publish over its state.

One active run per workspace avoids branching conversation histories. A second
submission returns `409`; exhaustion of the 16 pending/active process slots
returns `503`. The service serializes stages within each group but does not control other
clients using the same model server. There is no queue deadline; the configured
model timeout begins with the model HTTP call, after waiting for the local slot.
Search-enabled runs also have a deadline for the active model/search loop.

Native shutdown accepts Ctrl+C on all platforms and SIGTERM on Unix. Both
signals enter Axum's graceful shutdown and request the existing bounded Node
worker cleanup. Shutdown does not assert remote model/image completion.
Unfinished durable ownership remains subject to startup recovery without replay.

Titles contain 1–80 characters. Prompts contain 1–32768 UTF-8 bytes, excluding
whitespace-only input. Request bodies are limited to 64 KiB, except manual proposal
submission (8 MiB transport to allow JSON escaping; the decoded project still fits
1 MiB). Projected model context
plus the current prompt and serialized project snapshot is limited to 128 KiB, with explicit failure instead of
silent history dropping. This is a byte limit, not token-aware context admission.
Failed/cancelled/interrupted turns remain visible but are excluded from later
model history. Model responses are bounded to 1 MiB. The configured output-token
limit defaults to 4096; it uses `max_completion_tokens` for OpenAI/Groq and
`max_tokens` for the compatible/LM Studio adapters.
Streaming transport is limited to 8 MiB total, with 256 KiB SSE events/lines and
1 MiB accumulated text. Non-streaming JSON responses retain a 1 MiB transport cap.
Each assembled native tool argument string is bounded to 1 MiB in JSON and SSE.
The run's 128 KiB context budget and each tool's argument validation still apply
before external effects.
The browser preserves the full composer input and reports prompt byte overflows
before submission; it does not truncate input to a UTF-16 character limit.

App mode requires a complete lowercase `html` fence, with LF or CRLF line endings.
The final standalone closing fence delimits the document; literal backticks
inside HTML are retained. Missing/incomplete fences
or provider-reported length truncation fail the run and retain earlier revisions.
Revision fields are `id`, nullable `run_id`/`execution_id`/`edit_id` (exactly one source), `number`, `html`, `files`; numbering starts at one per
workspace. Both HTML and JSON generation create reviewed proposals and no automatic revision;
see [project artifacts](artifacts.md). Content is stored as untrusted text. HTML is not validated for quality
or promised to be safe outside the restricted preview.

The compatible adapter appends `/chat/completions` to the operator-configured API
base URL and requests `stream:true`. Named LM Studio/Groq/OpenAI backends also
request `stream_options.include_usage:true`. SSE bytes are framed before UTF-8
decoding; CRLF, fragmented characters, comments and multi-line data are supported.
A stream must reach `[DONE]` and a normal `stop` finish reason to publish success.
Permitted search calls use native `tool_calls` finishes for intermediate turns.
Truncation, unauthorized tool-call completions, malformed data, transport failures
and premature EOF fail the run. No automatic retries are performed.

If the same HTTP request returns ordinary JSON instead of SSE, the adapter reads
`choices[0].message.content`; this preserves servers that ignore streaming.
Search-enabled JSON requires an explicit final `stop` reason. The legacy
search-disabled JSON path also accepts an absent finish reason.
Backend response bodies and request URLs are excluded from returned error text.
Credentials remain server-side. The demo has no inference and returns scripted
text, a fixed HTML template or scripted file proposals. Only an explicitly
search-enabled real backend receives the fixed search function. No mode executes
model-generated host code.

Named backends are enabled only by nonempty explicit model settings, not by key presence.
Groq/OpenAI use fixed official API bases with independently scoped keys; SDK-global
endpoint settings are ignored. Missing required keys, invalid bases/models,
duplicate IDs or a default that is not configured reject startup. Model presence
in configuration does not establish provider availability or account access.

Browser API responses are validated before state publication. Malformed responses
leave the last valid state intact and show a diagnostic. Switching workspaces
cancels obsolete reads and prevents their late results from replacing the new
selection. SQLite schema version 21 is supported. Version-1 through version-20 databases are upgraded
in one transaction before startup recovery. Additive run columns, revision file
snapshots and the proposal table preserve prior messages, HTML and observations.
Old HTML becomes `index.html`; old usage/timings remain unknown where absent.
Version 4 adds execution receipts and worker ownership without changing accepted
projects. Model recovery follows migration; native recovery separately confirms
container cleanup and never replays an execution.
Version 5 adds captured output and rebuilds proposal/revision provenance while
preserving every existing ID, snapshot and source/base/application link. The
private startup connection uses SQLite's table-rebuild procedure, verifies all
foreign keys inside the transaction and reenables enforcement before recovery.
Version 6 adds optional proposal rejection reasons and private per-run
`context_messages` snapshots. Legacy reasons remain null, and prior runs have an
empty context snapshot because their exact context was not recorded. Prior
transcripts, outcomes, snapshots, ownership and observations remain intact.
Version 7 rebuilds proposals/revisions to add unique manual `edit_id` provenance
and enforce exactly one of three source IDs. Existing IDs/order/content and all
incoming links survive; manual revisions reference their originating proposal.
Version 8 adds the cleanup identity ledger without changing existing artifacts.
Version 9 adds nullable `runs.execution_result` snapshots. Existing runs retain
null attachments and their original saved context; no old execution is attached
retroactively. Deleting a workspace removes these snapshots with its runs.
Version 10 adds frozen private search configuration, search activity and loop
round accounting. Existing runs retain search off, no receipts and zero rounds.
Version 11 adds nullable `runs.execution_revision_id` and one optional
`run_executions` intent/result per run. Old runs gain no execution permission or
native execution record. Existing search receipts and manual execution identity
remain intact.
Version 12 adds workspace-owned `assets`, temporary `compute_node` executions
(`executions.revision_id` nullable with `code` and `input_assets`), `runs.node_scope`
and `runs.input_assets`, ordered multi-call `run_executions` (`seq`), and `asset_ids`
provenance on proposals and revisions. Existing version-11 saved-revision receipts
and input-base output proposals are preserved intact.
Version 13 adds frozen image configuration, durable image jobs and binary image
assets without changing earlier text/project/execution ownership. See the
[image contract](images.md).
Version 14 adds immutable image uploads, image names/origins, revision asset
bindings and manual asset changes. Earlier image IDs, binary bytes and job links
survive. See the [asset contract](assets.md).
A failed migration rolls back before recovery. A newer schema fails startup
without recovering or rewriting its data. Downgrading the binary after an upgrade
is unsupported; stop the service and back up its data before upgrading.
Initial status and workspace-list reads load independently and retry after
failure. Selected-workspace detail reads also retry; switching selections
disposes obsolete retry timers.

## Request composition

`prompt::messages` composes one common instruction with the selected chat, HTML
or file-proposal task, then optional capability instructions in a single leading
system message. This also supports strict local templates that reject a second
system message. Substantial prompt text lives in
`crates/server/src/prompts/{default,providers,families,models}/`, embedded at compile
time with `include_str!` and resolved per fragment (`models/<slug>` ->
`families/<family>` -> `providers/<provider>` -> `default`).
Prompt edits require rebuilding/restarting the API. There is no
runtime template format or prompt registry.

History and accepted project snapshots remain data messages. Their text cannot
add system messages, tools or permissions. `run.rs` selects tool definitions,
choices and budgets from the accepted run. `Model::complete` transports the supplied
messages and native tool options without choosing application instructions.
Rust validation controls external effects and final publication independently of
prompt wording. Scripted demo responses live in `demo.rs` and use the same run
progress/final publication path without constructing a model HTTP client.

Version 23 adds frozen `runs.image_attachments` metadata with an empty default.
Existing runs, messages and images remain intact. Explicit vision attachments
follow the [vision contract](vision.md).

## Model tools

`workspace_tools:true` grants the native file workflow described in
[project artifacts](artifacts.md#native-workspace-files). It requires `chat` mode
and a real backend. Admission freezes this grant and pins the current project
head. Native file, search, import, Node and image capabilities compose independently.
Saved-revision and temporary Node scopes remain distinct execution input contracts.
Workspace runs allow 16 file calls and 16 model turns. The final turn disables tools.
The browser enables workspace files and configured computation/image tools by
default. Web access remains explicit and resets after a successful send.

Legacy API capabilities default off and all grants freeze at admission. `search` grants the fixed
SearXNG function described by the [search contract](search.md). Each request selects
one explicit `node_scope`: `off`, `revision` (`execution_revision_id` granting one
check/run action on that exact accepted saved revision), or `compute` (granting up to
two temporary `compute_node` executions with optional selected workspace `input_asset_ids`),
described by the [execution contract](execution.md#model-driven-execution).
`allow_url_import` independently grants the [public file import tool](url-import.md).
Search, imports and Node require a real model backend. Search remains independent of
the selected Node scope.
An optional `image_profile_id` permits one `generate_image` call with a real backend.
It can combine with search, imports, Node, workspace files and execution attachments.
An image-only legacy request retains its two-turn budget. Per-stage execution
deadlines are defined by the [image contract](images.md).

The same selected model receives native function definitions and saved tool
results. The adapter supports ordinary JSON and fragmented SSE calls. Text that
resembles a call grants no authority. Before any effect, Rust validates the entire
batch: unique bounded call IDs, permitted names and strict argument schemas.
Identity, permission-name and argument violations fail the run before any
effect. A call whose per-request allowance is already spent rejects only that
call: it returns a `rejected` observation, plans no intent, spends nothing and
cannot fail the run by itself. Each batch has at most two calls. Effects
execute sequentially.

| Permission                                                               | Searches | Node actions       | Maximum model turns                   |
| ------------------------------------------------------------------------ | -------- | ------------------ | ------------------------------------- |
| Search only (`node_scope: "off"`)                                        | 2        | 0                  | 3                                     |
| Saved-revision script (`node_scope: "revision"`), optionally with search | 0 or 2   | 1 (`run_node`)     | 4                                     |
| Temporary computation (`node_scope: "compute"`), optionally with search  | 0 or 2   | 2 (`compute_node`) | 5                                     |
| Image profile alone                                                     | 0        | 0                  | 2, at most one image invocation       |
| Native workspace files, with any configured capabilities                 | 0 or 2   | 0, 1 or 2          | 16; 16 file calls, one image invocation |
| URL imports, optionally with search or Node                              | 0 or 2   | 0, 1 or 2          | 5, at most two imports                |
| Neither                                                                  | 0        | 0                  | One ordinary call (`model_rounds: 0`) |

Requests without an image profile have a 390-second enclosing tool-loop deadline.
Each URL import has its own 15-second deadline and, without an image profile,
must finish within the first 90 seconds of the loop. Image-enabled requests use
the image contract's stage deadlines so GPU queue waits do not consume an
enclosing timer. Each model request must fit the 128 KiB context budget. Tool budgets force `tool_choice:none`
on the final turn or when all actions are spent. Advertised tool definitions are
computed once from the frozen capability grant at loop start and never change
across rounds; exhaustion changes `tool_choice` and per-call admission only.
Search and execution intent precedes dispatch.
Search/execution observations persist before model continuation. File-tool
observations and candidates remain run-local until final publication. Only final normal `stop` text can publish
an answer/proposal. A model run may succeed after receiving a failed script
observation. Its success means that the model response completed, not that the
script passed or that generated code was tested.

Parent termination atomically marks its admitted active child `cancelling` and
prevents later native admission. An owned background task signals worker cleanup
after every orchestration outcome. Native admission retains task ownership across
SQL binding and task registration even when its caller is dropped. Parent local
cancellation can complete while worker cleanup is pending. The worker's own
terminal/admission/deletion rules continue to apply. Restart interrupts model
work and performs existing scoped worker recovery without replay.

Successful prior turns retain saved search/native observations as untrusted
context. The store retains every turn's messages and observations regardless of
outcome; the model context projection is composed at admission from succeeded
turns only, so failed/cancelled/interrupted turns stay user-visible and excluded
from later model context. Any future inclusion must be a bounded admission-time
projection, never a rewrite of captured context. Later requests gain
no tool permission from those observations. There is no automatic retry, provider
fallback, model-controlled host write or package installation. Native workspace
file changes use atomic automatic revision publication; legacy proposals require application.

## Local inference ownership

Local `lmstudio` and `compatible` calls commit a `model_calls` dispatch intent
before HTTP I/O. Each record freezes its UUID, run/workspace, model round,
resource group, backend/model, private runtime base URL and its SHA-256,
state and timestamps. Credentials, prompts and raw transport errors are excluded.
Existing conversations and receipts gain no invented call history on upgrade.
The parent must be running when dispatch intent commits. That commit is the
cancellation boundary. After the commit, the owned attempt may send/drain even
when the parent stops. The frozen runtime identifies the original target after
configuration changes and remains confined to offline operator inspection.

An owned HTTP task survives parent cancellation or failure of its progress
subscriber. Generic local transports drain the bounded response through the
configured transport deadline. The opt-in native runtime below can close its
transport and retain the slot through runtime reconciliation. Progress delivery
uses one latest snapshot. Subscriber loss cannot abort cleanup. Parent terminal
guards continue to reject late publication and subsequent tool effects. Hosted
backends retain their existing transport behavior.

The call starts `dispatching`. An SSE `[DONE]` marker or a complete non-SSE body
sets terminal evidence independently of output validation. Terminal evidence
records `completed`, even if validation then rejects the answer.
Provider error frames, invalid event JSON and rejected output fields stop content
processing and publication, but the transport continues draining within its
existing byte limits and deadline to look for `[DONE]`. The first output error
still fails the run; the marker only settles ownership. An error frame followed
by EOF without a marker remains unresolved.

Without terminal evidence or proof that no request was sent, a transport/decoder
error records `unresolved`. Task/process
loss leaves the existing dispatch intent blocking. Startup changes remaining
`dispatching` records to `unresolved` without dispatching inference again.
An unconfirmed HTTP error response is conservatively unresolved too.
For an explicitly configured direct native llama-server, a complete JSON HTTP
400 with `error.code:400`, a nonempty message and type `invalid_request_error`
or `exceed_context_size_error` is terminal rejection evidence for that call.
Deep reads at most 8 KiB within two seconds and never publishes the error body.
The parent remains failed; ownership records `completed/terminal_response`, with
no retry or idle probe. This relies on the checked native chat handler's
[validation and first-error response path](https://github.com/ggml-org/llama.cpp/blob/68d20b884/tools/server/server-context.cpp#L4824).
Generic compatible/LM Studio errors, other statuses, unrecognized types,
incomplete/malformed/oversized bodies and read deadlines retain uncertainty.
Historical unresolved rows are not reclassified using the new decoder or a
new runtime configuration; they still require exact offline resolution.

An error identified by reqwest as connection setup failure on `send()` proves
that the model POST was not transmitted. This includes a refused connection.
The exact live intent settles directly to `resolved/request_not_sent`, with its
finish timestamp, before the group slot is released. The parent remains failed
and publishes no assistant answer. This applies to generic compatible, LM Studio
and native llama-server transports without a runtime probe or operator action.
Redirects and reqwest protocol retries are disabled, so a later connection error
cannot conceal an earlier transmitted attempt. Response-body errors, resets
after connection setup, HTTP error statuses and timeouts without connector proof
remain uncertain. Existing unresolved records are never reclassified from an
error string, a new runtime configuration or current reachability.

Transport timeouts identify the configured deadline in the saved run error,
including timeouts before headers or while streaming. Private runtime URLs and
raw transport errors remain excluded. A timeout still requires completion
evidence or explicit operator resolution; an actionable error does not retire
ownership. Tracing warns with the blocked operation ID and resource group.

### Native llama-server recovery

`DEEP_MODEL_RUNTIME=llamacpp` explicitly selects native recovery for `compatible`.
It requires a direct plain-HTTP loopback `/v1` base URL and a standalone
llama-server started with `--metrics`. Inference and probes bypass system proxies
so both reach that direct native server. Support is not inferred from a model ID,
hostname or an OpenAI-compatible response. Hosted and LM Studio calls do not use
this recovery path. The total request deadline remains `DEEP_MODEL_TIMEOUT_SECONDS`
(default 180); the local demo sets 1800 seconds for longer generation.

The native owned task closes its HTTP receive operation when its subscriber
leaves. A timeout/error after possible transmission or Stop still records `unresolved`. Only a stream that
already supplied nonempty generated text, reasoning text or tool arguments from
this exact attempt is eligible for automatic reconciliation. Headers, role-only
events and a healthy/idle runtime before generation are insufficient: the HTTP
handler might still be preparing a request that will enter the inference queue.

The foreground returns its failure/cancellation while owned cleanup retains the
same group slot. Cleanup polls the captured runtime's `/metrics`, requiring both
`llamacpp:requests_processing` and `llamacpp:requests_deferred` to be zero in two
successive samples, 200 ms apart. Each probe has a two-second deadline and 64 KiB
body limit; the whole check is bounded to 30 seconds. Redirects, unavailable,
missing, duplicate or invalid gauges fail closed. These gauges account for both
running and deferred work; slot idleness alone is insufficient.

After confirmation, an exact conditional transaction changes that unresolved
call to `resolved` with `runtime_confirmed_idle`, then releases the group slot.
The parent remains failed/cancelled; no answer is published, retried or replayed.
If confirmation fails, ownership remains unresolved for offline operator recovery.
Restart never probes/relabels historical ownership using a newly selected runtime.
The runtime choice is captured only by the live owned attempt; no schema upgrade
is required. Other clients and separate data directories remain outside coordination.

This contract relies on the direct native server's admitted single-completion
lifecycle. The checked implementation is [llama.cpp `b12000-68d20b884`](https://github.com/ggml-org/llama.cpp/blob/68d20b884/tools/server/server-context.cpp): a generated
delta follows slot admission, and the admitted request does not reenter the queue
after release. Two idle samples alone are not an admission barrier for arbitrary
HTTP work, proxies or other compatible implementations.

### Public local-model activity

Run and workspace reads add nullable `local_model:{phase,round,operation_id}`.
The projection contains only evidence for that run. It excludes runtime URLs,
credentials, groups, foreign operations and provider error bodies.
Older runs with no model-call records return null. Older APIs may omit the field.

| Phase | Evidence |
| --- | --- |
| `waiting` | A live local-model stage is acquiring its existing resource slot. Its operation ID is null because dispatch intent does not exist yet. |
| `working` | The running parent has an exact dispatch intent and live transport owner. This does not identify loading, prefill or GPU execution. |
| `draining` | The parent has ended, but the original local-model transport still owns cleanup. |
| `recovering` | The exact unresolved call has a live native idle-confirmation task. |
| `blocked` | Active/unresolved durable ownership has no live transport/recovery evidence. Existing operator recovery remains required. |
| `settled` | The exact latest call has completed or been explicitly resolved. Other operations and resident model weights may still exist. |

Scoped guards register queue waits and owned work. Dropping a guard removes only
its own observation. A prior guard cannot erase a later model turn. Queue waits
and live cleanup are not persisted. Restart derives blocked/settled observations
from the existing journal without recreating a live task or replaying inference.

This projection is read-only. It neither retires ownership nor changes admission,
deadlines, cancellation, FIFO order or execution permissions. The browser continues
workspace reads after parent termination while draining/recovery is live.
Unresolved blocked calls do not cause endless polling. Workspace changes dispose
the polling owner. Native image-job observations remain separate.

### Stage scheduling

All stages use one shared resource scheduler per leased data directory. Each
group has one execution slot and at most 16 FIFO waiters. A local model call keeps
its slot in the owned transport task. An image task keeps its slot through remote
generation and cleanup. Hosted slots count active HTTP requests and retain the
existing transport cancellation behavior. They do not establish remote GPU idleness.
The scheduler releases model slots between tool turns. Search, import and Node
work do not retain a GPU slot. The LLM/image/LLM sequence acquires the shared slot
three separate times. It never acquires a second slot while retaining the first.

The first acquired slot conditionally marks the parent running. `queue_ms` ends
at that transition. Later stage waits count toward persisted active duration, but
image runs exclude them from execution deadlines: model HTTP and image execution
clocks begin after their respective slot acquisitions. Image runs have no enclosing
parent wall-clock timer. Other tool loops retain their enclosing deadline.
Tracing records `group_wait_ms`, group and model
round or image-job identity. Queue waits are not added to persisted telemetry.
The API advertises `max_active:16` as the foreground parent bound. The total
pending/active admission bound remains 16. Per-group inference concurrency is one.
The scripted demo uses no resource slot and starts immediately after admission.

Cancellation while a model stage waits drops its queue entry before an owned
transport is started. A planned image task polls its durable cancellation state
while preserving its FIFO position. A cancelled planned task exits without later
generation. Multiple planned image receipts can coexist without owning GPU
execution. Image and text stages have equal priority in each group's FIFO queue.
Order follows stage arrival, rather than parent submission, across all workspaces
and browser sessions. External model planning uses its provider's group, then joins
the local GPU queue when it requests image generation. Continuation model turns
join their group again after the image stage releases it.

Before granting dispatch, the scheduler checks durable model and image ownership.
An unresolved operation rejects conflicting acquisition, even after restart.
Ownership conflicts use either the frozen group or the identical frozen runtime
URL, so renaming a group cannot bypass its unresolved runtime. Image recovery
excludes only its own job from that check. Inactive historical image rows and
planned image work do not block unrelated model stages. The image dispatch and
local model-intent transactions repeat the durable check before external effects.

Group names are operator-owned, case-sensitive identifiers of 1–64 ASCII letters,
digits, dots, underscores or hyphens. Defaults and optional overrides are:

| Backend/profile             | Default     | Override                                      |
| --------------------------- | ----------- | --------------------------------------------- |
| `compatible` / llama-server | `local-gpu` | `DEEP_MODEL_RESOURCE_GROUP`                   |
| `lmstudio`                  | `local-gpu` | `DEEP_LMSTUDIO_RESOURCE_GROUP`                |
| `openai`                    | `openai`    | `DEEP_OPENAI_RESOURCE_GROUP`                  |
| `groq`                      | `groq`      | `DEEP_GROQ_RESOURCE_GROUP`                    |
| WanGP profile               | `local-gpu` | Optional `resource_group` in its JSON profile |

The selected backend configuration is captured before queuing. Model dispatch
persists its effective group. Image admission freezes the group in its private
profile JSON. Missing groups in older profiles mean `local-gpu` and retain the
original profile fingerprint. Schema 20 removes the older global image-admission
index. Shared slots and transactional group/runtime ownership guards enforce
dispatch exclusion while allowing planned jobs to queue.
Group configuration does not relax the loopback policy for image endpoints.

The hardware guarantee covers this Deep process/data directory's own operations.
Operators must map aliases of one runtime/shared GPU to the same group. Other
clients or Deep data directories can bypass the scheduler. Group labels and URL
matching do not discover physical GPU placement automatically.

Active/unresolved ownership blocks workspace deletion, turn deletion, branching
and guest cleanup. Completed/resolved records are removed with workspace data.
Operation records are not copied as new ownership when a workspace is branched.

Offline `deep-server model-calls` lists records without startup recovery. It uses
the exclusive current-schema maintenance connection. After stopping Deep and
checking that the affected runtime is idle, the operator can use
`model-calls --resolve <operation-id> --confirm-idle`. This changes only the exact
active/unresolved record to `resolved` with `operator_confirmed_idle`. It does not
claim inference succeeded, publish output, reset other operations or replay work.
No HTTP route exposes inspection or resolution. See the
[operator instructions](../operations/gpu-handoff.md#inspect-local-inference-ownership).

## Drafts and observations

The worker persists best-effort full draft snapshots in `output`, approximately
every 50 ms while chunks arrive, plus first-text and final updates. The browser
uses its existing 500 ms polling while a run is active. Reload and read retries
recover the same persisted draft without another generation request. Selecting
another workspace disposes the old poll and guards against late publication.

Progress updates require `running` state. Success atomically clears the draft and
commits the complete assistant message and optional generated proposal. Failed,
cancelled and interrupted runs retain their last saved draft as incomplete text;
it never enters completed model history or supplies an executable revision.
Late progress, completion or cancellation cannot rewrite a terminal run.
Transient chunks between snapshots can be lost on cancellation/crash.

`stats` contains nullable provider-reported `reported_model`, `finish_reason`,
`input_tokens`, `output_tokens`, `total_tokens` and observed `first_token_ms`.
Token counts are validated nonnegative integers within JavaScript's exact integer
range and are never estimated from
characters, derived from other counts, or replaced with zero when missing.
Cancellation can occur before the provider's final usage chunk. Demo mode reports
no provider observations. No USD cost is calculated. Provider model labels are
reported claims, not independently verified weight revisions.

`first_token_ms` measures monotonic time from starting the HTTP call to the first
nonempty visible text delta. Search-loop timing starts before the first turn and
includes preceding model/search work. Reasoning-only chunks do not count. It is unknown
for ordinary JSON responses. `queue_ms` and `duration_ms` derive from persisted
server wall-clock timestamps, clamped at zero if the clock moves backward. Queue
time stops at execution or queued termination. Duration begins at `running` and
stops at terminal settlement; it includes local history/storage work and network
time. Active values advance on reads; terminal values are fixed. Old runs lacking
timestamps retain null timings. These observations do not measure GPU utilization
or establish that an upstream GPU stopped after cancellation.
