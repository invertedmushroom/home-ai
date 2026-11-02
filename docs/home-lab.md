# Home-lab direction

This is a proposed deployment layout. No hypervisor, GPU driver, runtime or model
has been installed or reconfigured by this scaffold.
The [delivery plan](plan.md) owns implementation sequencing, including hosted
API inference, server-side SQLite storage, and requirements before shared access.

## Start with independent runtime hosts

```text
Browser
  → Deep web + Rust + SQLite on a CPU host/VM
      → chat endpoint on a GPU PC or Linux GPU VM
      → future image-job endpoint on a GPU PC or Linux GPU VM
```

Proxmox is a reasonable candidate when the same server has other workloads.
Keep the application and data independent of a GPU assignment. Start with a
Linux VM for a GPU runtime when passthrough is supported; containerize individual
runtimes inside that guest if useful. Ordinary whole-device passthrough gives
the VM control of that GPU, making it unavailable to the host and other VMs.
Verify motherboard/CPU IOMMU support, device groups and GPU behavior before
choosing a layout. See [Proxmox's PCI passthrough documentation](https://github.com/proxmox/pve-docs/blob/master/qm-pci-passthrough.adoc).

Two GPU PCs can first serve separate workloads or replicas. Distributing one
model across machines is a separate, measured decision involving the runtime,
model size and interconnect. This repository does not assume that their VRAM
can be combined just by placing them behind one endpoint.

## On this Windows PC

Use native Node/Rust for the app initially. Keep Pinokio/Conda runtime environments
separate, and point Deep at a running endpoint. WSL and Docker are available
options for Linux workloads; neither is required for demo mode. Models and
runtime checkouts remain outside the application repository. In particular,
the existing `../Wan2GP` checkout stays independent.

Record the GPU model/VRAM, RAM, CPU, free storage, LAN speed, intended simultaneous
workloads and runtime versions when preparing hardware. Those facts should drive
context limits and concurrency. Choose VRAM thresholds, context sizes and GPU
offload settings from measurements on the runtime host. Runtime-specific launch
settings belong with that deployment, not Deep's application process. The HTTP
adapter follows [llama-server's documented chat interface](https://github.com/ggml-org/llama.cpp/tree/master/tools/server).

## Next slices

1. Extend the initial local/hosted [smoke observations](reviews/backend-smoke-2026-10-29.md)
   into repeatable quality and capacity measurements with pinned runtime/model
   versions and explicit context/output limits. Streaming now retains request
   ownership across reconnects and cancellation. Do not equate local cancellation
   with GPU stop.
2. Extend the implemented [Node execution worker](contracts/execution.md) with
   explicitly permitted tools and model orchestration. Reviewed output files and
   check/run actions use disposable local Docker containers; browser previews remain separate.
3. Add image jobs through authoritative submission IDs, per-job input/output
   binding and actual upstream cancellation support. Keep WanGP template stacks
   intact unless a measured quality comparison supports changing them.
4. Add authentication/authorization and a deployment network policy before exposing
   the app to other users or a LAN. Add backups and versioned database migrations
   before depending on it for important work.

Avoid a scheduler, model registry, Kubernetes or distributed database until a
measured workload requires them. The current single process and HTTP model seam
are enough to test useful behavior now and change hardware later.
