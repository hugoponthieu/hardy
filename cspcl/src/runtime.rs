use std::{sync::Arc, time::Duration};

use hardy_async::async_trait;
use hardy_bpa::{
    Bytes,
    cla::{self, ClaAddress, CspAddress, ForwardBundleResult, Sink},
};
use tracing::warn;

use crate::{
    registry::Registry,
    transport::{self, IncomingBundle, Transport},
};

#[derive(Debug, Copy, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default, rename_all = "kebab-case"))]
pub struct Config {
    peer_idle_timeout_secs: Option<u64>,
}

impl Config {
    pub fn peer_idle_timeout(&self) -> Option<Duration> {
        self.peer_idle_timeout_secs
            .map(|secs| Duration::from_secs(secs.max(1)))
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            peer_idle_timeout_secs: None,
        }
    }
}

#[async_trait]
pub(crate) trait BundleTransport: Send + Sync {
    async fn send_bundle(&self, payload: Bytes, addr: u8, port: u8) -> Result<(), String>;
    async fn recv_bundle(&self, timeout_ms: u32) -> Result<transport::ReceiveResult, String>;
    async fn shutdown(&self) -> Result<(), String>;
}

#[async_trait]
impl BundleTransport for Transport {
    async fn send_bundle(&self, payload: Bytes, addr: u8, port: u8) -> Result<(), String> {
        Transport::send_bundle(self, payload, addr, port)
            .await
            .map_err(|err| err.to_string())
    }

    async fn recv_bundle(&self, timeout_ms: u32) -> Result<transport::ReceiveResult, String> {
        Transport::recv_bundle(self, timeout_ms)
            .await
            .map_err(|err| err.to_string())
    }

    async fn shutdown(&self) -> Result<(), String> {
        Transport::shutdown(self)
            .await
            .map_err(|err| err.to_string())
    }
}

pub struct Runtime {
    sink: Arc<dyn Sink>,
    registry: Arc<Registry>,
    transport: Arc<dyn BundleTransport>,
    tasks: hardy_async::TaskPool,
}

impl Runtime {
    pub(crate) fn new(
        sink: Arc<dyn Sink>,
        registry: Arc<Registry>,
        transport: Arc<dyn BundleTransport>,
        _config: Config,
    ) -> Self {
        Self {
            sink,
            registry,
            transport,
            tasks: hardy_async::TaskPool::new(),
        }
    }

    pub fn bootstrap_peers(&self) -> Vec<crate::registry::PeerSnapshot> {
        self.registry.bootstrap_peers()
    }

    pub fn mark_peer_announced(&self, addr: CspAddress) -> bool {
        self.registry.mark_announced(addr)
    }
}

