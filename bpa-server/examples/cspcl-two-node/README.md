Two local `hardy-bpa-server` instances over `cspcl`

This setup assumes both processes run on the same Linux host and share the same `vcan0` interface.

Start `vcan0` once:

```bash
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan
sudo ip link set up vcan0
```

Start node 1:

```bash
CSP_REPO_DIR=/home/hugo/code/libcsp cargo run --bin hardy-bpa-server --features cspcl -- \
  -c ./bpa-server/examples/cspcl-two-node/node1.toml
```

Start node 2 in another terminal:

```bash
CSP_REPO_DIR=/home/hugo/code/libcsp cargo run --bin hardy-bpa-server --features cspcl -- \
  -c ./bpa-server/examples/cspcl-two-node/node2.toml
```

What this config does:

- Node 1 is `ipn:1.0`, Node 2 is `ipn:2.0`
- Both nodes expose the built-in echo service on service `7`
- Each `cspcl` CLA pre-registers the other node as a peer
- Static routes forward `ipn:1.*` or `ipn:2.*` via the matching next-hop node ID

Expected behavior:

- Bundles destined to `ipn:2.*` on node 1 are forwarded to CSP address `2`, port `10`
- Bundles destined to `ipn:1.*` on node 2 are forwarded to CSP address `1`, port `10`
- Incoming CSP bundles are tagged with the configured peer address and node ID
