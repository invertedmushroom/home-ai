# Search in model runs

Status: implemented for one trusted local user. Search is an optional capability
of an existing model run. The [runtime contract](runtime.md) still owns admission,
cancellation, model selection and final publication. The standalone
[provider probe](search-probe.md) is independent of this path.

## Permission and configuration

`POST /api/workspaces/{id}/runs` accepts `search?: boolean`, default `false`.
When search is enabled, admission freezes the operator's SearXNG configuration
with the run. Missing configuration or the demo backend returns `400` before
creating a run or message. `/api/status.search_available` reports configuration,
not service health or a model's tool-calling ability. Endpoint URLs remain private.

The operator sets `DEEP_SEARXNG_URL` to a literal loopback HTTP origin, without
credentials, path prefix, query or fragment. `DEEP_SEARXNG_ENGINE` defaults to
`brave`. The supplied [deployment](../../deploy/search/README.md) enables only
that engine. Its container restarts with Docker unless explicitly stopped.
The model can supply a query, never an endpoint or engine.

The browser defaults search off and resets permission after an accepted submission
or workspace change. Failed submissions preserve the unsent choice. Permission is
bound to the accepted receipt. Later requests do not inherit permission from history.
Enabling search allows model-generated queries that may contain conversation or
project details to leave the application. The checkbox discloses that consequence.

## Native calls and budgets

`run::complete` owns a bounded loop through the same selected model adapter.
Search permission exposes the native `web_search({query})` function. A separately
permitted saved-script action can share the [model loop](runtime.md#model-tools).
The adapter supports ordinary JSON and fragmented SSE function calls. Text that
resembles a tool call is ordinary text and grants no authority.

- A run permits at most two searches. Search-only runs have three model turns.
  Script permission raises the maximum to four turns. Calls are sequential.
  The last model turn, or exhaustion of all tool budgets, sets `tool_choice: none`.
- Before any effect, the loop validates the entire batch: unique bounded call IDs,
  the permitted function name and query-only JSON without extra fields. Queries
  contain 1–512 UTF-8 bytes, exclude ASCII controls and reject token-leading
  SearXNG routing operators (`!`, `:` and `\`). Token boundaries follow Unicode
  White_Space on the server and browser. U+FEFF is not a boundary.
- Every accumulated model request must fit the existing 128 KiB context budget.
  Intermediate assistant content is checked before starting a search.
- A 90-second deadline covers the active loop after queueing. Each SearXNG request
  has an eight-second deadline. The configured per-model-call timeout also applies.
- Search grants no Node or file-write permission. There is no automatic retry,
  provider fallback or page fetch through `web_search`. Only final text with an explicit normal `stop`
  can pass through existing answer/proposal publication. Intermediate text remains
  a draft, even when it contains valid HTML or file-change JSON.

The loop uses native Chat Completions tool messages and returns each saved result
to the same model. See the upstream [function-calling guide](https://developers.openai.com/api/docs/guides/function-calling)
and SearXNG's [routing syntax](https://docs.searxng.org/user/search-syntax.html).
An incompatible model fails visibly without a text-parsing fallback.

## Search transport and sources

The server sends only `q`, `format=json` and the frozen engine to `/search`.
The separate HTTP client disables proxies and redirects. It supplies no model
credentials, cookies, project snapshot or conversation payload. A model can still
include details from its context in the query. SearXNG forwards that query to its
configured upstream engine. Sources and snippets return to the selected model.

Success requires HTTP 200, JSON content type, valid UTF-8/JSON within 256 KiB,
an empty `unresponsive_engines` array and matching engine provenance for every
result. Partial failures, malformed results and answer-only responses fail the
run. A genuine empty link result succeeds. The decoder validates all returned
results, examines at most the first 25 for selection and retains at most five
deduplicated sources per search. Titles and snippets are bounded to 200 and 700
Unicode code points. Fragments are removed from saved URLs.

Source URLs must be HTTP(S) domain URLs with bounded labels and length, without
credentials, IP literals, nondefault ports, trailing dots or reserved local/test
suffixes. This is a syntax policy, not DNS verification or a claim that a source
is trustworthy. Search never opens source URLs. Separately permitted [file imports](url-import.md)
can retrieve supported data files after DNS/address validation. The browser creates external
links only from validated saved sources, with no opener or referrer. Model text
and source titles remain text. A model-written URL or `[S99]` creates no link.

## Durable activity and recovery

Run receipts include `search_enabled`, `model_rounds` and `searches` (0–2).
The shared runtime contract defines round accounting for combined tools.
Each search records `id`, `call_id`, `query`, `engine`, `status`, `elapsed_ms`,
`error` and `sources`. Intents are saved before HTTP. Results are saved before
returning them to the model. Source IDs are sequential `S1`–`S10` within one run.
They identify captured observations, not verified support for an answer's claim.

Searches transition from `pending` to `succeeded`, `failed`, `cancelled` or
`interrupted`. Every terminal run closes pending intents in the same transaction.
Completed receipts survive cancellation, later failure and restart. Late results
cannot change a terminal run. Restart interrupts unfinished work without replay.
An interrupted intent does not establish whether an upstream request was received.

Successful prior turns include their bounded saved search observations in later
frozen model context. Those observations are untrusted data scoped to their
original run. Failed turns remain excluded. Source IDs can repeat across runs.
Workspace deletion removes search configuration and receipts with the runs.

Usage sums observed counts across model turns. If any turn omits a count, the
aggregate remains unknown. Reported model identity must agree across turns.
First-text timing includes preceding model/search time. Activity reports searches
separately. Runs with all tool permissions off retain the original
single-call behavior and report `model_rounds: 0` because loop accounting does not apply.

Schema version 10 adds private nullable `search_config`, `search_activity` and
`model_rounds` columns. Existing runs migrate with search off, no searches and
zero rounds. The shared atomic migration completes before startup recovery.

Verification: [search run checks](../reviews/search-run-smoke-2026-10-30.md).
