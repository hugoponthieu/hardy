# TCPCLv4 Internals in Hardy

This document explains how the `tcpclv4` crate works in this repository, with emphasis on:

- when a peer becomes available to the BPA
- whether TCPCL keeps a list of available connections
- how those connections are reused and removed
- which functions manage new peer connections
- how inbound and outbound bundle transfer works once a session exists

It is implementation-focused. It describes the current Hardy code, not just RFC 9174 in the abstract.

## Scope

The main implementation lives in:

- `tcpclv4/src/cla.rs`
- `tcpclv4/src/connect.rs`
- `tcpclv4/src/context.rs`
- `tcpclv4/src/connection.rs`
- `tcpclv4/src/listen.rs`
- `tcpclv4/src/session.rs`
- `tcpclv4/src/writer.rs`
- `bpa/src/cla/registry.rs`

Useful existing companion docs in this repo:

- `tcpclv4/docs/design.md`
- `docs/cspcl-peer-liveness.md`
- `TCPCLV4_DATAFLOW_ANALYSIS.md`
- `keepalive.md`

## High-Level Model

At a high level, `tcpclv4` has five layers:

1. `Cla` is the BPA-facing convergence layer adapter.
2. `Listener` accepts inbound TCP connections.
3. `Connector` creates outbound TCP connections.
4. `ConnectionRegistry` keeps per-remote-address pools of live sessions.
5. `Session` runs the actual TCPCLv4 protocol after session establishment.

The important design point is that Hardy does not treat a configured TCP peer as automatically available. A configured address is only a connection target; live session state and BPA-visible peer state are derived from successful runtime session establishment.

## Main Components

### `Cla`

`tcpclv4::Cla` is the top-level object registered with the BPA.

Its responsibilities are:

- store configuration
- create the shared connection registry
- create shared `ConnectionContext` snapshots on demand for connection-handling paths
- start listeners
- start reconnect loops for statically configured peers
- handle BPA forwarding requests

There is one `ConnectionRegistry` per `Cla` instance. Both passive and active connection-handling paths use that same shared registry.

`Cla` is also the orchestrator for connection establishment. It starts inbound listening through `start_listeners()`, starts proactive outbound reconnect loops through `start_static_peers()`, and triggers on-demand outbound connection attempts from `forward()`.

Relevant entry points:

- `Cla::new` in `tcpclv4/src/lib.rs`
- `hardy_bpa::cla::Cla::on_register` in `tcpclv4/src/cla.rs`
- `hardy_bpa::cla::Cla::forward` in `tcpclv4/src/cla.rs`

### `ConnectionContext`

`ConnectionContext` in `tcpclv4/src/context.rs` carries the shared runtime state needed by both active and passive session setup:

- session defaults
- negotiated MRU inputs
- local node IDs
- BPA sink
- connection registry
- cancellation tokens

It also contains helper logic for:

- contact timeout
- keepalive negotiation
- passive-side contact handling

`Cla` does not keep one `ConnectionContext` per peer. Instead, `Cla::connection_context()` builds a context value from shared CLA state and clones it into listener, connector, and per-connection tasks. Peer-specific live state is stored separately in `ConnectionRegistry`, `ConnectionPool`, and `Session`.

The `registry` field inside `ConnectionContext` is the same shared `Arc<ConnectionRegistry>` cloned from `Cla`, not a separate passive-side or active-side registry.

### `Connector`

`Connector` in `tcpclv4/src/connect.rs` handles active setup:

- open TCP socket
- send and receive contact header
- exchange `SESS_INIT`
- create session writer and session loop
- register the session in the registry

### `Listener`

`Listener` in `tcpclv4/src/listen.rs` handles passive setup:

- bind TCP listener
- rate-limit accepts
- spawn one task per inbound TCP connection
- hand accepted sockets to `ConnectionContext::new_contact`

This is where the listen logic lives. `Listener` owns socket bind/accept behavior; `ConnectionRegistry` does not.

### `ConnectionRegistry`

`ConnectionRegistry` in `tcpclv4/src/connection.rs` is the main session pool.

It maps:

- remote TCP socket address

to:

- one `ConnectionPool`

