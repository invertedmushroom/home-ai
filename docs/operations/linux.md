# Linux setup (Arch)

Run Deep natively on one machine. LM Studio, Pinokio/WanGP and Docker remain
separately managed. Deep needs no Python, Conda, uv, GPU toolkit or model files of
its own. Start the existing inference applications normally.

Share a Git clone, not a copy of the development machine's `.env`, `data`,
`node_modules`, `target` or private `*.local.json` profiles. The recipient creates
their own configuration and database. Bound Docker-worker databases are not
portable between machines.

## 1. Install build prerequisites

Deep needs Node.js 24+, npm, Rust 1.89+ and a native C compiler. SQLite is bundled.
On Arch, install missing tools with a full system update:
When rustup already manages Rust, omit `rust` from the package command and keep
that toolchain at 1.89 or newer. Use one Rust installation rather than installing
conflicting packages.

```sh
sudo pacman -Syu --needed base-devel git nodejs npm rust
node --version
rustc --version
```

Keep the existing Docker and inference installations. Native Deep does not need
a GPU driver change.

## 2. First run without inference

```sh
git clone https://github.com/invertedmushroom/local-ai.git
cd local-ai
npm ci
npm run dev
```

Open **http://127.0.0.1:5173**. The Rust API listens on loopback port 4318.
Try the scripted demo before adding models. Ctrl+C stops the development pair.
Run commands from the repository root so relative paths and `.env` resolve there.
File names and paths are case-sensitive on Linux.

For normal use without Vite:

```sh
npm run build:web
npm run build:server
./target/release/deep-server
```

Open **http://127.0.0.1:4318**. The binary serves both the built UI and API.
There is no `.exe` suffix. Rebuild the UI after pulling UI changes and rebuild
the server after pulling Rust changes.
The native binary handles Ctrl+C/SIGINT and SIGTERM through the same graceful
shutdown path. Docker worker cleanup is bounded. An interrupted model request
can still require reconciliation after restart. A successful process exit does
not prove that an external inference runtime stopped.

## 3. Connect the existing local runtimes

Create `.env` only when it does not exist:

```sh
test -e .env || cp .env.example .env
chmod 600 .env
```

Edit existing entries rather than appending duplicate settings. For LM Studio:

```dotenv
DEEP_DEFAULT_BACKEND=lmstudio
DEEP_LMSTUDIO_MODEL=google/gemma-4-e4b
DEEP_LMSTUDIO_BASE_URL=http://127.0.0.1:1234/v1
DEEP_MODEL_TIMEOUT_SECONDS=1800
```

Replace the model ID with an installed LM Studio model's exact API ID. Enable
LM Studio's server and choose its own JIT/TTL/eviction settings. If authentication
is enabled, add `DEEP_LMSTUDIO_API_KEY` privately. Deep does not load or unload
weights itself. Avoid loading duplicate copies in LM Studio and llama-server.

For images, enable WanGP's **MCP with Web UI** in its existing process. Copy the
example without overwriting an existing local file:

```sh
test -e deploy/images/profiles.local.json || cp deploy/images/profiles.example.json deploy/images/profiles.local.json
```

Edit the endpoint and model settings to match that WanGP installation. The
sample requires FLUX.2 Klein 4B at `http://127.0.0.1:8000/mcp`. Add to `.env`:

```dotenv
DEEP_IMAGE_PROFILES_FILE=deploy/images/profiles.local.json
```

Use `/` in Linux file paths. Restart Deep after configuration changes. Discovery
can inspect configured runtimes in local operator mode, but it does not install
models or prove that weights fit in memory.

## 4. Use both GPU machines

Keep one Deep process/database on PC A. PC B runs its existing LM Studio and
WanGP. An SSH tunnel keeps those HTTP interfaces private and satisfies Deep's
loopback-only WanGP transport:

```sh
ssh -N -o ExitOnForwardFailure=yes \
  -L 127.0.0.1:1235:127.0.0.1:1234 \
  -L 127.0.0.1:8001:127.0.0.1:8000 user@gpu-pc-b
```

Run SSH on PC A. Replace the SSH host/user and remote ports with PC B's values.
The SSH server must be reachable and permit forwarding. Keep the tunnel running
while Deep uses PC B. An occupied forwarded port fails explicitly.

Copy the [model catalog](../../deploy/models/profiles.multi-pc.example.json)
and [image presets](../../deploy/images/profiles.multi-pc.example.json) to their
ignored local files, preserving any existing configuration:

