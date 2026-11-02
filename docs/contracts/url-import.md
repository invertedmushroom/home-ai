# Public file imports

`allow_url_import?: boolean` on run submission defaults to false. Admission freezes
the permission as `import_enabled`. It requires a real model backend, independently
of SearXNG or Node configuration. Image generation cannot combine with imports.
The browser's default-off **Web access** control grants this permission and, when
configured, search permission. Each accepted submission or workspace change resets
the control. Failed submissions retain the pending choice.

`import_url({url,name})` imports UTF-8 CSV, TSV, JSON, plain text or Markdown into
the current workspace. The tool saves an immutable asset and returns metadata,
an input ID and a worker path. File bodies remain outside model context. With
temporary computation enabled, a later `compute_node` call can use that ID.
Search discovers links and snippets. HTML page reading remains a separate future
capability. The Node worker retains its existing offline boundary.

## Transport

`url_import.rs` is a self-contained retrieval module using a dedicated reqwest client.
Each hop uses an HTTP(S) domain URL on its default port without credentials, fragments or
reserved local/test names. DNS must return 1–16 addresses, all accepted by the
public-address policy. The policy rejects private, loopback, link-local,
documentation, multicast, reserved and translated IPv6 ranges. IPv6 is restricted
to ordinary global unicast outside the explicitly excluded special allocations.
The request pins those checked addresses and preserves the domain for Host/TLS.
Each hop creates a new client with proxies, automatic redirects and decompression
disabled, sending a fixed non-identifying `User-Agent` (`deep-url-import/0.1`) and
no application credentials, cookies or referrer.

The server follows at most three validated redirects (`301`, `302`, `303`, `307`
or `308`) with bounded ASCII `Location` headers. Each destination repeats
URL and DNS checks. HTTPS cannot redirect to HTTP. HTTP 200 and a supported
Content-Type (`text/csv` / `application/csv`, `text/tab-separated-values`,
`application/json`, `text/plain`, `text/markdown` / `text/x-markdown`) are required.
Declared charsets must be single-valued `utf-8` or `us-ascii`. Binary content, invalid
UTF-8, invalid JSON, compressed responses and HTML webpages fail with an explicit
diagnostic. The stream enforces the byte limit even without Content-Length.

Address filtering is a destination policy, not a separate network sandbox.
Services deployed on globally allocated addresses remain reachable according to
the host's routing and firewall policy. Deployments requiring stronger egress
isolation must enforce that boundary outside the application.

## Budgets and publication

Each run permits at most two imports. Import-enabled runs allow up to five model
turns within the shared 90-second active deadline. Each import has a 15-second
deadline including publication. The downloaded body is bounded to 1 MiB. The
filename and body together must fit below the existing 1 MiB asset/worker limit.
Computation additionally counts code and all selected inputs against its own
1 MiB limit, so large imported files can require a smaller script or input set.
The entire native-call batch validates before any effect.

`Store::save_import` checks the exact parent run, frozen permission, import count,
call identity and deadline inside the publication transaction. A delayed detached
database task must pass those checks after acquiring the lock. Expired or terminal
parents publish no new asset. There is no automatic retry or startup replay.
Transient in-progress import status stays in memory. Whole-batch syntax or budget
violations fail the run before any effect; runtime download or content errors on
an active run consume one import budget and return a structured `"status": "failed"`
tool observation so the model can recover or fall back to search snippets.
Successful imports remain visible even if subsequent model work fails or is cancelled.

The asset records `origin: "import"` and `source` containing requested URL,
final URL, SHA-256 of the original bytes, creating run ID and call ID. Conflicting
workspace names receive a UUID suffix. `Run.imports` derives successful imports
from that provenance. Both model-loop and transactional compute authorization
permit only this run's imported IDs, selected input IDs and prior successful
compute outputs. Admission's `input_assets` remains the frozen original selection.
Saved imports never change the accepted project. Existing asset review/application
and guest ownership rules apply. Workspace deletion removes the imported records.

Schema 18 adds default-off `runs.import_enabled` and rebuilds assets to include
import provenance. The atomic migration preserves existing asset IDs, content,
row order, execution links and project references before startup recovery.
