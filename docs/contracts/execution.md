# Isolated Node execution

Status: implemented manual and explicitly permitted model execution for one trusted local user.

Execution is an explicit action on one immutable saved revision. Fixed actions
are `check` (`node --check`) and `run` (`node`), with a selected `.js`, `.mjs` or
`.cjs` entry present in that revision. Manual execution requires a button action.
Model execution requires per-request permission and a native call. Preview,
proposal and revision selection never start execution. Browser DOM APIs are
not available in Node. No dependencies are installed. Writes to `/work` remain
ephemeral; explicit text outputs in `/output` can become reviewed proposals.

Selecting a `.js`, `.mjs` or `.cjs` file exposes **Run script** and **Check syntax**
beside its path. The exact selected file must exist in the saved input revision.
A deleted or unavailable script never falls back to another executable entry.
Other file selections expose a collapsed **Run JavaScript** picker of saved
scripts. Choosing a script there also selects its project source. HTML-only
projects use **Run preview**.
Scripts are ordinary project files when a task needs them.
The normal app workflow generates project files directly. Worker output capture
remains available for scripts that produce files.

Pending/rejected proposal views and the active manual draft cannot start execution.
Drafts must be saved and applied first; **Current files** can inspect/run
accepted files without discarding a draft. An applied proposal can
execute its exact saved application revision. **Current files** leaves a
proposal view, including when only one revision exists. Selecting any view or
opening execution tools never starts a model request or Node job. An uncertain
submission retains its captured revision/entry/action when the file view changes.
Its retry reuses that input and UUID.

The operator builds the dedicated worker image and opts in with
`DEEP_WORKER_IMAGE`. The image must already exist on the selected Linux Docker
engine, identify protocol `node-v2` and declare no volumes. The endpoint (local
Unix socket or Windows named pipe only) and daemon ID are persisted and checked;
later Docker context changes cannot redirect control operations. Each job freezes its
resolved image ID; the application never pulls an image implicitly. The web
application container is not given a Docker socket.

Each job receives only its selected revision files, entry and fixed action via
stdin. It has no application credentials, host project/data mounts, Docker socket,
published ports or external network. The root filesystem is read only; `/work`
and `/output` each use disposable 16 MiB tmpfs. Generated code runs as uid/gid 65534 with no effective
capabilities. A fixed trusted Node supervisor runs as namespace PID 1 and uid 0
with only SETUID/SETGID/KILL capabilities, allowing it to launch and stop unprivileged tasks.
Its 15-second watchdog exits the whole PID namespace independently of the host
application. The child cannot signal or become this supervisor. There is no init
wrapper or shared PID namespace. Both processes have no privilege escalation,
private namespaces and default Docker seccomp. Limits:
one CPU, 256 MiB memory with swap disabled, 64 processes, 16 MiB shared memory,
15-second execution deadline, bounded control calls, and 64 KiB saved text per
output stream. Daemon log persistence is disabled. These are container controls,
not a claim of protection against kernel exploits or hostile multi-tenant code.

SQLite owns execution identity, immutable input binding, status and observations.
A client-generated UUID names a submission; an identical retry returns the same
job without replay. Reusing an ID with different input is a conflict. One active
execution is admitted globally, independently of model generation concurrency.
Jobs never invoke the model, modify accepted revisions or install packages.

Cancellation of possibly started work remains pending until the entire container
is forcibly removed and absence confirmed on the bound daemon. Terminal success requires normal runner completion, zero Node
exit code, and confirmed removal. Timeout, failed execution and cancellation also
require cleanup. Cleanup failure blocks further execution admission. A per-database
random owner ID and exact job labels scope cleanup; unrelated containers are never
removed. Ownership is also bound to the canonical database path: a moved/copied
bound database cannot execute or clean up another instance's jobs. Chat and file
review remain available; restore the original path to use that worker binding.
Automatic worker rebinding is unsupported.

