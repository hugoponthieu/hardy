Two local `hardy-bpa-server` instances over `tcpclv4` without echo

This example is for BPA-to-BPA forwarding, not echo testing.

It shows:

- node 1 and node 2 connected by TCPCLv4
- bundles submitted into a BPA over gRPC with `bpa-send`
- delivery to a real application on the remote BPA with `bpa-recv`
- bundles staying in in-memory storage when the next hop or destination service is unavailable

## Topology

- Node 1 BPA: `ipn:1.0`
- Node 2 BPA: `ipn:2.0`
- Node 1 TCPCL listener: `127.0.0.1:4560`
- Node 2 TCPCL listener: `127.0.0.1:4561`
- Node 1 gRPC: `http://[::1]:50061`
- Node 2 gRPC: `http://[::1]:50062`

Each node has:

- in-memory metadata storage
- in-memory bundle storage
- a static route to the other node
- a configured outbound TCPCL peer so the BPA can bring up the session itself

There is no echo service in this example.

## Build

From the repository root:

```bash
cargo build --release -p hardy-bpa-server -p hardy-tools
```

The server enables `grpc` and `tcpclv4` by default, so no extra feature flags are required for the normal build.

## Start node 1

```bash
cargo run --bin hardy-bpa-server -- \
  -c ./bpa-server/examples/tcpcl-two-node/node1.toml
```

## Start node 2

In another terminal:

```bash
cargo run --bin hardy-bpa-server -- \
  -c ./bpa-server/examples/tcpcl-two-node/node2.toml
```

With `log-level = "debug"`, you should see the TCPCL session come up automatically because each node has the other one listed in `clas[].peers`.

## Scenario 1: deliver a bundle to a real application on node 2

Start a receiver on node 2, service `9000`:

```bash
cargo run -p hardy-tools --bin bpa-recv -- \
  --grpc 'http://[::1]:50062' \
  --service 9000
```

Then submit a bundle to node 1 over gRPC:

```bash
cargo run -p hardy-tools --bin bpa-send -- \
  --grpc 'http://[::1]:50061' \
  --destination ipn:2.9000 \
  --payload 'hello from node 1'
```

What happens:

1. `bpa-send` registers a temporary local application on node 1 over gRPC
2. it asks node 1 BPA to originate a bundle to `ipn:2.9000`
3. node 1 uses the static route `ipn:2.* via ipn:2.0`
4. the TCPCL CLA forwards the bundle to node 2
5. node 2 delivers the payload to `bpa-recv`

Expected output on `bpa-recv`:

```text
received payload from ipn:1.<some-service> ack_requested=false payload=hello from node 1
```

## Scenario 2: remote BPA is down, so node 1 stores the bundle

Stop node 2 completely.

Then submit a bundle into node 1:

```bash
cargo run -p hardy-tools --bin bpa-send -- \
  --grpc 'http://[::1]:50061' \
  --destination ipn:2.9000 \
  --payload 'stored on node 1 until node 2 is back'
```

Expected node 1 behavior:

- the bundle is accepted over gRPC
- there is no reachable TCPCL peer for `ipn:2.0`
- node 1 logs:

```text
No route available for bundle ... storing until a forwarding opportunity arises
```

That means the bundle is in node 1's in-memory store with `Waiting` status.

When node 2 starts again, the configured TCPCL peer reconnects automatically. The new peer registration updates the RIB and node 1 re-processes waiting bundles.

## Scenario 3: node 2 is up, but the destination service is not registered

Keep node 2 running, but do not start `bpa-recv`.

Send the same bundle:

```bash
cargo run -p hardy-tools --bin bpa-send -- \
  --grpc 'http://[::1]:50061' \
  --destination ipn:2.9000 \
  --payload 'stored on node 2 until service 9000 appears'
```

Expected behavior:

- node 1 forwards the bundle to node 2
- node 2 has no local application registered on `ipn:2.9000`
- node 2 logs the same `No route available ... storing until a forwarding opportunity arises` message

That means the bundle has reached node 2 and is now stored in node 2's in-memory store, waiting for a local route to appear.

Now start the receiver:

```bash
cargo run -p hardy-tools --bin bpa-recv -- \
  --grpc 'http://[::1]:50062' \
  --service 9000
```

Registering the service adds the local route for `ipn:2.9000`. The BPA re-checks waiting bundles and delivers the stored bundle to `bpa-recv`.

## Important notes

- `bpa-send` talks only to the local BPA over gRPC. It does not talk directly to the remote node.
- The BPA forwards only when the destination EID is routable in the RIB.
- In this repository state, there is no separate management API to list bundles inside the in-memory store.
- For the in-memory case, the practical way to observe storage is the debug log showing the bundle moved to `Waiting`, and then later seeing it forwarded or delivered when the route or service appears.
