# Run Two BPA Nodes over CSPCL

This guide brings up two `hardy-bpa-server` nodes with built-in `cspcl` and verifies forwarding to Node 2 with in-memory storage (not echo).

If you first need a downloadable `riscv64` package, build and publish it using
[Build And Publish A RISC-V BPA With CSPCL](riscv-cspcl-release.md), then run
this guide on the installed binary.

## Prerequisites

- Build environment that can compile `hardy-cspcl` (libcsp headers/libs available).
- Rust/Cargo installed.
- Two terminals.
- For CAN mode: Linux with `vcan` support (`iproute2`; `can-utils` optional).

Set libcsp environment variables (values for this machine):

```bash
export CSP_REPO_DIR=/home/hugo/code/libcsp
export CSP_BUILD_DIR=/home/hugo/code/libcsp/lib
```

## 1. Create node configs and routes

Create a working directory:

```bash
mkdir -p /tmp/hardy-cspcl-demo && cd /tmp/hardy-cspcl-demo
```

Create Node A routes (`node-a.routes`):

```text
ipn:2.*.* via ipn:2.0
```

Create Node B routes (`node-b.routes`):

```text
ipn:1.*.* via ipn:1.0
```

Create Node A config (`node-a.toml`) for **loopback mode**:

```toml
log-level = "debug"
node-ids = "ipn:1.0"

[grpc]
address = "[::1]:51051"
services = ["application", "service", "cla"]

[storage.metadata]
type = "memory"

[storage.bundle]
type = "memory"

[static-routes]
routes-file = "./node-a.routes"
watch = false

[[clas]]
name = "csp-a"
type = "cspcl"
local-addr = 1
port = 10
interface = "loopback"
interface-name = "loopback"

[[clas.peers]]
node-id = "ipn:2.0"
addr = 2
port = 10
```

Create Node B config (`node-b.toml`) for **loopback mode**:

```toml
log-level = "debug"
node-ids = "ipn:2.0"

[grpc]
address = "[::1]:52051"
services = ["application", "service", "cla"]

[storage.metadata]
type = "memory"

[storage.bundle]
type = "memory"

[static-routes]
routes-file = "./node-b.routes"
watch = false

[[clas]]
name = "csp-b"
type = "cspcl"
local-addr = 2
port = 10
interface = "loopback"
interface-name = "loopback"

[[clas.peers]]
node-id = "ipn:1.0"
addr = 1
port = 10
```

## 2. Start both BPA nodes

Terminal 1 (Node A):

```bash
cd /home/hugo/code/hardy
cargo run -p hardy-bpa-server --features cspcl -- --config /tmp/hardy-cspcl-demo/node-a.toml
```

Terminal 2 (Node B):

```bash
cd /home/hugo/code/hardy
cargo run -p hardy-bpa-server --features cspcl -- --config /tmp/hardy-cspcl-demo/node-b.toml
```

Configured `clas.peers` entries are bootstrap routes and identity overrides.
They are announced at registration time, so outbound traffic can work before any
inbound bundle is seen.

## 3. Send a bundle that Node 2 stores

Send to a Node 2 service that is **not** registered (for example `ipn:2.4242`), so Node 2 retains it:

```bash
cd /home/hugo/code/hardy
cargo run -p hardy-tools --bin bp -- app-send \
  --bpa http://[::1]:51051 \
  --source-service 4242 \
  --payload "store me on node2" \
  ipn:2.4242
```

Expected sender output:

```text
sent bundle <bundle-id> from ipn:1.4242 to ipn:2.4242
```

On Node 2 (`log-level = "debug"`), look for lines like:

```text
Queuing bundle for forwarding ...
Storing bundle until a forwarding opportunity arises
```

## Troubleshooting

- If `hardy-cspcl` fails to compile, verify libcsp development headers/libs are installed and visible to the `cspcl-sys` build.
- If CAN mode fails to initialize, rebuild libcsp with SocketCAN support (`./waf configure --with-os=posix --enable-can-socketcan && ./waf build`).
- If you run Node A and Node B as separate processes and see timeouts in loopback mode, switch to CAN mode below (`vcan0`). CSP loopback is process-local.
- If you only see send output on Node 1 and nothing on Node 2, confirm both nodes are on CAN (`vcan0`) and routes are:
  - Node A: `ipn:2.*.* via ipn:2.0`
  - Node B: `ipn:1.*.* via ipn:1.0`
- If peers do not become live, confirm `local-addr`/`addr` pairs are symmetric (`1<->2`) and both CLAs use the same `port`.

## Run the same demo over CAN (`vcan0`)

Use this when running two BPA nodes as separate processes on the same Linux host.

### 1. Prepare `vcan0`

Quick one-liner:

```bash
sudo modprobe vcan && sudo ip link add dev vcan0 type vcan 2>/dev/null || true; sudo ip link set up vcan0
```

Step-by-step:

```bash
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan 2>/dev/null || true
sudo ip link set up vcan0
ip -details link show vcan0
```

### 2. Update both node configs to CAN

In `node-a.toml`:

```toml
[[clas]]
name = "csp-a"
type = "cspcl"
local-addr = 1
port = 10
interface = "can"
interface-name = "vcan0"

[[clas.peers]]
node-id = "ipn:2.0"
addr = 2
port = 10
```

In `node-b.toml`:

```toml
[[clas]]
name = "csp-b"
type = "cspcl"
local-addr = 2
port = 10
interface = "can"
interface-name = "vcan0"

[[clas.peers]]
node-id = "ipn:1.0"
addr = 1
port = 10
```

Static routes stay the same (`node-a.routes` routes to `ipn:2.*.*`, `node-b.routes` routes to `ipn:1.*.*`).

If you want the default BP identity mapping, you may omit `node-id` from a peer
entry and Hardy will derive `ipn:<addr>.0`.

### 3. Start nodes and send store-demo bundle

Start Node A and Node B exactly as in sections 2 and 3 above, then run:

```bash
cd /home/hugo/code/hardy
cargo run -p hardy-tools --bin bp -- app-send \
  --bpa http://[::1]:51051 \
  --source-service 4242 \
  --payload "store me on node2 (can)" \
  ipn:2.4242
```
