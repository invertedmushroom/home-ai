# Task evaluation cases

Task-level regression cases for Deep itself, as distinct from unit and
integration tests of harness code. [docs/plan/plan.md](../../docs/plan/plan.md)
calls for repeatable tasks measuring chat quality, code correctness, context
limits, and failures. For cases without automated checks, replay a
case by hand: configure the listed setup in a fresh workspace, submit the
prompt, and check each entry in `expect` against the reply, the published
revision, and the run detail.

## Case format

One JSON file per case:

- `id` — stable slug matching the filename.
- `provenance` — where the case came from and when it was recorded. A case
  without provenance is an anecdote, not a regression pin.
- `capability` — which tool grants must be enabled for the run.
- `original_fault` — the failure the case exists to catch, described plainly.
- `setup` — files, assets, and any fixture that must be supplied manually
  (`fixture` names anything the repo does not contain).
- `prompt` — the user message to submit.
- `expect` — human-checked assertions on the reply and published state. Keep
  them about outcomes, never exact reply strings.
- `notes` — context that keeps future readers from "fixing" the case into
  something trivial.
- `checks` — optional executable workspace checks. `files` maps paths to exact
  expected text, `null` for a deleted path, or `{"nonempty":true}` for a saved
  nonempty file. Unlisted initial files must stay unchanged. Extra paths fail.
  `revision_delta` defaults to one and can be zero for a task with no net changes.

## Automated workspace tasks

Start an updated Deep server in local operator mode. Select an explicit configured
local-runtime profile with `tool_calls:true`. Run:

```powershell
node scripts/eval-workspace.mjs --url http://127.0.0.1:4318 --profile <local-profile-id>
node scripts/eval-workspace.mjs --url http://127.0.0.1:4318 --profile <local-profile-id> --case workspace-targeted-refactor
```

The default suite covers a multi-file refactor with deletion, literal search and
replacement, and deletion of the last project file. `--case` can repeat. The
existing `save-summary-direct-write` case also has executable checks. Its summary
quality and tool choice still require human assessment.

The runner creates a fresh workspace for each case, applies the supplied setup,
and grants only native workspace tools. It checks task output, preserved initial
revisions, the selected model/profile and atomic applied-proposal provenance.
The runner retains each workspace for inspection. It never retries generation.
On a ten-minute evaluation timeout or transport failure after receiving a run ID,
it requests cancellation of that run. A failed task stops the suite.

JSON output includes task success, run/workspace IDs, the selected model/profile,
model rounds, usage and elapsed time. `--report <new file>` also saves the report
in an existing directory. Per-call file-tool failures are not persisted by Deep,
so their count is reported as unknown. Successful execution or inference alone
cannot establish task correctness.
Use a retained directory such as `logs/` for reports. Standard Playwright runs
clear `test-results`, including any unrelated evidence placed inside it.

## Status

- The workspace runner checks text-file outcomes. It does not execute generated
  code, inspect images, assess summary quality or resume interrupted runs.
- The original CSV case remains manual because its source dataset is missing.
- The runner selects local runtimes. Hosted iteration can use the manual cases:
  [docs/reviews/node-tools-2026-10-30.md](../../docs/reviews/node-tools-2026-10-30.md)
  flags local-model native-call quality as untested.
