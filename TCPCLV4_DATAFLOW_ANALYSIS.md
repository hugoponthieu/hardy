# TCPCLv4 Data-Flow Analysis: `session.rs` and `listen.rs`

## Executive Summary

The user's hypothesis is **partially inverted**.

- `on_transfer` in `session.rs` does **not** carry bundles from the BPA to the remote peer.
  It does the **opposite**: it receives bundles **from** the remote peer and delivers them **to** the BPA.
- `listen` in `listen.rs` does **not** simply do the opposite of `on_transfer`.
  It sets up *passive* (server-side) sessions that handle **both** directions simultaneously.

The functions that truly form an opposed pair are:

| Direction              | Entry point                       | Key function          |
|------------------------|-----------------------------------|-----------------------|
| Remote → BPA           | `on_transfer` (session.rs)        | `sink.dispatch()`     |
| BPA → Remote           | `forward_to_peer` (session.rs)    | `send_segment()`      |

---

## Detailed Analysis

### `on_transfer` – `tcpclv4/src/session.rs` (lines 132–188)

#### What it actually does

`on_transfer` is the handler for **incoming** `XFER_SEGMENT` messages arriving **from the remote peer** over the TCP connection.

**Step-by-step flow:**

1. A `TransferSegmentMessage` arrives on `self.transport` (the framed TCP/TLS stream).
2. `Session::run()` (or `send_segment()` during a concurrent outbound transfer) routes it to `on_transfer`.
3. The function accumulates the segment payload into `self.ingress_bundle` (a `BytesMut`).
4. Once the **end flag** is set, the fully reassembled bundle is handed to the BPA via:
   ```rust
   self.sink.dispatch(bundle.freeze(), self.peer_node.as_ref(), self.peer_addr.as_ref()).await
   ```
5. A `XFER_ACK` is sent back to the remote peer confirming receipt.

**Data flow:**

```
Remote peer
  │  XFER_SEGMENT (start)
  │  XFER_SEGMENT (…)
  │  XFER_SEGMENT (end)
  ▼
on_transfer()  ──────────────────────────▶  BPA (self.sink.dispatch)
                                              │
                                              ▼
                                      Bundle processing / storage
```

#### What the user thought it does

> "receive bundle that come from the BP to the remote"

That description fits `forward_to_peer` / `send_segment`, **not** `on_transfer`.
`on_transfer` moves data in the **opposite** direction: remote → BPA.

---

### Outbound path for completeness – `forward_to_peer` / `send_segment`

The function that carries bundles **from the BPA to the remote peer** is `forward_to_peer` (lines 338–375), which internally calls `send_once` → `send_segment`.

**Data flow:**

```
BPA
  │  Cla::forward() → registry.forward() → ConnectionPool::try_send()
  │       sends (bundle, result_tx) on the mpsc channel
  ▼
Session::run()  (self.from_sink.recv())
  │
  ▼
forward_to_peer()
  │
  ▼
send_once() → send_segment()
  │  XFER_SEGMENT (start)
  │  XFER_SEGMENT (…)
  │  XFER_SEGMENT (end)
  ▼
Remote peer
```

---

### `listen` – `tcpclv4/src/listen.rs`

#### What it actually does

`listen` is a TCP **server loop** that handles **inbound connection establishment** (passive role, RFC 9174 §3.1). It does **not** correspond to only one data direction.

**Step-by-step flow:**

1. `listen()` binds a `TcpListener` and accepts connections (with optional rate limiting via Tower).
2. Each accepted TCP socket is handed to `new_contact()`, which:
   - Validates the `dtn!` contact header.
   - Optionally upgrades to TLS via `tls_negotiate()`.
3. `new_passive()` then:
   - Reads the peer's `SESS_INIT` message and sends back the local `SESS_INIT`.
   - Negotiates parameters (keepalive, segment MRU, transfer MRU).
   - Creates a `(tx, rx)` mpsc channel pair.
   - Constructs a `Session` with the `rx` end (`from_sink`).
   - Calls `registry.register_session(… tx …)` so the BPA can enqueue bundles for this peer.
   - Calls `session.run()` which handles **both** directions (see below).

Once `session.run()` is active, two concurrent paths exist within the same session:

| Path | Source | Sink | Functions involved |
|---|---|---|---|
| Inbound | Remote peer (XFER_SEGMENT) | BPA (sink.dispatch) | `on_transfer` |
| Outbound | BPA (from_sink channel) | Remote peer (XFER_SEGMENT) | `forward_to_peer`, `send_segment` |

`listen` is therefore responsible for the **passive session setup**, but not exclusively for one data direction.

#### Passive vs. Active sessions

The `listen`/`new_passive` path and the `connect`/`new_active` path (in `connect.rs`) both produce the same `Session` struct and both handle bidirectional traffic identically after setup. The only difference is the order of the `SESS_INIT` exchange:

| | Passive (listen.rs) | Active (connect.rs) |
|---|---|---|
| Who initiates TCP | Remote peer | Local node |
| SESS_INIT order | Receive first, then send | Send first, then receive |
| Triggered by | Incoming TCP connection | `Cla::forward()` when no existing session |

---

## Corrected Mental Model

```
                        ┌──────────────────────────────────────────────────┐
                        │              Session (session.rs)                │
                        │                                                  │
 Remote Peer            │   on_transfer()          forward_to_peer()       │   BPA
 ─────────────          │   ───────────────────    ──────────────────────  │   ─────────
                        │                                                  │
 XFER_SEGMENT ─────────▶│─▶ reassemble segments ─▶ sink.dispatch()  ──────│──▶ receive bundle
                        │                                                  │
 XFER_SEGMENT ◀─────────│◀─ send_segment()   ◀──── from_sink.recv() ◀─────│──── forward bundle
                        │                                                  │
                        └──────────────────────────────────────────────────┘
                                 ▲                          ▲
                    Passive setup│                          │Active setup
                     listen.rs   │                          │connect.rs
                    (new_passive)│                          │(new_active)
```

---

## Conclusion

1. **`on_transfer` handles inbound bundles**: it reassembles `XFER_SEGMENT` data arriving from the remote peer and delivers the complete bundle to the BPA via `sink.dispatch()`. It does **not** send bundles from BPA to the remote.

2. **`listen` sets up passive sessions**: it accepts incoming TCP connections from remote peers, negotiates the TCPCLv4 session, and starts a bidirectional `Session`. The resulting session handles both directions — it is **not** a one-way inbound-only path.

3. The outbound direction (BPA → remote) is handled by `forward_to_peer` / `send_segment` in `session.rs`, triggered either from a passive session established by `listen` or an active session established by `connect.rs`.