Each `ConnectionPool` tracks:

- `idle` connections
- `active` connections
- `peers`, a set of remote `NodeId`s observed for that remote address and still tracked locally

`ConnectionRegistry` intentionally has no listen logic and no dial logic. It does not bind sockets, accept inbound TCP streams, or decide when to reconnect. Its job is to store, reuse, and remove established sessions once the listener or connector hands them off.

### `Session`

`Session` in `tcpclv4/src/session.rs` runs the established TCPCLv4 session:

- receive inbound protocol messages
- reassemble inbound transfers
- dispatch received bundles to the BPA
- send outbound bundles to the peer
- track acknowledgements
- manage shutdown and termination handling

### `SessionWriter`

`SessionWriter` in `tcpclv4/src/writer.rs` is split from the main session loop so keepalives can still be sent even if bundle dispatch blocks. This is an important design detail: receive-side delivery into the BPA may block, but the TCPCL session should still stay alive.

## Lifecycle Overview

There are two ways a session appears:

- passive: a remote peer connects to us
- active: we connect to a remote peer

Once setup is complete, both sides converge to the same `Session` runtime behavior.

## Passive Session Flow

The passive path is:

1. `Listener::listen` accepts a TCP socket.
2. It spawns a task calling `ConnectionContext::new_contact`.
3. `new_contact` reads the 6-byte contact header.
4. It validates the `dtn!` magic.
5. It sends our contact header back.
6. It rejects unsupported protocol versions if needed.
7. It enters `new_passive`.
8. `new_passive` waits for the peer `SESS_INIT`.
9. It selects the most appropriate local node ID to report back.
10. It sends our `SESS_INIT`.
11. It negotiates keepalive and validates unsupported critical extensions.
12. It creates the per-session forwarding channel and writer task.
13. It builds a `Session`.
14. It calls `ConnectionRegistry::register_session(...)`.
15. It runs `session.run().await`.
16. When the session ends, it calls `ConnectionRegistry::unregister_session(...)`.

The key functions on the passive side are:

- `Listener::listen`
- `ConnectionContext::new_contact`
- `ConnectionContext::new_passive`

## Active Session Flow

The active path is:

1. A bundle needs forwarding and `Cla::forward` finds no usable existing session.
2. Or a static peer reconnect loop in `start_static_peers` decides there is no session and attempts one.
3. `Connector::connect` opens a TCP socket.
4. It sends our contact header.
5. It reads the peer contact header.
6. It validates version and handles special v3 shutdown compatibility.
7. It enters `new_active`.
8. `new_active` sends our `SESS_INIT`.
9. It waits for the peer `SESS_INIT`.
10. It negotiates keepalive and validates unsupported critical extensions.
11. It creates the per-session forwarding channel and writer task.
12. It builds a `Session`.
13. It spawns a background task that:
    - registers the session
    - runs the session
    - unregisters the session on exit

The key functions on the active side are:

- `Cla::forward`
- `Connector::connect`
- `Connector::new_active`

## When Is a Peer Considered Available?

This is the most important behavioral point.

There is no explicit `joinable` flag or `peer_state = Joinable` enum in this code.

The implementation has two different notions of availability:

- session availability inside `tcpclv4`
- peer visibility inside the BPA

`ConnectionRegistry::register_session(...)` inserts the session into the per-address pool first. That means `Cla::forward` can reuse the session by remote `SocketAddr` as soon as registration into the connection registry has happened.

If a remote `NodeId` was learned from `SESS_INIT`, `register_session(...)` then asks the pool to announce that node via:

- `sink.add_peer(ClaAddress::Tcp(remote_addr), &[node_id])`

On the BPA side, `bpa/src/cla/registry.rs` turns a successful `add_peer(...)` into:

- a new CLA peer entry
- queue poller startup
- routing entries in the RIB for the provided `NodeId`

So, in practical terms:

- a configured address is only a candidate for connection attempts
- a registered session is usable by the TCPCL CLA for forwarding by `SocketAddr`
- a BPA-visible peer exists only after `add_peer(...)` succeeds

Those are related, but they are not the same state.

## Static Peers vs Live Peers

