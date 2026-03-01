# Keepalive Interval in `tcpclv4/src/listen.rs`

## Purpose

The `keepalive_interval` (an `Option<u16>`, in seconds) controls how frequently peers send **keepalive "heartbeat" messages** to each other over an idle TCPCLv4 connection. This prevents the TCP connection from being silently dropped by firewalls or NAT devices that time out idle connections. `None` (or `0`) means keepalives are disabled.

---

## How it flows through the functions

### 1. `Listener::new_passive` — negotiation (lines 288–307)

```rust
// Send our proposed keepalive in the SESS_INIT message
transport.send(codec::Message::SessionInit(codec::SessionInitMessage {
    keepalive_interval: self.keepalive_interval.unwrap_or(0),  // our proposal
    ...
})).await;

// Negotiate: take the MINIMUM of ours and the peer's — RFC 9174 §5.1.1
let keepalive_interval = self
    .keepalive_interval
    .map(|ki| peer_init.keepalive_interval.min(ki))  // min(peer, ours)
    .unwrap_or(0);  // 0 if we disabled keepalives
```

Both sides advertise their preferred interval in `SESS_INIT`. The **negotiated value is the minimum** of the two — the more conservative peer wins. If our config disables keepalives (`None`), the result is `0` (disabled) regardless of what the peer wants.

### 2. Used as a timeout for `transport::terminate` (lines 313–319)

```rust
return transport::terminate(
    transport,
    codec::SessionTermReasonCode::ContactFailure,
    keepalive_interval * 2,   // <-- used as a graceful shutdown timeout
    ...
).await;
```

When a critical unsupported extension is detected, the connection is terminated gracefully using `keepalive_interval * 2` as the timeout — a reasonable upper bound to wait for the `SESS_TERM` exchange.

### 3. Passed to `session::Session::new` (lines 326–341)

```rust
if keepalive_interval != 0 {
    Some(tokio::time::Duration::from_secs(keepalive_interval as u64))
} else {
    None
}
```

The negotiated interval is converted to a `Duration` (or `None` if disabled) and handed to the `Session`, which uses it internally to send `KEEPALIVE` messages and to detect dead connections when no traffic is received within the interval.

---

## Summary flow

```
Config (keepalive_interval)
    ↓
Sent in SESS_INIT  →  min(ours, peer's)  =  negotiated_interval
                                              ↓
                              session::Session uses it to:
                              • send periodic KEEPALIVE frames
                              • timeout/terminate dead connections
                              Also used as shutdown timeout (×2)
```