impl Runtime {
    pub async fn send_bundle(
        self: Arc<Self>,
        bundle: Bytes,
        csp_addr: &CspAddress,
    ) -> cla::Result<ForwardBundleResult> {
        if self
            .transport
            .send_bundle(bundle, csp_addr.addr, csp_addr.port)
            .await
            .is_err()
        {
            self.handle_peer_down(*csp_addr).await;
            return Ok(ForwardBundleResult::NoNeighbour);
        }
        self.registry.mark_outbound_activity(*csp_addr);
        Ok(ForwardBundleResult::Sent)
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

    async fn handle_peer_down(&self, addr: CspAddress) {
        if self.registry.mark_down(addr).is_some()
            && let Err(e) = self.sink.remove_peer(&ClaAddress::Csp(addr)).await
        {
            warn!("remove_peer failed for {addr:?}: {e}");
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

            self.process_incoming_bundle(incoming).await;
        }
    }

    async fn process_incoming_bundle(&self, incoming: IncomingBundle) {
        let inbound_peer = CspAddress {
            addr: incoming.src_addr,
            port: incoming.src_port,
        };
        let peer_state = self.registry.resolve_or_discover(inbound_peer);

        if let Err(e) = self.ensure_peer_announced(&peer_state).await {
            warn!("add_peer failed for {:?}: {}", peer_state.address, e);
            return;
        }

        let peer_addr = ClaAddress::Csp(peer_state.address);
        if let Err(e) = self
            .sink
            .dispatch(
                incoming.data.into(),
                Some(&peer_state.node_id),
                Some(&peer_addr),
            )
            .await
        {
            warn!("dispatch failed for {:?}: {}", peer_state.address, e);
        }
    }

    async fn ensure_peer_announced(&self, peer: &crate::registry::PeerSnapshot) -> cla::Result<()> {
        if self.registry.mark_announced(peer.address) {
            self.sink
                .add_peer(
                    ClaAddress::Csp(peer.address),
                    core::slice::from_ref(&peer.node_id),
                )
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hardy_async::sync::spin::Mutex;
    use hardy_bpv7::eid::NodeId;

    #[derive(Default)]
    struct FakeSinkState {
        dispatches: Vec<(Bytes, NodeId, ClaAddress)>,
        added_peers: Vec<(ClaAddress, Vec<NodeId>)>,
        removed_peers: Vec<ClaAddress>,
        unregister_calls: usize,
    }

    #[derive(Default)]
    struct FakeSink {
        state: Mutex<FakeSinkState>,
    }

    #[async_trait]
    impl Sink for FakeSink {
        async fn unregister(&self) {
            self.state.lock().unregister_calls += 1;
        }

        async fn dispatch(
            &self,
            bundle: Bytes,
            peer_node: Option<&NodeId>,
            peer_addr: Option<&ClaAddress>,
        ) -> cla::Result<()> {
            self.state.lock().dispatches.push((
                bundle,
                peer_node.cloned().expect("peer node should be provided"),
                peer_addr.cloned().expect("peer addr should be provided"),
            ));
            Ok(())
        }

        async fn add_peer(&self, cla_addr: ClaAddress, node_ids: &[NodeId]) -> cla::Result<bool> {
            self.state
                .lock()
                .added_peers
                .push((cla_addr, node_ids.to_vec()));
            Ok(true)
        }

        async fn remove_peer(&self, cla_addr: &ClaAddress) -> cla::Result<bool> {
            self.state.lock().removed_peers.push(cla_addr.clone());
            Ok(true)
        }
    }

    #[derive(Default)]
    struct FakeTransportState {
        sent: Vec<(Bytes, u8, u8)>,
        recv: Vec<Result<transport::ReceiveResult, String>>,
        fail_send: bool,
        shutdown_calls: usize,
    }

    #[derive(Default)]
    struct FakeTransport {
        state: Mutex<FakeTransportState>,
    }

    #[async_trait]
    impl BundleTransport for FakeTransport {
        async fn send_bundle(&self, payload: Bytes, addr: u8, port: u8) -> Result<(), String> {
            let mut guard = self.state.lock();
            if guard.fail_send {
                return Err("send failed".to_string());
            }
            guard.sent.push((payload, addr, port));
            Ok(())
        }

        async fn recv_bundle(&self, _timeout_ms: u32) -> Result<transport::ReceiveResult, String> {
            self.state
                .lock()
                .recv
                .pop()
                .unwrap_or(Ok(transport::ReceiveResult::Timeout))
        }

        async fn shutdown(&self) -> Result<(), String> {
            self.state.lock().shutdown_calls += 1;
            Ok(())
        }
    }

    fn runtime_with(
        sink: Arc<FakeSink>,
        registry: Registry,
        transport: Arc<FakeTransport>,
    ) -> Runtime {
        Runtime::new(sink, Arc::new(registry), transport, Config::default())
    }

    #[tokio::test]
    async fn send_bundle_uses_raw_bytes() {
        let sink = Arc::new(FakeSink::default());
        let transport = Arc::new(FakeTransport::default());
        let runtime = Arc::new(runtime_with(
            sink,
            Registry::new(&[crate::config::PeerConfig {
                node_id: Some("ipn:2.0".parse().unwrap()),
                addr: 2,
                port: 10,
            }]),
            transport.clone(),
        ));

        let result = runtime
            .send_bundle(
                Bytes::from_static(b"bundle"),
                &CspAddress { addr: 2, port: 10 },
            )
            .await
            .unwrap();
        assert!(matches!(result, ForwardBundleResult::Sent));

        let sent = transport.state.lock().sent.clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0.as_ref(), b"bundle");
        assert_eq!((sent[0].1, sent[0].2), (2, 10));
    }

    #[tokio::test]
    async fn send_failure_removes_announced_peer() {
        let sink = Arc::new(FakeSink::default());
        let transport = Arc::new(FakeTransport::default());
        transport.state.lock().fail_send = true;
        let registry = Registry::new(&[crate::config::PeerConfig {
            node_id: Some("ipn:2.0".parse().unwrap()),
            addr: 2,
            port: 10,
        }]);
        assert!(registry.mark_announced(CspAddress { addr: 2, port: 10 }));
        let runtime = Arc::new(runtime_with(sink.clone(), registry, transport));

        let result = runtime
            .send_bundle(
                Bytes::from_static(b"bundle"),
                &CspAddress { addr: 2, port: 10 },
            )
            .await
            .unwrap();
        assert!(matches!(result, ForwardBundleResult::NoNeighbour));

        let removed = sink.state.lock().removed_peers.clone();
        assert_eq!(
            removed,
            vec![ClaAddress::Csp(CspAddress { addr: 2, port: 10 })]
        );
    }

    #[tokio::test]
    async fn process_incoming_bundle_discovers_and_dispatches_peer() {
        let sink = Arc::new(FakeSink::default());
        let transport = Arc::new(FakeTransport::default());
        let runtime = runtime_with(sink.clone(), Registry::new(&[]), transport);

        runtime
            .process_incoming_bundle(IncomingBundle {
                src_addr: 7,
                src_port: 9,
                data: b"raw-bundle".to_vec(),
            })
            .await;

        let state = sink.state.lock();
        assert_eq!(state.added_peers.len(), 1);
        assert_eq!(
            state.added_peers[0],
            (
                ClaAddress::Csp(CspAddress { addr: 7, port: 9 }),
                vec!["ipn:7.0".parse().unwrap()]
            )
        );
        assert_eq!(state.dispatches.len(), 1);
        assert_eq!(state.dispatches[0].0.as_ref(), b"raw-bundle");
        assert_eq!(state.dispatches[0].1, "ipn:7.0".parse().unwrap());
    }
}
