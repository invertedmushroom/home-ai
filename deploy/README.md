# Container option

From the repository root:

```sh
docker compose up --build
```

Open http://127.0.0.1:4317. Compose defaults to demo mode and publishes only
on loopback. The named `deep-data` volume retains SQLite across container
recreation. Environment variables from `.env` can select a compatible endpoint.
Rust binds a wildcard interface inside the container, but the request policy
still accepts only exact `localhost` or literal loopback Host values. The published
loopback URL works normally. Arbitrary hostnames, foreign Origins and opaque
(`null`) Origins receive JSON `403`. Do not expose the container as an
unauthenticated LAN service or use forwarding headers to bypass the policy.
The container keeps internal/published port 4317; native development defaults to
4318 and browser tests use 14318/14319. If you configure native `DEEP_BIND` to
4317, stop the container with `docker compose stop` or choose another native
port. Stop retains the named data volume. Changing the container's internal port
requires updating the Compose mapping as well.

Inside a container, `127.0.0.1` refers to that container. Use a reachable runtime
hostname or `host.docker.internal` for a runtime reachable on the Docker host;
the runtime must listen on an interface the container can reach. Docker Desktop
networking and a Linux VM are different deployment environments; verify reachability.

The image contains the application only. GPU drivers, models and inference
runtimes remain separately provisioned. Node/Rust/Debian image tags are floating
within their release families; pin image digests when recording a real deployment.

The optional Node execution worker is currently controlled by the native
application through a local Linux Docker engine. This Compose setup does not
mount a Docker socket or enable native execution. Chat, proposals and browser
previews continue to work without it; see the root README for native worker setup.
