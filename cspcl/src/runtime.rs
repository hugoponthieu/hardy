use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use hardy_bpa::{
    Bytes,
    cla::{self, ClaAddress, CspAddress, ForwardBundleResult, Sink},
};
use tracing::{debug, warn};

use crate::{
    frame::Frame,
    registry::Registry,
    transport::{self, Transport},
};

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
    pub sink: Arc<dyn Sink>,
    pub registry: Arc<Registry>,
    pub transport: Arc<Transport>,
    pub config: Config,
    next_bundle_id: AtomicU64,
    pub pending_acks:
        hardy_async::sync::spin::Mutex<HashMap<u64, tokio::sync::oneshot::Sender<()>>>,
    pub tasks: hardy_async::TaskPool,
}

impl Runtime {
    pub fn new(
        sink: Arc<dyn Sink>,
        registry: Arc<Registry>,
        transport: Arc<Transport>,
        config: Config,
    ) -> Self {
        Self {
            sink,
            registry,
            transport,
            config,
            next_bundle_id: AtomicU64::new(1),
            pending_acks: Default::default(),
            tasks: hardy_async::TaskPool::new(),
        }
    }

    pub fn start(self: Arc<Self>) {
        self.clone().start_receive_loop();
        self.clone().start_hearbeat_loop();
        self.clone().start_initial_probe_loop();
    }
}

impl Runtime {
    pub async fn send_bundle(
        self: Arc<Self>,
        bundle: Bytes,
        csp_addr: &CspAddress,
    ) -> cla::Result<ForwardBundleResult> {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        let bundle_id = self.next_bundle_id.fetch_add(1, Ordering::Relaxed);
        self.pending_acks.lock().insert(bundle_id, ack_tx);

        if self
            .transport
            .send_bundle(
                Frame::Bundle {
                    id: bundle_id,
                    payload: bundle.to_vec(),
                },
                csp_addr.addr,
                csp_addr.port,
            )
            .await
            .is_err()
        {
            self.pending_acks.lock().remove(&bundle_id);
            self.handle_peer_down(*csp_addr).await;
            return Ok(ForwardBundleResult::NoNeighbour);
        }

        match tokio::time::timeout(self.config.bundle_ack_timeout(), ack_rx).await {
            Ok(Ok(())) => Ok(ForwardBundleResult::Sent),
            Ok(Err(_)) | Err(_) => {
                self.pending_acks.lock().remove(&bundle_id);
                self.handle_peer_down(*csp_addr).await;
                Ok(ForwardBundleResult::NoNeighbour)
            }
        }
    }

    async fn handle_peer_down(self: Arc<Runtime>, addr: CspAddress) {
        if self.registry.mark_down(addr).is_some()
            && let Err(e) = self.sink.remove_peer(&ClaAddress::Csp(addr)).await
        {
            warn!("remove_peer failed for {addr:?}: {e}");
        }
    }

    pub fn start_hearbeat_loop(self: Arc<Runtime>) {
        hardy_async::spawn!(self.clone().tasks, "cspcl_heartbeat_loop", async move {
            self.heartbeat_loop().await;
        });
    }

    async fn heartbeat_loop(self: Arc<Runtime>) {
        let cancel = self.tasks.cancel_token().clone();
        let mut ticker = tokio::time::interval(self.config.heartbeat_interval());
        while !cancel.is_cancelled() {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = ticker.tick() => {
                    for peer in self.registry.heartbeat_targets(self.config.heartbeat_interval()) {
                        if let Err(e) = self.transport.send_bundle(Frame::Heartbeat, peer.addr, peer.port).await {
                            warn!("heartbeat send failed for {peer:?}: {e}");
                        }
                    }

                    for timed_out in self.registry.timed_out_peers(self.config.heartbeat_interval()) {
                        if let Err(e) = self.sink.remove_peer(&ClaAddress::Csp(timed_out.address)).await {
                            warn!("remove_peer failed for {:?}: {}", timed_out.address, e);
                        }
                    }
                }
            }
        }
    }
    pub fn start_initial_probe_loop(self: Arc<Runtime>) {
        hardy_async::spawn!(self.clone().tasks, "cspcl_initial_probe_loop", async move {
            self.initial_probe_loop().await;
        });
    }

    async fn initial_probe_loop(self: Arc<Runtime>) {
        let cancel = self.tasks.cancel_token().clone();
        let mut ticker = tokio::time::interval(self.config.initial_probe_interval());
        while !cancel.is_cancelled() {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = ticker.tick() => {
                    for peer in self.registry.probe_targets() {
                        if let Err(e) = self.transport.send_bundle(Frame::Heartbeat, peer.addr, peer.port).await {
                            warn!("initial probe failed for {peer:?}: {e}");
                        }
                    }
                }
            }
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

            let Some(peer_state) = self.registry.snapshot_by_addr(inbound_peer.addr) else {
                debug!(
                    "dropping frame from unknown or ambiguous peer {:?}",
                    inbound_peer
                );
                continue;
            };

            if inbound_peer.port != peer_state.address.port {
                debug!(
                    "resolved inbound peer {:?} to configured {:?}",
                    inbound_peer, peer_state.address
                );
            }

            if let Some(announced) = self.registry.mark_live(peer_state.address) {
                if let Err(e) = self
                    .sink
                    .add_peer(ClaAddress::Csp(announced.address), &[announced.node_id])
                    .await
                {
                    warn!("add_peer failed for {:?}: {}", announced.address, e);
                }
            }

            let frame = match Frame::try_from(&incoming.data) {
                Ok(frame) => frame,
                Err(e) => {
                    warn!("dropping malformed frame from {:?}: {}", inbound_peer, e);
                    continue;
                }
            };

            match frame {
                Frame::Bundle { id, payload } => {
                    let peer_addr = ClaAddress::Csp(peer_state.address);
                    match self
                        .sink
                        .dispatch(payload.into(), Some(&peer_state.node_id), Some(&peer_addr))
                        .await
                    {
                        Ok(()) => {
                            if let Err(e) = self
                                .transport
                                .send_bundle(
                                    Frame::BundleAck { id },
                                    peer_state.address.addr,
                                    peer_state.address.port,
                                )
                                .await
                            {
                                warn!(
                                    "failed to send bundle ack to {:?}: {}",
                                    peer_state.address, e
                                );
                            }
                        }
                        Err(e) => {
                            warn!("dispatch failed for {:?}: {}", peer_state.address, e);
                        }
                    }
                }
                Frame::BundleAck { id } => {
                    let tx = self.pending_acks.lock().remove(&id);
                    if let Some(tx) = tx {
                        let _ = tx.send(());
                    } else {
                        debug!(
                            "received unexpected bundle ack id {} from {:?}",
                            id, inbound_peer
                        );
                    }
                }
                Frame::Heartbeat => {
                    if let Err(e) = self
                        .transport
                        .send_bundle(
                            Frame::HeartbeatAck,
                            peer_state.address.addr,
                            peer_state.address.port,
                        )
                        .await
                    {
                        warn!(
                            "failed to send heartbeat ack to {:?}: {}",
                            peer_state.address, e
                        );
                    }
                }
                Frame::HeartbeatAck => {}
            }
        }
    }
}
