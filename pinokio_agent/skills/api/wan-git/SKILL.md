---
name: api-wan-git
description: Call WanGP's shared-process MCP interface to discover models, preflight generation, track jobs and retrieve image outputs.
---

# WanGP API

## Clients

Use [clients/mcp.mjs](clients/mcp.mjs) with an explicit `--endpoint`.
Omitting `--tool` lists advertised tools.
Tool calls accept `--tool <name>` and either `--arguments <JSON>` or
`--arguments-file <file>`.
`--result-file <file>` saves the returned receipt without overwriting an existing file.

The client uses Node built-ins and sends each request once.
It is a standalone integration probe, not Deep's application adapter.
The [protocol tests](clients/mcp.test.mjs) run with Node's test runner.

## Operations

1. Discover exact IDs through `wangp_models`.
   Read defaults through `wangp_model(model_type=..., action="defaults")`.
2. Preflight with `wangp_validate(settings=...)`.
   Check `valid`, model availability and queue state before submitting.
   Availability is not proof that every ancillary asset is installed.
3. Before generation, save exact settings and a unique `request_id`.
   Call `wangp_generate(settings=..., wait=false, request_id=...)`.
   Retain `job_id` and `agent_context`.
4. Read that job with `wangp_session(action="get_job", arguments={"job_id": ...})`.
   Require `done=true`, `result.success=true` and an output in `result.gallery_items`.
   An accepted job is not generation success.
5. Download one image with `--download <media_id> --agent-context <context> --output <file>`.
   The client requests a download grant and verifies its same-origin URL, media type,
   exact byte length and SHA-256 before writing.
   Decode the image separately when verifying dimensions or image validity.

## Runtime Inputs

Reuse `agent_context` after reconnecting.
Treat workspace IDs, job IDs, request IDs and transfer capabilities as per-run data.
Keep transfer tokens out of shared documentation and logs.

For advanced toolbox actions, `action` without `arguments` reads the contract.
An explicit `arguments={}` executes the action with defaults.

## Outputs

Image generation can return JPEG rather than PNG.
Use the returned media type and verify downloaded bytes.
Job results bind output media IDs to the generating job.
Gallery listings alone do not prove which request created an output.

## Notes

When the same `request_id` has the same payload, WanGP can return the original job
with `reused=true`.
Changed-payload reuse is rejected.
Reuse that identity only to reconcile the original submission.
A timeout or `unknown_after_restart` is not permission to submit a new generation.

The shared web-UI MCP mode supports asynchronous jobs without another model process.
Use only the required generation, job and transfer actions.
WanGP's advertised Python, file and queue-management tools are not automatically
permissions for Deep's chat model.
