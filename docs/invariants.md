# Invariants and load-bearing rationale

- **`@MODULE`:** `crates/server/src/{lib,request_policy}.rs`
  - **`!INVARIANT` HTTP-1:** Every API and static request passes the configured
    authority policy. A supplied Origin must match the local authority or the
    exact configured HTTPS demo origin. Guest mode requires a loopback listener.
  - **Enforcement:** One outer middleware after API and static fallback assembly.
    DNS resolution and forwarding headers do not expand accepted authorities.
  - **Source:** Authority-denial, IPv6/wildcard transport and Vite browser regressions.
- **`@MODULE`:** `crates/server/src/{lib,store}.rs`
  - **`!INVARIANT` RUN-1:** Each accepted request has a server-issued run ID.
    Progress and cancellation use that exact identity, never global queue guesses.
  - **Enforcement:** `Store::begin`, submission receipt, run lookup/cancel routes.
  - **Source:** [Runtime contract](contracts/runtime.md); Rust HTTP tests.
- **`@MODULE`:** `crates/server/src/store.rs`
  - **`!INVARIANT` RUN-2:** A terminal run cannot publish a late response.
    Success, assistant message and any generated proposal commit atomically.
  - **Enforcement:** Conditional terminal transitions and `Store::finish` transaction.
  - **Source:** Cancellation/late-completion regression; runtime contract.
- **`@MODULE`:** `crates/server/src/{artifacts,store}.rs`, `crates/server/src/store/workspace_files.rs`, `apps/web/src/Artifacts.tsx`
  - **`!INVARIANT` ARTIFACT-1:** Explicit proposal application or successful native
    workspace publication appends a revision. Native publication atomically commits
    its reply, applied proposal and revision against the pinned head. Failed,
    cancelled and interrupted candidates cannot change accepted files.
    Existing revision content is immutable through the application interface.
    Native edits preserve text outside one exact unique match. Native deletions
    remove only current project text paths and retain earlier revisions and assets.
  - **Enforcement:** Insert-only revision operations, unique workspace revision numbers,
    `Store::finish` transaction and the run-owned `WorkspaceFiles` candidate.
  - **Source:** [Artifact contract](contracts/artifacts.md); native workspace, cancellation,
    image/file composition, Docker output and browser automatic-save checks.
  - **`!INVARIANT` ARTIFACT-2:** HTML and JSON generation publish a proposal against a
    pinned accepted base; application requires that same current head and no
    active workspace run. Unchanged files retain exact text. Settlement retries
    cannot append duplicate revisions or reverse an applied/rejected decision.
  - **Enforcement:** Transactional base/admission checks, validated write/delete
    operations, unique proposal run and applied revision, atomic application.
  - **Source:** Proposal HTTP, restart/text/deletion, stale-client and receipt-recovery tests.
- **`@MODULE`:** `apps/web/src/{App,WorkspaceLayout,Artifacts}.tsx`
  - **`!INVARIANT` WORKSPACE-UI-1:** [Workspace presentation](contracts/workspace-ui.md)
    never owns saved project state, model permissions or server jobs. Expansion
    preserves pane/editor/iframe identity and operation owners, as do divider
    resizing and Preview hide/show. Closing auxiliary tools retains their
    operations. Preview disposal requires explicit Stop/Run
    preview, workspace teardown or reload.
  - **Enforcement:** Shared App workspace/selection/receipts, stable portal hosts,
    bounded retained tool instances, gesture/observer cleanup and explicit preview stop.
  - **Source:** Browser checks for retained DOM/drafts, run polling and Stop with
    Chat hidden, divider bounds/interrupted input, Preview hide/show, proposal
    navigation, preview lifetime, floating tool bounds, model permission separation
    and image/run inspection.
- **`@MODULE`:** `apps/web/src/{Artifacts.tsx,preview.ts,preview-project.js}`
  - **`!INVARIANT` PREVIEW-1:** Responses and revision selection never execute
    generated code automatically. Preview scripts have an opaque origin.
  - **Enforcement:** Explicit preview button, iframe `allow-scripts` alone.
  - **Source:** Browser checks for explicit capture/update while browsing history;
    architecture artifact boundary. Update preview captures the accepted head;
    selection/application changes only status labels until an explicit capture.
  - **`!INVARIANT` PREVIEW-2:** The resource-blocking CSP precedes all generated
    markup, including parser-inserted external scripts, styles and images.
  - **Enforcement:** `previewDocument` prefixes the policy before the document;
    multi-file preparation parses and inlines only inside the opaque child,
    with policy before parsing and final document replacement.
  - **Source:** Browser CSP violation and zero-resource-request regression.
