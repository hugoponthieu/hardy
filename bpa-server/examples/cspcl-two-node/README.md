Two local `hardy-bpa-server` instances over `cspcl`

This example shows two Hardy BPAs running on the same Linux host over `vcan0` with same-port CSPCL session management enabled.

It demonstrates four behaviors:

- each node uses one CSPCL port for both session traffic and bundle traffic
- configured CSP peers are only candidate peers until in-band session traffic is observed
- bundles stay queued when the remote peer is not verified up
- `Sent` means the remote Hardy node dispatched the bundle and returned an in-band bundle acknowledgement

## Topology

- Node 1 BPA: `ipn:1.0`
- Node 2 BPA: `ipn:2.0`
- Shared interface: `vcan0`
- Node 1 gRPC: `http://[::1]:50061`
- Node 2 gRPC: `http://[::1]:50062`
- Node 1 CSPCL port: `11`
- Node 2 CSPCL port: `11`

Each node has:

- in-memory metadata storage
- in-memory bundle storage
- one configured CSP peer on the same CSPCL port used for session init, keepalive, bundles, and bundle acknowledgements
- no static route file, because direct neighbors are learned from `sink.add_peer(...)`

## One-Time Setup

Create `vcan0` once on the Linux host:

```bash
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan
sudo ip link set up vcan0
```

If `vcan0` already exists, just make sure it is up:

```bash
sudo ip link set up vcan0
```

## Start Node 1

From the repository root:

```bash
CSP_REPO_DIR=/home/hugo/code/libcsp cargo run --bin hardy-bpa-server --features cspcl -- \
  -c ./bpa-server/examples/cspcl-two-node/node1.toml
```

## Start Node 2

In another terminal:

```bash
CSP_REPO_DIR=/home/hugo/code/libcsp cargo run --bin hardy-bpa-server --features cspcl -- \
  -c ./bpa-server/examples/cspcl-two-node/node2.toml
```

## What The Config Means

- `local_port = 11` is the single CSPCL port used for session init, keepalive, bundle data, and bundle acknowledgements
- `remote_port = 11` is the peer port for the same in-band session
- `probe_interval_ms` and `probe_timeout_ms` control how aggressively Hardy tries to establish the session
- `keepalive_interval_ms` and `peer_expiry_ms` control when a live peer is refreshed or declared down
- `ack_timeout_ms` controls how long the sender waits for remote dispatch acknowledgement

The important runtime behavior is:

- the peer is not registered with the BPA at startup
- Hardy sends periodic in-band `SessionInit` frames to the configured peer
- when a valid `SessionInit` or `SessionKeepalive` arrives, the peer is registered with the BPA
- the direct route to `ipn:2.0` or `ipn:1.0` comes from peer registration, not a static `via` rule
- if keepalives stop arriving or bundle acknowledgements time out, the peer is removed from the BPA
- a forwarded bundle is only reported as `Sent` after the receiver dispatches it and replies with an in-band acknowledgement

## Scenarios

## Scenario 1: only node 1 is up

If you start only node 1 and send a bundle toward node 2, the bundle should remain queued in Hardy because the CSP peer is not verified up yet.

```bash
cargo run -p hardy-tools --bin bpa-send -- \
  --grpc 'http://[::1]:50061' \
  --destination ipn:2.9000 \
  --payload 'stored while node 2 is down'
```

Expected behavior:

- node 1 does not treat node 2 as an available BPA neighbor
- `forward()` returns `NoNeighbour`
- the bundle remains stored in Hardy waiting for the peer to come up

## Scenario 2: node 2 starts later

When node 2 starts, the same-port session handshake succeeds and Hardy registers the peer.

Expected behavior:

- node 1 logs that the CSP peer is verified up
- the BPA can now forward traffic to `ipn:2.0`
- newly submitted bundles can flow over CSPCL

## Scenario 3: node 2 is up and accepting bundles

Start a receiver on node 2:

```bash
cargo run -p hardy-tools --bin bpa-recv -- \
  --grpc 'http://[::1]:50062' \
  --service 9000
```

Then send from node 1:

```bash
cargo run -p hardy-tools --bin bpa-send -- \
  --grpc 'http://[::1]:50061' \
  --destination ipn:2.9000 \
  --payload 'hello after peer up'
```

Expected behavior:

- node 1 forwards the bundle over CSPCL
- node 2 receives the bundle and dispatches it locally
- node 2 sends an in-band bundle acknowledgement on the same CSPCL port
- node 1 only then treats the forwarding result as `Sent`

## Validation

Use the validation script to exercise the expected lifecycle:

```bash
./bpa-server/examples/cspcl-two-node/validate_cspcl_peer_liveness.sh
```

The script checks:

- node 1 only: the peer is not verified up and bundles stay queued
- node 2 starts: the peer becomes available
- normal delivery: a bundle reaches node 2 and the receiver sees the payload
- node 2 stops again: the peer is removed and later bundles stay queued again

## Important Notes

- This example requires `vcan0`.
- It also requires `CSP_REPO_DIR=/home/hugo/code/libcsp` so the local libcsp build can be used.
- This README describes the same-port CSPCL session implementation, not the older split control-port behavior.
- The detailed troubleshooting companion for this example is [RUNBOOK.md](/home/hugo/code/hardy/bpa-server/examples/cspcl-two-node/RUNBOOK.md).
- The protocol rationale is documented in [docs/cspcl-peer-liveness.md](/home/hugo/code/hardy/docs/cspcl-peer-liveness.md).
