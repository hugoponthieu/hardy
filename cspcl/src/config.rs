use hardy_bpv7::eid::NodeId;

use crate::runtime;

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
    pub node_id: Option<NodeId>,
    pub addr: u8,
    pub port: u8,
}

impl Default for PeerConfig {
    fn default() -> Self {
        Self {
            node_id: None,
            addr: 0,
            port: cspcl_bindings::cspcl_sys::CSPCL_PORT_BP as u8,
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
    pub peers: Vec<PeerConfig>,
    pub runtime_config: runtime::Config,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            local_addr: 1,
            port: cspcl_bindings::cspcl_sys::CSPCL_PORT_BP as u8,
            interface: Interface::Loopback,
            interface_name: "loopback".to_string(),
            peers: Vec::new(),
            runtime_config: runtime::Config::default(),
        }
    }
}
