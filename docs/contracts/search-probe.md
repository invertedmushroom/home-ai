# Search provider probe

Status: implemented standalone developer tool. The app and offline Node worker
do not invoke this tool. Application search uses a separate bounded Rust path
defined by the [search contract](search.md). OpenAI hosted search remains a
standalone probe option. See the [initial investigation](../reviews/search-options-2026-10-30.md)
and [provider verification](../reviews/search-providers-2026-10-30.md).

## Interface and ownership

`node scripts/search-smoke.mjs --provider duckduckgo|searxng|openai --query "terms"`
performs one explicit provider attempt. Keyless adapters do not load `.env` or
use model API keys. The probe never retries, rotates endpoints, solves challenges
or selects another provider. SearXNG
additionally requires an operator-supplied `--searxng-url` and one `--engine`.
DuckDuckGo supports a trusted installed browser path through `--browser` or the
existing `DEEP_BROWSER_PATH`; otherwise it uses installed Playwright Chromium.
There is no implicit browser download or arbitrary launch-argument option.

The result is a JSON receipt with `provider`, `query`, `status`, `elapsed_ms`,
`results`, optional HTTP status, and a diagnostic code/message on failure.
SearXNG includes the selected engine. Invalid input can omit query/timing.
Exit codes are 0 for success (including explicit genuine empty results), 1 for
operational/provider failure or cancellation, and 2 for invalid input/config.
Text and URLs in the receipt remain untrusted data.

Queries contain 1–512 UTF-8 bytes, no ASCII control characters and no leading
search-routing operators at a token boundary (`!`, `:` or `\`). Operators such
as `site:rust-lang.org` remain supported. Output has at most five deduplicated
sources, with titles capped at 200 and snippets at 700 Unicode code points, and
URLs at 2048 characters. No source URL is opened or fetched automatically.

## Provider controls

All HTTP transports use a fresh direct Node HTTP(S) agent with ambient proxy settings
disabled, no automatic redirects, a 256 KiB streamed body
limit and strict UTF-8/content-type checks. Non-200 bodies are not consumed.
Keyless requests send no auth/cookie headers. OpenAI alone receives its explicit
bearer key at the fixed official endpoint.

DuckDuckGo uses only `https://html.duckduckgo.com/html/?q=...`. Its 20-second
operation deadline includes local browser setup and the request. Chromium only
parses the bounded HTML in an inert DOM document: it never navigates to provider
or result pages. All browser page traffic is aborted. The browser has a fresh
profile/context, Chromium sandbox enabled, JavaScript/service workers/downloads
disabled and a curated child environment. Ambient Selenium remote configuration
or proxy configuration rejects the browser probe before launch, including its
local control connection. No custom user agent or challenge execution is used.

HTTP 403, 429 and 202 produce blocked, rate-limited and challenge failures.
Recognized challenge markup also fails. Unknown markup is `layout_changed`;
recognized result markup with no usable links fails. Success with zero results
requires an explicit recognized no-results page, not an absent selector.

SearXNG accepts only a literal loopback HTTP origin (`127.0.0.1` or `[::1]`),
without credentials, path prefix, query or fragment. It requests `/search` with
`format=json` and one explicitly named engine under an eight-second deadline.
The service must enable JSON format. An incompatible response, malformed result,
unexpected engine, or any reported unresponsive engine fails the whole probe;
partial results are not reported as complete. The operator must configure the
SearXNG instance and its engines; this client does not make a paid engine keyless.
Answer/infobox-only responses fail as `unsupported_result_type` rather than
masquerading as an empty ordinary-link search. The optional
[local deployment](../../deploy/search/README.md) keeps only the verified Brave
engine and supplies no chat-model keys to the service.

## OpenAI hosted search

```powershell
node scripts/search-smoke.mjs --provider openai --model gpt-5.4-mini --query "Rust programming language official documentation"
```

Explicit OpenAI selection loads the working directory's optional `.env` using
Node's environment-file loader. It requires `OPENAI_API_KEY` and a separate
`--model` or `DEEP_OPENAI_SEARCH_MODEL`. It does not inherit the configured chat
model or SDK endpoint overrides. Key presence alone initiates no search.

One POST to `https://api.openai.com/v1/responses` supplies only the search query
and fixed instructions. It sends no workspace files, conversation, prior response
ID, cookies or location setting. The request forces `web_search`, requests source
metadata, sets `store:false`, limits built-in tool calls to one and output to
2,048 tokens, and has a 30-second client deadline. The hosted model may rewrite
the query. One built-in tool call is not a promise of one upstream HTTP request.
OpenAI receives the query and may charge for model tokens and hosted search.
`store:false` does not establish zero provider retention or zero billing.

A successful response requires completed response/message/tool states, exactly
one search action, valid consulted HTTP(S) source URLs and nonempty bounded
summary text. Refusal, absent search evidence, incomplete output, malformed data
or unsafe source links fail explicitly. The probe never retries or switches model.
Cancellation closes the owned HTTP connection and rejects late success. This
does not establish that hosted execution or billing stopped.

OpenAI receipts identify `result_kind:consulted_sources`. Up to five deduplicated
URLs come from the tool's `action.sources`, not links invented in summary text.
Missing titles use the URL hostname. Their snippets remain empty because this
interface does not supply extracted page snippets. `summary` is separately
labeled generated text, bounded to 32 KiB. Summary links/citations remain
untrusted text and are not opened or rendered by this CLI. The consulted-source
list is bounded and need not contain every URL cited in the generated summary.

`reported_model`, `search_calls` and validated provider `usage` accompany success.
Missing token counts stay null. No cost is estimated and no query/response is
automatically saved to SQLite. Defaults and wire fields follow the
[official web-search guide](https://developers.openai.com/api/docs/guides/tools-web-search).

## Source links

Source links require HTTP(S), default ports, no credentials, valid domain labels
and no trailing dot, literal IP, localhost or local/reserved suffix. DuckDuckGo
redirect wrappers are unwrapped once, without double-decoding the destination.
These are syntax checks, not DNS resolution or a network firewall guarantee.

## Cancellation and limits

SIGINT/SIGTERM/SIGHUP abort the CLI's request. Pre-cancelled requests create no
provider traffic. In-flight HTTP cancellation closes the owned connection. A
browser cancellation shares one cleanup attempt with normal completion, prevents
late success and reports failure if process exit cannot be confirmed.

Browser cleanup attempts graceful close for one second, then force-kill with
bounded asynchronous waits. **Windows limitation:** installed Playwright invokes
`taskkill` synchronously, so a stalled OS utility can block the Node event loop
beyond these deadlines. A pending-Promise test does not establish a hard bound on
that synchronous path. A detected cleanup failure reports the owned browser PID and exits
nonzero; it is not a successful search. This developer tool has no production
containment claim against process/OS failures, CPU/RAM exhaustion or browser bugs.

No search state is durable. Restarting the app does not resume or replay probes;
running the CLI again is a new deliberate request. Independently supervised
browser termination, stable intent/receipts and crash recovery must be designed
and verified before any browser adapter enters the app's model tool loop.

The offline test suite uses loopback HTTP fixtures, injected OpenAI protocol
responses and local browser parsing. It opens no hosted API/search engine.
Separate opt-in live observations verify the recorded providers on this machine;
they do not establish broad coverage or production reliability.