Deleting an inactive workspace removes its execution history but retains the
execution UUIDs in a cleanup ledger. Recovery accepts recorded execution rows or
those retired IDs, with the same daemon/path/owner/name/label checks. It does not
claim a container merely because its labels contain an unrecorded UUID. Retired
IDs cannot admit new jobs; cleanup IDs remain durable to cover late stopped creates.

Startup attempts cleanup even when execution has since been disabled or its image
is missing. Unreachable/mismatched daemons retain nonterminal cleanup state and
block admission while the other application features remain available. Successful
recovery marks unfinished work interrupted, without replay. A transport retry
never starts a second container. An interrupted create has no acknowledged ID and
is never started; it can leave a stopped container after an absence probe. This
case records a warning and permits terminal failure/cancellation without claiming
complete container removal. Scoped recovery sweeps this stopped residue. Start always
uses the exact ID returned by successful creation, so a late start cannot resurrect
a removed container.

Saved stdout/stderr, exit code and reported signal are untrusted observations.
They do not certify test quality or establish that generated code behaved safely.
All subprocesses and uncaptured writes are discarded with the container. API input,
container results and browser state must be validated before publication.

## Model-driven execution

Each request selects one `node_scope`: `off` (default), `revision` (saved-revision
script execution via `execution_revision_id: UUID`), or `compute` (temporary Node
computation via `compute_node` with optional `input_asset_ids: UUID[]`). Both active
scopes require a real backend and a configured worker (`execution_available`).

### Saved-revision scope (`node_scope: "revision"`)

Run submission accepts `execution_revision_id: UUID` (with `node_scope` omitted or
`"revision"`). Admission requires that exact current accepted revision containing
at least one script. A stale or foreign revision returns `409` before a run/message
is created. Missing capabilities or scripts return `400`. The run freezes the
revision independently of file-output mode. Conversation mode includes its saved
files when saved-revision execution is permitted.

