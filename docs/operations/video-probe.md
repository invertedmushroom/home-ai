# WanGP video runtime probe

[video-smoke.mjs](../../scripts/video-smoke.mjs) measures one text-to-video job
against an already running loopback WanGP MCP endpoint. Deep's application remains
image-only. The probe does not add a chat tool, profile environment variable,
database migration, project binding or video player to the workbench.
Image-to-video, multiple references and 3D remain deferred.

## Run

Start WanGP separately with **MCP with Web UI** enabled. Use an idle runtime and
stop submitting work from other clients during the measurement. The queue check
is a snapshot, not an atomic reservation. The standalone probe does not participate
in Deep's resource scheduler or inspect other GPU processes.
It never clears the queue, unloads another model or installs a checkpoint.
WanGP may still load models or download ancillary assets during generation.

Run from the repository root with Node 24+ and the existing npm dependencies.
Use new report and output filenames. Default mode performs CPU-only capability,
checkpoint, LoRA, canvas, duration and queue checks:

```powershell
rtk proxy node scripts\video-smoke.mjs --endpoint http://127.0.0.1:8000/mcp --settings deploy\videos\wan21-fusionx.probe.json --report video-preflight.jsonl
```

The [sample settings](../../deploy/videos/wan21-fusionx.probe.json) require Wan 2.1
Text2video 14B and the matching local FusionX LoRA. They request a deliberately tiny
256x256 clip with nine frames at 16 FPS, four steps and a fixed seed.
Checkpoint availability does not establish memory fit or visual quality.
For actual inference and browser playback:

```powershell
rtk proxy node scripts\video-smoke.mjs --endpoint http://127.0.0.1:8000/mcp --settings deploy\videos\wan21-fusionx.probe.json --generate --output video-smoke.mp4 --report video-generation.jsonl
```

Chromium must be available through Playwright or `DEEP_BROWSER_PATH`. The probe
plays the downloaded local MP4, checks dimensions and duration, and requires a
nonzero playback frame count. The browser's cumulative playback count is not an
exact encoded-frame count. No local HTTP server or application database is needed.

To exercise cancellation instead of publishing an output:

```powershell
rtk proxy node scripts\video-smoke.mjs --endpoint http://127.0.0.1:8000/mcp --settings deploy\videos\wan21-fusionx.probe.json --generate --cancel --output video-cancel.mp4 --report video-cancel.jsonl
```

Cancellation waits for a recognized phase from the exact owned task. The probe
sends one cancellation for that job ID and requires a terminal cancelled receipt.
If the job completes first, the cancellation check fails instead of claiming success.
The reserved output file is removed without downloading the result.

## Ownership and limits

The append-only JSONL report records literal settings, a new request ID, the
assigned context, dispatch intent, validated job receipts and the final outcome.
Each event is synced before proceeding. The report excludes transfer grants and
raw progress/error events. Keep reports private: they contain prompts and runtime
identities. After forced termination, inspect the last complete JSONL event.

Generation is sent once. A lost submission response produces `submission_unknown`.
A verified job with unknown completion produces `unresolved` and retains its ID.
Neither outcome permits automatic replay. On post-admission failure, cleanup
cancels only the verified job and observes it for at most 30 seconds. A failed
cleanup remains visible. Incomplete or invalid downloads are removed.

When a job ID is known, inspect that exact job using
[the standalone MCP client](../../pinokio_agent/skills/api/wan-git/clients/mcp.mjs)
with `wangp_session`, `action:"get_job"`, its `job_id` and saved `agent_context`.
The client also supports `--download-video <media_id> --agent-context <context>
--output <new.mp4>`. Select a final media ID only from a successfully completed,
identity-validated job. When the submission response is lost, the request ID alone
does not provide a read-only job lookup. Inspect WanGP before starting other work.

The probe fixes one task, one repetition, video mode, no media inputs and no
upscaling. It rejects templates, file prompts, output paths and other settings
that would expand that scope. Bounds are 128-832 pixels per side, 399360 pixels,
5-97 frames on the model's declared grid, at most eight seconds and 1-8 steps.
An MP4 transfer permits at most 64 MiB with same-origin grants, no redirects,
exact MIME/length and SHA-256. Image transfer limits remain unchanged.

`--timeout-seconds` accepts 10-1800, default 900. This deadline includes submission,
queue wait, polling and transfer. MCP requests separately have a 30-second limit.
Browser playback has a separate bounded check. Closing the MCP session does not
prove that a generation was cancelled.

## Verification

```powershell
rtk proxy node --test pinokio_agent\skills\api\wan-git\clients\mcp.test.mjs tests\scripts\video-smoke.test.mjs
rtk proxy npm run test:scripts
```

The 2026-11-02 live run passed CPU preflight and generated a 77460-byte MP4.
Chromium played it at 256x256 with a measured duration of 0.5625 seconds.
The SHA-256 was
`7e3185f380a690d62cf2e8ff4cb921b0b542aca2355ca251e254fc92347ec691`.
A second real job was cancelled during inference with a terminal cancelled
receipt and no downloaded MP4. Fixture tests separately cover identity conflicts,
lost responses, unresolved cleanup, transfer budgets and invalid playback.
These checks do not establish longer-video quality, image-conditioned generation
or 3D support.
