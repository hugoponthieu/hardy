use std::{collections::HashMap, sync::Arc, time::Duration};

use hardy_bpa::{
    Bytes,
    cla::{self, ClaAddress, CspAddress, ForwardBundleResult, Sink},
};
use hardy_bpv7::eid::NodeId;
use tracing::warn;

use crate::transport::{self, Transport};

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

#[derive(Clone)]
pub struct Runtime {
    sink: Arc<dyn Sink>,
    transport: Arc<Transport>,
    tasks: hardy_async::TaskPool,
    csp_to_endpoint: HashMap<CspAddress, NodeId>,
}

impl Runtime {
    pub fn new(
        sink: Arc<dyn Sink>,
        transport: Arc<Transport>,
        csp_to_endpoint: HashMap<CspAddress, NodeId>,
    ) -> Self {
        Self {
            sink,
            transport,
            tasks: hardy_async::TaskPool::new(),
            csp_to_endpoint,
        }
    }

    pub fn start(self: Arc<Self>) {
        self.clone().start_receive_loop();
    }
}

impl Runtime {
    pub async fn send_bundle(
        self: Arc<Self>,
        bundle: Bytes,
        csp_addr: &CspAddress,
    ) -> cla::Result<ForwardBundleResult> {
        let test = self
            .transport
            .send_bundle(bundle.clone(), csp_addr.addr, csp_addr.port)
            .await;

        match test {
            Ok(_) => Ok(ForwardBundleResult::Sent),
            Err(e) => match e {
                transport::Error::Send(_) => Ok(ForwardBundleResult::NoNeighbour),
                _ => Err(cla::Error::Internal(Box::new(e))),
            },
        }
    }

    pub async fn unregister_sink(self: Arc<Runtime>) {
        self.sink.unregister().await;
    }

    pub async fn shutdown(self: Arc<Runtime>) {
        self.tasks.shutdown().await;
        if let Err(e) = self.transport.shutdown().await {
            warn!("transport shutdown failed: {e}");
        }
    }

    pub fn start_receive_loop(self: Arc<Runtime>) {
        hardy_async::spawn!(self.clone().tasks, "cspcl_recv_loop", async move {
            self.receive_loop().await;
        });
    }

    async fn receive_loop(self: Arc<Runtime>) {
        let cancel = self.tasks.cancel_token().clone();
        while !cancel.is_cancelled() {
            let incoming = match self.transport.recv_bundle(1000).await {
                Ok(transport::ReceiveResult::Timeout) => continue,
                Ok(transport::ReceiveResult::Bundle(bundle)) => bundle,
                Err(e) => {
                    warn!("receive loop transport error: {e}");
                    continue;
                }
            };

            let inbound_peer = CspAddress {
                addr: incoming.src_addr,
                port: incoming.src_port,
            };
            let node_id = self.csp_to_endpoint.get(&inbound_peer);

            let peer_addr = ClaAddress::Csp(inbound_peer);
            match self
                .sink
                .dispatch(incoming.data.into(), node_id, Some(&peer_addr))
                .await
            {
                Ok(()) => {}
                Err(e) => {
                    warn!("dispatch failed for {:?}: {}", peer_addr, e);
                }
            }
        }
    }
}