This native scope remains available to API clients. The browser uses the selected
script actions described under [Script explanations](#script-explanations) instead
of a composer-wide saved-script permission. Status `execution_available` reports
worker configuration, not Docker health.

The native `run_node({entry,action})` function accepts only a saved entry and
`check` or `run`. Rust supplies the pinned revision and reserved execution UUID.
Inline code, shell commands, alternate revisions and extra arguments are rejected.
One check or run is permitted per request. New proposals require application
before a later permitted request can execute them. The shared
[model loop](runtime.md#model-tools) owns batch/turn/context/time budgets.

### Temporary computation scope (`node_scope: "compute"`)

Temporary computation runs in any workspace, including an empty project, without
creating a project revision or requiring a saved script proposal. Run submission
accepts `node_scope: "compute"` and optional `input_asset_ids: UUID[]` referencing
assets owned by that workspace. Foreign or unknown asset IDs return `404` before
creating a run or message. Selected input assets are resolved to deterministic
worker paths (`inputs/...` when names collide or `name` directly) within the
existing 64-file and 1 MiB portable-file budget.

The composer groups tool permissions under **Tools** and exposes **Analyze files
and create outputs** alongside **Attach text files** (`POST /api/workspaces/{id}/assets`)
and **Attach from workspace**. The workspace picker selects existing Assets or
copies one explicitly selected text file from the current accepted Project revision
into Assets. The copy preserves the saved path and exact text. Unsaved drafts and
proposal contents remain outside this picker. A newer accepted revision invalidates
an unconfirmed file choice. Already attached copies retain their original contents.
Attachment creates no model request, execution or project revision. A confirmed copy
and its selection remain usable after a failed workspace refresh. Workspace changes
abort attachment transport and prevent late selections or errors in another workspace.
Input selection grants computation access
to those asset IDs: the model receives their metadata (`id`, `name`, `media_type`,
`bytes`, worker `path`), while asset bytes do not enter model context merely
because they are selected for computation. The permission disclosure explicitly
states that tool logs (`stdout`/`stderr`) do return to the selected model and can
reveal input data. Enabling analysis selects Conversation mode. The UI suggests
CSV totals and JSON comparisons when the composer is empty. Successful outputs
appear under **Analysis and outputs** and remain available in Assets. Enabling
analysis preserves the independent workspace file and image capabilities.

Implicit creation of the first workspace for upload or submission preserves the
composer's unsent tool permissions and input selection. Explicit workspace changes
clear selected inputs. Configured computation stays enabled by default.
Accepted submissions clear selected inputs. Failed submissions preserve them.
With `workspace_tools:true`, successful compute outputs also enter the run's
candidate at their output paths and save automatically with the successful final
reply. A failed or cancelled parent discards candidate changes while retaining
already saved assets. Legacy compute-only runs keep explicit output review.

The native `compute_node({code,input_asset_ids})` function accepts a nonempty
UTF-8 JavaScript source string (`code`) and an `input_asset_ids` array naming
assets selected for the request, files saved by a permitted [URL import](url-import.md)
in this request, or output assets produced by an earlier successful
`compute_node` call in the same active request. No input selection persists
across requests: authorization derives only from the admission-frozen selection,
this request's imports, and this request's successful compute outputs. Other
unselected or foreign asset IDs and combined script + input payloads exceeding
64 files or 1 MiB fail validation before effects. Rust freezes exact `code`
(`__compute.cjs`) and `input_assets`
in `run_executions` before dispatch. The worker materializes copies in `/work`
and executes `node __compute.cjs` directly with the same isolation, PID 1
watchdog, task-level quiescence output capture and container cleanup as saved-revision
jobs. Up to **two** `compute_node` executions are permitted per request (with up
to **five** legacy model turns, or 16 with workspace tools), allowing an explicit model-requested correction after a
failed first script. Failed calls return observations without automatic replay.
A call beyond the spent computation allowance rejects only that call as a
`rejected` observation before effects; argument and input-authorization
violations still fail the whole batch.

### Shared intent, observation, and cleanup rules

`run_executions` reserves immutable intents (`seq` 1..2) per run before dispatch.
Each execution UUID is unique and intentionally precedes the worker job. Conditional
admission rechecks the running parent, exact workspace, and revision/code/input
binding in the worker transaction. Matching existing receipts do not relaunch work.
Manual admission cannot launch a reserved model identity.

Run receipts expose `node_scope`, `input_assets`, `execution_revision_id`,
`tool_executions` (ordered list of invocations), and `tool_execution` (the latest
invocation). `result` is a frozen terminal `ExecutionAttachment` saved before
model continuation. For successful temporary computations (`revision_id: null`),
captured output files in `/output` or `./output` are atomically persisted as
workspace-owned `assets` (`origin: "execution"`, `execution_id`) when cleanup
completes, and their metadata (`output_assets: [{id,workspace_id,name,media_type,bytes,origin,execution_id,created_at_ms}]`)
is returned in the tool observation while file bodies stay out of model context.
Failed, cancelled, timed-out or incompletely cleaned-up jobs publish no eligible
output assets.

Parent cancellation/failure marks all linked active children `cancelling` in
the terminal transaction. Owned admission survives dropped callers, and the
parent's background task cancels every linked worker job. Cleanup retains its
existing durable owner/daemon/path/UUID rules.

In the browser, `RunActivity` shows bounded invocation evidence beside the model
response. Saved-revision executions appear in the Project panel's `Execution results`
card, while temporary computations appear in the Conversation panel's
`Temporary computation result` card with status, logs, downloadable output files,
restricted `<img>` previews for SVG outputs (`data:image/svg+xml;utf8,...` so
active content stays out of the parent document), **Review selected files in project**
(`POST /api/workspaces/{id}/asset-proposals` against the current project head),
and `<details>` disclosure of submitted code and per-call execution details.

## Script explanations

**Run script** and **Check syntax** remain direct actions on the selected saved
script. **Run and explain** starts that same idempotent manual execution and
captures the model backend selected at the click. After execution settles, the
browser submits one Conversation request with its exact `execution_result_id`.
The model receives the saved observation and completed conversation. The request
grants no Node, search or image permission. The composer draft, mode, attachments
and tool selections remain unchanged.

The execution module owns the pending explanation in tab memory. Lost execution
responses reconcile through the existing job UUID and polling. A matched terminal
receipt consumes the explanation intent once. Cancelled/interrupted jobs do not
trigger explanation. **Stop execution** and workspace disposal clear the intent.
Reload does not replay the continuation. An active model request in the workspace
prevents explanation admission.

**Explain result** sends the selected terminal result without running a script.
If explanation admission fails, the saved result remains available for an explicit
retry through this action. Explanation requests retain ordinary model-submission
semantics without automatic retries. Their accepted receipts identify the exact
attached execution. Backend selection is frozen per action, independently of later
composer changes. These actions require a real model backend.

## Conversation results and repair requests

One project execution module owns polling, history selection, logs and **Ask AI
to fix**. Its result card remains visible independently of collapsed controls or
file navigation. Logs and prior executions use separate disclosures. The selected
result names its exact execution, input revision, entry,
action and status. Older results explicitly identify that the accepted head has
advanced. Logs render as plain text. The conversation does not duplicate live
execution history. Saved results remain readable when the worker is disabled.

**Ask AI to fix** selects a terminal result, selects file-proposal mode and
drafts a repair request. It preserves existing composer text and the chosen
backend. It sends nothing until **Send**. The composer names the attached result,
allows inspection/removal, and explains that logs leave the application for the
selected backend when sent. Switching workspaces clears this unsent attachment;
a failed submission preserves it. Active/cleanup-pending results cannot be attached.

Run submission accepts optional `execution_result_id:UUID`. Admission resolves the
result within the target workspace and requires a terminal execution. Missing or
foreign results return `404`, active results `409`, malformed IDs `422`, before
creating a run/message or invoking a model. Clients cannot supply result text.
The same transaction captures an immutable `execution_result` observation on the
run and inserts it into that run's frozen model context. It includes identity,
input revision, entry/action/status, exit/signal/error, worker truncation and log
excerpts. Each excerpt keeps at most the last 8192 UTF-8 bytes without splitting
characters; `logs_shortened` discloses shortening separately from worker truncation.
Captured file contents are excluded. Logs are serialized and labeled as untrusted
observations; they are not promoted to system messages, execution permission or
accepted file authority.

An older result stays attachable: the UI identifies its older input revision,
while file proposals target the current accepted head. Attaching a result neither
restores that old revision nor rebases a pending proposal. Conversation mode can
include an observation without including accepted file contents, as before.
Successful model turns retain their attached observation in later projected
history; failed/cancelled/interrupted turns remain excluded. Unattached execution
history is not automatically sent. Observation text counts toward the existing
128 KiB total model-context budget; overflow fails explicitly without an upstream
call or silent history dropping. Model submission retains its existing retry
semantics, without a submission-idempotency guarantee.

Returned fixes use ordinary immutable proposals and explicit apply/reject. Only
an explicit subsequent Node action reruns accepted code. There is no automatic
repair loop, tool-call conversation rewrite or inferred test success.

## Reviewed output files

Run scripts write candidate files under `/output` (also named by
`DEEP_OUTPUT_DIR`) or `/work/output` (`./output` with the default working
directory). Both directories map to project-relative paths. After the direct child exits, namespace PID 1 kills remaining
project processes. It checks every task under `/proc/<tgid>/task`, including
threads of zombie group leaders. Missing/empty task enumeration while the group
still exists, or any live task after the bounded drain, fails capture. The
independent watchdog stays armed throughout preparation, execution and capture.
The runner refuses to execute outside namespace PID 1.

Only after quiescence does the supervisor walk `/output` and optional
`/work/output`. Each root must be a real directory; root symlinks/files are
rejected. Both trees share the same limits, and duplicate/case-colliding paths
across roots fail capture instead of choosing one source. All files under either
root are candidates, including any input files copied under `output/`.
It accepts at most 64
portable regular UTF-8 files with a combined 1 MiB of paths/content, 256 traversal
entries and 16 directory levels. Symlinks, multiple hard links, devices, sockets,
FIFOs, invalid UTF-8, unsafe/colliding names and oversized output fail capture.
Regular files are opened without following links, checked through their file
handles and read with bounded buffers. Empty files, BOMs and line endings remain
exact. The Rust adapter validates again; protocol transport is bounded to 8 MiB
to accommodate JSON escaping. There is no host tar extraction or output mount.

Only successful, cleaned-up runs retain output files. Failed/cancelled/timed-out/
interrupted runs and cleanup-pending states expose no eligible outputs. Job
receipts contain only `outputs:[{path,bytes}]`; stored content is used when the user
chooses **Review output changes**. The interface explains that these files need
review/application before appearing in Project files. Successful checks explain
that they did not execute the script; successful runs with no captured files show
output directory/rebuild guidance rather than implying a project save.
Review creates one durable proposal per execution,
idempotently, against the exact input revision. It contains only changed writes;
missing outputs never delete accepted files. An unchanged output produces no
proposal. The merged candidate must also satisfy existing project bounds and path
rules; otherwise proposal creation returns `400` without changing the project.

Proposals/revisions identify exactly one source: nullable `run_id`, `execution_id`
or `edit_id`. Output review sets only `execution_id`; manual edits set only
`edit_id`. An execution does not create a model run or conversation reply.
Output from an older revision remains reviewable; application still requires the
proposal base to equal the accepted head. Apply/reject and immutable snapshots use
the existing artifact contract. No output is applied automatically.

## HTTP interface

- `GET /api/worker` reports `{enabled,ready,message}` and retries scoped recovery.
- `GET /api/workspaces/{id}/executions` lists that workspace's durable jobs.
- `POST /api/workspaces/{id}/executions` accepts
  `{id:UUID,revision_id:UUID,entry:string,action:"check"|"run"}` and returns `202`
  with the persisted receipt. Repeated matching IDs return the existing receipt.
- `GET /api/executions/{id}` retrieves a job; `DELETE` requests cancellation and
  returns the current receipt. It can remain `cancelling` while cleanup completes.
- `POST /api/executions/{id}/proposal` returns its output proposal, creating it if
  eligible. Repeated requests return the same pending/applied/rejected proposal.

Job fields: identity/input fields above, `workspace_id`, pinned `image_id`, `status`,
`stdout`, `stderr`, nullable `exit_code`, nullable `signal`, `truncated`, nullable
`error`, `created_at_ms`, nullable `finished_at_ms`, and `outputs`. Active statuses are
`queued`, `running`, `cancelling`; terminal statuses are `succeeded`, `failed`,
`cancelled`, `timed_out`, `interrupted`. Exit zero reports success only after
cleanup. Terminal records reject late settlement. Invalid/missing entries return
`400`, missing workspace revisions `404`, identity/admission conflicts `409`, and
unavailable workers `503`. Endpoints inherit the loopback/Origin policy.

The browser retains the submission UUID through uncertain transport outcomes,
reads the durable list, and can retry that same input without duplicating work.
Definitive admission rejection permits a new request. Workspace disposal aborts
reads/actions and prevents stale publication; it does not undo a server commit.
Completed jobs and their output remain visible after reload and feature disabling.
An accepted output-proposal receipt survives a failed workspace refresh. If that
view has not loaded the proposal's base revision, diff/preview/application stay
unavailable until a successful refresh supplies it; the browser never substitutes
an empty base or the currently selected revision.