`Config.peers` in `tcpclv4/src/config.rs` is only a list of candidate remote addresses for proactive connection attempts.

It is not itself the list of available peers.

The live peer state is instead distributed across two places:

- `tcpclv4::ConnectionRegistry`, keyed by remote socket address
- BPA CLA registry state, keyed by `ClaAddress`, after `add_peer(...)`

This distinction matters:

- configured peer: we may try to connect to it
- live session: there is a negotiated TCPCL session in the connection pool
- BPA-visible peer: the BPA can route bundles to it

## Does TCPCL Keep a List of Available Connections?

Yes.

The list is the `ConnectionRegistry` in `tcpclv4/src/connection.rs`.

Its top-level structure is:

- `HashMap<SocketAddr, Arc<ConnectionPool>>`

Each `ConnectionPool` is the set of known sessions for one remote TCP address.

Inside a pool:

- `active: HashMap<SocketAddr, ConnectionTx>`
- `idle: Vec<Connection>`

The `SocketAddr` key inside `active` is the local socket address for that specific TCP connection. That lets the pool distinguish multiple simultaneous sessions to the same remote peer.

So the implementation keeps:

- a pool per remote peer address
- multiple sessions per remote peer
- each session either idle or active

One subtlety matters here: the pool can remember multiple `NodeId`s locally, but BPA peer registration is keyed by `ClaAddress::Tcp(remote_addr)`. In practice the BPA creates at most one peer entry per TCP remote address.

## How Forwarding Uses the Pool

When the BPA asks the CLA to forward a bundle:

1. `Cla::forward` calls `registry.forward(remote_addr, bundle)`.
2. `ConnectionRegistry::forward` looks up the pool for that remote address.
3. `ConnectionPool::try_send` tries to use an existing session.

`try_send` behavior is:

1. Prefer an idle connection.
2. Move that connection from `idle` to `active`.
3. Send the bundle over the connection's channel to the session task.
4. Wait for a `ForwardBundleResult`.
5. If the transfer succeeded, move the connection back out of `active`.
6. If the pool still fits within the configured limit, return it to `idle`.
7. If not, do not reinsert it, effectively dropping that connection from the idle pool.

If there is no idle connection:

- and the pool is below capacity, `try_send` returns `Err(bundle)` so the caller can create a new connection
- otherwise it picks a random active connection and enqueues onto it

This is the core reuse logic.

## How Connections Are Evicted or Removed

There are several different removal paths.

### 1. Session teardown

Every session eventually exits `session.run()`.

Both passive and active setup paths call:

- `ConnectionRegistry::unregister_session(local_addr, remote_addr)`

That removes the specific connection from:

- `active`
- `idle`

If the pool becomes empty after that, `ConnectionPool::remove` also calls:

- `sink.remove_peer(&ClaAddress::Tcp(remote_addr))`

and the whole remote-address pool is removed from the registry.

This is the main peer-liveness removal path.

### 2. Idle pool shrink after successful use

After a connection is used successfully, `try_send` may or may not put it back into `idle`.

The rule is:

- if `idle.len() + active.len() <= max_idle`, keep it in `idle`
- otherwise drop it from the idle list

This is how excess connections are naturally trimmed after use.

### 3. Failed send path

If sending on a connection channel fails, or the response channel fails, `try_send` assumes the connection is bad and removes it from `active`.

That does not directly remove the pool entry from the top-level registry. Full removal happens when the session task exits and unregisters, or when all sessions have disappeared.

### 4. CLA shutdown

`ConnectionRegistry::shutdown` clears all pools.

This closes the per-session channels and causes session tasks to wind down.

## Important Note About `max_idle_connections`

The configuration name says "idle connections", but in `ConnectionPool::try_send` it also acts as the threshold used to decide whether another connection may be created at all.

Specifically, new connection creation is allowed when:

- `max_idle == 0`, or
- `active.len() + idle.len() <= max_idle`

Otherwise the implementation queues onto a random active connection instead of asking the caller to create a new one.

So in the current implementation, `max_idle_connections` behaves like:

- an idle-pool cap
- and also a practical pool-size cap for deciding whether to grow the number of sessions

