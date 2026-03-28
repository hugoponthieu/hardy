use std::sync::Arc;

use cspcl::{BundleStream, Cspcl};
use futures::StreamExt;
use hardy_async::TaskPool;
use hardy_bpa::{
    Bytes, async_trait,
    cla::{ClaAddress, ForwardBundleResult, Sink},
};
use tracing::{debug, error, info, warn};

use crate::{Cla, ClaInner};

impl ClaInner {
    fn start_listener(&self, tasks: &Arc<TaskPool>, mut stream: BundleStream) {
        let cancel = tasks.cancel_token().clone();
        let peers = Arc::clone(&self.peers);
        let sink = Arc::clone(&self.sink);

        tasks.spawn(async move {
            loop {
                let next_bundle = tokio::select! {
                    _ = cancel.cancelled() => break,
                    next_bundle = stream.next() => next_bundle,
                };

                let Some(bundle_result) = next_bundle else {
                    warn!("No bundle");
                    continue;
                };

                let (payload, src_addr, src_port) = match bundle_result {
                    Ok(bundle) => bundle,
                    Err(e) => {
                        warn!("Bundle receive failed: {e}");
                        continue;
                    }
                };

                debug!("Got bundle for addr {src_addr} and port {src_port}");
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
            Ok(cspcl) => cspcl,
            Err(e) => {
                error!("Failed to initialize CSPCL instance: {e}");
                return;
            }
        };

        let (cspcl_sender, bundle_stream) = cspcl.into_bundle_driver().start();

        self.inner.call_once(|| ClaInner {
            sink: sink.into(),
            node_ids: node_ids.into(),
            cspcl_sender,
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

            cla_inner.start_listener(&self.tasks, bundle_stream);
        };
    }

    async fn on_unregister(&self) {
        if let Some(inner) = self.inner.get() {
            if let Err(err) = inner.cspcl_sender.shutdown() {
                debug!("CSP driver already stopped during unregister: {err}");
            }
        }

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

            match inner
                .cspcl_sender
                .send_bundle(bundle.to_vec(), *remote_addr, *remote_port)
                .await
            {
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
