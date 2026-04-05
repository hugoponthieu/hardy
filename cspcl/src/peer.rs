use std::collections::HashMap;
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use hardy_bpa::cla::{ClaAddress, Sink};
use hardy_bpv7::eid::NodeId;
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerKey {
    pub remote_addr: u8,
    pub remote_port: u8,
}

#[derive(Debug, Clone)]
pub struct ResolvedPeer {
    pub node_ids: Vec<NodeId>,
    pub remote_addr: u8,
    pub remote_port: u8,
}

impl ResolvedPeer {
    pub fn cla_addr(&self) -> ClaAddress {
        ClaAddress::Csp(self.remote_addr, self.remote_port)
    }

    pub fn peer_key(&self) -> PeerKey {
        PeerKey {
            remote_addr: self.remote_addr,
            remote_port: self.remote_port,
        }
    }
}

struct PeerFlags {
    up: bool,
    registered: bool,
    last_seen: Option<Instant>,
}

impl Default for PeerFlags {
    fn default() -> Self {
        Self {
            up: false,
            registered: false,
            last_seen: None,
        }
    }
}

type ProbeWaiters = HashMap<(u8, u8), oneshot::Sender<()>>;
type AckWaiters = HashMap<(u8, [u8; 32]), oneshot::Sender<()>>;

pub struct RuntimeState {
    peers: HashMap<PeerKey, ResolvedPeer>,
    flags: Mutex<HashMap<PeerKey, PeerFlags>>,
    probe_waiters: Mutex<ProbeWaiters>,
    ack_waiters: Mutex<AckWaiters>,
    nonce_counter: AtomicU64,
}

impl RuntimeState {
    pub fn new(peers: Vec<ResolvedPeer>) -> Self {
        let mut by_peer = HashMap::with_capacity(peers.len());
        let mut flags = HashMap::with_capacity(peers.len());

        for peer in peers {
            let peer_key = peer.peer_key();
            flags.insert(peer_key.clone(), PeerFlags::default());
            by_peer.insert(peer_key, peer);
        }

        Self {
            peers: by_peer,
            flags: Mutex::new(flags),
            probe_waiters: Mutex::new(HashMap::new()),
            ack_waiters: Mutex::new(HashMap::new()),
            nonce_counter: AtomicU64::new(1),
        }
    }

    pub fn peers(&self) -> impl Iterator<Item = &ResolvedPeer> {
        self.peers.values()
    }

    pub fn peer_by_addr_only(&self, remote_addr: u8) -> Option<&ResolvedPeer> {
        self.peers.values().find(|peer| peer.remote_addr == remote_addr)
    }

    pub fn peer_by_addr(&self, remote_addr: u8, remote_port: u8) -> Option<&ResolvedPeer> {
        self.peers.get(&PeerKey {
            remote_addr,
            remote_port,
        })
    }

    pub fn next_nonce(&self) -> u64 {
        self.nonce_counter.fetch_add(1, Ordering::Relaxed)
    }