That is worth knowing because it is slightly broader than the config name alone suggests.

## Which Function Manages New Peer Connections?

There is not one single function. The responsibility is split by role.

### Passive-side new connection handling

The passive-side chain is:

- `Listener::listen`
- `ConnectionContext::new_contact`
- `ConnectionContext::new_passive`

This is the path that accepts and negotiates a brand-new inbound peer connection.

If you want the best answer to "what function handles a new inbound peer connection?", it is:

- `ConnectionContext::new_contact` for contact/header handling
- `ConnectionContext::new_passive` for session setup

### Active-side new connection handling

The active-side chain is:

- `Connector::connect`
- `Connector::new_active`

If you want the best answer to "what function creates a new outbound peer connection?", it is:

- `Connector::connect`

### Registry integration point

The function that makes a newly established session visible to the rest of the CLA and BPA is:

- `ConnectionRegistry::register_session`

That is the point where:

- the session is inserted into the per-address pool
- the remote `NodeId` is optionally announced to the BPA with `add_peer`

Those two effects are separate. A session can exist in the pool even if BPA peer registration does not happen or fails.

This is also where the separation of responsibilities shows up clearly: `ConnectionContext` is shared setup/runtime state, while per-peer and per-session liveness is tracked in the registry and session objects.

The same shared registry is used on both sides:

- passive setup calls `ConnectionRegistry::register_session(...)` and `unregister_session(...)` from `ConnectionContext::new_passive`
- active setup calls the same functions from `Connector::new_active`

So the handoff boundary is:

- `Listener` and `Connector` do network establishment and session negotiation
- `ConnectionRegistry` starts being involved when the session is ready to be pooled and tracked

## How Inbound Transfer Works

Inbound transfer means:

- remote peer -> Hardy BPA

The main function is:

- `Session::on_transfer`

The flow is:

1. `Session::run` receives an `XFER_SEGMENT`.
2. `on_transfer` checks `start` and `end` flags.
3. It appends segment data into `self.ingress_bundle`.
4. It rejects the transfer if the negotiated transfer MRU would be exceeded.
5. When the final segment arrives, it calls:
   - `sink.dispatch(bundle, peer_node, peer_addr)`
6. After each processed segment, it sends `XFER_ACK`.

This means the remote sender only gets an acknowledgement after the segment has been processed locally. For the final segment, processing includes dispatching the complete bundle into the BPA.

For the exact function-by-function path, see the next section.

## Full Function Trace: Hardy Receives a Bundle from a Peer

This section traces the full inbound path for:

- remote TCPCL peer -> `tcpclv4` -> Hardy BPA

The first important point is that Hardy does not need an existing pooled connection before it can receive a bundle. Inbound receive starts from the listener accepting a brand-new TCP connection. The session is added to `ConnectionRegistry` later, after passive session establishment succeeds.

### 1. `Cla` starts the passive listener

The passive path begins in:

- `Cla::start_listeners()`

That function constructs:

- `listen::Listener { connection_rate_limit, ctx }`

and spawns:

- `Listener::listen(...)`

So the passive receive path exists because `Cla` starts a listener task, not because the registry already has a session entry.

### 2. `Listener::listen` binds and accepts inbound sockets

`Listener::listen` is responsible for:

1. binding `TcpListener`
2. wrapping accept handling with rate limiting
3. waiting for inbound TCP connections
4. spawning one task per accepted socket

For each accepted connection it calls:

- `ctx.new_contact(stream, remote_addr).await`

This is the true start of a new inbound peer contact.

### 3. `ConnectionContext::new_contact` performs passive contact handling

`ConnectionContext::new_contact` handles the transport-level pre-session phase:

1. set `TCP_NODELAY`
2. read the 6-byte contact header
3. validate the `dtn!` magic
4. send Hardy's contact header in reply
5. reject unsupported protocol versions if needed
6. hand off to:
   - `ConnectionContext::new_passive(...)`

At this stage there is still no `ConnectionRegistry` entry for this connection. The inbound peer can reach Hardy before anything is pooled.

### 4. `ConnectionContext::new_passive` establishes the passive TCPCL session

`ConnectionContext::new_passive` handles passive session establishment:

