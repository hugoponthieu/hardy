use cspcl::Interface;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
// #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
// #[cfg_attr(feature = "serde", serde(default))]
pub struct Config {
    pub local_addr: u8,
    pub local_port: u8,
    pub interface: Interface,
}
