# CSPCL Two-Node Runbook

This runbook shows how to:

1. Create the `vcan0` interface
2. Start the two `hardy-bpa-server` nodes
3. Send a test bundle
4. Understand the current known problem

## Create `vcan0`

Run these commands once on the Linux host:

```bash
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan
sudo ip link set up vcan0
ip -details link show vcan0
```

If `vcan0` already exists, `ip link add` will fail. In that case just make sure it is up:

```bash
sudo ip link set up vcan0
```

## Start Node 1

Run this in one terminal from the repository root:

```bash
HARDY_BPA_SERVER_LOG_LEVEL=debug \
CSP_REPO_DIR=/home/hugo/code/libcsp \
cargo run --bin hardy-bpa-server --features cspcl -- \
  -c ./bpa-server/examples/cspcl-two-node/node1.toml
```

Node 1 uses:

- Node ID: `ipn:1.0`
- gRPC: `http://[::1]:50061`
- Echo service: `ipn:1.7`

## Start Node 2

Run this in a second terminal from the repository root:

```bash
HARDY_BPA_SERVER_LOG_LEVEL=debug \
CSP_REPO_DIR=/home/hugo/code/libcsp \
cargo run --bin hardy-bpa-server --features cspcl -- \
  -c ./bpa-server/examples/cspcl-two-node/node2.toml
```

Node 2 uses:

- Node ID: `ipn:2.0`
- gRPC: `http://[::1]:50062`
- Echo service: `ipn:2.7`

## Send A Message

To send from node 1 to node 2's echo service:

```bash
cargo run -p hardy-tools --bin bpa-send -- \
  --grpc 'http://[::1]:50061' \
  --destination ipn:2.7 \
  --payload 'hello' \
  --wait 5s
```

What this means:

- `--grpc 'http://[::1]:50061'`: talk to node 1
- `--destination ipn:2.7`: send to node 2, service 7
- no `--source-service`: let `bpa-send` request an auto-assigned temporary local application service
- `--wait 5s`: wait for the echo reply

If you want to be explicit, `--source-service 0` means the same thing.

If you do not want to wait for a reply:

```bash
cargo run -p hardy-tools --bin bpa-send -- \
  --grpc 'http://[::1]:50061' \
  --destination ipn:2.7 \
  --payload 'hello'
```

## Current Problem

The gRPC registration deadlock in `bpa-send` was fixed, and the BPA now clearly routes and forwards the bundle on node 1.

The current state is:

- Node 1 successfully registers the client application over gRPC
- Node 1 resolves `ipn:2.7` to peer `1`
- Node 1 enqueues and dequeues the bundle for CSP forwarding
- Node 1 calls `cspcl.send_bundle(...)`
- Node 2 never logs receipt of the bundle

The current debug logs show that the failure is below the BPA routing layer, in the CSP or SocketCAN path:

- Hardy routing is working up to the `cspcl` send call
- Node 2 does not receive anything from `cspcl_recv_bundle`
- `vcan0` remains at `RX=0` and `TX=0` packet counters during the test

So the remaining issue is not the Hardy RIB or gRPC registration anymore. The remaining issue is in the lower layer, likely one of:

- `cspcl` transmission
- libcsp routing or connection setup
- SocketCAN integration on `vcan0`

## Updated Behavior

The Hardy `cspcl` CLA now uses one in-band session on the same CSPCL port for:

- session init
- keepalive traffic
- bundle delivery acknowledgements

That changes the expected behavior in two important ways:

- if node 2 is down, node 1 does not register the CSP peer with the BPA and outbound bundles remain queued
- a bundle is only reported as `Sent` after node 2 dispatches it and replies with an in-band acknowledgement

To exercise this end-to-end flow, use:

```bash
./bpa-server/examples/cspcl-two-node/validate_cspcl_peer_liveness.sh
```

## Useful Checks

Check that `vcan0` is up:

```bash
ip -details link show vcan0
ip -s link show vcan0
```

Expected at this point:

- The interface should be `UP`
- If CSP frames are actually traversing the interface, the RX/TX counters should increase

At the moment, the interface is up, but the counters stay at zero during the bundle send test.