1. wait for peer `SESS_INIT`
2. choose the most appropriate local node ID to report back
3. send local `SESS_INIT`
4. negotiate keepalive
5. reject unsupported critical session extensions
6. create the per-session forwarding channel
7. split the transport into reader/writer halves
8. create the writer task
9. build `Session`

Only after that setup does it call:

- `ConnectionRegistry::register_session(...)`

So the pool entry is a consequence of successful passive setup, not a prerequisite for receiving.

### 5. `ConnectionRegistry::register_session` pools the passive session

Once `new_passive` calls `register_session(...)`, the shared registry:

1. looks up or creates the `ConnectionPool` for `remote_addr`
2. inserts the connection into that pool
3. optionally announces the peer node ID to the BPA with `add_peer(...)`

After this point the passive session is both:

- running as the current inbound session
- available for later pooling/reuse decisions

But inbound receive did not need that pool entry to begin.

### 6. `session.run()` becomes the established receive loop

After registration, `new_passive` calls:

- `session.run().await`

Inside `Session::run`, the session waits on:

- cancellation
- outbound bundles from `from_sink`
- inbound protocol messages from the peer

For the inbound receive case, the important branch is:

- `codec::Message::TransferSegment(msg)` -> `Session::on_transfer(msg).await`

### 7. `Session::on_transfer` reassembles the inbound bundle

`Session::on_transfer` is the function that performs inbound bundle assembly.

It does the following:

1. if the segment has `start`, create `ingress_bundle`
2. reject out-of-order or unexpected segments
3. append segment bytes into `ingress_bundle`
4. reject the transfer if it would exceed `transfer_mru`
5. compute the cumulative acknowledged length

If the segment is not the final one:

- the bundle remains in `ingress_bundle`
- an `XFER_ACK` is sent for the processed segment

If the segment has `end`:

- the complete bundle is taken out of `ingress_bundle`
- Hardy calls:
  - `sink.dispatch(bundle, peer_node, peer_addr)`

### 8. BPA dispatch happens before the final `XFER_ACK`

For the final segment, the code deliberately dispatches the complete bundle into the BPA before acknowledging the segment back to the peer.

So the ordering is:

1. receive final `XFER_SEGMENT`
2. assemble the complete bundle
3. call `sink.dispatch(...)`
4. only then send `XFER_ACK`

That means the sender's acknowledgement is stronger than “the bytes arrived.” It means Hardy processed that segment fully, and for the final segment that includes handing the full bundle to the BPA.

### 9. Session teardown removes the passive session from the registry

When `session.run()` exits, `ConnectionContext::new_passive` finally calls:

- `ConnectionRegistry::unregister_session(local_addr, remote_addr)`

If that was the last session for the remote address:

- the pool entry is removed
- `sink.remove_peer(...)` is called

So the registry tracks the passive session during its lifetime, but it is not what enables the first inbound bundle to arrive.

## How Outbound Transfer Works

Outbound transfer means:

- BPA -> remote peer

The main functions are:

- `Cla::forward`
- `ConnectionRegistry::forward`
- `ConnectionPool::try_send`
- `Session::forward_to_peer`
- `Session::send_once`
- `Session::send_segment`

The flow is:

1. BPA routes a bundle to a TCP peer.
2. `Cla::forward` tries an existing session.
3. If none is usable, it tries to establish a new active session.
4. The bundle is sent through a per-session channel to the session task.
5. `forward_to_peer` allocates a transfer ID.
6. `send_once` fragments the bundle according to negotiated segment size.
7. `send_segment` writes `XFER_SEGMENT` messages.
8. The session waits for matching `XFER_ACK` responses.
9. Only when the transfer completes cleanly does it return:
   - `ForwardBundleResult::Sent`

This is a strong semantic guarantee: `Sent` means protocol-level acknowledgement from the remote TCPCL peer, not merely a successful local socket write.

If the peer refuses with `Retransmit`, the code retries.

If the peer refuses with `NoResources`, the session shuts down with resource exhaustion.

If no session can be used or established, `Cla::forward` eventually returns:

- `ForwardBundleResult::NoNeighbour`

For the exact function-by-function path, see the next section.

