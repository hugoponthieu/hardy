use std::sync::Arc;

use cspcl_bindings::{Cspcl, SessionMessage, SessionStream};
use futures::StreamExt;
use hardy_async::TaskPool;
use hardy_bpa::{
    Bytes, async_trait,
    cla::{ClaAddress, ForwardBundleResult, Sink},
};
use tracing::{debug, error, info, warn};

use crate::{
    Cla, ClaInner,
    config::{Config, PeerConfig},
    control::bundle_digest,
    peer::{ResolvedPeer, RuntimeState},
};

fn resolve_peer(peer: &PeerConfig) -> ResolvedPeer {
    ResolvedPeer {
        node_ids: peer.node_ids.clone(),
        remote_addr: peer.remote_addr,
        remote_port: peer.remote_port,
    }
}

impl ClaInner {
    fn start_session_listener(&self, tasks: &Arc<TaskPool>, mut stream: SessionStream) {
        let cancel = tasks.cancel_token().clone();
        let peers = Arc::clone(&self.peers);
        let sink = Arc::clone(&self.sink);
        let cspcl_sender = self.cspcl_sender.clone();

        tasks.spawn(async move {
            loop {
                let next_message = tokio::select! {
                    _ = cancel.cancelled() => break,
                    next_message = stream.next() => next_message,
                };

                let Some(message_result) = next_message else {
                    warn!("CSP session stream ended");
                    break;
                };

                let message = match message_result {
                    Ok(message) => message,
                    Err(err) => {
                        warn!("Session receive failed: {err}");
                        continue;
                    }
                };

                match message {
                    SessionMessage::SessionInit {
                        nonce,
                        src_addr,
                        src_port,
                    } => {
                        let Some(peer) = peers.peer_by_addr_only(src_addr).cloned() else {
                            warn!("Ignoring session init from unknown CSP peer {src_addr}:{src_port}");
                            continue;
                        };

                        peers.mark_up(&sink, &peer).await;
                        peers.complete_probe_waiter(peer.remote_addr, peer.remote_port);

                        if let Err(err) = cspcl_sender
                            .send_keepalive(peer.remote_addr, peer.remote_port)
                            .await
                        {
                            warn!(
                                "Failed to send session keepalive to {}:{}: {err}",
                                peer.remote_addr, peer.remote_port
                            );
                        } else {
                            debug!(
                                "Accepted CSP session init from {src_addr}:{src_port} for configured peer {}:{} nonce={nonce}",
                                peer.remote_addr, peer.remote_port
                            );
                        }
                    }
                    SessionMessage::SessionKeepalive { src_addr, src_port } => {
                        let Some(peer) = peers.peer_by_addr_only(src_addr).cloned() else {
                            warn!("Ignoring keepalive from unknown CSP peer {src_addr}:{src_port}");
                            continue;
                        };

                        peers.mark_up(&sink, &peer).await;
                        peers.complete_probe_waiter(peer.remote_addr, peer.remote_port);
                        debug!(
                            "Received CSP keepalive from {src_addr}:{src_port} for configured peer {}:{}",
                            peer.remote_addr, peer.remote_port
                        );
                    }
                    SessionMessage::BundleData {
                        payload,
                        src_addr,
                        src_port,
                    } => {
                        let Some(peer) = peers.peer_by_addr_only(src_addr).cloned() else {
                            warn!("Ignoring bundle from unknown CSP peer {src_addr}:{src_port}");
                            continue;
                        };

                        peers.mark_up(&sink, &peer).await;

                        let peer_node = peer.node_ids.first();
                        let peer_addr = peer.cla_addr();
                        let payload: Bytes = payload.into();

                        info!(
                            "Received bundle over CSP from {src_addr}:{src_port} ({} bytes) peer_node={:?}",
                            payload.len(),
                            peer_node
                        );

                        match sink
                            .dispatch(payload.clone(), peer_node, Some(&peer_addr))
                            .await
                        {
                            Ok(()) => {
                                info!(
                                    "Dispatched bundle from CSP peer {}:{} to BPA",
                                    peer.remote_addr, peer.remote_port
                                );
                                let digest = bundle_digest(&payload);
                                if let Err(err) = cspcl_sender
                                    .send_bundle_ack(digest, peer.remote_addr, peer.remote_port)
                                    .await
                                {
                                    warn!(
                                        "Failed to send CSP bundle ack to {}:{}: {err}",
                                        peer.remote_addr, peer.remote_port
                                    );
                                } else {
                                    info!(
                                        "Sent CSP bundle ack to {}:{}",
                                        peer.remote_addr, peer.remote_port
                                    );
                                }
                            }
                            Err(err) => {
                                error!(
                                    "Failed to dispatch bundle from CSP peer {src_addr}:{src_port}: {err}"
                                );
                            }
                        }
                    }
                    SessionMessage::BundleAck {
                        digest,
                        src_addr,
                        src_port,
                    } => {
                        if let Some(peer) = peers.peer_by_addr_only(src_addr).cloned() {
                            peers.mark_up(&sink, &peer).await;
                            info!(
                                "Received CSP bundle ack from {}:{}",
                                peer.remote_addr, peer.remote_port
                            );
                        }
                        let _ = src_port;
                        peers.complete_ack_waiter(src_addr, digest);
                    }
                }
            }
        });
    }

