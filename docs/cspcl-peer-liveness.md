# CSPCL Peer Liveness and Bundle Acceptance

## Summary

In the current repository state, `tcpclv4` and `cspcl` do not expose peer availability to the BPA in the same way.

- TCPCL only registers a peer with the BPA after a real session has been established.
- TCPCL only reports outbound success after the remote peer has acknowledged the transfer.
- CSPCL registers configured peers immediately at startup, before any proof that the remote node is up.
- CSPCL currently reports success as soon as the local `send_bundle(...)` call returns `Ok(())`.

That difference explains the current failure mode: if only one CSP node is up, Hardy still believes the peer exists and forwards bundles into the CSP/SocketCAN path, where they may be lost.

## Implemented Hardy Design

The implemented fix keeps BPA unchanged and moves the new behavior into the upstream Rust bindings plus Hardy's `cspcl` crate.

- The upstream Rust bindings now expose one in-band session stream on the normal CSPCL port.
- Hardy `cspcl` no longer calls `sink.add_peer(...)` during startup.
- Each configured peer is treated as a candidate and is probed periodically with an in-band `SessionInit`.
- A peer is registered with BPA only after valid same-port session traffic is observed.
- A peer is removed from BPA when keepalives or bundle acknowledgements time out.
- `forward(...)` returns `NoNeighbour` while the peer is not verified up.
- `forward(...)` returns `Sent` only after the remote Hardy node dispatches the received bundle and sends back an in-band bundle acknowledgement.

The session protocol uses the same configured CSPCL port for `SessionInit`, keepalive, bundle data, and bundle acknowledgement frames.

## Current TCPCL Behavior

### 1. Peer registration happens after session setup

For TCPCL, a peer is not exposed to the BPA just because it appears in configuration.

On the active side, the connector:

1. opens the TCP socket
2. exchanges the contact header
3. exchanges `SESS_INIT`
4. negotiates session parameters
5. only then registers the session in the connection registry

This happens in [tcpclv4/src/connect.rs](/home/hugo/code/hardy/tcpclv4/src/connect.rs), where `register_session(...)` is called only after the contact and `SESS_INIT` work is complete.

The passive side behaves the same way. In [tcpclv4/src/context.rs](/home/hugo/code/hardy/tcpclv4/src/context.rs), `new_passive(...)` first validates the contact, reads the peer `SESS_INIT`, sends the local `SESS_INIT`, and only then calls `register_session(...)`.

That means the BPA only sees a TCPCL peer when there is an established, negotiated session.

### 2. Peer removal happens when the session closes

When the session ends, TCPCL removes the session from the registry with `unregister_session(...)`. That removes the peer from the BPA view once no session remains for that remote address.

This matters because later forwarding attempts will no longer see a live neighbor and the BPA can keep bundles waiting instead of treating the link as available.

### 3. Outbound success is tied to transfer acknowledgement

TCPCL does not treat a local write as enough to mark a bundle as sent.

In [tcpclv4/src/session.rs](/home/hugo/code/hardy/tcpclv4/src/session.rs):

- `forward_to_peer(...)` sends the bundle as one or more `XFER_SEGMENT` messages
- each segment is tracked in the local ack queue
- the session waits for matching `XFER_ACK` messages from the remote peer
- only when the transfer completes cleanly does TCPCL return `ForwardBundleResult::Sent`

This is the key semantic point: in TCPCL, `Sent` means the remote TCPCL peer acknowledged the transfer at the protocol level, not merely that bytes were pushed into the local kernel socket buffer.

## Current CSPCL Failure

The current CSPCL implementation behaves very differently.

### 1. Peers are registered from configuration at startup

In [cspcl/src/cla.rs](/home/hugo/code/hardy/cspcl/src/cla.rs), `on_register(...)` iterates over `self.config.peers` and immediately calls:

```rust
cla_inner.sink.add_peer(peer_addr.clone(), &peer.node_ids).await
```

This happens before any liveness check, handshake, keepalive, or remote confirmation.

As a result, the BPA believes the CSP peer is available as soon as the CLA starts.

### 2. Forward success is mapped to local send completion

In the same file, `forward(...)` does:

```rust
inner.cspcl_sender.send_bundle(bundle.to_vec(), *remote_addr, *remote_port).await
```

and maps:

- `Ok(())` to `ForwardBundleResult::Sent`
- `Err(...)` to `cla::Error::Disconnected`

So today, for CSPCL, `Sent` means only that the local CSPCL binding accepted the request. It does not mean:

- the remote node is up
- the remote Hardy instance received the bundle
- the remote BPA accepted the bundle

### 3. There is no TCPCL-equivalent peer lifecycle

The current CSPCL implementation has no equivalent of:

- contact establishment
- session handshake
- keepalive-based liveness tracking
- peer removal when the remote side disappears
- transfer acknowledgement proving remote acceptance