## Full Function Trace: Hardy Sends a Bundle to a Peer

This section traces the full outbound path for:

- Hardy BPA -> `tcpclv4::Cla` -> remote TCPCL peer

The trace starts when the BPA has already decided to forward a bundle to a TCPCL peer address.

### 1. BPA calls `Cla::forward`

The entry point is:

- `hardy_bpa::cla::Cla::forward`
- implemented by `tcpclv4::Cla::forward`

`Cla::forward` first builds a `ConnectionContext` from shared CLA state. It then checks whether the target CLA address is:

- `ClaAddress::Tcp(remote_addr)`

If so, it enters a retry loop that can run up to 5 times. The reason for the loop is simple:

- pooled sessions can disappear while forwarding is in progress
- a newly established active session may only become usable on the next iteration

### 2. `Cla::forward` tries the shared `ConnectionRegistry`

The first concrete forwarding attempt is:

- `self.registry.forward(remote_addr, bundle).await`

That calls:

- `ConnectionRegistry::forward`

`ConnectionRegistry::forward` is only a lookup and handoff layer. It:

1. looks up the `ConnectionPool` for `remote_addr`
2. if a pool exists, calls `ConnectionPool::try_send(bundle)`
3. if no pool exists, returns `Err(bundle)` back to `Cla::forward`

At this stage there is no new connection establishment yet. This is only the reuse path.

### 3. `ConnectionPool::try_send` decides reuse vs growth

`ConnectionPool::try_send` is the core decision point for an already-known remote address.

It tries the following, in order:

1. Pop an idle connection from `idle`.
2. Move it into `active`.
3. Send the bundle over that session's channel.
4. Wait for the per-transfer result from the session task.

If that succeeds:

- the connection is removed from `active`
- if the pool is still within the configured threshold, it is returned to `idle`
- the result is returned back up the stack

If that idle connection is broken:

- it is removed from `active`
- the pool keeps searching

If no idle connection is usable, `try_send` chooses between:

- return `Err(bundle)` to tell the caller that another active connection may be created
- or enqueue onto a random already-active connection if the pool is already at its growth threshold

So `ConnectionPool::try_send` does not open sockets itself. It only decides whether:

- an existing session can carry the bundle
- or `Cla::forward` must try to establish another active session

### 4. If reuse fails, `Cla::forward` creates a `Connector`

When `ConnectionRegistry::forward` returns `Err(bundle)`, `Cla::forward` does:

- construct `connect::Connector { tasks, ctx }`
- call `Connector::connect(remote_addr).await`

`Connector::connect` is responsible for:

- opening the TCP socket
- exchanging the contact header
- validating the remote protocol version
- handing off to `Connector::new_active`

At this point the bundle is still not sent. The goal of this step is to create a usable session and register it.

### 5. `Connector::new_active` establishes the active TCPCL session

`Connector::new_active` performs the session-establishment part of the active path:

1. send local `SESS_INIT`
2. wait for peer `SESS_INIT`
3. negotiate keepalive
4. validate unsupported critical session extensions
5. create the per-session forwarding channel
6. create the split writer/reader runtime
7. build `Session`

Then it spawns background tasks that:

1. call `ConnectionRegistry::register_session(...)`
2. run `session.run().await`
3. call `ConnectionRegistry::unregister_session(...)` when the session exits

This is the important handoff:

- `Connector` establishes the session
- `ConnectionRegistry` starts tracking it only after establishment has succeeded enough for pooling

### 6. The bundle is usually sent on a later `Cla::forward` loop iteration

`Connector::connect` returning `Ok(())` does not itself mean the bundle was transferred.

What it means is:

- a session was established
- that session was or will be registered in the shared pool

`Cla::forward` then loops again and retries:

- `self.registry.forward(remote_addr, bundle).await`

On that next iteration, the newly registered session can now be selected by `ConnectionPool::try_send`.

### 7. The session task receives the bundle

Once `ConnectionPool::try_send` successfully sends the bundle through the per-session channel, the bundle arrives at the session task's receiver:

- `from_sink`

Inside `Session::run`, receiving from that channel leads to:

- `Session::forward_to_peer(bundle, result).await`

