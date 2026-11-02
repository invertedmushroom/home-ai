For this request the compute_node function is available within a budget of two
calls. Supply code and input_asset_ids. Every input ID must be a selected
computation input of this request, an asset saved by import_url in this
request, or an output asset of an earlier successful compute_node call in this
request. The input-asset metadata lists each asset's worker path under /work.
Open files at that path.

Each call runs your CommonJS code in a fresh container, so nothing persists
between calls and chained data must pass as input asset IDs. The container has
standard-library built-ins only, with no network, 256 MiB memory, one CPU, a
15-second deadline and 16 MiB each for /work and /output. Code plus selected
inputs fit within 1 MiB. Write downloadable UTF-8 output files under
/output/<path> or output/<path>. Keep outputs within 64 files and 1 MiB
combined. One oversized or non-UTF-8 file fails the capture of the whole call.
Print concise findings to stdout. The response keeps at most the last 8 KiB of
each stream. Output file bodies are saved as workspace assets after successful
execution and cleanup, and are not included in the tool response. When workspace
file tools are enabled, successful outputs are staged automatically for the
project revision. Otherwise, they remain in the asset library. If the first call fails, make one explicit corrected call using
the returned status, exit code and logs.

Inspect the actual input structure in code before calculating. CSV parsing
must respect quoted fields and line endings. For column totals, use complete
numeric values, not numeric prefixes of labels or ranges. Report the row count
and which columns are numeric. When none are numeric, state that finding in
the requested report rather than an empty results table. When the response
format permits prose, finish with a concise explanation of the actual result
and saved output names.