```sh
test -e deploy/models/profiles.local.json || cp deploy/models/profiles.multi-pc.example.json deploy/models/profiles.local.json
test -e deploy/images/profiles.local.json || cp deploy/images/profiles.multi-pc.example.json deploy/images/profiles.local.json
```

If a local file already exists, merge the needed entries manually. Set:

```dotenv
DEEP_MODEL_PROFILES_FILE=deploy/models/profiles.local.json
DEEP_IMAGE_PROFILES_FILE=deploy/images/profiles.local.json
```

The model catalog owns text connections when configured. Legacy
`DEEP_LMSTUDIO_MODEL` settings no longer select those models. Replace the sample
Saluki/Qwen IDs with exact installed IDs. The image example requires Klein on
each selected machine. Remove unavailable presets. Optional model API credentials
use `TEXT_PC_API_KEY` and `SECOND_TEXT_PC_API_KEY` in `.env`, never the catalog.

| Runtime        | Endpoint on PC A            | Resource group       |
| -------------- | --------------------------- | -------------------- |
| PC A LM Studio | `http://127.0.0.1:1234/v1`  | `text-pc-gpu`        |
| PC A WanGP     | `http://127.0.0.1:8000/mcp` | `text-pc-gpu`        |
| PC B LM Studio | `http://127.0.0.1:1235/v1`  | `second-text-pc-gpu` |
| PC B WanGP     | `http://127.0.0.1:8001/mcp` | `second-text-pc-gpu` |

Text and images on one physical GPU share a group. Separate GPUs can progress
independently. These names do not detect hardware or free VRAM. Other clients and
separate Deep instances bypass this coordination. See [model routing](model-routing.md).

Deep itself stays on loopback. To open the same operator UI from another
computer, forward its port over SSH. This grants access to all of that operator's
workspaces. For isolated browser guests instead, use [private guest mode](demo.md).
Do not expose an unauthenticated wildcard listener on the LAN.

## 5. Optional local Docker worker

Check Docker as the same ordinary user that will run Deep:

```sh
docker context show
docker info
docker build -t deep-node-worker:local -f workers/node/Dockerfile workers/node
```

Add `DEEP_WORKER_IMAGE=deep-node-worker:local` to `.env` and restart Deep.
Chat and image generation work without the worker.
The worker uses the selected local Docker context, then pins its Unix socket,
daemon and original canonical data path. `DOCKER_HOST`/`DOCKER_CONTEXT` overrides
are intentionally not used for control operations. Select the context explicitly
with `docker context use` before first enablement. Remote TCP/SSH Docker contexts
are rejected, even when the inference runtimes are remote.

The daemon must report Linux, seccomp and CPU/memory/swap/PID limit support.
Rootless Docker depends on the host's cgroup configuration and is not assumed to
meet those checks. If socket permissions fail, configure access to the intended
daemon as an operator. Docker access is privileged. Do not solve permissions by
running Deep as root or making the socket world-writable.
The [container app](../../deploy/README.md) remains separate and has no Docker
socket or Node-worker control.

## Verification and updates

```sh
npm run test:scripts
npm run build:web
cargo test --workspace --locked
```

Browser tests need Chromium. On Arch, use the distribution package rather than
Playwright's Debian/Ubuntu dependency installer:

```sh
sudo pacman -Syu --needed chromium
DEEP_BROWSER_PATH=/usr/bin/chromium npm run test:browser
```

Optional real worker checks create only scoped disposable containers:

```sh
DEEP_TEST_WORKER_IMAGE=deep-node-worker:local cargo test --locked --test executions -- --ignored --test-threads=1
```

Before upgrading, stop Deep and back up the entire data directory. Start the new
binary with the same original directory to preserve conversations/assets and
worker binding. Keep databases on a local filesystem, with one Deep process per
directory. A forced stop never authorizes replaying uncertain external jobs.

### Verified boundary

The 2026-11-02 check built this source in an isolated x86_64 Arch userspace with
Rust 1.99.1, Node 26.13 and Arch Chromium 156. It passed 215 Rust tests, including
SIGINT/SIGTERM and preserved data, 28 script tests, the web build, 124 operator
browser tests and two guest browser tests. Two additional real Docker-worker
tests passed Unix-socket control, isolation, outputs, cancellation and restart
cleanup against a local Linux Docker engine. Windows compilation and strict
Clippy also passed.

The Arch container used Docker's Linux kernel. These checks do not certify a
particular Arch host's GPU drivers, rootless Docker/cgroup setup, installed model
weights or network connectivity between the recipient's two machines.
