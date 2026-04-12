use hardy_bpv7::eid::NodeId;
use std::time::Duration;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum Interface {
    Loopback,
    Can,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, rename_all = "kebab-case"))]
pub struct PeerConfig {
    pub node_id: NodeId,
    pub addr: u8,
    pub port: u8,
    pub heartbeat_interval: Option<u64>,
}

impl Default for PeerConfig {
    fn default() -> Self {
        Self {
            node_id: "ipn:1.0".parse().expect("valid default node id"),
            addr: 0,
            port: cspcl_bindings::cspcl_sys::CSPCL_PORT_BP as u8,
            heartbeat_interval: None,
        }
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, rename_all = "kebab-case"))]
pub struct Config {
    pub local_addr: u8,
    pub port: u8,
    pub interface: Interface,
    pub interface_name: String,
    pub bundle_ack_timeout: u64,
    pub heartbeat_interval: u64,
    pub heartbeat_timeout: u64,
    pub initial_probe_interval: u64,
    pub peers: Vec<PeerConfig>,
}

impl Config {
    pub fn bundle_ack_timeout(&self) -> Duration {
        Duration::from_secs(self.bundle_ack_timeout.max(1))
    }

    pub fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs(self.heartbeat_interval.max(1))
    }

    pub fn heartbeat_timeout(&self) -> Duration {
        Duration::from_secs(self.heartbeat_timeout.max(1))
    }

    pub fn initial_probe_interval(&self) -> Duration {
        Duration::from_secs(self.initial_probe_interval.max(1))
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            local_addr: 1,
            port: cspcl_bindings::cspcl_sys::CSPCL_PORT_BP as u8,
            interface: Interface::Loopback,
            interface_name: "loopback".to_string(),
            bundle_ack_timeout: 3,
            heartbeat_interval: 5,
            heartbeat_timeout: 15,
            initial_probe_interval: 2,
            peers: Vec::new(),
        }
    }
}
