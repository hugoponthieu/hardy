use std::sync::{Arc, Mutex};

use cspcl::Cspcl;
use hardy_async::TaskPool;
use hardy_bpa::{
    Bytes, async_trait,
    cla::{ClaAddress, Error, ForwardBundleResult, Sink},
};
use tracing::{error, info};

use crate::{Cla, ClaInner};

impl ClaInner {
    fn start_listener(&self, tasks: &Arc<TaskPool>) {
        let cspcl = Arc::clone(&self.cspcl);
        let sink = Arc::clone(&self.sink);
        tasks.spawn(async move {
            loop {
                let bundle = {
                    let mut guard = cspcl.lock().expect("Could not lock cspcl to listen");
                    let (bundle, _, _) = guard.recv_bundle(1000).unwrap();
                    bundle
                };
                sink.dispatch(bundle.into(), None, None).await.unwrap();
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
                error!("Failed to get current working directory: {e}");
                return;
            }
        };

        self.inner.call_once(|| ClaInner {
            sink: sink.into(),
            node_ids: node_ids.into(),
            cspcl,
        });

        if let Some(cla_inner) = self.inner.get() {
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

        // TODO: Define elemenent for the csp addr
        if let ClaAddress::Csp(remote_addr, remote_port) = cla_addr {
            info!("Forwarding bundle to CSPCL peer at {remote_addr}");
            let mut cspcl = inner
                .cspcl
                .lock()
                .expect("Could not acquire lock on cspcl instance");
            let _sent_data = cspcl
                .send_bundle(&bundle, *remote_addr, *remote_port)
                .map_err(|e| Error::Internal(e.into()))?;
            return Ok(ForwardBundleResult::Sent);
        }
        Ok(ForwardBundleResult::NoNeighbour)
    }
}
