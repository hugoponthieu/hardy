use cspcl_bindings::InboundStream;
use futures_util::TryStreamExt;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tracing::warn;

use hardy_bpa::{
    Bytes,
    cla::{ClaAddress, CspAddress, Sink},
};
use hardy_bpv7::eid::NodeId;
use tokio::task;

#[derive(Debug, Copy, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, rename_all = "kebab-case"))]
pub struct Config {
    bundle_ack_timeout: u64,
    heartbeat_interval: u64,
    heartbeat_timeout: u64,
    initial_probe_interval: u64,
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
            bundle_ack_timeout: 3,
            heartbeat_interval: 5,
            heartbeat_timeout: 15,
            initial_probe_interval: 2,
        }
    }
}

pub struct Runtime {
    pub csp_to_endpoint: HashMap<CspAddress, NodeId>,
}

impl Runtime {
    pub fn new(csp_to_endpoint: HashMap<CspAddress, NodeId>) -> Self {
        Self { csp_to_endpoint }
    }

    pub async fn start_inbound(&self, sink: Arc<dyn Sink>, mut inbound: InboundStream) {
        let csp_to_endpoint = self.csp_to_endpoint.clone();

        let csp_to_addr_iter = self.csp_to_endpoint.iter();
        for csp_node in csp_to_addr_iter {
            let _ = sink
                .add_peer(ClaAddress::Csp(*csp_node.0), &[csp_node.1.clone()])
                .await;
        }

        task::spawn(async move {
            loop {
                let next_bundle = inbound.try_next().await;
                let bundle = match next_bundle {
                    Ok(bundle) => match bundle {
                        Some(bundle) => bundle,
                        None => continue,
                    },
                    Err(e) => {
                        warn!("Error occured when receiving bundle: {}", e.to_string());
                        continue;
                    }
                };
                let bundle_data: Bytes = bundle.data.into();
                let csp_peer_addr = CspAddress {
                    addr: bundle.src_addr,
                    port: bundle.src_port,
                };
                let node_id = csp_to_endpoint.get(&csp_peer_addr);
                let _ = sink
                    .dispatch(bundle_data, node_id, Some(&ClaAddress::Csp(csp_peer_addr)))
                    .await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn peer_timeout_is_distinct_from_heartbeat_interval() {
        let config = Config::default();

        assert_eq!(config.heartbeat_interval().as_secs(), 5);
        assert_eq!(config.heartbeat_timeout().as_secs(), 15);
    }
}
