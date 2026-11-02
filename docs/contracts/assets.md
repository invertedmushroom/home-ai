# Workspace assets and project bindings

Status: implemented for one trusted local user. A workspace owns immutable assets.
A project revision selects assets through pinned relative-path bindings. A job
produces or consumes data under its own permissions. Library availability is
neither project application nor execution authority.

## Library and immutable images

The **Files & assets** window groups uploaded/generated images and the existing UTF-8
upload/computation assets. Generation output appears automatically without a
download/re-upload step. Uploading or generating changes no project revision.
Replacing or removing a binding does not delete the underlying asset.
Individual asset deletion/garbage collection is not implemented. Workspace
deletion removes its assets and references through the existing transaction.

PNG/JPEG images share one `image_assets` lane regardless of origin:

- `id`, `workspace_id`, immutable display `name`, `media_type`, byte length,
  actual width/height, SHA-256, creation time and stored BLOB.
- `origin:"generation"` has an `image_job_id`. `origin:"upload"` has no image job.
- Image names contain 1–160 UTF-8 bytes without controls or slashes. Names are
  display metadata, not host paths or model instructions.
- The [image decoder and byte limits](images.md#binary-output) apply equally to
  uploads and generated output.

`POST /api/workspaces/{id}/image-assets?id=<UUID>&name=<name>` accepts raw image
bytes with the matching `image/png` or `image/jpeg` Content-Type.
It returns `201` with immutable metadata. The caller captures its upload UUID
before sending. Matching workspace/name/media/hash retries return the same asset.
UUID reuse for another input or a generated asset returns `409`.
Invalid content returns `400`. Oversized HTTP bodies return `413`.
The UI preserves the File/UUID through uncertain responses and offers explicit
retry. Uploading creates no synthetic model run or image-generation job.

Image detail/content endpoints remain defined by the
[image contract](images.md#binary-output). Binary data is excluded from workspace
and run JSON. Image upload alone sends nothing to a model or image provider.
Explicit [chat attachments](vision.md) and [editing references](images.md#reference-image-input)
grant separate transfers.

The UTF-8 lane retains its existing `assets` identities, upload API, computation
input selection and write-only asset-proposal operation described in
[project artifacts](artifacts.md#workspace-assets-and-asset-proposals).
The library presents both lanes without converting binary content into text.
`GET /api/status` advertises `asset_library_available:true`. Older APIs default to
unsupported in the browser. The library remains readable and explains that a
Rust-server restart is required before upload/binding/export actions are enabled.

## Reviewed project bindings

A revision adds `asset_bindings:[{path,asset_id}]`. Each binding pins one immutable
image in that workspace at a portable project path. PNG paths end in `.png`.
JPEG paths end in `.jpg` or `.jpeg`. Bindings share the source file namespace:
case-fold collisions and file/directory prefix collisions are invalid.

Projects have at most 64 total source files and image bindings.
Text retains the 1 MiB UTF-8 path/content budget. Bound image bytes have a separate
32 MiB aggregate budget, counting each bound path. Each image remains at most
8 MiB. Asset IDs from another workspace cannot be bound or exported.

The existing manual-proposal API accepts optional `asset_changes`:

```json
{
  "id": "submission-UUID",
  "base_revision": { "id": "accepted-revision-UUID" },
  "changes": [],
  "asset_changes": [
    { "op": "bind", "path": "assets/hero.jpg", "asset_id": "image-UUID" }
  ]
}
```

`bind` adds a path or replaces its asset ID. Binding the same ID again is a no-op
error. `unbind` requires an existing path and removes only that binding.
Source write/delete operations cannot target a binary binding. Source and asset
operations in one proposal must have distinct paths. At least one operation is
required, with at most 64 combined operations.

**Use in project** and **Remove binding** create ordinary manual proposals with
`edit_id` provenance. They never apply automatically or create chat/model work.
Retry identity includes workspace, accepted base, ordered text operations and
ordered asset operations. A changed input under the same submission ID returns
`409`. Existing receipts are resolved before current-head admission checks.

All proposal origins validate the combined candidate before publication and again
at apply. The existing stale-base, active-run and idempotent settlement guarantees
apply. Text generation, execution-output review and text asset import preserve
omitted bindings. New revisions copy the complete binding snapshot.
Historical revisions continue to reference their original image bytes.

Editor drafts retain bindings independently of editable text. Cloning a pending
proposal and saving a new manual proposal preserves its candidate bindings.
Draft images never enter the text editor or become accepted without review/apply.

## Model context and execution scope

File/app requests receive approved binding paths and bounded image metadata from
their pinned accepted revision. The metadata includes asset ID, media type,
dimensions, byte length and SHA-256. It counts toward the 128 KiB context budget.
The source snapshot, pending proposal history and image observations contain no
automatically encoded image payload. A later source edit preserves bindings.

Generated file-change JSON still contains only text write/delete operations.
The model cannot insert binding operations into that schema. The text model has
not inspected the image pixels merely because a binding exists.

With native workspace tools enabled, `save_asset({path,asset_id})` can stage an
owned text asset or image binding for the run's automatic final revision.
The same ownership, extension, collision and combined size checks apply.
Image pixels remain outside model context and Node inputs.

Node jobs still receive only their saved text snapshot or explicitly selected
UTF-8 computation inputs. Binary bindings are not mounted into the worker.
Binding an asset grants no Node, host-file or additional model-tool permission.

Single-reference image editing follows the [image contract](images.md#reference-image-input).
Image-to-video/3D inputs and explicit vision attachments remain follow-up work.
Their input references must use immutable asset IDs rather
than mutable names, provider-local paths or a dynamically reevaluated “last image”.
Bounded source read/search tools are also separate from the current full-text
context projection.

## Captured preview

Explicit **Run preview** captures the selected revision/proposal's text, entry and
bindings. The parent fetches only those bound images and verifies exact byte length,
media type and SHA-256. It encodes them into the transient render payload.
These render-only data URLs are never stored as source or sent in source-edit
context. The child decodes each distinct captured image into a child-owned blob
URL, so repeated references do not duplicate large data URLs in markup.
The iframe document owns those URLs through its lifetime. Parent-origin blob URLs
are not used across the opaque iframe origin.

Only the child parses generated HTML. The existing CSP precedes parsing and the
final document. `sandbox="allow-scripts"` retains an opaque origin.
No arbitrary workbench API access or resource-host allowlist is added.

Preview resolves literal relative image references in HTML `src`/`srcset`,
picture sources, video posters, SVG image links, icons and CSS `url()`.
Linked CSS uses its own path as the relative base. Literal CSS strings/comments
are preserved. SVG source files can render as images without running SVG scripts.
Missing local image references fail visibly rather than fetching another file.

CSS imports, modules/packages and JavaScript that constructs new relative resource
URLs at runtime remain unsupported. The preview is not a full application server.
Binding and explicitly previewing authorizes generated app code to consume those
pixels. The existing sandbox is not a complete network-isolation guarantee.

An active captured preview does not retarget when newer revisions arrive.
Another explicit run replaces it. Workspace changes abort capture and prevent
stale frame publication. Temporary download object URLs and timers belong to
their owning UI surface and are disposed on unmount.

## Portable export

**Download app bundle (.zip)** captures the viewed accepted/proposed snapshot or
editor draft. `POST /api/workspaces/{id}/project-export` accepts exactly
`{files,asset_bindings}`. It validates namespace, ownership and budgets, then
returns a ZIP containing ordinary source files and exact image bytes at bound
paths. It creates no proposal/revision and executes no project code.
It writes no host project files. There is no implicit current-head substitution.

The ZIP uses stored entries with portable validated names. Responses use
`application/zip`, attachment disposition, `no-store` and `nosniff`.
The browser bounds export response bytes to source/assets plus ZIP overhead.

The existing JSON project download remains a manifest. When images are bound it
includes their IDs, not binary payloads, and is labelled **Download project
manifest**. Use the app bundle for portable media. Opening exported code outside
Deep does not retain preview isolation.

Schema version 14 makes image-job provenance nullable for uploads, adds image
name/origin and adds revision bindings/proposal asset changes. Existing image IDs,
bytes, job links and project/history IDs survive. Old bindings/changes default
empty. All pending upgrades commit before restart recovery.
