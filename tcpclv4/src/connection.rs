use super::*;
use rand::seq::IteratorRandom;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    slice,
    sync::{Arc, Mutex},
};

pub type ConnectionTx = tokio::sync::mpsc::Sender<(
    hardy_bpa::Bytes,
    tokio::sync::oneshot::Sender<hardy_bpa::cla::ForwardBundleResult>,
)>;

pub struct Connection {
    pub tx: ConnectionTx,
    pub local_addr: SocketAddr,
}

struct ConnectionPoolInner {
    active: HashMap<SocketAddr, ConnectionTx>,
    idle: Vec<Connection>,
    peers: HashSet<NodeId>,
}

struct ConnectionPool {
    inner: Mutex<ConnectionPoolInner>,
    sink: Arc<dyn hardy_bpa::cla::Sink>,
    max_idle: usize,
    remote_addr: hardy_bpa::cla::ClaAddress,
}

impl ConnectionPool {
    fn new(
        conn: Connection,
        sink: Arc<dyn hardy_bpa::cla::Sink>,
        remote_addr: SocketAddr,
        max_idle: usize,
    ) -> Self {
        metrics::gauge!("tcpclv4.pool.idle").increment(1.0);
        Self {
            inner: Mutex::new(ConnectionPoolInner {
                active: HashMap::new(),
                idle: vec![conn],
                peers: HashSet::new(),
            }),
            sink,
            max_idle,
            remote_addr: hardy_bpa::cla::ClaAddress::Tcp(remote_addr),
        }
    }

    fn idle_count(&self) -> usize {
        self.inner
            .lock()
            .trace_expect("Failed to lock mutex")
            .idle
            .len()
    }

    fn add(&self, conn: Connection) {
        self.inner
            .lock()
            .trace_expect("Failed to lock mutex")
            .idle
            .push(conn);
        metrics::gauge!("tcpclv4.pool.idle").increment(1.0);
    }

    #[cfg_attr(feature = "instrument", instrument(skip(self)))]
    async fn add_peer(&self, node_id: NodeId) {
        if self
            .inner
            .lock()
            .trace_expect("Failed to lock mutex")
            .peers
            .insert(node_id.clone())
            && !self
                .sink
                .add_peer(self.remote_addr.clone(), slice::from_ref(&node_id))
                .await
                .unwrap_or_else(|e| {
                    warn!("add_peer failed: {e:?}");
                    false
                })
        {
            self.inner
                .lock()
                .trace_expect("Failed to lock mutex")
                .peers
                .remove(&node_id);
        }
    }

    async fn remove(&self, local_addr: &SocketAddr) -> bool {
        let (empty, owned_peers) = {
            let mut inner = self.inner.lock().trace_expect("Failed to lock mutex");
            inner.active.remove(local_addr);
            let before = inner.idle.len();
            inner.idle.retain(|c| &c.local_addr != local_addr);
            let removed = before - inner.idle.len();
            if removed > 0 {
                metrics::gauge!("tcpclv4.pool.idle").decrement(removed as f64);
            }

            let empty = inner.active.is_empty() && inner.idle.is_empty();
            // Only withdraw the BPA peer if THIS pool registered it. A peer that
            // was added out-of-band (e.g. a statically-configured peer) had its
            // `add_peer` rejected here as already-registered, so `peers` is empty
            // — its route must survive transient connection loss so the next
            // forward can re-dial.
            let owned_peers = if empty {
                let owned = !inner.peers.is_empty();
                inner.peers.clear();
                owned
            } else {
                false
            };
            (empty, owned_peers)
        };

        if owned_peers {
            _ = self.sink.remove_peer(&self.remote_addr).await;
        }
        empty
    }

