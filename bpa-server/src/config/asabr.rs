use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::default_config_dir;

fn default_protocol_id() -> String {
    "asabr".to_string()
}

fn default_router() -> String {
    "SpsnHybridParenting".to_string()
}

fn default_contact_plan_path() -> PathBuf {
    default_config_dir().join("asabr.cp")
}

fn default_local_node_id() -> hardy_bpv7::eid::NodeId {
    "ipn:1.0".parse().unwrap()
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default, rename_all = "kebab-case")]
pub struct Config {
    pub protocol_id: String,
    pub router: String,
    pub contact_plan_path: PathBuf,
    pub local_node_id: hardy_bpv7::eid::NodeId,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            protocol_id: default_protocol_id(),
            router: default_router(),
            contact_plan_path: default_contact_plan_path(),
            local_node_id: default_local_node_id(),
        }
    }
}

impl From<&Config> for hardy_asabr_routing::Config {
    fn from(config: &Config) -> Self {
        Self {
            protocol_id: config.protocol_id.clone(),
            router: config.router.clone(),
            contact_plan_path: config.contact_plan_path.clone(),
            local_node_id: config.local_node_id.clone(),
        }
    }
}
