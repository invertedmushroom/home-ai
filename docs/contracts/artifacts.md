# Project artifacts

Status: implemented for one trusted local user. SQLite owns all accepted files
and proposals. Paths are virtual project names, never host filesystem authority.

## Snapshots and proposals

### Native workspace files

`workspace_tools:true` on a real-backend `chat` request grants direct workspace
file tools. Admission pins the current accepted revision. `list_files` returns
the candidate project inventory and pages of 64 saved text/image asset records.
`read_file` reads the candidate text or an owned `asset:<UUID>` reference in
8192-byte UTF-8 chunks. `search_files` searches current project text for a
case-sensitive literal query of 1–256 UTF-8 bytes. Results follow path and byte-offset
order. Each page contains at most 32 matches and 8192 serialized bytes in its
matches array. Each match includes a byte offset, a one-based line number and a
UTF-8 excerpt of at most 256 bytes. Search pagination counts matches. File offsets
count UTF-8 bytes. Search includes staged changes and excludes the asset library.
An empty result has no next offset.

`write_file` stages a text-file create/replace. `edit_file` replaces one exact,
unique, nonempty `old_text` with `new_text` in an existing project text file.
Missing or ambiguous matches fail whole-batch validation before external effects.
`delete_file` removes an existing project text file from the candidate. It does
not remove image bindings or saved workspace assets. The final diff records
deletions, including deletion of the last project file.
`save_asset` stages a text asset or pins a saved image at a portable project path.
These tools use virtual paths and grant no host filesystem access.

The run owns its candidate in memory. Reads observe earlier staged writes.
Whole-batch validation precedes external effects. Successful temporary computation
outputs are staged at their output names when workspace tools are enabled.
The final successful reply, applied proposal receipt and one immutable revision
commit in the same transaction. A failed, cancelled or interrupted run publishes
no candidate files or bindings. Already saved input/output assets retain their
existing lifecycle. Unrelated files and bindings survive unchanged.

The candidate uses the existing path, collision, count, ownership and byte limits.
No-op writes are idempotent. A run with no net changes creates no revision.
An edit followed by restoration or a file created and deleted within the same run
creates no revision. Earlier revisions retain the exact contents of deleted files.
Native requests supply a file inventory rather than all file bodies to the model.
Exact text remains available through `read_file`. Native tools permit 16 file calls
and at most 16 model turns, with the last turn restricted to a final reply.
The workspace instructions state the existing two-call limit per model response.
The existing byte limit still bounds each model request.
Later context preserves the native assistant's prose alongside its saved-change
summary. Legacy settled file-format replies still collapse to proposal summaries.

Historical proposals and the legacy `app`/`files` generation formats retain their
review/apply behavior. Manual edits and explicit asset/output proposals retain
their existing settlement rules. New real-backend browser requests enable
workspace files by default. Demo requests continue to use scripted proposals.

A revision is an immutable full snapshot: `id`, nullable `run_id`, nullable
`execution_id`, nullable `edit_id`, nullable `asset_ids`, `number`, `files`, and
the legacy `html` projection and `asset_bindings`. `files` is an array of `{path,content}` text files,
sorted by ASCII path for new snapshots. `html` equals `index.html` or an empty
string when that file is absent. Revision numbering starts at one per workspace.
The highest number is the accepted head; selecting older revisions only changes
the browser view. There is no branch, merge, rename or host checkout operation.
Exactly one provenance source is non-null: a model run (`run_id`), a completed
saved-revision execution (`execution_id`), a manual edit submission (`edit_id`),
or selected workspace assets (`asset_ids`). Existing
model revisions/proposals retain their run IDs. Execution output, manual edits
and workspace asset additions create no synthetic model run, user turn or
assistant reply.

## Workspace assets and asset proposals

An asset (`assets` table) is an immutable UTF-8 text file owned by one workspace:
`id`, `workspace_id`, portable `name`, `media_type`, `bytes` (at most 1 MiB),
`origin` (`"upload"`, `"execution"` or `"import"`), nullable `execution_id` (required iff
`origin == "execution"`), `created_at_ms`, nullable `source` (required iff
`origin == "import"`), and stored UTF-8 `content`. The [import contract](url-import.md)
owns source provenance and URL retrieval. Selecting, uploading, importing or
computing an asset alone does not change the accepted project snapshot. Native
workspace runs can stage these assets for their automatic final revision.
Assets remain in workspace history until workspace deletion.