    #[cfg_attr(feature = "instrument", instrument(skip(self, bundle)))]
    async fn try_send(
        &self,
        bundle: hardy_bpa::Bytes,
    ) -> Result<hardy_bpa::cla::ForwardBundleResult, hardy_bpa::Bytes> {
        // We repeatedly search as this function is async, so changes can happen while running
        loop {
            // Try to use an idle session
            while let Some(conn) = {
                let mut inner = self.inner.lock().trace_expect("Failed to lock mutex");
                let conn = inner.idle.pop();
                if let Some(conn) = &conn {
                    inner.active.insert(conn.local_addr, conn.tx.clone());
                    metrics::gauge!("tcpclv4.pool.idle").decrement(1.0);
                    metrics::counter!("tcpclv4.pool.reused").increment(1);
                }
                conn
            } {
                let (tx, rx) = tokio::sync::oneshot::channel();
                if conn.tx.send((bundle.clone(), tx)).await.is_ok() {
                    if let Ok(r) = rx.await {
                        let mut inner = self.inner.lock().trace_expect("Failed to lock mutex");
                        inner.active.remove(&conn.local_addr);
                        if inner.idle.len() + inner.active.len() <= self.max_idle {
                            inner.idle.push(conn);
                            metrics::gauge!("tcpclv4.pool.idle").increment(1.0);
                        }
                        return Ok(r);
                    }
                    debug!("Connection failed to transfer bundle");
                }

                // By the time we got here, conn is in a bad state
                self.inner
                    .lock()
                    .trace_expect("Failed to lock mutex")
                    .active
                    .remove(&conn.local_addr);
            }

            // Pick a random active connection and enqueue
            while let Some((local_addr, conn_tx)) = {
                self.inner
                    .lock()
                    .trace_expect("Failed to lock mutex")
                    .active
                    .iter()
                    .choose(&mut rand::rng())
                    .map(|(l, c)| (*l, c.clone()))
            } {
                let (tx, rx) = tokio::sync::oneshot::channel();
                if conn_tx.send((bundle.clone(), tx)).await.is_ok() {
                    if let Ok(r) = rx.await {
                        return Ok(r);
                    }
                    debug!("Connection failed to transfer bundle");
                }

                // By the time we got here, conn is in a bad state
                self.inner
                    .lock()
                    .trace_expect("Failed to lock mutex")
                    .active
                    .remove(&local_addr);
            }

            if self.max_idle == 0 || {
                let inner = self.inner.lock().trace_expect("Failed to lock mutex");
                inner.active.len() + inner.idle.len()
            } <= self.max_idle
            {
                // We can support more active connections
                return Err(bundle);
            }
        }
    }
}

pub struct ConnectionRegistry {
    pools: Mutex<HashMap<SocketAddr, Arc<connection::ConnectionPool>>>,
    max_idle: usize,
}

impl ConnectionRegistry {
    pub fn new(max_idle: usize) -> Self {
        Self {
            pools: Mutex::new(HashMap::new()),
            max_idle,
        }
    }

    #[cfg_attr(feature = "instrument", instrument(skip(self)))]
    pub fn shutdown(&self) {
        let mut pools = self.pools.lock().trace_expect("Failed to lock mutex");

        // Count remaining idle connections before clearing
        let idle: usize = pools.values().map(|pool| pool.idle_count()).sum();
        if idle > 0 {
            metrics::gauge!("tcpclv4.pool.idle").decrement(idle as f64);
        }

        // Closing tx channels causes session::run tasks to exit
        pools.clear();
    }

    #[cfg_attr(feature = "instrument", instrument(skip(self, sink, conn)))]
    pub async fn register_session(
        &self,
        sink: Arc<dyn hardy_bpa::cla::Sink>,
        conn: Connection,
        remote_addr: SocketAddr,
        node_id: Option<NodeId>,
    ) {
        let pool = match self
            .pools
            .lock()
            .trace_expect("Failed to lock mutex")
            .entry(remote_addr)
        {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let pool = e.get_mut();
                pool.add(conn);
                pool.clone()
            }
            std::collections::hash_map::Entry::Vacant(e) => e
                .insert(Arc::new(connection::ConnectionPool::new(
                    conn,
                    sink,
                    remote_addr,
                    self.max_idle,
                )))
                .clone(),
        };

