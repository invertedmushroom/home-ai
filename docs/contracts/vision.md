# Chat image input

Status: implemented through native Chat Completions image content parts.

`POST /api/workspaces/{id}/runs` accepts `image_attachment_ids:[UUID,...]`.
The selected model profile must declare `vision:true`. Omitted vision flags
default to false. Direct legacy backend selections cannot accept chat images.
Auto filters its existing local preferences for vision and requested tool support.
Runtime inventory does not grant or change this permission.

Admission resolves up to four unique immutable PNG/JPEG assets from the same
workspace, with at most 16 MiB combined stored bytes. Uploaded and generated
assets use the same path. Unknown, foreign, duplicate, excessive or unsupported
attachments reject before a run/message or upstream request is created.
The receipt freezes `image_attachments` metadata and the selected route's
`vision` declaration. Existing image byte/decoder limits still apply.

Rust verifies saved metadata, byte length and SHA-256 before adding inline
`image_url` data URLs to the current user message. Text remains a text content
part. Each tool continuation in that run retains the same images on that
original message. Image bytes have a separate bounded budget and do not consume
the existing 128 KiB textual conversation limit. Pixels, data URLs and credentials
never enter run JSON, saved context or telemetry. The selected connection receives
the attached bytes, including hosted connections. Deep does not fetch arbitrary
image URLs or automatically attach library assets.

The composer labels this selection **Images for chat**. It preserves unsent
attachments on failed submissions and model changes. An incompatible model
blocks sending until the user changes models or removes the attachments.
Accepted submissions and explicit workspace changes clear the selection.
The transcript shows attached images and restores receipts after reload.

Later runs include metadata of images attached to successful visible turns,
without replaying pixels. The user reattaches an image for another visual
question. This keeps pixel transfer explicit when switching models or providers.
Branches copy image bytes and remap attachment IDs to the branch. Schema 23 adds
the metadata column with an empty default and preserves older data.

Chat attachments and [image generation reference input](images.md#reference-image-input)
are independent. The former transfers pixels to the chat model. The latter
transfers one reference to the chosen image provider. Selecting one never grants
the other. A generation result can be attached to a later message for inspection.

The wire format follows the [Chat Completions image input schema](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create).
The local runtime needs its matching vision projector loaded or otherwise
available. A profile declaration establishes intent, not verified runtime readiness.