- `POST /api/workspaces/{id}/assets` accepts `{name,content,media_type?}`, validates
  the portable path and 1 MiB UTF-8 budget, and returns `201` with the created
  `upload` asset metadata.
- `GET /api/assets/{id}` returns `200` with asset metadata and exact UTF-8 `content`.
  Asset IDs are references scoped to their workspace, never permission to enumerate
  another workspace.
- `POST /api/workspaces/{id}/asset-proposals` accepts
  `{base_revision:{id:UUID|null},asset_ids:UUID[]}` naming 1–64 distinct assets
  owned by that workspace. The base must match the current accepted head (`409`
  when stale). It creates a pending write-only proposal against that base with
  `asset_ids` provenance (`400` if no files differ from base or if the merged
  candidate exceeds the 64-file / 1 MiB project budget; `404` if any asset ID is
  missing or belongs to another workspace). Later head changes require a fresh
  review against the new head, while existing saved-revision output proposals
  (`POST /api/executions/{id}/proposal`) keep their input revision base.

Legacy `app`/`files` generation creates pending proposals. `app` remains an API input format for whole-document models:
a successful complete HTML fence becomes one write to `index.html`. It automatically
pins the current accepted snapshot when submitted. `files` accepts write/delete
JSON. Both formats use the same validation, publication and settlement path.
For `files`, the caller must send `base_revision:{id:<head UUID>}`,
or `base_revision:{id:null}` for an empty project. Omitting the object or its `id`
is invalid. Chat/app submissions omit this object. Existing app revisions remain
readable without a migration. Repeating an unchanged HTML write fails the same
no-op validation as any other file write.

A file run receives the pinned accepted snapshot and a separate model-context
projection, including content originating from other backends. Admission freezes
that projection in the run's transaction. It includes completed turns; earlier
settled assistant file proposals become compact summaries with identities, paths,
operations and current outcomes instead of their full JSON source. The full
transcript and proposals remain stored and inspectable. Pending model proposal
contents stay in context so follow-up requests can revise them. Project state
identifies the accepted revision and pending/applied/rejected proposals, including
execution output/manual proposals and rejection reasons. These proposals
contribute metadata; their contents enter the accepted snapshot only after explicit
application. App and Conversation modes receive the same decision context;
Conversation mode receives no accepted-file snapshot. Earlier conversation turns
can still contain code. Later
decisions cannot rewrite an already accepted request, including queued work.
Context, current prompt and serialized accepted snapshot together must fit the
runtime's 128 KiB context budget. Exceeding it fails explicitly.

The model returns one JSON object:

```json
{
  "changes": [
    { "op": "write", "path": "index.html", "content": "<h1>Hello</h1>" },
    { "op": "delete", "path": "old.txt" }
  ]
}
```

Plain JSON and complete lowercase `json`/`files` fences with LF/CRLF are accepted.
The envelope and operations reject unknown fields. A write contains the full new
text and must change an existing file or create a new one. A delete must name an
existing file. Omitted files retain their exact text, including line endings.
No partial JSON repair, automatic retry or backend fallback is performed.
Normal provider completion and valid syntax/schema do not establish code quality.

Each proposal records `id`, `workspace_id`, nullable `run_id`/`execution_id`/`edit_id`, `base_revision_id`, `changes`,
`status` (`pending`, `applied`, `rejected`), nullable `applied_revision_id`, and
nullable `rejection_reason`. A reason belongs only to a rejected proposal.
The run's success, assistant message and validated proposal commit atomically.
Cancellation, parsing/validation failure and interrupted drafts create no proposal
or revision. Pending/rejected proposals remain available for inspection after
reload or restart.

