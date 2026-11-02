# Workspace presentation

Chat, Editor and Preview are three views of the selected workspace. Each has a
top-right expand/restore button and no title bar. Expanding fills the workspace
stage; restoring returns the visible panes. Preview also has a Hide control;
Editor takes its space. Show Preview in the common toolbar restores it. Narrow
screens stack the panes in a scrollable stage. Layout is tab memory and resets
on reload.

Hiding an expanded Preview exits expansion and shows Chat alongside Editor.
Monaco source/diff surfaces lay out against explicit visible host dimensions;
hidden zero-size observations do not overwrite their layout. Restoring the split
reflows the existing editor and models without a refresh or remount.

The dividers between adjacent panes resize them by pointer drag or arrow keys.
Minimum pane sizes keep controls reachable. The lower divider resizes rows when
Preview sits below Chat/Editor; phone layouts use automatic stacked sizes and
hide the dividers. Expansion and Preview hide/show retain the split proportions.

## Authority and modification lifecycle

SQLite owns saved conversations, revisions, proposals, assets and jobs. App owns
the selected workspace, validated observations and receipts, selected
version (current, revision, proposal or draft), unsent request choices and per-workspace editor drafts.
Artifacts resolves that selection against the same workspace detail for source,
diff, preview capture and execution inputs. Views do not copy accepted files.

All changes use existing validated workspace operations. Native workspace-tool
runs publish their candidate atomically on success. Manual edits and explicit
asset/output imports create proposals settled by Apply/Reject with exact-base
checks. Presentation does not change these policies or introduce a second
command or event framework. Saved proposal IDs are sufficient for navigation;
there is no editor-selection synchronization or reference-resolution subsystem.

## View and resource lifetimes

`WorkspaceLayout.tsx` owns only primary-pane expansion, split proportions, Preview
visibility and eight bounded auxiliary windows: Workspaces, Models, Files & assets, Runs,
Tools & attachments, Revisions, Run details and Image. Its public
navigation operation opens a named view, without accessing that view's internals.
No geometry, view state or new preference is persisted.

Primary panes keep stable portal hosts and remain mounted when hidden. Auxiliary
windows also keep their bounded instances mounted when closed. Expanding,
restoring, resizing, hiding Preview or closing a tool window cannot discard a
draft, reset an iframe, abort an upload, cancel a job or transfer its ownership. Workspace changes retain
existing owner cleanup and draft behavior. Reload preserves established saved
data; unsent requests, layout and preview captures remain transient.

App continues run polling while Chat is hidden. While a run is active, the
composer offers Stop request in Send's place, and when Chat is hidden by an
expanded pane the common toolbar shows Stop request instead, so cancellation
stays reachable without restoring the pane. Auxiliary execution and image
operations retain their existing polling, cancellation and receipt ownership.

Chat shows [local-model activity](runtime.md#public-local-model-activity)
separately from the parent run status. A cancelled run may still show cleanup
pending while its original request drains or native recovery confirms idleness.
Ordinary queue waiting and unconfirmed blocked ownership have different wording.
The UI does not infer loading/prefill progress, GPU idleness or released VRAM.
Settlement describes only that request.

App keeps polling live model cleanup after a parent ends. Settlement stops that
polling unless another run/image job needs it. Blocked ownership remains visible
without a polling loop. Turn deletion/branching stays unavailable for unsettled
model ownership. These observations do not disable unrelated model choices or
grant a browser action for resolving ownership. Older APIs without activity remain
readable without inventing cleanup evidence.

Preview executes only after **Run preview** captures the selected saved revision
or proposal. The immutable capture, opaque-origin sandbox and resource policy
remain unchanged. Expand/restore and hide/show preserve the same iframe.
**Stop preview** disposes it. Running again captures a fresh input. Reload and workspace changes
discard captures and never restart generated code automatically.

The Preview footer compares its capture with the current accepted head. Update
preview is an explicit new capture of that head even while Editor shows history
or a draft; selecting/applying files only updates the status text.

## Auxiliary tools and navigation

The common toolbar opens auxiliary windows in either layout. Their title handles
support pointer movement and arrow keys; Shift plus arrows resizes. Resize handles
also support pointer and keyboard input. Viewport changes clamp bounds. Pointer
cancellation, capture loss, blur, document hiding and page hiding release gestures.
Listeners and observers have effect cleanup. Close restores focus to a visible
opener when available.

Chat has one pane scroll area. Replies, disclosed model responses and execution
logs flow within it without independent height limits or scrollbars. The composer
stays at the bottom while reading ordinary replies; expanding its attachment
details lets it flow with the conversation so long logs remain reachable. The
editable message input keeps its normal text-input scrolling. Floating tools and
the Editor retain their own appropriate scrolling.

Models opens beside New workspace and contains chat selection, task preference,
runtime discovery, image presets and WanGP discovery. Choosing an image preset
does not grant image permission; disabling Images retains the preset. Request
permissions remain visible in the Chat composer. Tools & attachments contains
separate chat vision, image-generation reference and computation input sections.
Closing either window preserves its choices.

Chat leads with the reply and collapses run statistics and execution logs. The
expanded summary can open Run details, showing saved messages and the validated
run receipt, including available tool records, settings, timings and token counts.
It does not invent a tool-call trace or expose frozen request internals absent
from the run API. One selected run ID resolves against current workspace detail.

Successful image results show an image with details and expand controls.
Selecting the image itself expands it; the info control opens job details.
Image opens the selected job's download, reference provenance, prompt, actual
adapters and job identity. The expanded view shows only the image across the
stage, and the Image window carries only a close control in both views;
reopening job details restores the prior bounds while retaining the same image
node and underlying panes. One selected run ID owns
this transient inspection; the original card retains reconciliation ownership.
Live progress, errors and unresolved-outcome recovery remain visible in Chat.

Manual edits, asset/output imports and proposals whose conversation turn was
removed remain accessible in Revisions, without synthetic PROJECT chat entries.
Next request context explains that proposal paths, status and outcomes are sent
as workspace metadata, separately from completed conversation and file contents.
This presentation does not change the server's admission-time context capture.

Review/View changes selects the saved proposal ID in App and reveals Editor.
Selecting a saved script in Runs reveals Editor; Fix/Explain reveals Chat.
Resource navigation hides auxiliary windows without unmounting their operations.
Runs provides existing saved-script operations, not a general shell. Files &
assets groups library images and text files, with an accepted-project count.
The project file tree lives in Editor.

The Editor's version button opens Revisions, grouping Current, Working and
History resources. Source/Diff remains separate from that shared selection.
Save for review and Apply changes name distinct stages of manual publication;
native model-file publication retains its existing automatic semantics. Export
groups the existing download commands without adding stored export state.

## Verification

`tests/browser/workspace-layout.spec.ts` covers stable pane/iframe/editor identity,
expand/restore, divider gestures and keyboard resizing, Preview hide/show,
single-area Chat scrolling, prompt/draft retention, explicit preview lifetime,
floating-window interactions, small-screen bounds, proposal navigation and polling/Stop with Chat
hidden. Existing editor, execution, attachment and receipt-recovery checks cover
the shared operations. Use isolated test ports and fresh synthetic data:

```powershell
rtk npm run build -w @deep/web
rtk proxy npx playwright test --config playwright.workspace.config.ts
```

The browser loads the built SPA served by the Rust server (`apps/web/dist` is
its fallback `web_dir`), not the Vite dev server; stale builds fail in
misleading ways. Only the Vite proxy check loads Vite directly.

This configuration separates browser output and synthetic server data under
`logs/`. Keep retained evaluation records outside Playwright output directories,
which the runner clears.
