# Private guest mode

Use this guide to give someone private browser access to Deep through Tailscale.
Guest mode separates browsers' workspaces. Tailscale controls who can reach Deep.
There is no Deep account, password, invitation token or administrator login.

| Mode           | Enable with                                        | Browser access                                                                                                                                             |
| -------------- | -------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Local operator | `DEEP_PUBLIC_ORIGIN` unset or whitespace-only      | Trusted local access to every workspace in that data directory, including guest workspaces; runtime model discovery is available.                          |
| Private guest  | `DEEP_PUBLIC_ORIGIN` set to one exact HTTPS origin | Each browser sees its own workspaces; configured model selection works, while runtime model discovery and operator-only recovery actions stay unavailable. |

The setting applies to the whole Deep process. When guest mode is enabled, you
are also a guest, even on the host PC. Browser ownership separates guests from
each other; the person operating Deep can inspect the stored data locally.

```text
Guest browser, with Tailscale connected
  → https://YOUR-DEVICE.YOUR-TAILNET.ts.net (HTTPS port 443)
  → Tailscale Serve on the Deep host
  → http://127.0.0.1:4320 (Deep in guest mode)
  → your configured model runtimes and optional tools
```

## 1. Prepare the host and choose the address

Run the commands below from the repository root in PowerShell. Complete the
[local setup](../../README.md#run-locally) first. Install and sign in to
[Tailscale](https://tailscale.com/download) on the Deep host and on the device
that will open Deep. Use a current browser with Web Locks support and allow
cookies for the Deep site.

Check the host's existing proxy configuration:

```powershell
tailscale status
tailscale serve status
```

Find the host's full DNS name on the Tailscale admin console's
[Machines page](https://console.tailscale.com/admin/machines). The examples use
`https://YOUR-DEVICE.YOUR-TAILNET.ts.net`. Replace that placeholder with your
host's HTTPS origin, without an application path, query or fragment.

If HTTPS port 443 already serves another application, use an unused HTTPS port
such as 8443. Include `:8443` in `DEEP_PUBLIC_ORIGIN` and use `--https=8443` in
each Serve command below. Deep's internal port remains 4320.

## 2. Enable guest mode and start Deep

Build the UI, then start a separate instance:

```powershell
npm run build:web
$env:DEEP_BIND = '127.0.0.1:4320'
$env:DEEP_DATA_DIR = 'data/demo'
$env:CARGO_TARGET_DIR = 'target/demo'
$env:DEEP_PUBLIC_ORIGIN = 'https://YOUR-DEVICE.YOUR-TAILNET.ts.net'
cargo run --locked -p deep-server
```

Wait for `Deep is ready`. Keep this terminal open. This single server serves
both the built UI and API; sharing Vite's development port is unnecessary.

`data/demo` keeps guest data separate from your normal `data` directory. Reuse
the same directory when restarting to preserve guest workspaces. A directory
can be opened by only one Deep process at a time. `target/demo` avoids locking
the executable of a running local instance on Windows.

Deep still reads the repository's `.env`. These shell variables override the
same settings in `.env` and last for this terminal session. Model catalogs,
credentials and optional tools otherwise keep their existing configuration.
To make guest mode the default for future starts, set these `DEEP_*` values in
`.env` instead; this also makes ordinary starts use guest mode and its directory.

Guests share configured models, hosted API spending and queues within this
instance. Deep has no per-guest model allowlist or spending quota. Separate Deep
instances do not coordinate GPU use, even when they use the same resource group
names. Avoid simultaneous generations from separate instances on shared hardware.

## 3. Expose the guest instance privately

In a second PowerShell terminal, run:

```powershell
tailscale serve --https=443 http://127.0.0.1:4320
```

```
tailscale funnel --bg --https=443 off
tailscale serve --bg 4320
```

If Tailscale prompts you to enable HTTPS certificates, follow its consent link.
Keep this foreground terminal open. The printed HTTPS address must match
`DEEP_PUBLIC_ORIGIN`; if it differs, correct that setting and restart Deep.
[Tailscale Serve](https://tailscale.com/docs/features/tailscale-serve) supplies
HTTPS and applies tailnet access rules. Deep itself continues listening on
loopback. These commands require your Tailscale login, with no Tailscale API or
auth key in Deep's `.env`.

Point Serve at the guest instance on loopback port 4320. Keep the operator
instance, model runtime ports and observability interfaces private. This
walkthrough uses Serve, which requires Tailscale access. Funnel publishes to the
internet and needs a separate deployment design.

## 4. Grant a guest access

If the guest is already in your tailnet, allow their identity to reach this
host's TCP port 443 in your Tailscale access policy (8443 if you chose that port).
An existing broader rule may already allow access.

If the guest uses a separate tailnet, open **Machines**, choose **Share** for
the Deep host, and create a share invitation. The guest accepts the invitation
with their own Tailscale account and connects their client. Machine sharing and
your access policy determine which services they can reach; restrict access to
the intended HTTPS port. See
[Tailscale machine sharing](https://tailscale.com/docs/features/sharing).

Send the guest the full **HTTPS Serve URL**. A machine-share invitation grants
network access; the Serve URL opens Deep. The guest needs both when joining
from another tailnet. Sharing only the URL does not grant network access.

## 5. Open Deep and verify browser separation

Open the HTTPS Serve URL on your own device, then on the guest's device. Use
that address even when testing on the host. `127.0.0.1:4320` is the internal
listener, and opening a raw LAN or Tailscale IP does not replace the HTTPS origin.

The page should say **Your workspaces belong to this browser.** Create a
workspace and reload to check that it remains accessible. Open the same URL in
a private window or another browser and create a different workspace. The two
browser sessions should show separate workspace lists. Model discovery stays
hidden in both; configured model choices remain selectable.

Tabs in the same browser profile share a guest. The secure cookie lasts 30 days.
Closing a private session, clearing site data, cookie expiry or using a different
browser loses automatic access to that guest's workspaces. The data remains
stored until deleted. Deep has no account login to restore browser access, and
pre-existing ownerless workspaces never become guest-owned automatically.

## Stop sharing or revoke access

For the foreground setup above, press Ctrl+C in the Serve terminal, then in the
Deep terminal. Guest data remains in `data/demo`.

To keep only the proxy running after closing its terminal, first stop the
foreground Serve command, then use:

```powershell
tailscale serve --bg --https=443 http://127.0.0.1:4320
```

Deep must still be running separately. A background Serve configuration survives
closing the terminal. To disable that HTTPS listener, run:

```powershell
tailscale serve --https=443 off
tailscale serve status
```

Use the HTTPS port you originally chose. This disables the listener on that port,
including any other paths configured there. See the
[Serve CLI reference](https://tailscale.com/docs/reference/tailscale-cli/serve).

To revoke one person's access while keeping Deep available to others, revoke
their machine share or remove their access-policy grant in Tailscale. Check for
other policy rules that still grant access. Revocation does not delete guest
data. Deep does not manage individual guest invitations or revocation.

## Use operator access to inspect guest data

Stop Serve and the guest Deep process first. In the original Deep terminal,
reuse the guest data directory on a different, unshared loopback port:

```powershell
$env:DEEP_BIND = '127.0.0.1:4321'
$env:DEEP_DATA_DIR = 'data/demo'
$env:CARGO_TARGET_DIR = 'target/demo'
$env:DEEP_PUBLIC_ORIGIN = ' '
cargo run --locked -p deep-server
```

Open **http://127.0.0.1:4321** on the host PC. Operator mode lists all workspaces
in that directory and enables runtime model discovery. The operator can inspect
or delete data and resolve unknown image outcomes through provider-specific
operator actions. Changing
modes preserves data and guest ownership.

The space in `' '` is intentional: Deep treats whitespace-only as disabled.
An empty assignment can remove the variable in Windows PowerShell, letting a
`DEEP_PUBLIC_ORIGIN` value in `.env` take effect again. Stopping sharing before
this change keeps the operator UI private.

To share again, stop the operator process, repeat step 2 with the original
HTTPS origin and port 4320, then restart Serve as in step 3.

## Troubleshooting

| Symptom                                                           | Check                                                                                                                                                                     |
| ----------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| The guest cannot reach the URL                                  | Both Tailscale clients must be connected. Confirm the share was accepted, the full DNS name is used, and the access policy allows the HTTPS port.                         |
| Serve reports a proxy/backend error                               | Deep must be running at the exact loopback address and port in the Serve command. Check its terminal for startup errors.                                                  |
| Deep rejects the host or browser origin with `403`                | Compare the printed Serve URL with `DEEP_PUBLIC_ORIGIN`, including scheme and any non-default port. Open Deep directly at that URL. Restart after changing configuration. |
| Startup requires an exact HTTPS origin and loopback bind          | Use `https://…` without an application path, query or fragment, and `DEEP_BIND=127.0.0.1:4320`.                                                                           |
| The browser cannot initialize a guest or repeatedly returns `401` | Use the HTTPS URL, allow site cookies, use a browser with Web Locks support, and reload. Clearing cookies starts a new guest.                                             |
| Your usual workspaces are missing                                 | Check `DEEP_DATA_DIR`. Guest mode shows only this browser's data; operator inspection uses the same data directory with guest mode disabled.                              |
| Runtime models is missing                                         | Guest mode hides discovery for everyone, including the host. Use the operator procedure above. Model selection still uses configured profiles.                            |
| Model requests fail although the page opens                       | Check the selected model's configured runtime or API credentials. Serve exposes Deep; it does not start model runtimes.                                                   |

The [runtime contract](../contracts/runtime.md#private-browser-guests) defines
the exact guest authorization and cookie behavior.

## Clean up abandoned guests

Guests can delete their own workspaces with the × control. Confirmation is
required, and active work or pending cleanup blocks deletion.

For offline maintenance, stop Deep and keep the same `DEEP_DATA_DIR`.
Preview guests inactive for at least seven days:

```powershell
$env:DEEP_DATA_DIR = 'data/demo'
cargo run --locked -p deep-server -- cleanup-guests --inactive-days 7
```

The report shows a short guest identifier, last activity timestamp, workspace
count and whether pending work blocks cleanup. Preview does not run migrations,
startup recovery or deletions. Maintenance requires the current schema and the
exclusive database lease. Start the current app once after an upgrade before
using maintenance against an older database.

Apply the same selection explicitly:

```powershell
cargo run --locked -p deep-server -- cleanup-guests --inactive-days 7 --apply
```

Cleanup skips an entire guest if any of that guest's model runs, Node executions
or image jobs remain active or await cleanup/resolution. It deletes eligible
guest workspaces and their database-owned content atomically. It retains
ownerless local data and the execution cleanup ledger. It does not remove models,
WanGP gallery copies or downloaded files. SQLite may retain allocated file space
for reuse after deletion. No background retention task deletes data automatically.