An explicit [execution output review](execution.md#reviewed-output-files) creates
one write-only proposal per successful execution against that job's input revision.
Repeating review returns the same proposal, including after apply/reject. Only
changed captured files are included; omissions preserve input files. Proposal
creation validates the merged project as well as standalone capture bounds.

## Manual edits

`POST /api/workspaces/{id}/proposals` accepts exactly
`{id:UUID,base_revision:{id:UUID|null},changes:[...],asset_changes?:[...]}`.
The [asset contract](assets.md#reviewed-project-bindings) owns binding operations.
The caller's `id` becomes
the proposal's `edit_id`; the server issues its separate proposal ID. Matching
retries return the same pending/applied/rejected receipt, even after the head
advances. Reusing that submission ID with another workspace, base or ordered
text or asset change list returns `409`. A new submission must name the current head; stale
bases return `409`. Missing workspaces return `404`; invalid input returns `400`.
Creation validates the complete candidate snapshot and inserts only a pending
proposal. It creates no model run/message or accepted revision.

**Edit** starts a draft from the current accepted snapshot or an empty project.
**Edit copy** starts one from a nonstale pending proposal, creating a separate
manual proposal; its original content and source remain immutable. Drafts support
text edits, portable file creation/deletion/restoration and Source/Diff views against the
captured accepted base. **Restore file** reinstates a deleted base file's exact
original content, subject to the same path/collision/project limits as Add file.
**Save for review** persists the changes for ordinary
review/apply/reject. Cloned drafts retain their candidate image bindings alongside
text. Saving does not accept or execute them.

Ctrl/Cmd+S in the active draft invokes the same save command and admission checks.
A stale draft offers Export draft and Discard; a captured uncertain save still
offers Retry save with the same UUID/input. File selection survives draft-to-proposal
save/recovery when the selected path remains inspectable. Discard/history navigation
never accepts draft files or discards a different retained draft.

Unsubmitted drafts live only in tab memory. They survive file/view/workspace
switches and can be downloaded; the browser warns before leaving a dirty tab.
Closing/reloading after accepting that warning loses unsaved text. Stale drafts
remain inspectable/exportable but cannot be saved against a different head.
There is no automatic rebase. A submission captures its UUID/input before HTTP;
an uncertain response locks editing and retries that exact request. Returning to
a workspace can recover the durable proposal from its matching receipt. Aborting
HTTP does not undo a commit. Draft text enters neither model context nor Node
input; apply its saved proposal first.

## Review and settlement

- Generation checks the caller's base against the current head before creating
  the run and checks it again when publishing the proposal.
- `POST /api/proposals/{id}/apply` requires a pending proposal, its base still
  matching the head, and no queued/running run in that workspace. It atomically
  inserts the resulting full snapshot and marks the proposal applied. It returns
  the revision with `200`. Retrying an already applied proposal returns that same
  revision, even if the workspace has since advanced; it never inserts twice.
- `DELETE /api/proposals/{id}` marks a pending proposal rejected and returns it
  with `200`. Repeating rejection is idempotent. Rejecting an applied proposal or
  applying a rejected proposal returns `409`. Missing proposal IDs return `404`.
- Stale application and application during active work return `409`. There is no
  force apply or automatic rebase. Refresh and generate against the current head.

`POST /api/proposals/{id}/reject` accepts `{reason?:string|null}` with no unknown
fields. Reasons are trimmed; absent/blank reasons become null, and nonempty
reasons are limited to 4096 UTF-8 bytes. The reason and rejection commit atomically
without changing accepted files or creating model work. Repeating the same
decision returns the original receipt; attempting to replace an existing reason
returns `409`. The legacy DELETE remains an idempotent reason-free rejection and
never erases a saved reason.

The browser validates proposal/base/applied-snapshot relationships before
publication. Shared workspace state retains accepted settlement receipts across failed detail reads;
switching workspaces aborts obsolete actions and prevents late view publication.
Aborting a request does not undo a server commit; a later read recovers it.
If an output-proposal receipt arrives before its base revision has loaded, the
browser retains it but withholds diff/preview/application until that base is read.

Chat shows proposal outcomes and reasons alongside the original model response;
execution output and manual proposals have separate project cards rather than invented
assistant turns. **Next request context** explains the projected history,
accepted revision, file inclusion and decisions. It describes the next request;
the server resolves authoritative state when admitting it. Rejection does not
rewind the conversation, generate a reply or remove the original proposal.
Execution cards and explicit [Fix from result](execution.md#conversation-results-and-repair-requests)
attachments preserve that separation: the result is an observation, and a repair
uses the same reviewed proposal/current-head application rules. No result applies
files, rewinds messages or starts another execution implicitly.

## Project workspace

The composer defaults to **Files**; unchecking it requests conversation without file changes. The counter
starter requests actual HTML/CSS/JavaScript files. It does not introduce a build
script. One App-owned selection identifies Current, a historical revision, a saved
proposal or the retained unsaved draft. Source/Diff is a separate presentation
choice used consistently for each resource. App owns per-workspace editor drafts.

Review navigation shows the complete candidate project and marks changed files.
One Editor file list remains available in source and diff. Deleted files remain
selectable for inspection. Preview uses the selected HTML file, otherwise
`index.html`, otherwise the first HTML entry. The source selection does not have
to be an HTML file. New proposals open review in Editor when
the user is viewing source/diff. They leave the separate Preview capture and an
active editor draft in place. **View current** returns to the accepted head without
discarding a draft. The Editor's version button opens the floating **Revisions**
tool: Current contains the accepted head, Working contains drafts and pending
proposals, History contains older revisions and settled proposals. Choosing a
resource reveals Editor. It changes the view, not accepted files or model input.
The **Export** menu groups individual-file, JSON manifest and portable bundle
downloads for the viewed resource. Proposal review and Apply/Reject stay beside
the editor surface; saved metadata and rejection feedback remain inspectable.

JavaScript execution and its selected result share the floating Runs tool.
Execution tools start collapsed and open explicitly. Optional rejection feedback
also starts collapsed. The execution panel remains mounted while collapsed,
so its polling and cleanup owner does not depend on presentation. Conversation
shows only execution observations explicitly included in model requests.

## Bounds and paths

A proposal contains 1–64 distinct text/asset operations. A snapshot contains at most 64
source files and image bindings combined. Text has a 1 MiB UTF-8 path/content
budget. Bound images have a separate budget in the [asset contract](assets.md).
Empty snapshots are
valid after deletion. Paths are case-sensitive but cannot collide when folded to
ASCII lowercase. File/directory prefix collisions are invalid.

Paths contain 1–240 ASCII characters from letters, digits, `.`, `_`, `-` and `/`.
They must be relative, with no empty, `.` or `..` components, trailing component
dots, backslashes, drive prefixes or Windows reserved device names. These portable
names do not grant host access. The application stores text and pinned immutable
images. It downloads individual files, a JSON project manifest or a portable ZIP
bundle. It installs no packages.

## Source, diff and preview

Source/download preserves the full accepted or proposed text. Monaco source and
diff editors, language workers and fonts are lazy-loaded from local application
assets. Rich documentation rendering strips resource/link attributes; project
comments cannot load embedded hover media. JSON schema downloads are disabled.
Per-surface models, listeners and observers are disposed on view/file changes;
draft text is owned above those surfaces. Undo history belongs to the open surface.
Pane expansion/hiding retains these resources. The owning ResizeObserver supplies
visible host dimensions explicitly, including on split restoration; it skips
zero-size observations while hidden so diff layout cannot retain a collapsed height.
The diff wrapper takes its height from the pane, avoiding feedback from its sized editor child.
If Monaco cannot load, a plain-text editor retains access to the draft/source.
Line diffs compare
a proposal against its exact base, or a revision against its previous revision.
Display computation is bounded to 80,000 combined characters, 2,000 combined
lines, 1,000 edits and 50 ms. An exceeded diff budget shows an explicit message;
it does not truncate the stored files or prevent Source/download.

Preview runs only after an explicit click, in an iframe with `allow-scripts`
alone. A captured preview does not change when new files arrive or a proposal is
settled. Pane expansion preserves the same frame. Stop preview disposes it;
workspace changes and reload discard the capture. See the
[workspace UI contract](workspace-ui.md) for view lifetimes. For multi-file
HTML projects, a bootstrap inside that opaque frame resolves relative classic
script and stylesheet links from the snapshot and inlines their exact text with
HTML closing-tag escaping. The parent never parses generated HTML. A resource
policy precedes both parsing and the final document; external scripts/styles,
ordinary resource loads, fetches and forms remain blocked. No frame messages or
tool calls are accepted by the application.

The Preview footer names its captured revision/proposal. When the current accepted
head differs, it shows both versions and **Update preview**, which explicitly
captures the current accepted head, independent of Editor history/draft selection.
Applying/selecting a revision never reruns the frame. Update preserves the old
capture until the replacement succeeds; a current revision with no HTML entry
cannot be captured. Captured proposal status may identify its applied revision
without replacing the document or restarting its code.

Static local image references use the captured-resource rules in the
[asset contract](assets.md#captured-preview).
Modules, import maps, packages, CSS imports and native execution are
not supported by this preview. Inlining classic scripts does not reproduce
defer/async scheduling or a complete web server. Missing local scripts/styles and
modules show an error. The iframe is not a CPU/memory quota or a complete network
sandbox. Exported content opened elsewhere does not retain Deep's isolation.