    fn start_probe_tasks(&self, tasks: &Arc<TaskPool>, config: &Config) {
        let cancel = tasks.cancel_token().clone();
        let sink = Arc::clone(&self.sink);
        let peers = Arc::clone(&self.peers);
        let cspcl_sender = self.cspcl_sender.clone();
        let probe_interval = std::time::Duration::from_millis(config.probe_interval_ms);
        let probe_timeout = std::time::Duration::from_millis(config.probe_timeout_ms);

        for peer in peers.peers().cloned().collect::<Vec<_>>() {
            let cancel = cancel.clone();
            let sink = sink.clone();
            let peers = peers.clone();
            let cspcl_sender = cspcl_sender.clone();

            tasks.spawn(async move {
                loop {
                    if peers.is_up(&peer) {
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = tokio::time::sleep(probe_interval) => {}
                        }
                        continue;
                    }

                    let nonce = peers.next_nonce();
                    let probe_waiter = peers.register_probe_waiter(&peer);

                    debug!(
                        "Sending CSP session init to {}:{} nonce={nonce}",
                        peer.remote_addr, peer.remote_port
                    );

                    if let Err(err) = cspcl_sender
                        .send_session_init(nonce, peer.remote_addr, peer.remote_port)
                        .await
                    {
                        warn!(
                            "Session init send failed for {}:{}: {err}",
                            peer.remote_addr, peer.remote_port
                        );
                        peers.remove_probe_waiter(&peer);
                        peers.mark_down(&sink, &peer).await;
                    } else {
                        match tokio::time::timeout(probe_timeout, probe_waiter).await {
                            Ok(Ok(())) => {
                                peers.mark_up(&sink, &peer).await;
                            }
                            Ok(Err(_)) | Err(_) => {
                                peers.remove_probe_waiter(&peer);
                                peers.mark_down(&sink, &peer).await;
                            }
                        }
                    }

                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(probe_interval) => {}
                    }
                }
            });
        }
    }

    fn start_keepalive_tasks(&self, tasks: &Arc<TaskPool>, config: &Config) {
        let cancel = tasks.cancel_token().clone();
        let sink = Arc::clone(&self.sink);
        let peers = Arc::clone(&self.peers);
        let cspcl_sender = self.cspcl_sender.clone();
        let keepalive_interval = std::time::Duration::from_millis(config.keepalive_interval_ms);
        let peer_expiry = std::time::Duration::from_millis(config.peer_expiry_ms);

        for peer in peers.peers().cloned().collect::<Vec<_>>() {
            let cancel = cancel.clone();
            let sink = sink.clone();
            let peers = peers.clone();
            let cspcl_sender = cspcl_sender.clone();

            tasks.spawn(async move {
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(keepalive_interval) => {}
                    }

                    if !peers.is_up(&peer) {
                        continue;
                    }

                    if peers.is_expired(&peer, peer_expiry) {
                        peers.mark_down(&sink, &peer).await;
                        continue;
                    }

                    if let Err(err) = cspcl_sender
                        .send_keepalive(peer.remote_addr, peer.remote_port)
                        .await
                    {
                        warn!(
                            "Keepalive send failed for {}:{}: {err}",
                            peer.remote_addr, peer.remote_port
                        );
                        peers.mark_down(&sink, &peer).await;
                    }
                }
            });
        }
    }
}

#[async_trait]
impl hardy_bpa::cla::Cla for Cla {
    async fn on_register(&self, sink: Box<dyn Sink>, _node_ids: &[hardy_bpv7::eid::NodeId]) {
        let resolved_peers = self
            .config
            .peers
            .iter()
            .map(resolve_peer)
            .collect::<Vec<_>>();

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

        let (cspcl_sender, session_stream) = cspcl.into_bundle_driver().start();

        self.inner.call_once(|| ClaInner {
            sink: sink.into(),
            cspcl_sender,
            peers: Arc::new(RuntimeState::new(resolved_peers)),
        });

        if let Some(cla_inner) = self.inner.get() {
            cla_inner.start_session_listener(&self.tasks, session_stream);
            cla_inner.start_probe_tasks(&self.tasks, &self.config);
            cla_inner.start_keepalive_tasks(&self.tasks, &self.config);
        }
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

        let ClaAddress::Csp(remote_addr, remote_port) = cla_addr else {
            return Ok(ForwardBundleResult::NoNeighbour);
        };

        let Some(peer) = inner
            .peers
            .peer_by_addr(*remote_addr, *remote_port)
            .cloned()
        else {
            return Ok(ForwardBundleResult::NoNeighbour);
        };

        if !inner.peers.is_up(&peer) {
            debug!("CSP peer {remote_addr}:{remote_port} is not verified up");
            return Ok(ForwardBundleResult::NoNeighbour);
        }

        let digest = bundle_digest(&bundle);
        let ack_waiter = inner.peers.register_ack_waiter(&peer, digest);

        info!(
            "Forwarding bundle over CSP to {remote_addr}:{remote_port} ({} bytes)",
            bundle.len()
        );

        if let Err(err) = inner
            .cspcl_sender
            .send_bundle(bundle.to_vec(), *remote_addr, *remote_port)
            .await
        {
            inner.peers.remove_ack_waiter(&peer, digest);
            inner.peers.mark_down(&inner.sink, &peer).await;
            warn!("Could not forward bundle to {remote_addr}:{remote_port}: {err}");
            return Ok(ForwardBundleResult::NoNeighbour);
        }

        match tokio::time::timeout(
            std::time::Duration::from_millis(self.config.ack_timeout_ms),
            ack_waiter,
        )
        .await
        {
            Ok(Ok(())) => {
                info!("Bundle forwarding to {remote_addr}:{remote_port} completed");
                Ok(ForwardBundleResult::Sent)
            }
            Ok(Err(_)) | Err(_) => {
                inner.peers.remove_ack_waiter(&peer, digest);
                inner.peers.mark_down(&inner.sink, &peer).await;
                warn!("Timed out waiting for bundle ack from {remote_addr}:{remote_port}");
                Ok(ForwardBundleResult::NoNeighbour)
            }
        }
    }
}