        if let Some(node_id) = node_id {
            pool.add_peer(node_id).await
        }
    }

    #[cfg_attr(feature = "instrument", instrument(skip(self)))]
    pub async fn unregister_session(&self, local_addr: &SocketAddr, remote_addr: &SocketAddr) {
        let pool = self
            .pools
            .lock()
            .trace_expect("Failed to lock mutex")
            .get(remote_addr)
            .cloned();

        if let Some(pool) = pool
            && pool.remove(local_addr).await
        {
            let mut pools = self.pools.lock().trace_expect("Failed to lock mutex");
            if let Some(current) = pools.get(remote_addr) {
                if Arc::ptr_eq(current, &pool) {
                    pools.remove(remote_addr);
                }
            }
        }
    }

    #[cfg_attr(feature = "instrument", instrument(skip(self, bundle)))]
    pub async fn forward(
        &self,
        remote_addr: &SocketAddr,
        mut bundle: hardy_bpa::Bytes,
    ) -> Result<hardy_bpa::cla::ForwardBundleResult, hardy_bpa::Bytes> {
        let pool = self
            .pools
            .lock()
            .trace_expect("Failed to lock mutex")
            .get(remote_addr)
            .cloned();

        if let Some(pool) = pool {
            match pool.try_send(bundle).await {
                Ok(r) => return Ok(r),
                Err(b) => {
                    bundle = b;
                }
            }
        }
        Err(bundle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hardy_bpa::async_trait;
    use hardy_bpa::cla::{ClaAddress, Sink};
    use hardy_bpv7::eid::{IpnNodeId, NodeId};
    use std::sync::Mutex as StdMutex;

    // A Sink that records remove_peer calls and returns a configurable add_peer
    // result (true = this pool registered the peer; false = already registered,
    // e.g. a statically-configured peer).
    struct MockSink {
        add_peer_result: bool,
        removed: Arc<StdMutex<Vec<ClaAddress>>>,
    }

    #[async_trait]
    impl Sink for MockSink {
        async fn unregister(&self) {}

        async fn dispatch(
            &self,
            _bundle: hardy_bpa::Bytes,
            _peer_node: Option<&NodeId>,
            _peer_addr: Option<&ClaAddress>,
        ) -> hardy_bpa::cla::Result<()> {
            Ok(())
        }

        async fn add_peer(
            &self,
            _cla_addr: ClaAddress,
            _node_ids: &[NodeId],
        ) -> hardy_bpa::cla::Result<bool> {
            Ok(self.add_peer_result)
        }

        async fn remove_peer(&self, cla_addr: &ClaAddress) -> hardy_bpa::cla::Result<bool> {
            self.removed.lock().unwrap().push(cla_addr.clone());
            Ok(true)
        }
    }

    fn make_pool(
        add_peer_result: bool,
    ) -> (ConnectionPool, Arc<StdMutex<Vec<ClaAddress>>>, SocketAddr) {
        let removed = Arc::new(StdMutex::new(Vec::new()));
        let sink = Arc::new(MockSink {
            add_peer_result,
            removed: removed.clone(),
        });
        let remote_addr: SocketAddr = "[::1]:24556".parse().unwrap();
        let local_addr: SocketAddr = "[::1]:50000".parse().unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel::<(
            hardy_bpa::Bytes,
            tokio::sync::oneshot::Sender<hardy_bpa::cla::ForwardBundleResult>,
        )>(1);
        let conn = Connection { tx, local_addr };
        let pool = ConnectionPool::new(conn, sink, remote_addr, 6);
        (pool, removed, local_addr)
    }

    fn ipn(node_number: u32) -> NodeId {
        NodeId::Ipn(IpnNodeId {
            allocator_id: 0,
            node_number,
        })
    }

    // A statically-configured peer (sink reports it as already registered) must
    // NOT be withdrawn when its connection drops — the route has to persist so
    // the next forward can re-dial. Regression for: restarting the peer node
    // permanently broke forwarding because the static route was torn down.
    #[tokio::test]
    async fn static_peer_survives_connection_drop() {
        let (pool, removed, local_addr) = make_pool(false);

        // Session learns the peer, but the address is already registered statically.
        pool.add_peer(ipn(2)).await;

        // The connection drops; pool becomes empty.
        let empty = pool.remove(&local_addr).await;
        assert!(
            empty,
            "pool should report empty after its only connection drops"
        );
        assert!(
            removed.lock().unwrap().is_empty(),
            "statically-registered peer must not be withdrawn on connection drop"
        );
    }

    // A dynamically-learned peer (this pool registered it) IS withdrawn when its
    // last connection drops, so stale routes don't linger.
    #[tokio::test]
    async fn dynamic_peer_withdrawn_on_connection_drop() {
        let (pool, removed, local_addr) = make_pool(true);

        pool.add_peer(ipn(2)).await;

        let empty = pool.remove(&local_addr).await;
        assert!(empty);
        assert_eq!(
            removed.lock().unwrap().len(),
            1,
            "dynamically-registered peer should be withdrawn on connection drop"
        );
    }
}
