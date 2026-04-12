use crate::config;
use hardy_async::sync::spin::Mutex;
use hardy_bpa::cla::CspAddress;
use hardy_bpv7::eid::NodeId;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct PeerSnapshot {
    pub node_id: NodeId,
    pub address: CspAddress,
    pub live: bool,
}

#[derive(Clone)]
pub struct PeerDown {
    pub node_id: NodeId,
    pub address: CspAddress,
}

#[derive(Clone)]
struct PeerState {
    node_id: NodeId,
    address: CspAddress,
    heartbeat_interval: Option<Duration>,
    live: bool,
    announced: bool,
    last_seen: Instant,
}

#[derive(Default)]
struct RegistryState {
    peers: HashMap<CspAddress, PeerState>,
}

pub struct Registry {
    state: Mutex<RegistryState>,
}

impl Registry {
    pub fn new(peers: &[config::PeerConfig]) -> Self {
        let now = Instant::now();
        let mut state = RegistryState::default();

        for peer in peers {
            let address = CspAddress {
                addr: peer.addr,
                port: peer.port,
            };
            state.peers.insert(
                address,
                PeerState {
                    node_id: peer.node_id.clone(),
                    address,
                    heartbeat_interval: peer.heartbeat_interval.map(Duration::from_secs),
                    live: false,
                    announced: false,
                    last_seen: now,
                },
            );
        }

        Self {
            state: Mutex::new(state),
        }
    }

    pub fn snapshot(&self, addr: CspAddress) -> Option<PeerSnapshot> {
        let guard = self.state.lock();
        guard.peers.get(&addr).map(|p| PeerSnapshot {
            node_id: p.node_id.clone(),
            address: p.address,
            live: p.live,
        })
    }

    pub fn is_live(&self, addr: CspAddress) -> bool {
        self.state
            .lock()
            .peers
            .get(&addr)
            .map(|p| p.live)
            .unwrap_or(false)
    }

    pub fn mark_live(&self, addr: CspAddress) -> Option<PeerSnapshot> {
        let mut guard = self.state.lock();
        let peer = guard.peers.get_mut(&addr)?;
        peer.last_seen = Instant::now();
        peer.live = true;

        if peer.announced {
            return None;
        }

        peer.announced = true;
        Some(PeerSnapshot {
            node_id: peer.node_id.clone(),
            address: peer.address,
            live: true,
        })
    }

    pub fn mark_down(&self, addr: CspAddress) -> Option<PeerDown> {
        let mut guard = self.state.lock();
        let peer = guard.peers.get_mut(&addr)?;
        peer.live = false;
        if !peer.announced {
            return None;
        }

        peer.announced = false;
        Some(PeerDown {
            node_id: peer.node_id.clone(),
            address: peer.address,
        })
    }

    pub fn heartbeat_targets(&self, default_interval: Duration) -> Vec<CspAddress> {
        let now = Instant::now();
        self.state
            .lock()
            .peers
            .values()
            .filter(|p| {
                if !p.live {
                    return true;
                }
                now.duration_since(p.last_seen) >= p.heartbeat_interval.unwrap_or(default_interval)
            })
            .map(|p| p.address)
            .collect()
    }

    pub fn probe_targets(&self) -> Vec<CspAddress> {
        self.state
            .lock()
            .peers
            .values()
            .filter(|p| !p.live)
            .map(|p| p.address)
            .collect()
    }

    pub fn timed_out_peers(&self, timeout: Duration) -> Vec<PeerDown> {
        let now = Instant::now();
        let mut out = Vec::new();
        let mut guard = self.state.lock();
        for peer in guard.peers.values_mut() {
            if peer.live && now.duration_since(peer.last_seen) > timeout {
                peer.live = false;
                if peer.announced {
                    peer.announced = false;
                    out.push(PeerDown {
                        node_id: peer.node_id.clone(),
                        address: peer.address,
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_lifecycle_transitions() {
        let node_id: NodeId = "ipn:2.0".parse().unwrap();
        let registry = Registry::new(&[config::PeerConfig {
            node_id,
            addr: 22,
            port: 10,
            heartbeat_interval: None,
        }]);
        let addr = CspAddress { addr: 22, port: 10 };

        assert!(!registry.is_live(addr));
        assert!(registry.mark_live(addr).is_some());
        assert!(registry.is_live(addr));
        assert!(registry.mark_live(addr).is_none());

        let down = registry.mark_down(addr).expect("peer should go down");
        assert_eq!(down.address, addr);
        assert!(!registry.is_live(addr));
    }
}
