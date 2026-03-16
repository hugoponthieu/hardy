use cspcl::Interface;
use hardy_bpv7::eid::NodeId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub local_addr: u8,
    pub local_port: u8,
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
