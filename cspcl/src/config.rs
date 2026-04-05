use cspcl_bindings::Interface;
use hardy_bpv7::eid::NodeId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub local_addr: u8,
    pub local_port: u8,
    pub probe_interval_ms: u64,
    pub probe_timeout_ms: u64,
    pub keepalive_interval_ms: u64,
    pub peer_expiry_ms: u64,
    pub ack_timeout_ms: u64,
    pub interface: Interface,
    pub peers: Vec<PeerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PeerConfig {
    pub node_ids: Vec<NodeId>,
    pub remote_addr: u8,
    pub remote_port: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            local_addr: 0,
            local_port: 0,
            probe_interval_ms: 1_000,
            probe_timeout_ms: 500,
            keepalive_interval_ms: 1_000,
            peer_expiry_ms: 3_000,
            ack_timeout_ms: 2_000,
            interface: Interface::default(),
            peers: Vec::new(),
        }
    }
}

impl Default for PeerConfig {
    fn default() -> Self {
        Self {
            node_ids: Vec::new(),
            remote_addr: 0,
            remote_port: 0,
        }
    }
}
