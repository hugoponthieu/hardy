use std::sync::Arc;

use cspcl::{Cspcl, cspcl_sys};
use futures::stream::StreamExt;
use hardy_async::TaskPool;
use hardy_bpa::{
    Bytes, async_trait,
    cla::{ClaAddress, ForwardBundleResult, Sink},
};
use tokio::sync::Mutex;
use tokio::task;
use tracing::{debug, error, info};

use crate::{Cla, ClaInner};

impl ClaInner {
    fn start_listener(&self, tasks: &Arc<TaskPool>) {
        let cspcl = Arc::clone(&self.cspcl);
        let sink = Arc::clone(&self.sink);
        let peers = Arc::clone(&self.peers);
        tasks.spawn(async move {
            let cspcl = Arc::clone(&cspcl);
            let mut guard = cspcl.lock().await;
            let bundle_stream = guard.bundle_stream().await;
            while let bundle_option = bundle_stream.next().await {
                let bundle_result = match bundle_option {
                    Some(full_bundle) => full_bundle,
                    None => continue,
                };
                let (payload, src_addr, src_port) = match bundle_result {
                    Ok(bundle) => bundle,
                    Err(_) => continue,
                };
                let peer = peers
                    .iter()
                    .find(|peer| peer.remote_addr == src_addr && peer.remote_port == src_port);
                let peer_node = peer.and_then(|peer| peer.node_ids.first());
                let peer_addr = hardy_bpa::cla::ClaAddress::Csp(src_addr, src_port);
                info!(
                    "Received bundle over CSP from {src_addr}:{src_port} ({} bytes) peer_node={:?}",
                    payload.len(),
                    peer_node
                );

                if let Err(err) = sink
                    .dispatch(payload.into(), peer_node, Some(&peer_addr))
                    .await
                {
                    error!("Failed to dispatch bundle from CSP peer {src_addr}:{src_port}: {err}");
                }
            }
        });
    }
}

#[async_trait]
impl hardy_bpa::cla::Cla for Cla {
    async fn on_register(&self, sink: Box<dyn Sink>, node_ids: &[hardy_bpv7::eid::NodeId]) {
        let cspcl = match Cspcl::new(
            self.config.local_addr,
            self.config.local_port,
            self.config.interface.clone(),
        ) {
            Ok(cspcl) => Arc::new(Mutex::new(cspcl)),
            Err(e) => {
                error!("Failed to initialize CSPCL instance: {e}");
                return;
            }
        };

        self.inner.call_once(|| ClaInner {
            sink: sink.into(),
            node_ids: node_ids.into(),
            cspcl,
            peers: self.config.peers.clone().into(),
        });

        if let Some(cla_inner) = self.inner.get() {
            for peer in &self.config.peers {
                let peer_addr = ClaAddress::Csp(peer.remote_addr, peer.remote_port);
                match cla_inner
                    .sink
                    .add_peer(peer_addr.clone(), &peer.node_ids)
                    .await
                {
                    Ok(true) => {
                        info!(
                            "Registered CSP peer {} at {}",
                            peer.node_ids
                                .first()
                                .map(ToString::to_string)
                                .unwrap_or_else(|| "<unknown>".to_string()),
                            peer_addr
                        );
                    }
                    Ok(false) => {
                        debug!("CSP peer {peer_addr} already registered");
                    }
                    Err(err) => {
                        error!("Failed to register CSP peer {peer_addr}: {err}");
                    }
                }
            }

            cla_inner.start_listener(&self.tasks);
        };
    }

    async fn on_unregister(&self) {
        self.tasks.shutdown().await;
    }

    async fn forward(
        &self,
        _queue: Option<u32>,
        cla_addr: &ClaAddress,
        bundle: Bytes,
    ) -> hardy_bpa::cla::Result<hardy_bpa::cla::ForwardBundleResult> {
        let Some(inner) = self.inner.get() else {
            error!("forward called before on_register!");
            return Err(hardy_bpa::cla::Error::Disconnected);
        };

        if let ClaAddress::Csp(remote_addr, remote_port) = cla_addr {
            info!(
                "Forwarding bundle over CSP to {remote_addr}:{remote_port} ({} bytes)",
                bundle.len()
            );

            let send_result = {
                let cspcl = Arc::clone(&inner.cspcl);
                let bundle = bundle.clone();
                let remote_addr = *remote_addr;
                let remote_port = *remote_port;
                let mut cspcl = cspcl.lock().await;
                cspcl.send_bundle(&bundle, remote_addr, remote_port)
            };

            match send_result {
                Ok(()) => {
                    debug!("Bundle sent");
                    return Ok(ForwardBundleResult::Sent);
                }
                Err(e) => {
                    error!("Could not forward bundle: {e}");
                    return Err(hardy_bpa::cla::Error::Disconnected);
                }
            }
        }
        Ok(ForwardBundleResult::NoNeighbour)
    }
}