This is the point where actual TCPCL transfer begins.

### 8. `Session::forward_to_peer` manages transfer-level success and retries

`Session::forward_to_peer` first checks whether another transfer ID can be allocated safely.

Then it repeatedly calls:

- `Session::send_once(bundle.clone()).await`

The loop handles transfer outcomes as follows:

- success or `TransferRefuseReasonCode::Completed` => return `ForwardBundleResult::Sent`
- `TransferRefuseReasonCode::Retransmit` => retry the whole transfer
- `TransferRefuseReasonCode::NoResources` => shut the session down with resource exhaustion
- other abnormal outcomes => shut the session down

So `forward_to_peer` is the function that turns protocol-level transfer outcomes into the forwarding result seen by `ConnectionPool::try_send` and then `Cla::forward`.

### 9. `Session::send_once` fragments the bundle if needed

`Session::send_once` allocates one transfer ID for the whole bundle and then:

1. splits the bundle into segments if it exceeds the negotiated segment size
2. calls `Session::send_segment(...)` for each segment
3. marks the first segment with `start`
4. marks the last segment with `end`

All segments for the transfer share the same transfer ID.

### 10. `Session::send_segment` writes protocol messages and waits for peer responses

`Session::send_segment` does the lowest-level outbound transfer work:

1. queue the expected acknowledgement metadata in `self.acks`
2. write `XFER_SEGMENT` through the writer task
3. flush on the last segment
4. wait for peer responses

While waiting, it can process:

- `XFER_ACK`
- `XFER_REFUSE`
- inbound `XFER_SEGMENT`
- `SESS_TERM`
- unexpected messages

This is important because an outbound transfer is not a simple write-only path. While waiting for an acknowledgement, the session may still need to:

- handle inbound traffic
- react to peer termination
- reject malformed or unexpected protocol behavior

### 11. What `ForwardBundleResult::Sent` actually means

The successful terminal path is:

- `Session::forward_to_peer` sends `ForwardBundleResult::Sent`
- `ConnectionPool::try_send` returns that result
- `ConnectionRegistry::forward` returns it
- `Cla::forward` returns it to the BPA

In this implementation, `ForwardBundleResult::Sent` means:

- the remote TCPCL peer acknowledged the transfer at the protocol level

It does not mean merely:

- the socket write succeeded
- the session existed
- the bundle was queued locally

That is why the result is stronger than a plain transport write success.

## Keepalive and Liveness

Keepalive is negotiated from the two peers' advertised values in `SESS_INIT`.

The rule in `ConnectionContext::negotiate_keepalive` is:

- if we configured keepalive, use `min(our_value, peer_value)`
- otherwise disable keepalive

After setup:

- `SessionWriter` sends `KEEPALIVE` messages when idle
- `Session::run` and `recv_from_peer` treat lack of traffic for `2 * keepalive_interval` as an idle timeout

If negotiated keepalive is zero, that timeout path is disabled and the read side waits indefinitely for peer traffic.

If the peer goes silent beyond that timeout, the session shuts down and later unregisters from the connection registry. Once the last session for that remote address is gone, the BPA peer is removed.

So liveness is session-driven, not configuration-driven.

## Shutdown and Termination

Shutdown logic is split across:

- `Session::shutdown`
- `Session::on_terminate`
- `transport::terminate`

Important behaviors:

- outbound queue is closed before shutdown finishes
- a `SESS_TERM` handshake is attempted unless cancellation already happened
- simultaneous termination is handled
- if the peer starts a new transfer during `Ending`, the session can refuse it with `SessionTerminating`
- codec garbage generally leads to session shutdown
- plain hangup closes the session without keeping the pool entry alive
- active-side session and writer tasks are spawned via the CLA task pool
- passive-side session runs in the listener-spawned task, while its writer task is detached with `tokio::spawn`

The cleanup invariant is simple:

- when the session task ends, it unregisters

## BPA-Side Peer Semantics

On the BPA side, `Sink::add_peer` and `Sink::remove_peer` are implemented in `bpa/src/cla/registry.rs`.

When TCPCL adds a peer:

- BPA creates a CLA peer object
- starts its egress queue pollers
- inserts routing entries for each provided `NodeId`
- keys that peer by `ClaAddress`, so one TCP remote address maps to at most one BPA peer entry

When TCPCL removes a peer:

- BPA removes the CLA peer object
- stops forwarding via that peer
- removes the related routing entries

This is why the peer only becomes routable after TCPCL session establishment.

## Answers to the Original Questions

### 1. How is a peer considered "joinable"?

There is no explicit "joinable" state in the code.

The closest concrete meaning is:

- the peer has an established TCPCLv4 session
- that session has been registered in `ConnectionRegistry`
- optionally, the peer's `NodeId` has been announced to the BPA through `sink.add_peer(...)`

That makes the session reusable by the CLA. The BPA only sees the peer as routable after `add_peer(...)` succeeds.

### 2. Does the CL keep a list of available connections?

Yes.

It keeps them in `ConnectionRegistry`, which maps remote socket addresses to `ConnectionPool`s. Each pool tracks active and idle sessions for that remote address.

### 3. How are available connections evicted?

Connections are removed by:

- session exit followed by `unregister_session`
- failure during send
- idle-pool trimming after a successful transfer when the pool is above the configured limit
- CLA shutdown

The BPA peer is removed only when the last session for that remote address disappears.

### 4. What function manages a new peer connection?

For inbound connections:

- `ConnectionContext::new_contact`
- `ConnectionContext::new_passive`

For outbound connections:

- `Connector::connect`
- `Connector::new_active`

For making the session visible to the rest of the system:

- `ConnectionRegistry::register_session`

### 5. Is configuration itself enough to make the peer available?

No.

`Config.peers` only drives proactive connection attempts. It does not directly register peers with the BPA.

### 6. Does the CLA store one `ConnectionContext` per peer, passive or active?

No.

`ConnectionContext` is not a per-peer object. `Cla::connection_context()` creates a reusable snapshot of shared CLA runtime state, and that value is cloned into active and passive connection-handling paths. Peer-specific live state is stored in `ConnectionRegistry`, `ConnectionPool`, and `Session`.

### 7. Is the same connection registry used by passive and active connections?

Yes.

`Cla` owns one shared `ConnectionRegistry`. `Cla::connection_context()` clones that same registry into `ConnectionContext`, and both passive and active session setup paths register and unregister sessions through it.

### 8. Why does `ConnectionRegistry` have no listen logic?

Because listening and session storage are separate responsibilities in this design.

`Listener` binds and accepts inbound TCP connections. `Connector` opens outbound TCP connections. `Cla` decides when those paths run. `ConnectionRegistry` is only the shared live-session table used after those paths produce a session that can be registered.

### 9. When is a bundle considered sent?

Only after the remote TCPCL peer acknowledges the transfer with `XFER_ACK` through the logic in `Session::forward_to_peer` and `send_segment`.

### 10. What is the exact function trace when Hardy sends a bundle to a peer?

See:

- `Full Function Trace: Hardy Sends a Bundle to a Peer`

That section gives the full outbound call path from `Cla::forward` through registry reuse or active session establishment and down into `Session::forward_to_peer`, `send_once`, and `send_segment`.

### 11. Do we need an open connection in the connection pool before Hardy can receive a bundle?

No.

Inbound receive starts when `Listener::listen` accepts a brand-new TCP connection and hands it to `ConnectionContext::new_contact`. The passive session is only added to `ConnectionRegistry` later, during `ConnectionContext::new_passive`, after contact handling and `SESS_INIT` negotiation succeed.

## Practical Mental Model

The easiest correct mental model is:

- `Config.peers` is a wish list
- `ConnectionContext` is shared CLA runtime context, not a per-peer record
- `ConnectionRegistry` is the shared live session table for both passive and active sessions
- `Listener` and `Connector` create connections; `ConnectionRegistry` only tracks them after establishment
- `sink.add_peer` makes a remote address routable by the BPA
- `Session::run` is the real protocol engine
- `unregister_session` plus `sink.remove_peer` removes liveness when sessions disappear

Or in one sentence:

TCPCL peer availability in Hardy is driven by negotiated session existence, not by configuration.
