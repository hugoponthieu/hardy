mod config;
mod frame;
mod registry;
mod transport;

pub use config::{Config, Interface, PeerConfig};

use hardy_async::async_trait;
use hardy_bpa::Bytes;
use hardy_bpa::bpa::BpaRegistration;
use hardy_bpa::cla::{self, ClaAddress, ClaAddressType, CspAddress, ForwardBundleResult};
use hardy_bpv7::eid::NodeId;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, warn};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("transport initialization failed: {0}")]
    Init(#[from] transport::Error),
    #[error("registration failed: {0}")]
    Registration(#[from] cla::Error),
}

struct Runtime {
    sink: Arc<dyn cla::Sink>,
    registry: Arc<registry::Registry>,
    transport: Arc<transport::Transport>,
    bundle_ack_timeout: std::time::Duration,
    heartbeat_interval: std::time::Duration,
    heartbeat_timeout: std::time::Duration,
    initial_probe_interval: std::time::Duration,
    next_bundle_id: AtomicU64,
    pending_acks: hardy_async::sync::spin::Mutex<HashMap<u64, tokio::sync::oneshot::Sender<()>>>,
    tasks: hardy_async::TaskPool,
}

pub struct Cla {
    config: Config,
    runtime: hardy_async::sync::spin::Mutex<Option<Arc<Runtime>>>,
}

impl Cla {
    pub fn new(config: &Config) -> Result<Self, Error> {
        Ok(Self {
            config: config.clone(),
            runtime: hardy_async::sync::spin::Mutex::new(None),
        })
    }

    pub async fn register(
        self: &Arc<Self>,
        bpa: &dyn BpaRegistration,
        name: String,
        policy: Option<Arc<dyn hardy_bpa::policy::EgressPolicy>>,
    ) -> Result<(), Error> {
        bpa.register_cla(name, Some(ClaAddressType::Csp), self.clone(), policy)
            .await?;
        Ok(())
    }

    pub async fn unregister(&self) {
        if let Some(runtime) = self.runtime.lock().as_ref() {
            runtime.sink.unregister().await;
        }
    }

    async fn handle_peer_down(runtime: &Runtime, addr: CspAddress) {
        if runtime.registry.mark_down(addr).is_some()
            && let Err(e) = runtime.sink.remove_peer(&ClaAddress::Csp(addr)).await
        {
            warn!("remove_peer failed for {addr:?}: {e}");
        }
    }
}

async fn receive_loop(runtime: Arc<Runtime>) {
    let cancel = runtime.tasks.cancel_token().clone();
    while !cancel.is_cancelled() {
        let incoming = match runtime.transport.recv_bundle(1000).await {
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

        let Some(peer_state) = runtime.registry.snapshot_by_addr(inbound_peer.addr) else {
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

        if let Some(announced) = runtime.registry.mark_live(peer_state.address) {
            if let Err(e) = runtime
                .sink
                .add_peer(ClaAddress::Csp(announced.address), &[announced.node_id])
                .await
            {
                warn!("add_peer failed for {:?}: {}", announced.address, e);
            }
        }

        let frame = match frame::Frame::try_from(&incoming.data) {
            Ok(frame) => frame,
            Err(e) => {
                warn!("dropping malformed frame from {:?}: {}", inbound_peer, e);
                continue;
            }
        };

        match frame {
            frame::Frame::Bundle { id, payload } => {
                let peer_addr = ClaAddress::Csp(peer_state.address);
                match runtime
                    .sink
                    .dispatch(payload.into(), Some(&peer_state.node_id), Some(&peer_addr))
                    .await
                {
                    Ok(()) => {
                        let ack: Vec<u8> = frame::Frame::BundleAck { id }.into();
                        if let Err(e) = runtime
                            .transport
                            .send_bundle(&ack, peer_state.address.addr, peer_state.address.port)
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
            frame::Frame::BundleAck { id } => {
                let tx = runtime.pending_acks.lock().remove(&id);
                if let Some(tx) = tx {
                    let _ = tx.send(());
                } else {
                    debug!(
                        "received unexpected bundle ack id {} from {:?}",
                        id, inbound_peer
                    );
                }
            }
            frame::Frame::Heartbeat => {
                let ack: Vec<u8> = frame::Frame::HeartbeatAck.into();
                if let Err(e) = runtime
                    .transport
                    .send_bundle(&ack, peer_state.address.addr, peer_state.address.port)
                    .await
                {
                    warn!(
                        "failed to send heartbeat ack to {:?}: {}",
                        peer_state.address, e
                    );
                }
            }
            frame::Frame::HeartbeatAck => {}
        }
    }
}

async fn heartbeat_loop(runtime: Arc<Runtime>) {
    let cancel = runtime.tasks.cancel_token().clone();
    let mut ticker = tokio::time::interval(runtime.heartbeat_interval);
    while !cancel.is_cancelled() {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = ticker.tick() => {
                for peer in runtime.registry.heartbeat_targets(runtime.heartbeat_interval) {
                    let heartbeat:Vec<u8> =frame::Frame::Heartbeat.into();
                    if let Err(e) = runtime.transport.send_bundle(&heartbeat, peer.addr, peer.port).await {
                        warn!("heartbeat send failed for {peer:?}: {e}");
                    }
                }

                for timed_out in runtime.registry.timed_out_peers(runtime.heartbeat_timeout) {
                    if let Err(e) = runtime.sink.remove_peer(&ClaAddress::Csp(timed_out.address)).await {
                        warn!("remove_peer failed for {:?}: {}", timed_out.address, e);
                    }
                }
            }
        }
    }
}

async fn initial_probe_loop(runtime: Arc<Runtime>) {
    let cancel = runtime.tasks.cancel_token().clone();
    let mut ticker = tokio::time::interval(runtime.initial_probe_interval);
    while !cancel.is_cancelled() {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = ticker.tick() => {
                for peer in runtime.registry.probe_targets() {
                    let heartbeat: Vec<u8> = frame::Frame::Heartbeat.into();
                    if let Err(e) = runtime.transport.send_bundle(&heartbeat, peer.addr, peer.port).await {
                        warn!("initial probe failed for {peer:?}: {e}");
                    }
                }
            }
        }
    }
}

#[async_trait]
impl cla::Cla for Cla {
    async fn on_register(&self, sink: Box<dyn cla::Sink>, _node_ids: &[NodeId]) {
        let sink: Arc<dyn cla::Sink> = sink.into();
        let transport = match transport::Transport::new(&self.config) {
            Ok(t) => Arc::new(t),
            Err(e) => {
                warn!("cspcl transport init failed: {e}");
                return;
            }
        };

        let runtime = Arc::new(Runtime {
            sink,
            registry: Arc::new(registry::Registry::new(&self.config.peers)),
            transport,
            bundle_ack_timeout: self.config.bundle_ack_timeout(),
            heartbeat_interval: self.config.heartbeat_interval(),
            heartbeat_timeout: self.config.heartbeat_timeout(),
            initial_probe_interval: self.config.initial_probe_interval(),
            next_bundle_id: AtomicU64::new(1),
            pending_acks: Default::default(),
            tasks: hardy_async::TaskPool::new(),
        });

        let recv_rt = runtime.clone();
        hardy_async::spawn!(runtime.tasks, "cspcl_recv_loop", async move {
            receive_loop(recv_rt).await;
        });
        let hb_rt = runtime.clone();
        hardy_async::spawn!(runtime.tasks, "cspcl_heartbeat_loop", async move {
            heartbeat_loop(hb_rt).await;
        });
        let probe_rt = runtime.clone();
        hardy_async::spawn!(runtime.tasks, "cspcl_initial_probe_loop", async move {
            initial_probe_loop(probe_rt).await;
        });

        *self.runtime.lock() = Some(runtime);
    }

    async fn on_unregister(&self) {
        let runtime = self.runtime.lock().take();
        if let Some(runtime) = runtime {
            runtime.tasks.shutdown().await;
            if let Err(e) = runtime.transport.shutdown().await {
                warn!("transport shutdown failed: {e}");
            }
        }
    }

    async fn forward(
        &self,
        _queue: Option<u32>,
        cla_addr: &ClaAddress,
        bundle: Bytes,
    ) -> cla::Result<ForwardBundleResult> {
        let Some(runtime) = self.runtime.lock().clone() else {
            return Err(cla::Error::Disconnected);
        };

        let ClaAddress::Csp(csp_addr) = cla_addr else {
            return Ok(ForwardBundleResult::NoNeighbour);
        };

        if !runtime.registry.is_live(*csp_addr) {
            return Ok(ForwardBundleResult::NoNeighbour);
        }

        let bundle_id = runtime.next_bundle_id.fetch_add(1, Ordering::Relaxed);
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        runtime.pending_acks.lock().insert(bundle_id, ack_tx);

        let payload: Vec<u8> = frame::Frame::Bundle {
            id: bundle_id,
            payload: bundle.to_vec(),
        }
        .into();

        if runtime
            .transport
            .send_bundle(&payload, csp_addr.addr, csp_addr.port)
            .await
            .is_err()
        {
            runtime.pending_acks.lock().remove(&bundle_id);
            Self::handle_peer_down(runtime.as_ref(), *csp_addr).await;
            return Ok(ForwardBundleResult::NoNeighbour);
        }

        match tokio::time::timeout(runtime.bundle_ack_timeout, ack_rx).await {
            Ok(Ok(())) => Ok(ForwardBundleResult::Sent),
            Ok(Err(_)) | Err(_) => {
                runtime.pending_acks.lock().remove(&bundle_id);
                Self::handle_peer_down(runtime.as_ref(), *csp_addr).await;
                Ok(ForwardBundleResult::NoNeighbour)
            }
        }
    }
}
