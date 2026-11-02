# Deferred Capabilities

This register records material capabilities and boundaries that are intentionally outside the current checkpoint.

| Capability | Target Phase or Trigger | Current Boundary |
| :--- | :--- | :--- |
| Native LM Studio prediction cancellation | A suitable patched official SDK and verified terminal acknowledgement/tool-history compatibility | The [SDK investigation](operations/gpu-handoff.md#lm-studio-lifecycle-and-native-cancellation) found a vulnerable schema-validation dependency. No SDK or private WebSocket replacement was retained. Existing HTTP draining and ownership safeguards remain unchanged. |
| Video tool, durable assets and chat playback | After the measured one-video MCP workflow | The standalone [video probe](operations/video-probe.md) validates text-to-video, MP4 transfer/playback and scoped cancellation. Deep's application profiles, jobs and project bindings still accept images only. |
| Image-conditioned video/3D jobs and multiple reference images | After one measured reference-image workflow | Image editing accepts one immutable workspace reference. Video/3D outputs, multiple references and their profiles/storage remain outside the image slice. |
| General application build/test environment | Declared runtime targets and dependency policy | The existing offline Node worker executes selected saved scripts or temporary computation. It has no DOM, package installation or binary project mounts. |
| Source search and structural retrieval | Projects outgrow direct bounded reads | Native workspace requests have a file inventory and 8192-byte read chunks. Content search, repository maps and structural indexes remain outside this slice. |
| Full application preview server | Modules/packages or dynamic resource loading | Captured previews resolve classic scripts/styles and static image references. Dynamic relative image URLs created by JavaScript, CSS imports and module builds remain unsupported. |

When delivery or deferral changes a recorded scope boundary, update this register. Use the [ADR creation guidance](adr/README.md) when an architectural decision merits a record.
