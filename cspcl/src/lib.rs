mod config;
mod frame;
mod registry;
mod runtime;
mod transport;

pub use config::{Config, Interface, PeerConfig};

use hardy_async::async_trait;
use hardy_bpa::Bytes;
use hardy_bpa::bpa::BpaRegistration;
use hardy_bpa::cla::{self, ClaAddress, ClaAddressType, CspAddress, ForwardBundleResult};
use hardy_bpv7::eid::NodeId;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tracing::warn;

use crate::frame::Frame;
use crate::registry::Registry;
use crate::runtime::Runtime;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("transport initialization failed: {0}")]
    Init(#[from] transport::Error),
    #[error("registration failed: {0}")]
    Registration(#[from] cla::Error),
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

        let runtime = Arc::new(Runtime::new(
            sink,
            Arc::new(Registry::new(&self.config.peers)),
            transport,
            self.config.runtime_config,
        ));

        runtime.clone().start_receive_loop();
        runtime.clone().start_hearbeat_loop();
        runtime.clone().start_initial_probe_loop();

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

        if runtime
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
            runtime.pending_acks.lock().remove(&bundle_id);
            Self::handle_peer_down(runtime.as_ref(), *csp_addr).await;
            return Ok(ForwardBundleResult::NoNeighbour);
        }

        match tokio::time::timeout(runtime.config.bundle_ack_timeout(), ack_rx).await {
            Ok(Ok(())) => Ok(ForwardBundleResult::Sent),
            Ok(Err(_)) | Err(_) => {
                runtime.pending_acks.lock().remove(&bundle_id);
                Self::handle_peer_down(runtime.as_ref(), *csp_addr).await;
                Ok(ForwardBundleResult::NoNeighbour)
            }
        }
    }
}
