use std::sync::Arc;

use cspcl::{Cspcl, cspcl_sys};
use hardy_async::TaskPool;
use hardy_bpa::{
    Bytes, async_trait,
    cla::{ClaAddress, ForwardBundleResult, Sink},
};
use tokio::task;
use tokio::sync::Mutex;
use tracing::{debug, error, info};

use crate::{Cla, ClaInner};

impl ClaInner {
    fn recv_bundle(cspcl: &mut Cspcl, timeout_ms: u32) -> cspcl::Result<(Vec<u8>, u8, u8)> {
        let mut buffer = vec![0u8; cspcl_sys::CSPCL_MAX_BUNDLE_SIZE as usize];
        let mut len = buffer.len();
        let mut src_addr: u8 = 0;
        let mut src_port: u8 = 0;

        unsafe {
            cspcl::Error::from_code(cspcl_sys::cspcl_recv_bundle(
                cspcl.inner_mut(),
                buffer.as_mut_ptr(),
                &mut len,
                &mut src_addr,
                &mut src_port,
                timeout_ms,
            ))?;
        }

        buffer.truncate(len);
        Ok((buffer, src_addr, src_port))
    }

    fn start_listener(&self, tasks: &Arc<TaskPool>) {
        let cspcl = Arc::clone(&self.cspcl);
        let sink = Arc::clone(&self.sink);
        let peers = Arc::clone(&self.peers);
        tasks.spawn(async move {
            loop {
                let recv_result = {
                    let cspcl = Arc::clone(&cspcl);
                    task::spawn_blocking(move || {
                        let mut guard = cspcl.blocking_lock();
                        Self::recv_bundle(&mut guard, 1000)
                    })
                    .await
                };

                let (bundle, src_addr, src_port) = match recv_result {
                    Ok(Ok(bundle)) => bundle,
                    Ok(Err(_)) => continue,
                    Err(err) => {
                        error!("CSP listener task failed: {err}");
                        continue;
                    }
                };
                let peer = peers
                    .iter()
                    .find(|peer| peer.remote_addr == src_addr && peer.remote_port == src_port);
                let peer_node = peer.and_then(|peer| peer.node_ids.first());
                let peer_addr = hardy_bpa::cla::ClaAddress::Csp(src_addr, src_port);

                info!(
                    "Received bundle over CSP from {src_addr}:{src_port} ({} bytes) peer_node={:?}",
                    bundle.len(),
                    peer_node
                );

                if let Err(err) = sink
                    .dispatch(bundle.into(), peer_node, Some(&peer_addr))
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
                error!("Failed to get current working directory: {e}");
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

        // TODO: Define elemenent for the csp addr
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

                task::spawn_blocking(move || {
                    let mut cspcl = cspcl.blocking_lock();
                    cspcl.send_bundle(&bundle, remote_addr, remote_port)
                })
                .await
            };

            match send_result {
                Ok(Ok(())) => debug!("Bundle sent"),
                Ok(Err(e)) => error!("Could not forward bundle: {e}"),
                Err(err) => error!("CSP send task failed: {err}"),
            }
            return Ok(ForwardBundleResult::Sent);
        }
        Ok(ForwardBundleResult::NoNeighbour)
    }
}
