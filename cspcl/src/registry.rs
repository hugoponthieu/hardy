use crate::config;
use hardy_async::sync::spin::Mutex;
use hardy_bpa::cla::CspAddress;
use hardy_bpv7::eid::NodeId;
use std::collections::HashMap;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerSource {
    Configured,
    Discovered,
}

#[derive(Clone)]
pub struct PeerSnapshot {
    pub node_id: NodeId,
    pub address: CspAddress,
    pub source: PeerSource,
    pub announced: bool,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct PeerDown {
    pub address: CspAddress,
}

#[derive(Clone)]
struct PeerState {
    node_id: NodeId,
    address: CspAddress,
    source: PeerSource,
    announced: bool,
    usable: bool,
    last_activity: Option<Instant>,
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
        let mut state = RegistryState::default();

        for peer in peers {
            let address = CspAddress {
                addr: peer.addr,
                port: peer.port,
            };
            state.peers.insert(
                address,
                PeerState {
                    node_id: peer
                        .node_id
                        .clone()
                        .unwrap_or_else(|| default_node_id(address.addr)),
                    address,
                    source: PeerSource::Configured,
                    announced: false,
                    usable: true,
                    last_activity: None,
                },
            );
        }

        Self {
            state: Mutex::new(state),
        }
    }

    #[allow(dead_code)]
    pub fn snapshot(&self, addr: CspAddress) -> Option<PeerSnapshot> {
        self.state.lock().peers.get(&addr).map(PeerSnapshot::from)
    }

    pub fn resolve_or_discover(&self, addr: CspAddress) -> PeerSnapshot {
        let mut guard = self.state.lock();
        let peer = guard.peers.entry(addr).or_insert_with(|| PeerState {
            node_id: default_node_id(addr.addr),
            address: addr,
            source: PeerSource::Discovered,
            announced: false,
            usable: true,
            last_activity: None,
        });
        peer.last_activity = Some(Instant::now());
        peer.usable = true;
        PeerSnapshot::from(&*peer)
    }

    pub fn bootstrap_peers(&self) -> Vec<PeerSnapshot> {
        self.state
            .lock()
            .peers
            .values()
            .filter(|peer| peer.source == PeerSource::Configured && peer.usable)
            .map(PeerSnapshot::from)
            .collect()
    }

    pub fn mark_announced(&self, addr: CspAddress) -> bool {
        let mut guard = self.state.lock();
        let Some(peer) = guard.peers.get_mut(&addr) else {
            return false;
        };
        if peer.announced {
            return false;
        }
        peer.announced = true;
        true
    }

    pub fn mark_down(&self, addr: CspAddress) -> Option<PeerDown> {
        let mut guard = self.state.lock();
        let peer = guard.peers.get_mut(&addr)?;
        peer.usable = false;
        if !peer.announced {
            return None;
        }

        peer.announced = false;
        Some(PeerDown {
            address: peer.address,
        })
    }

    pub fn mark_outbound_activity(&self, addr: CspAddress) {
        let mut guard = self.state.lock();
        if let Some(peer) = guard.peers.get_mut(&addr) {
            peer.last_activity = Some(Instant::now());
            peer.usable = true;
        }
    }
}

impl From<&PeerState> for PeerSnapshot {
    fn from(value: &PeerState) -> Self {
        Self {
            node_id: value.node_id.clone(),
            address: value.address,
            source: value.source,
            announced: value.announced,
        }
    }
}

fn default_node_id(addr: u8) -> NodeId {
    format!("ipn:{addr}.0")
        .parse()
        .expect("derived CSP node id should be valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_uses_configured_or_derived_identity() {
        let registry = Registry::new(&[
            config::PeerConfig {
                node_id: Some("ipn:200.0".parse().unwrap()),
                addr: 22,
                port: 10,
            },
            config::PeerConfig {
                node_id: None,
                addr: 23,
                port: 10,
            },
        ]);

        let peers = registry.bootstrap_peers();
        assert_eq!(peers.len(), 2);
        assert!(
            peers
                .iter()
                .any(|peer| peer.address == CspAddress { addr: 22, port: 10 }
                    && peer.node_id == "ipn:200.0".parse::<NodeId>().unwrap())
        );
        assert!(
            peers
                .iter()
                .any(|peer| peer.address == CspAddress { addr: 23, port: 10 }
                    && peer.node_id == "ipn:23.0".parse::<NodeId>().unwrap())
        );
    }

    #[test]
    fn snapshot_matches_exact_address() {
        let registry = Registry::new(&[
            config::PeerConfig {
                node_id: Some("ipn:2.0".parse().unwrap()),
                addr: 22,
                port: 10,
            },
            config::PeerConfig {
                node_id: Some("ipn:3.0".parse().unwrap()),
                addr: 22,
                port: 11,
            },
        ]);

        let snap = registry
            .snapshot(CspAddress { addr: 22, port: 10 })
            .expect("exact address should match");
        assert_eq!(snap.address, CspAddress { addr: 22, port: 10 });
        assert_eq!(snap.node_id, "ipn:2.0".parse().unwrap());

        let other = registry
            .snapshot(CspAddress { addr: 22, port: 11 })
            .expect("exact port variant should match");
        assert_eq!(other.node_id, "ipn:3.0".parse().unwrap());
    }

    #[test]
    fn resolve_or_discover_creates_unknown_peer() {
        let registry = Registry::new(&[]);

        let peer = registry.resolve_or_discover(CspAddress { addr: 7, port: 9 });
        assert_eq!(peer.address, CspAddress { addr: 7, port: 9 });
        assert_eq!(peer.node_id, "ipn:7.0".parse().unwrap());
        assert_eq!(peer.source, PeerSource::Discovered);
    }

    #[test]
    fn configured_peer_wins_over_discovery() {
        let registry = Registry::new(&[config::PeerConfig {
            node_id: Some("ipn:200.0".parse().unwrap()),
            addr: 22,
            port: 10,
        }]);

        let peer = registry.resolve_or_discover(CspAddress { addr: 22, port: 10 });
        assert_eq!(peer.node_id, "ipn:200.0".parse().unwrap());
        assert_eq!(peer.source, PeerSource::Configured);
    }

    #[test]
    fn mark_announced_and_down_are_single_cycle() {
        let registry = Registry::new(&[config::PeerConfig {
            node_id: Some("ipn:2.0".parse().unwrap()),
            addr: 22,
            port: 10,
        }]);
        let addr = CspAddress { addr: 22, port: 10 };

        assert!(registry.mark_announced(addr));
        assert!(!registry.mark_announced(addr));

        let down = registry
            .mark_down(addr)
            .expect("announced peer should go down");
        assert_eq!(down.address, addr);
        assert!(registry.mark_down(addr).is_none());
    }
}
