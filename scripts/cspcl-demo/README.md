# Four-Node CSPCL + A-SABR Demo

Four Hardy BPA nodes on a shared `vcan0` link, with **A-SABR** providing live
contact-graph routing. Each node runs its own `hardy-bpa-server` process. The
csp layer carries bundles between direct neighbours; A-SABR decides, per bundle,
which neighbour is the next hop towards the destination.

```
  A (ipn:1.0)  <->  B (ipn:2.0)  <->  C (ipn:3.0)  <->  D (ipn:4.0)
   csp addr 1       csp addr 2        csp addr 3        csp addr 4
   grpc 51051       grpc 52051        grpc 53051        grpc 54051
```

All nodes use csp port `10` on `vcan0`. The chain shape comes from the `peers:`
blocks (who each node is willing to talk to directly) and from the shared
**contact plan** (`contact-plan.cp`) — the topological input that A-SABR routes
over.

## Why A-SABR?

The previous version of this demo used `static-routes:` per node to hard-code
next hops. A-SABR replaces that with a single shared contact plan: each node
loads the same file, builds its own router, and computes next hops dynamically
per bundle. The dispatcher consults A-SABR first; only if it returns no answer
does it fall back to the RIB (which is still populated by the csp `peers:`
entries).

You can confirm A-SABR is engaged from the startup log:

```text
A-SABR live routing enabled (router=SpsnHybridParenting, contact-plan=./scripts/cspcl-demo/contact-plan.cp)
```

## Files

| File              | Purpose                                                  |
| ----------------- | -------------------------------------------------------- |
| `node-a.yaml`     | Node A config (ipn:1.0, csp addr 1, grpc `[::1]:51051`)  |
| `node-b.yaml`     | Node B config (ipn:2.0, csp addr 2, grpc `[::1]:52051`)  |
| `node-c.yaml`     | Node C config (ipn:3.0, csp addr 3, grpc `[::1]:53051`)  |
| `node-d.yaml`     | Node D config (ipn:4.0, csp addr 4, grpc `[::1]:54051`)  |
| `contact-plan.cp` | A-SABR contact plan, shared by all four nodes            |
| `node-*.routes`   | Static-routes equivalents (kept for reference, unwired)  |
| `start.sh`        | Optional helper that launches all four nodes from one shell |
| `stop.sh`         | Stops the demo if `start.sh` was used                    |

## Contact plan

`contact-plan.cp` declares the topology A-SABR sees:

```text
node 0 root
node 1 a
node 2 b
node 3 c
node 4 d
contact 1 2 0 9999999999 10000 1
contact 2 1 0 9999999999 10000 1
contact 2 3 0 9999999999 10000 1
contact 3 2 0 9999999999 10000 1
contact 3 4 0 9999999999 10000 1
contact 4 3 0 9999999999 10000 1
```

Notes:

- A-SABR requires node ids declared from 0 upward without gaps. Node 0 is the
  mandatory "root" allocator placeholder — it's not part of the chain.
- Real nodes 1..4 map to ipn node numbers (`local-node-id: "ipn:N.0"`).
- Each link declares **two** contacts (one per direction). They use Unix-time
  seconds; `0 → 9999999999` means "always active" for the foreseeable future.
- Rate (`10000` bits/s) and delay (`1` s) are placeholder values; the demo
  doesn't exercise capacity-constrained scheduling.

A-SABR is configured identically in each `node-*.yaml`, except for
`local-node-id`:

```yaml
asabr:
  protocol-id: "asabr"
  router: "SpsnHybridParenting"
  contact-plan-path: "./scripts/cspcl-demo/contact-plan.cp"
  local-node-id: "ipn:1.0"   # ipn:2.0 / ipn:3.0 / ipn:4.0 on B / C / D
```

The router strategy (`SpsnHybridParenting`) is one of A-SABR's contact-graph
algorithms; any of A-SABR's supported strategies could be plugged in here.

## Prerequisites

- `libcsp` built with ZMQ + SocketCAN support. Export before running:
  ```bash
  export CSP_REPO_DIR=/path/to/libcsp
  export CSP_BUILD_DIR=/path/to/libcsp/build       # or build/src
  export CSP_USE_STUBS=0
  ```
- `vcan0` up:
  ```bash
  sudo modprobe vcan && sudo ip link add dev vcan0 type vcan 2>/dev/null || true; sudo ip link set up vcan0
  ```
  For persistence see [`docs/user-docs/operations/vcan-on-arch.md`](../../docs/user-docs/operations/vcan-on-arch.md).
