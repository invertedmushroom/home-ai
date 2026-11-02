# Optional self-hosted search

This isolated developer service supplies application search and the standalone
search probe. The service forwards queries to its explicit Brave HTML engine.
It is not an offline index. OpenAI
hosted search is a separate option in the [probe contract](../../docs/contracts/search-probe.md#openai-hosted-search).

Run from the repository root with a local Linux Docker engine:

```powershell
node scripts/search-init.mjs
docker compose --env-file data/search/.env -f deploy/search/compose.yaml up -d --wait --wait-timeout 60
node scripts/search-smoke.mjs --provider searxng --searxng-url http://127.0.0.1:8888 --engine brave --query "Rust programming language official documentation"
```

Initialization creates an ignored random signing secret and preserves an existing
file. The explicit environment file contains no chat API keys. The service binds
only `127.0.0.1:8888`, requires no search subscription, and runs from a pinned
SearXNG image digest. `settings.yml` enables JSON and keeps only the Brave engine.
Each probe must select that engine. Upstream denial or failure stays a failure.

To enable the application's per-request checkbox, add these nonsecret settings
to the root `.env` and restart Deep:

```dotenv
DEEP_SEARXNG_URL=http://127.0.0.1:8888
DEEP_SEARXNG_ENGINE=brave
```

Select a model backend, enable **Web access for this request**, and send a
request. Search defaults off for every request. Model-generated queries may
contain conversation/project details. The [search contract](../../docs/contracts/search.md)
defines the loop, durable sources, budgets and failure behavior. The Node worker
does not call SearXNG.

The container runs as UID 977 with dropped capabilities, no privilege escalation,
a read-only filesystem, disposable cache and bounded CPU/memory/processes/logs.
Its `unless-stopped` policy restarts search after a container failure or Docker
restart. An explicit stop keeps search stopped until started again. Docker must
be running. Access logging is disabled. Error logs can still
contain provider diagnostics. Outbound networking is required for search.
This loopback developer deployment supplies no account authorization or general
network-isolation guarantee. The Node execution worker retains its own network
restriction and receives no search credentials.

Inspect or remove only this separate Compose project:

```powershell
docker compose --env-file data/search/.env -f deploy/search/compose.yaml ps
docker compose --env-file data/search/.env -f deploy/search/compose.yaml logs --tail 40
docker compose --env-file data/search/.env -f deploy/search/compose.yaml down --volumes
```

Shutdown removes this project's containers, network and disposable volumes. The
ignored signing-secret file and downloaded image remain. Deep's database and
other Compose projects are independent. Start again with the same `up` command.

Configuration follows the upstream [container guide](https://docs.searxng.org/admin/installation-docker.html)
and [engine selection settings](https://docs.searxng.org/admin/settings/settings.html).
Live observations and limitations are in the [verification record](../../docs/reviews/search-providers-2026-10-30.md).