    pub fn register_probe_waiter(&self, peer: &ResolvedPeer) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.probe_waiters
            .lock()
            .expect("probe waiters lock")
            .insert((peer.remote_addr, peer.remote_port), tx);
        rx
    }

    pub fn complete_probe_waiter(&self, remote_addr: u8, remote_port: u8) {
        if let Some(tx) = self
            .probe_waiters
            .lock()
            .expect("probe waiters lock")
            .remove(&(remote_addr, remote_port))
        {
            let _ = tx.send(());
        }
    }

    pub fn remove_probe_waiter(&self, peer: &ResolvedPeer) {
        self.probe_waiters
            .lock()
            .expect("probe waiters lock")
            .remove(&(peer.remote_addr, peer.remote_port));
    }

    pub fn register_ack_waiter(
        &self,
        peer: &ResolvedPeer,
        digest: [u8; 32],
    ) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.ack_waiters
            .lock()
            .expect("ack waiters lock")
            .insert((peer.remote_addr, digest), tx);
        rx
    }

    pub fn complete_ack_waiter(&self, remote_addr: u8, digest: [u8; 32]) {
        if let Some(tx) = self
            .ack_waiters
            .lock()
            .expect("ack waiters lock")
            .remove(&(remote_addr, digest))
        {
            let _ = tx.send(());
        }
    }

    pub fn remove_ack_waiter(&self, peer: &ResolvedPeer, digest: [u8; 32]) {
        self.ack_waiters
            .lock()
            .expect("ack waiters lock")
            .remove(&(peer.remote_addr, digest));
    }

    pub fn is_up(&self, peer: &ResolvedPeer) -> bool {
        self.flags
            .lock()
            .expect("peer flags lock")
            .get(&peer.peer_key())
            .map(|flags| flags.up)
            .unwrap_or(false)
    }

    pub fn touch(&self, peer: &ResolvedPeer) {
        if let Some(flags) = self
            .flags
            .lock()
            .expect("peer flags lock")
            .get_mut(&peer.peer_key())
        {
            flags.last_seen = Some(Instant::now());
        }
    }

    pub fn is_expired(&self, peer: &ResolvedPeer, expiry: Duration) -> bool {
        self.flags
            .lock()
            .expect("peer flags lock")
            .get(&peer.peer_key())
            .and_then(|flags| flags.last_seen)
            .map(|last_seen| last_seen.elapsed() > expiry)
            .unwrap_or(true)
    }

    pub async fn mark_up(&self, sink: &std::sync::Arc<dyn Sink>, peer: &ResolvedPeer) {
        let should_add = {
            let mut flags = self.flags.lock().expect("peer flags lock");
            let Some(flags) = flags.get_mut(&peer.peer_key()) else {
                return;
            };
            flags.up = true;
            flags.last_seen = Some(Instant::now());
            if flags.registered {
                false
            } else {
                flags.registered = true;
                true
            }
        };

        if should_add {
            info!(
                "CSP peer {}:{} is verified up; registering with BPA",
                peer.remote_addr, peer.remote_port
            );
            match sink.add_peer(peer.cla_addr(), &peer.node_ids).await {
                Ok(true) | Ok(false) => {}
                Err(err) => {
                    warn!("Failed to register CSP peer {}: {err}", peer.remote_addr);
                    if let Some(flags) = self
                        .flags
                        .lock()
                        .expect("peer flags lock")
                        .get_mut(&peer.peer_key())
                    {
                        flags.registered = false;
                    }
                }
            }
        }
    }

    pub async fn mark_down(&self, sink: &std::sync::Arc<dyn Sink>, peer: &ResolvedPeer) {
        let should_remove = {
            let mut flags = self.flags.lock().expect("peer flags lock");
            let Some(flags) = flags.get_mut(&peer.peer_key()) else {
                return;
            };
            flags.up = false;
            if flags.registered {
                flags.registered = false;
                true
            } else {
                false
            }
        };

        if should_remove {
            info!(
                "CSP peer {}:{} is down; removing from BPA",
                peer.remote_addr, peer.remote_port
            );
            match sink.remove_peer(&peer.cla_addr()).await {
                Ok(true) | Ok(false) => {}
                Err(err) => {
                    debug!("Failed to remove CSP peer {}: {err}", peer.remote_addr);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ack_waiter_roundtrip() {
        let peer = ResolvedPeer {
            node_ids: Vec::new(),
            remote_addr: 2,
            remote_port: 11,
        };
        let runtime = RuntimeState::new(vec![peer.clone()]);
        let digest = [9u8; 32];
        let mut rx = runtime.register_ack_waiter(&peer, digest);
        runtime.complete_ack_waiter(2, digest);
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn peer_expires_without_touch() {
        let peer = ResolvedPeer {
            node_ids: Vec::new(),
            remote_addr: 2,
            remote_port: 11,
        };
        let runtime = RuntimeState::new(vec![peer.clone()]);
        assert!(runtime.is_expired(&peer, Duration::from_millis(1)));
        runtime.touch(&peer);
        assert!(!runtime.is_expired(&peer, Duration::from_secs(1)));
    }
}