- **`@MODULE`:** `crates/server/src/{lib,store}.rs`
  - **`!INVARIANT` RUN-3:** At most 16 parent runs are admitted in process,
    each resource group executes one inference stage, and at most one parent is active per workspace.
    Owned local transport can outlive a cancelled parent.
  - **Enforcement:** Bounded parent admission, shared FIFO group slots and partial SQLite unique index.
    `ACTIVE_RUN_PREDICATE` is shared by the index, admission, cancellation and recovery.
  - **Source:** Runtime contract; workspace-conflict, bounded-admission and
    same-group serialization, independent-group overlap and cancelled-waiter regressions.
- **`@MODULE`:** `crates/server/src/store.rs`
  - **`!INVARIANT` RUN-4:** One process owns a data directory. Restart interrupts
    unfinished runs without replaying external work or deleting prior results.
  - **Enforcement:** Standard-library exclusive file lease retained across `Store`
    clones and startup recovery statement.
  - **Source:** Restart, data-directory ownership and last-clone lease regressions.
  - **`!INVARIANT` WORKSPACE-1:** Explicit confirmed deletion removes only the
    target workspace and its history, atomically. Active model jobs and Node jobs
    awaiting cleanup block deletion; failure preserves data and retry is idempotent.
  - **Enforcement:** Serialized transaction, active-job checks, deferred FK
    verification at commit, acknowledged-only browser state removal. Execution
    UUIDs alone remain in a cleanup ledger and cannot admit another job.
  - **Source:** [Runtime contract](contracts/runtime.md); deletion durability,
    active/cleanup-blocking, rollback and browser response-loss/draft tests.
  - **`!INVARIANT` WORKSPACE-2:** Chat turn deletion and workspace branching
    keep accepted workspace files and unapplied proposals intact.
  - **Enforcement:** `Store::delete_turn` removes only `messages` for a settled
    run while keeping `runs`, `revisions`, `assets` and `proposals` intact;
    `Store::branch_workspace` atomically clones `revisions`, `assets` and all
    `proposals` (including `pending` reviews) with remapped UUIDs and FK checks
    while truncating `messages` at `up_to_run_id`. Preserved proposals remain
    reviewable in Revisions; Chat does not synthesize replacement messages.
  - **Source:** Runtime contract; Rust HTTP and Playwright branching/turn-deletion tests.

- **`@MODULE`:** `crates/server/src/{config,routing,discovery,lib,model}.rs`, `apps/web/src/{api,App.tsx,ModelInventory.tsx}`
  - **`!INVARIANT` ROUTE-1:** An accepted run uses its selected configured backend
    and model, resolved from an explicit backend/profile or configured Auto task
    preferences before admission. Tool rounds retain that selection. No failure silently retries through another backend. Browser
    responses never include backend credentials or endpoint URLs.
  - **Enforcement:** Validated operator configuration, submission-time model
    selection and persisted provider/model/route fields, bounded calls to that same model.
    Connection kind selects protocol behavior independently of its name. Read-only
    inventory never changes configured choices or model residency and is absent
    from the private guest resource inventory.
  - **Source:** Runtime contract; routing/credential/failure HTTP tests and
    browser selection/provenance/metadata validation checks.

- **`@MODULE`:** `crates/server/src/{model,store}.rs`, `apps/web/src/{api,App.tsx}`
  - **`!INVARIANT` STREAM-1:** Drafts remain incomplete text until success commits.
    Terminal runs reject late progress. Missing usage remains unknown, and a
    stream without a normal finish and completion marker cannot publish an answer.
  - **Enforcement:** Bounded SSE decoding, conditional progress writes, existing
    success transaction and terminal predicates, validated browser observations.
  - **Source:** Runtime contract; gated stream/cancellation/protocol tests,
    browser draft/reload/validation checks.
