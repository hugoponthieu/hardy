mod config;
mod registry;
mod runtime;
mod transport;

pub use config::{Config, Interface, PeerConfig};

use hardy_async::async_trait;
use hardy_bpa::Bytes;
use hardy_bpa::bpa::BpaRegistration;
use hardy_bpa::cla::{self, ClaAddress, ClaAddressType, ForwardBundleResult};
use hardy_bpv7::eid::NodeId;
use std::sync::Arc;
use tracing::warn;

use crate::registry::Registry;
use crate::runtime::{BundleTransport, Runtime};

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
            runtime.clone().unregister_sink().await;
        }
    }

    pub fn set_runtime(&self, runtime: Arc<Runtime>) {
        *self.runtime.lock() = Some(runtime);
    }

    pub fn try_get_runtime(&self) -> cla::Result<Arc<Runtime>> {
        self.runtime
            .lock()
            .clone()
            .ok_or_else(|| cla::Error::Disconnected)
    }
}

#[async_trait]
impl cla::Cla for Cla {
    async fn on_register(&self, sink: Box<dyn cla::Sink>, _node_ids: &[NodeId]) {
        let sink: Arc<dyn cla::Sink> = sink.into();
        let sink_for_bootstrap = sink.clone();
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
            transport as Arc<dyn BundleTransport>,
            self.config.runtime_config,
        ));

        runtime.clone().start_receive_loop();
        for peer in runtime.bootstrap_peers() {
            if runtime.mark_peer_announced(peer.address) {
                if let Err(e) = sink_for_bootstrap
                    .add_peer(
                        ClaAddress::Csp(peer.address),
                        core::slice::from_ref(&peer.node_id),
                    )
                    .await
                {
                    warn!("add_peer failed for {:?}: {}", peer.address, e);
                }
            }
        }

        self.set_runtime(runtime);
    }

    async fn on_unregister(&self) {
        if let Some(runtime) = self.runtime.lock().take() {
            runtime.shutdown().await;
        }
    }

    async fn forward(
        &self,
        _queue: Option<u32>,
        cla_addr: &ClaAddress,
        bundle: Bytes,
    ) -> cla::Result<ForwardBundleResult> {
        let runtime = self.try_get_runtime()?;
        let ClaAddress::Csp(csp_addr) = cla_addr else {
            return Ok(ForwardBundleResult::NoNeighbour);
        };
        runtime.send_bundle(bundle, csp_addr).await
    }
}