- `hardy-bpa-server` built with the `cspcl` feature:
  ```bash
  cargo build -p hardy-bpa-server --features cspcl
  ```
  (A-SABR is a hard dep — it's always linked in, no extra feature flag needed.)

## Manual launch (one terminal per node)

All commands assume CWD = workspace root (`/home/hugo/code/hardy`), because the
yamls reference `./scripts/cspcl-demo/contact-plan.cp` relatively.

**Terminal 1 — Node A**
```bash
cargo run -p hardy-bpa-server --features cspcl -- \
  --config scripts/cspcl-demo/node-a.yaml
```

**Terminal 2 — Node B**
```bash
cargo run -p hardy-bpa-server --features cspcl -- \
  --config scripts/cspcl-demo/node-b.yaml
```

**Terminal 3 — Node C**
```bash
cargo run -p hardy-bpa-server --features cspcl -- \
  --config scripts/cspcl-demo/node-c.yaml
```

**Terminal 4 — Node D**
```bash
cargo run -p hardy-bpa-server --features cspcl -- \
  --config scripts/cspcl-demo/node-d.yaml
```

In each node's log, look for:

- `A-SABR live routing enabled (router=SpsnHybridParenting, contact-plan=...)` — config wired.
- `A-SABR router worker ready (local-node-id=N)` — the dedicated worker thread
  built the router from the contact plan and is accepting lookups.
- The CSPCL CLA binding `vcan0` and announcing its `peers`.

Per-bundle, with `log-level: "debug"`, each routing decision logs two lines on
the node that asks A-SABR:

```text
A-SABR routing lookup for ipn:0.4.4242
A-SABR returned next-hop ipn:0.2.0 for ipn:0.4.4242
```

If A-SABR can't find a path you get `A-SABR found no route for ...` instead,
and the dispatcher falls back to the RIB (or stores the bundle if the RIB has
no answer either).

## Send a bundle through the chain

Three-hop store demo: A → B → C → D, where service `4242` is unregistered on D
so D stores the bundle on arrival.

**Terminal 5 — Sender**
```bash
cargo run -p hardy-tools --bin bp -- app-send \
  --bpa http://[::1]:51051 \
  --source-service 4242 \
  --payload "hello D from A" \
  ipn:4.4242
```

Expected:

- Sender prints `sent bundle <id> from ipn:1.4242 to ipn:4.4242`.
- `[node-a]` logs `A-SABR routing lookup for ipn:0.4.4242` →
  `A-SABR returned next-hop ipn:0.2.0 for ipn:0.4.4242`, then
  `Queuing bundle for forwarding to CLA peer ...`.
- `[node-b]` receives over csp, then `A-SABR ... next-hop ipn:0.3.0 ...`, forwards.
- `[node-c]` receives, `A-SABR ... next-hop ipn:0.4.0 ...`, forwards.
- `[node-d]` receives, finds service `4242` unregistered, logs
  `Storing bundle until a forwarding opportunity arises`.

The key point: **none of the nodes know the full route**. Each one only consults
the contact plan, learns its own next hop, and emits onto the wire. Without
A-SABR there is no `static-routes:` configured — forwarding past direct
neighbours would fail.

### Quick experiments

- **Reverse (D → A)** — exercises the chain in the other direction:
  ```bash
  cargo run -p hardy-tools --bin bp -- app-send \
    --bpa http://[::1]:54051 \
    --source-service 4242 \
    --payload "hello A from D" \
    ipn:1.4242
  ```
- **Break a link** — open `contact-plan.cp`, change the two `contact 2 3` /
  `contact 3 2` lines so the window is in the past (e.g. `0 1`). Restart all
  four nodes. Now `ipn:1 → ipn:4` cannot complete: A-SABR no longer finds a
  path. B (and the sender via store-and-forward) hold the bundle. Restore the
  contacts and restart to recover.
- **Swap routers** — change `router: "SpsnHybridParenting"` in every yaml to
  another A-SABR strategy (e.g. one of the CGR variants exposed by the
  `hardy-asabr-routing` crate). Same topology, different routing math.

## Stopping

Ctrl-C in each terminal. If you used `start.sh`, `./stop.sh` reaps the four
backgrounded processes recorded in `pids`.

## Troubleshooting

- **Startup error `Failed to initialise A-SABR routing`** — usually a contact
  plan parse error or an invalid `local-node-id`. The contact plan must list
  every node id you reference (here: 0..=4), declared in increasing order with
  no gaps. The `local-node-id` must be `ipn:N.0` with N ≤ u16::MAX.
- **A bundle is queued but never forwarded** — A-SABR returned `None` for that
  destination. Check that the destination node is declared in the contact plan
  and reachable through currently-active contacts. With `log-level: debug`,
  successful forwards show up as `Queuing bundle for forwarding`.
- **`vcan0` missing / port collisions** — same as the cspcl-only demo
  (`ip link show vcan0`, `ss -ltn | grep -E ':5[1-4]051'`).
- **Comparing against static routes** — the `node-*.routes` files in this
  directory are the equivalent static-route tables for this topology. They are
  no longer referenced by any yaml, but you can wire them back in by replacing
  the `asabr:` block with `static-routes: { routes-file: "..." }` to see the
  fallback path in action.