- **`@MODULE`:** `crates/server/src/store{.rs,/migrations.rs}`
  - **`!INVARIANT` SCHEMA-1:** Schema upgrades commit atomically before
    startup recovery; failed upgrades and unknown newer versions preserve data.
  - **Enforcement:** Version guard, all pending v1–v18 to v19 upgrades in one
    transaction, FK verification before commit and enforcement restored before
    recovery. Provenance rebuild preserves IDs, snapshots and incoming links.
  - **Source:** Migration preservation/rollback and newer-version tests.

- **`@MODULE`:** `crates/server/src/{guests,request_policy,lib}.rs`, `crates/server/src/store/guests.rs`
  - **`!INVARIANT` GUEST-1:** Guest browser credentials authorize only their owned
    workspaces and resources. Existing ownerless data remains operator-only.
  - **Enforcement:** Hashed server-issued cookies, closed route inventory,
    server-side owner joins, existing checks on nested workspace references,
    matching Origin and Fetch Metadata checks before handlers.
  - **Source:** [Guest contract](contracts/runtime.md#private-browser-guests);
    foreign-resource, nested-asset, bootstrap and browser-isolation checks.
  - **`!INVARIANT` GUEST-2:** Cleanup preview cannot recover or delete work.
    Active or uncertain work blocks cleanup for the entire guest.
  - **Enforcement:** Exclusive current-schema maintenance opener without startup
    recovery, whole-guest preflight and one deletion transaction.
  - **Source:** Guest contract; preview preservation, active/cleanup blocking,
    transaction rollback and operator-data retention checks.

- **`@MODULE`:** `crates/server/src/{images,run,lib}.rs`, `crates/server/src/store/images.rs`
  - **`!INVARIANT` IMAGE-1:** Image generation requires a frozen named profile and
    an exact persisted job/call identity. The chat model cannot change runtime,
    model or fixed preset adapters, or invoke WanGP's other MCP tools. It may add
    only optional LoRA IDs from the admitted preset's catalog at configured strengths.
    Jobs retain the exact resolved settings independently of later configuration.
    Local operator inventory uses fixed read-only MCP calls, grants no tool
    authority and remains excluded from the guest resource inventory.
  - **Enforcement:** Admission, whole-batch catalog/combination validation, frozen
    preset/catalog and persisted resolved job settings, conditional
    dispatch and exact remote job/task/output binding.
  - **Source:** [Image contract](contracts/images.md); permission, zero-effect
    batch rejection and fixture/live generation checks.
  - **`!INVARIANT` IMAGE-2:** Unknown remote outcomes remain explicit and block
    conflicting image dispatch and deletion of the owning workspace. Planned jobs
    can queue together without consuming image execution deadlines. Restart never
    replays generation. Parent termination
    prevents late image publication without deleting earlier settled images.
  - **Enforcement:** Owned background task, shared FIFO stage slots, transactional
    group/runtime ownership checks, durable state, terminal
    transaction guards and scoped reconciliation. Operator resolution records
    an unknown interrupted outcome rather than claiming verified cancellation.
  - **Source:** [Image recovery](contracts/images.md#durable-ownership-and-failure);
    cancellation races, restart, lost-receipt and parent-failure tests.
  - **`!INVARIANT` IMAGE-3:** Only validated static PNG/JPEG bytes can become an
    immutable image asset. Model context and UTF-8 project/compute APIs never
    acquire image bytes or transfer capabilities implicitly.
  - **Enforcement:** Bounded provider transfers, SHA-256 and decoder limits,
    separate BLOB storage, validated browser evidence and metadata-only saved context.
  - **Source:** [Binary output](contracts/images.md#binary-output);
    byte/format/dimension/integrity and browser ownership regressions.
  - **`!INVARIANT` IMAGE-4:** Progress is advisory and bound to the owned remote
    job. Progress cannot settle work, reopen terminal jobs or imply fresh contact
    after restart.
  - **Enforcement:** Bounded phase/count parsing, optional exact task binding,
    conditional progress writes and preserved contact timestamps.
  - **Source:** [Progress contract](contracts/images.md#progress-and-diagnostics);
    malformed/foreign progress, restart, late-update and browser regressions.
  - **`!INVARIANT` IMAGE-5:** A reference image pins one immutable same-workspace
    asset and role at admission. The model cannot replace the input. Pixel transfer
    requires that explicit attachment and a verified same-origin upload receipt.
  - **Enforcement:** Transactional input resolution, metadata-only model context,
    bounded upload/hash validation and parent-conditional generation dispatch.
  - **Source:** [Reference input contract](contracts/images.md#reference-image-input);
    foreign input, upload failure, cancellation, reload and restart checks.
  - **`!INVARIANT` VISION-1:** Chat pixels require exact same-workspace attachments
    and a frozen vision-capable route. Later runs retain metadata without replaying
    pixels. Chat attachments and image-editing references grant independent permissions.
  - **Enforcement:** Transactional ID/count/byte validation, immutable metadata/hash
    verification, bounded multipart user content and validated browser receipts.
  - **Source:** [Vision contract](contracts/vision.md); provider wire, tool
    continuation, foreign/duplicate/oversized input, branch/restart and browser checks.
  - **`!INVARIANT` IMAGE-6:** OpenAI dispatch commits intent before I/O, drains
    through parent cancellation and never retries or publishes a late result.
    Unknown outcomes remain unknown until explicit operator resolution.
  - **Enforcement:** Owned task, provider-specific durable groups, bounded HTTP
    transport, terminal-response validation and parent-conditional publication.
  - **Source:** [OpenAI image presets](contracts/images.md#openai-image-presets);
    generation/editing, invalid output, lost response, group independence and cancellation tests.

- **`@MODULE`:** `crates/server/src/{artifacts,store}.rs`, `crates/server/src/store/project_assets.rs`
  - **`!INVARIANT` ASSET-1:** Project image paths pin immutable same-workspace
    asset IDs. Rebinding/unbinding never mutates old revision bytes or deletes
    library content. Source, image and execution identities remain distinct.
  - **Enforcement:** Combined namespace/ownership/budget validation at proposal
    creation, application and export. All revisions copy complete bindings.
  - **Source:** [Asset contract](contracts/assets.md#reviewed-project-bindings);
    foreign/path/stale-base, historical byte/export and migration tests.
  - **`!INVARIANT` ASSET-2:** Stored binary bytes never enter source-edit context
    or Node snapshots implicitly. Render-only preview encoding and explicit ZIP
    materialization do not become accepted source.
  - **Enforcement:** Metadata-only projection, separate text/binding fields,
    scoped preview capture and bounded immutable-byte export.
  - **Source:** [Context and scope](contracts/assets.md#model-context-and-execution-scope);
    captured model requests and real preview/export checks.

- **`@MODULE`:** `crates/server/src/{lib,run,model,search}.rs`, `crates/server/src/store/searches.rs`
  - **`!INVARIANT` SEARCH-1:** Search requires permission frozen on the accepted
    run. The model supplies only a bounded query to the fixed function. A whole
    batch must validate before any search starts. Search never grants Node,
    host-file or proposal-application authority.
  - **Enforcement:** `lib.rs::submit`, `run.rs::{queries,complete}`,
    `model.rs::Model::complete` and `search.rs::SearchConfig::search` validate admission,
    native calls and fixed loop/search/context/time budgets.
  - **Source:** [Search contract](contracts/search.md); zero-effect batch rejection,
    protocol, budget, transport, context and final-only-publication tests.
  - **`!INVARIANT` SEARCH-2:** Intent precedes HTTP and saved results precede model
    continuation. Every terminal run closes pending searches and preserves
    completed sources. Restart never replays search. Late results cannot publish.
  - **Enforcement:** `Store::{search_intents,settle_search,terminate,open}` use
    transactional writes, running-state guards and post-migration recovery.
  - **Source:** [Search recovery contract](contracts/search.md#durable-activity-and-recovery);
    gated cancellation/late result, failure/restart and migration tests.
- **`@MODULE`:** `crates/server/src/search.rs`, `apps/web/src/{api.ts,RunActivity.tsx}`
  - **`!INVARIANT` SEARCH-3:** Source links come only from validated saved search
    receipts. Model-written URLs/citations and search snippets confer no authority.
    Server-accepted query whitespace remains readable by browser validation.
  - **Enforcement:** `search.rs::{decode,valid_query,valid_source_url}`,
    `api.ts::searchReceiptSchema` and `RunActivity` check provenance and bounds,
    align Unicode White_Space semantics and render receipt-only anchors.
  - **Source:** [Search source contract](contracts/search.md#search-transport-and-sources);
    decoder checks and browser permission/reload/source/Unicode tests;
    [live check](reviews/search-run-smoke-2026-10-30.md).

- **`@MODULE`:** `crates/server/src/store{.rs,/edits.rs}`, `apps/web/src/{App,Artifacts,ProjectEditor}.tsx`
  - **`!INVARIANT` EDIT-1:** A manual submission binds one UUID to an exact
    workspace/base/ordered change list. Matching retries return one immutable
    proposal; conflicting reuse fails. Saving changes no accepted files and
    creates no model run or conversation message. Exactly one source ID is set.
  - **Enforcement:** Transactional receipt-before-head lookup, unique edit ID,
    full candidate validation, schema source checks/FKs and shared settlement.
    Save for review and Ctrl/Cmd+S call the same guarded save operation; a stale
    draft cannot create a new input, while an uncertain captured input can retry.
  - **Source:** [Artifact contract](contracts/artifacts.md); HTTP/restart/migration
    tests and browser lost-response/receipt-binding checks.
  - **`!INVARIANT` EDIT-2:** Unsaved drafts survive view/workspace changes in the
    same tab but never supply model context or Node input. Obsolete editor actions
    cannot publish over a different workspace. Project documentation cannot load
    hover media into the parent page.
  - **Enforcement:** App-owned keyed drafts, leave warning, workspace-owned
    cancellation, saved-revision-only execution, locally served Monaco assets and
    private persistent sanitizer resource stripping.
  - **Source:** Browser switch/reload/disposal/draft execution tests and repeated
    documentation-hover resource probe; Revisions/history draft retention,
    deleted-base-file restoration and selected-file save/recovery continuity.

- **`@MODULE`:** `crates/server/src/store{.rs,/context.rs}`, `apps/web/src/{App,Artifacts}.tsx`
  - **`!INVARIANT` CONTEXT-1:** Proposal rejection never changes accepted files,
    rewinds the transcript or starts model work. Later requests receive the
    decision and optional immutable reason, including for execution output.
    Model context is frozen at admission; later decisions cannot rewrite it.
  - **Enforcement:** Atomic bounded rejection, shared settlement receipts,
    completed-turn projection and per-run context captured in admission's transaction.
  - **Source:** [Artifact contract](contracts/artifacts.md); captured adapter
    requests, queued-context/restart, migration rollback and browser feedback tests.
  - **`!INVARIANT` CONTEXT-2:** Execution observations enter model context only
    through explicit attachment or a successful prior turn that included one.
    They remain untrusted and confer no execution/file authority.
  - **Enforcement:** Same-workspace terminal-result resolution and bounded frozen
    snapshot in the admission transaction, successful-turn projection, context
    budget and browser request/receipt binding.
  - **Source:** [Execution contract](contracts/execution.md#conversation-results-and-repair-requests);
    HTTP scope/terminal/budget tests, adapter capture, migration/restart and browser
    ownership/receipt checks; [live repair](reviews/result-repair-smoke-2026-10-30.md).

- **`@MODULE`:** `crates/server/{build.rs,src/{prompt,model,run}.rs}`, `crates/server/src/prompts/**`
  - **`!INVARIANT` PROMPT-1:** [Request composition](contracts/runtime.md#request-composition)
    combines task/capability instructions into one leading system message,
    separate from untrusted context data.
    Prompt text grants no tool or file-application authority.
  - **Enforcement:** `prompt::messages`, `run::complete` and existing Rust
    validators. `Model::complete` transports supplied messages/tool options.
  - **Source:** Captured HTTP requests across chat/HTML/files and search permissions.

- **`@MODULE`:** `crates/server/src/{lib,run,worker,store}.rs`, `crates/server/src/store/{assets,tool_executions}.rs`, `apps/web/src/{App,ComputeResultCard}.tsx`, `apps/web/src/api.ts`
  - **`!INVARIANT` TOOL-1:** Each request selects one explicit Node scope (`off`,
    `revision`, or `compute`). Saved-revision consent binds one accepted revision
    and one native action; temporary computation (`compute_node`) binds up to two
    executions to exact frozen script code and workspace assets selected for the
    request or produced earlier in the same request. A changed head cannot broaden
    consent. A terminal parent cannot admit another job or publish a late result.
    The independent `workspace_tools` grant freezes at admission. Its in-memory
    candidate and owned asset reads stay inside that workspace. It does not grant
    host execution or replace Node's selected-input contract.
  - **Enforcement:** `Store::begin`, `tool_executions::{authorize,plan_compute_execution}`,
    `assets::resolve_input_assets`, conditional `Store::{admit_execution,record_tool_execution_by_id}`,
    run whole-batch validation and App's scope/revision/asset permission binding.
  - **Source:** [Model execution contract](contracts/execution.md#model-driven-execution);
    stale-pin, whole-batch, late-admission, native-context, head-change and
    temporary-computation (`compute.rs`, `compute.spec.ts`) regressions.
  - **`!INVARIANT` TOOL-2:** Dropped callers retain ownership of admitted native
    work. Parent termination cancels all linked active children. Worker cleanup
    remains durable after a parent becomes terminal.
  - **Enforcement:** Owned `Worker::submit_tool` task, admission/task-registration
    mutex, `Store::terminate` child transition and background `Worker::cancel_run`.
  - **Source:** Contract; store terminal/restart/deletion tests, actual Docker
    parent cancellation and fresh-context ownership review.
  - **`!INVARIANT` ASSET-1:** Workspace assets are immutable UTF-8 text records
    scoped to one workspace. Input selection exposes only metadata and worker
    paths in model context. Computation observations contain output metadata;
    explicitly granted native `read_file` calls can retrieve owned text asset bodies.
    Failed or uncleaned jobs publish no output assets. Browser SVG previews
    render strictly in an `<img>` context so active content never executes in
    the parent document. Reviewing selected assets captures the current project
    head without altering existing saved-revision output proposals.
  - **Enforcement:** `store/assets.rs`, `store/executions.rs::finish_execution`
    atomic asset creation on `succeeded`, `prompt.rs` metadata-only input block,
    `ComputeResultCard.tsx` `data:image/svg+xml;utf8,...` `<img>` rendering and
    `POST /api/workspaces/{id}/asset-proposals` head check.
  - **Source:** [Artifact contract](contracts/artifacts.md#workspace-assets-and-asset-proposals);
    Rust store/HTTP/Docker tests (`crates/server/tests/compute.rs`) and browser
    isolation/review tests (`tests/browser/compute.spec.ts`).
- **`@MODULE`:** `apps/web/src/{App,Artifacts,Executions}.tsx`
  - **`!INVARIANT` EXEC-5:** A selected script executes only its exact saved entry.
    An unavailable/deleted entry never selects another executable implicitly.
    Uncertain submission retries retain their original revision/entry/action/UUID.
  - **Enforcement:** `Artifacts` supplies the saved revision and selected path.
    `Executions::start` binds the input before transport and retains it on failure.
  - **Source:** [Execution contract](contracts/execution.md); selected-file,
    deleted-script, lost-response, navigation/reload and disposal browser tests.
  - **`!INVARIANT` EXEC-6:** A native intent can precede worker admission. The
    project result owner keeps observing while a permitted model run is active,
    and newly observed native IDs trigger a refresh even after model completion.
  - **Enforcement:** `Artifacts` supplies native activity; `Executions` owns the
    polling effect, generation guard, abort and timer disposal.
  - **Source:** Delayed-admission browser regression and live native execution.
  - **`!INVARIANT` EXEC-7:** Run and explain binds one exact manual job and the
    backend selected at the click. A terminal receipt consumes its tab-owned intent
    once. Explain result never launches execution. Neither action consumes drafts.
  - **Enforcement:** `Executions` retains the callback/job identity, clears pending
    intent before explanation, and disposes it on Stop or workspace unmount.
    `App::explainResult` submits only the saved result with no tool permission.
  - **Source:** [Script explanations](contracts/execution.md#script-explanations);
    lost-response, backend-freeze, cancellation and workspace-disposal browser checks.

- **`@MODULE`:** `crates/server/src/{worker.rs,store/executions.rs}`, `workers/node/{runner,outputs}.cjs`
  - **`!INVARIANT` EXEC-1:** Native execution requires an explicit saved revision,
    entry and fixed action. A submission UUID binds that exact input; matching
    retries never launch twice. Execution never changes accepted revisions.
  - **Enforcement:** Validated scoped input, transactional receipt/admission,
    created-only launch and one-active-execution unique index; captured output
    requires a separate reviewed proposal and explicit application.
  - **Source:** [Execution contract](contracts/execution.md); identity/restart,
    cross-workspace, browser retry and real Docker tests.
  - **`!INVARIANT` EXEC-2:** Possibly started code cannot settle terminal before
    its whole container is removed. Cleanup failure blocks admission. Restart
    cleans only exact owned jobs on the bound local daemon and original database
    path, without replay; recovery never sweeps live tasks owned by that manager.
  - **Enforcement:** Exact endpoint/daemon/path binding, labels and immutable
    container ID, cleanup verification, mutex across SQL settlement and task
    bookkeeping. Unknown creation is never started and records stopped-residue risk.
    Deletion retains execution cleanup IDs, so scoped recovery can still find a
    stopped container without retaining its chat, files or execution history.
  - **Source:** Contract; cancellation, copied-path, watchdog and forced-crash
    verification; fresh-context review of settlement/recovery ordering.
  - **`!INVARIANT` EXEC-3:** Generated Node has no application credentials, host
    mounts, Docker socket or external network, and cannot disable the watchdog.
  - **Enforcement:** Dedicated image/input, curated child environment, uid/gid
    65534 without effective capabilities, separate trusted PID 1 watchdog,
    no-new-privileges, read-only root, bounded tmpfs/cgroups/output/time.
  - **Source:** Contract; real uid/capability/parent-signal/cgroup/filesystem/
    network/output/timeout checks and server-crash probe.
  - **`!INVARIANT` EXEC-4:** Only successful, cleaned-up jobs retain eligible
    output; capture reads bounded regular UTF-8 files after all project tasks
    stop, never follows filesystem links and never applies files itself.
  - **Enforcement:** Trusted PID 1 task drain, missing-task fail-closed behavior,
    no-follow handle/type/link checks, traversal/file/byte budgets, Rust validation,
    conditional output persistence and idempotent execution-source proposals.
    `/output` and `/work/output` share all capture budgets and mapped-path checks;
    roots must be real directories.
  - **Source:** [Output verification](reviews/output-smoke-2026-10-30.md); real
    filesystem/process/transport tests, failure/cancellation discard, provenance
    and exact-base browser/Store regressions.

- **`@MODULE`:** `crates/server/src/{url_import,run}.rs`, `crates/server/src/store/{assets,tool_executions}.rs`
  - **`!INVARIANT` IMPORT-1:** URL retrieval uses checked and pinned public addresses
    for every hop. The Node worker remains offline. Only explicitly permitted,
    active, unexpired imports can publish workspace assets.
  - **Enforcement:** Dedicated no-proxy/no-redirect client, URL/DNS/IP checks,
    bounded streaming and publication guards inside the transaction.
  - **`!INVARIANT` IMPORT-2:** Same-run computation can read imported assets only
    through their creating-run provenance. Original input selection stays frozen.
    Saved imports survive subsequent failure and require review to enter a project.
  - **Enforcement:** Immutable asset/source records, derived `Run.imports`, model-loop
    and transactional input allowlists, existing asset proposals.
  - **Source:** [Import contract](contracts/url-import.md), transport/policy, delayed
    publication, ownership, migration and browser provenance tests.

- **`@MODULE`:** `crates/server/src/{resources,model,run,images}.rs`, `crates/server/src/store{.rs,/model_calls.rs,/images.rs}`
  - **`!INVARIANT` INFERENCE-1:** Local dispatch ownership precedes HTTP I/O and
    survives parent cancellation, subscriber failure and process restart.
    Conflicting local model/image dispatch cannot cross active or unresolved ownership.
  - **Enforcement:** Shared stage slots, durable intent, unique active ownership,
    frozen groups/targets, owned bounded HTTP task, terminal-evidence settlement,
    connector-stage proof of no HTTP transmission with protocol retries disabled,
      bounded complete native validation-rejection evidence,
    reciprocal image/model admission guards, opt-in native generated-stream
    eligibility plus running/deferred idle confirmation
    and exclusive offline operator resolution. Parent cancellation does not retire ownership.
    Public per-run activity is a read-only projection of these records and scoped
    live guards. Displayed settlement cannot release a resource or imply empty VRAM.
  - **Source:** [Local inference ownership](contracts/runtime.md#local-inference-ownership);
    streamed stop, timeout, invalid output, publication failure, migration and restart tests;
    native busy/deferred, pre-generation, invalid metrics and exact idle-resolution tests;
    refused-connection recovery, broken-connection-after-send and streamed-error
    drain with/without terminal-marker regressions.
    Local-model activity also covers pre-token Stop, queue-wait disposal,
    cleanup polling, blocked ownership, restart and older-server browser checks.

- **`@MODULE`:** `crates/server/src/{main,telemetry,resources,model,images}.rs`, `crates/server/src/store/telemetry.rs`
  - **`!INVARIANT` OBS-1:** Optional diagnostics do not authorize effects, settle
    ownership or require a reachable telemetry backend for generation.
    Private request data and user/run/job metric dimensions remain excluded.
  - **Enforcement:** Explicit trace target, bounded nonblocking SDK export,
    local receiver validation, fixed resource/metric attributes, one read-only
    sampler query in flight and separate blocking-thread shutdown.
  - **Source:** [Observability contract](contracts/observability.md);
    decoded payload/privacy/parentage, stalled collector, failed-dispatch and
    durable ownership metrics tests; real-run evidence and dashboard query checks.

- **`@MODULE`:** `deploy/observability/{compose,otelcol-config,runtime-logs}.yaml`, `deploy/observability/deep-overview.json`
  - **`!INVARIANT` OBS-2:** Raw runtime logs remain an explicit operator-selected
    feed, independent of Deep's redacted telemetry and application authorization.
  - **Enforcement:** Optional `runtime-logs` profile, read-only selected source
    mounts, bounded agent pipeline and loopback-only published receiver/UI ports.
    Runtime logs have their own service namespace and dashboard filter.
  - **Source:** [Raw-log contract](contracts/observability.md#operator-owned-raw-runtime-logs);
    pinned-collector validation, real file ingestion, offset restart and browser checks.

## Rationale

- When the listener binds only to loopback, retain the request-authority policy.
  DNS rebinding can send an attacker's hostname to a loopback socket. Checking
  only that Origin matches Host would accept that attacker's origin.
- When a request is cancelled, preserve the conditional success update. Removing
  it lets a late model response append output after cancellation was acknowledged.
- When several app versions exist, preserve insert-only revisions. Replacing the
  latest HTML destroys the history the user needs to inspect or revisit.
- When another proposal advances the head, reject stale application instead of
  merging implicitly. Otherwise reviewed changes would apply to different files
  than the user inspected. The artifact contract defines regeneration and retries.
- When generated scripts need access to application data, design a narrow
  permission interface. Adding `allow-same-origin` defeats the current DOM/storage
  boundary. The preview does not confer host-tool authority.
- When model hardware changes, preserve the HTTP runtime seam. Model installation,
  GPU allocation and runtime-specific launch flags belong with the model host.
- When execution deadlines are changed, retain the independent PID 1 watchdog.
  A host-only timer disappears with the server; running untrusted code at the
  supervisor's uid would let it stop that timer. The execution contract separates
  those identities and requires kernel namespace teardown.
- When execution cleanup moves, preserve the worker mutex across durable
  settlement and live-task removal. Otherwise a recovery flag from an older job
  can sweep a newly admitted job whose task is still able to start a container.
- When output collection changes, retain task-level quiescence and no-follow
  capture inside the child namespace. Direct-child exit or a zombie group leader
  does not prove other threads stopped writing. The execution contract requires
  capture failure when their termination cannot be established.
- When search orchestration changes, validate the entire native-call batch before
  saving intents or making requests. Per-call validation can send the first query
  before discovering that the same batch asks for an unauthorized second tool.
  See [native calls](contracts/search.md#native-calls-and-budgets).
- When model cancellation drops a worker-admission caller, retain the owned
  admission task and parent-state check in the SQL transaction. Dropping between
  a committed job and task registration can otherwise strand work without a live
  owner. The [model execution contract](contracts/execution.md#model-driven-execution)
  requires both durable binding and registered-task cancellation.
- When a search run ends, close all pending intents in its terminal transaction.
  A timeout can leave an unstarted second search pending unless termination owns
  that cleanup. Preserving completed receipts retains the evidence already sent
  back to the model without replaying uncertain external work.
  See [search recovery](contracts/search.md#durable-activity-and-recovery).