That is why a statically configured peer can remain visible even when the remote node is down.

## What Is Failing Today

The current implementation fails in four specific ways.

### 1. Peer existence is configuration-driven

The BPA routing layer sees the peer because `add_peer(...)` was called during startup, not because any live adjacency was proven.

### 2. Delivery success is defined too early

`send_bundle(...) -> Ok(())` is treated as bundle success even though this only proves local submission into the CSP path.

### 3. Lost-peer handling is missing

There is no mechanism that removes a CSP peer from the BPA when the remote node is absent or disappears.

### 4. Bundles can leave Hardy storage without remote receipt

Because `ForwardBundleResult::Sent` causes the BPA to treat forwarding as complete, a bundle can be dropped from Hardy storage even though the remote node never received it.

This is exactly the problem seen when only one node is up and the bundle is emitted toward `vcan0`.

## Proposed Solutions

## 1. Immediate behavioral fix

Do not register CSP peers with `sink.add_peer(...)` only because they are listed in config.

Instead, treat configured peers as candidates. Only expose a peer to the BPA after a positive liveness check proves that the remote node is currently reachable.

If no peer is currently verified, `forward(...)` should return:

```rust
ForwardBundleResult::NoNeighbour
```

instead of `Sent`.

That would make the BPA keep the bundle in `Waiting` rather than dropping it into the CSP path.

## 2. Add a CSPCL liveness phase

The TCPCL equivalent for CSPCL is a lightweight contact protocol.

At minimum, CSPCL should add:

- an explicit peer probe before first use
- periodic keepalive or heartbeat while the peer is considered up
- peer expiry and `remove_peer(...)` when the remote side stops responding

This liveness check may be implemented using CSP-level request/reply traffic, but it must drive BPA peer registration and removal.

## 3. Separate node liveness from bundle acceptance

A CSP ping or route check can prove that the remote node is alive at the network level, but that is not enough to conclude that a bundle was received by the remote Hardy BPA.

To claim bundle success, CSPCL needs a Hardy-level acceptance signal, for example:

- a CSPCL protocol acknowledgement sent by the remote Hardy node after `sink.dispatch(...)` succeeds
- or another explicit application/protocol response that confirms remote BPA acceptance

Without that, the best CSPCL can honestly claim is something like "locally transmitted" or "submitted to CSP", not `Sent` in the same sense as TCPCL.

## 4. Preferred end-state

The most robust design is:

1. configured peer exists only as a candidate
2. CSPCL establishes liveness with an explicit handshake or probe
3. only then CSPCL calls `sink.add_peer(...)`
4. bundles are sent only while the peer is in the verified-up state
5. success is reported only after remote Hardy acknowledges bundle acceptance
6. liveness loss triggers `remove_peer(...)`

That gives CSPCL the same high-level semantics TCPCL already has:

- register on verified link up
- unregister on verified link down
- mark a bundle sent only after protocol-level acknowledgement

## Recommended Implementation Direction

If you want the smallest change first, implement this in two stages.

### Stage 1

- stop calling `add_peer(...)` during startup
- add a peer-probe task for configured CSP peers
- call `add_peer(...)` only after probe success
- call `remove_peer(...)` after probe timeout/failure
- return `NoNeighbour` when the peer is not currently verified

This stage fixes the "single node up" failure and preserves bundles in Hardy storage.

### Stage 2

- add an explicit CSPCL bundle acknowledgement from the remote Hardy node
- only map that acknowledgement to `ForwardBundleResult::Sent`

This stage fixes the stronger semantic problem: deciding when the other part has really received the bundle.

## Test Scenarios

The eventual CSPCL implementation should be validated against these cases.

### Only node 1 is up

- node 1 must not report `Sent`
- the bundle must remain stored in Hardy with `Waiting`
- no peer should be considered available unless liveness succeeds

### Node 2 starts later

- the liveness mechanism should detect node 2
- CSPCL should register the peer at that moment
- the waiting bundle should then be forwarded

### Node 2 goes down after being up

- keepalive/probe failure should remove the peer
- later bundles should stay queued instead of being discarded into CSP

### Remote node alive but remote BPA not accepting bundles

- node-level liveness alone must not be treated as bundle acceptance
- the bundle should only become `Sent` after the remote Hardy acceptance signal

### Normal positive path

- with both nodes up and the remote Hardy side accepting bundles, CSPCL should return `Sent` only after the chosen acknowledgement is received

## Bottom Line

TCPCL solves this by making peer availability session-driven and delivery success acknowledgement-driven.

CSPCL currently does neither. It exposes peers too early and declares success too early.

If the goal is "do not send any packets unless the other node is up", the first correction is to make CSPCL peer registration depend on a verified liveness check.

If the goal is "know when the bundle is received by the other part", the full correction requires a remote Hardy-level acknowledgement, not just a CSP transport send.
