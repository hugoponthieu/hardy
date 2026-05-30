use super::*;
use hardy_bpa::async_trait;

impl Cla {
    fn start_listeners(&self) {
        if let Some(address) = self.address {
            // Only start listener if TLS is not required, or we have server TLS config
            if !self.session_config.require_tls
                || self
                    .tls_config
                    .as_ref()
                    .and_then(|c| c.server_config.as_ref())
                    .is_some()
            {
                let ctx = self
                    .connection_context()
                    .trace_expect("start_listeners called before registration");

                let listener = listen::Listener {
                    connection_rate_limit: self.connection_rate_limit,
                    ctx,
                };
                self.tasks
                    .spawn(listener.listen(self.tasks.clone(), address));
            }
        }
    }
}

#[async_trait]
impl hardy_bpa::cla::Cla for Cla {
    fn address_type(&self) -> Option<hardy_bpa::cla::ClaAddressType> {
        Some(hardy_bpa::cla::ClaAddressType::Tcp)
    }

    #[cfg_attr(feature = "instrument", instrument(skip(self, sink)))]
    async fn on_register(&self, sink: Box<dyn hardy_bpa::cla::Sink>, node_ids: &[NodeId]) {
        // Store sink and node_ids in single atomic operation
        self.inner.call_once(|| Inner {
            sink: sink.into(),
            node_ids: node_ids.into(),
        });

        // Register any statically configured peers so the BPA can route to them
        // immediately. Connections are established lazily on first forward.
        if let Some(inner) = self.inner.get() {
            for peer in &self.peers {
                match inner
                    .sink
                    .add_peer(
                        hardy_bpa::cla::ClaAddress::Tcp(peer.address),
                        std::slice::from_ref(&peer.node_id),
                    )
                    .await
                {
                    Ok(true) => {
                        info!(
                            "Registered static peer {} at {}",
                            peer.node_id, peer.address
                        )
                    }
                    Ok(false) => debug!(
                        "Static peer {} at {} was already registered",
                        peer.node_id, peer.address
                    ),
                    Err(e) => warn!(
                        "Failed to register static peer {} at {}: {e}",
                        peer.node_id, peer.address
                    ),
                }
            }
        }

        // Start listeners now that we have a sink
        self.start_listeners();
    }

    #[cfg_attr(feature = "instrument", instrument(skip(self)))]
    async fn on_unregister(&self) {
        // Cancel sessions first so they exit promptly when channels close
        self.session_cancel_token.cancel();

        // Shutdown all pooled connections (drops tx senders)
        self.registry.shutdown();

        // Wait for all session tasks to complete
        self.tasks.shutdown().await;
    }

    #[cfg_attr(feature = "instrument", instrument(skip(self, bundle)))]
    async fn forward(
        &self,
        _queue: Option<u32>,
        cla_addr: &hardy_bpa::cla::ClaAddress,
        mut bundle: hardy_bpa::Bytes,
    ) -> hardy_bpa::cla::Result<hardy_bpa::cla::ForwardBundleResult> {
        let ctx = self.connection_context().ok_or_else(|| {
            error!("forward called before on_register!");
            hardy_bpa::cla::Error::Disconnected
        })?;

        if let hardy_bpa::cla::ClaAddress::Tcp(remote_addr) = cla_addr {
            debug!("Forwarding bundle to TCPCLv4 peer at {remote_addr}");

            // We try this 5 times, because peers can close at random times
            for _ in 0..5 {
                // See if we have an active connection already
                bundle = match self.registry.forward(remote_addr, bundle).await {
                    Ok(r) => {
                        debug!("Bundle forwarded successfully using existing connection");
                        return Ok(r);
                    }
                    Err(bundle) => {
                        debug!("No live connections, will attempt to create new one");
                        bundle
                    }
                };

                // Do a new active connect
                let conn = connect::Connector {
                    tasks: self.tasks.clone(),
                    ctx: ctx.clone(),
                };
                match conn.connect(remote_addr).await {
                    Ok(()) | Err(transport::Error::Timeout) => {}
                    Err(_) => {
                        // No point retrying
                        break;
                    }
                }
            }
        }

        Ok(hardy_bpa::cla::ForwardBundleResult::NoNeighbour)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hardy_bpa::cla::{Cla as ClaTrait, ClaAddress, Sink};
    use hardy_bpv7::eid::{IpnNodeId, NodeId};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    type RecordedPeers = Arc<Mutex<Vec<(ClaAddress, Vec<NodeId>)>>>;

    // A Sink that records every add_peer call for later assertions.
    struct RecordingSink {
        added: RecordedPeers,
    }

    #[async_trait]
    impl Sink for RecordingSink {
        async fn unregister(&self) {}

        async fn dispatch(
            &self,
            _bundle: hardy_bpa::Bytes,
            _peer_node: Option<&NodeId>,
            _peer_addr: Option<&ClaAddress>,
        ) -> hardy_bpa::cla::Result<()> {
            Ok(())
        }

        async fn add_peer(
            &self,
            cla_addr: ClaAddress,
            node_ids: &[NodeId],
        ) -> hardy_bpa::cla::Result<bool> {
            self.added
                .lock()
                .unwrap()
                .push((cla_addr, node_ids.to_vec()));
            Ok(true)
        }

        async fn remove_peer(&self, _cla_addr: &ClaAddress) -> hardy_bpa::cla::Result<bool> {
            Ok(true)
        }
    }

    // A Sink whose add_peer always fails, counting how many times it was called.
    struct FailingSink {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Sink for FailingSink {
        async fn unregister(&self) {}

        async fn dispatch(
            &self,
            _bundle: hardy_bpa::Bytes,
            _peer_node: Option<&NodeId>,
            _peer_addr: Option<&ClaAddress>,
        ) -> hardy_bpa::cla::Result<()> {
            Ok(())
        }

        async fn add_peer(
            &self,
            _cla_addr: ClaAddress,
            _node_ids: &[NodeId],
        ) -> hardy_bpa::cla::Result<bool> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(hardy_bpa::cla::Error::Disconnected)
        }

        async fn remove_peer(&self, _cla_addr: &ClaAddress) -> hardy_bpa::cla::Result<bool> {
            Ok(true)
        }
    }

    fn ipn(node_number: u32) -> NodeId {
        NodeId::Ipn(IpnNodeId {
            allocator_id: 0,
            node_number,
        })
    }

    #[tokio::test]
    async fn registers_configured_peers() {
        let peer_addr: std::net::SocketAddr = "192.168.1.10:4556".parse().unwrap();

        // No listener (address: None) keeps the test from binding a port.
        let config = config::Config {
            address: None,
            peers: vec![config::PeerConfig {
                node_id: ipn(2),
                address: peer_addr,
            }],
            ..Default::default()
        };

        let cla = Cla::new(&config).expect("CLA construction should succeed");

        let recorded = Arc::new(Mutex::new(Vec::new()));
        let sink = RecordingSink {
            added: recorded.clone(),
        };

        ClaTrait::on_register(&cla, Box::new(sink), &[ipn(1)]).await;

        let calls = recorded.lock().unwrap();
        assert_eq!(calls.len(), 1, "exactly one peer should be registered");
        assert_eq!(calls[0].0, ClaAddress::Tcp(peer_addr));
        assert_eq!(calls[0].1, vec![ipn(2)]);
    }

    #[tokio::test]
    async fn no_peers_registers_nothing() {
        let config = config::Config {
            address: None,
            ..Default::default()
        };
        let cla = Cla::new(&config).expect("CLA construction should succeed");

        let recorded = Arc::new(Mutex::new(Vec::new()));
        let sink = RecordingSink {
            added: recorded.clone(),
        };

        ClaTrait::on_register(&cla, Box::new(sink), &[ipn(1)]).await;

        assert!(recorded.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn registers_all_configured_peers() {
        let addr1: std::net::SocketAddr = "192.168.1.10:4556".parse().unwrap();
        let addr2: std::net::SocketAddr = "192.168.1.11:4556".parse().unwrap();

        let config = config::Config {
            address: None,
            peers: vec![
                config::PeerConfig {
                    node_id: ipn(2),
                    address: addr1,
                },
                config::PeerConfig {
                    node_id: ipn(3),
                    address: addr2,
                },
            ],
            ..Default::default()
        };

        let cla = Cla::new(&config).expect("CLA construction should succeed");

        let recorded: RecordedPeers = Arc::new(Mutex::new(Vec::new()));
        let sink = RecordingSink {
            added: recorded.clone(),
        };

        ClaTrait::on_register(&cla, Box::new(sink), &[ipn(1)]).await;

        let calls = recorded.lock().unwrap();
        assert_eq!(calls.len(), 2, "both configured peers should be registered");
        assert_eq!(calls[0].0, ClaAddress::Tcp(addr1));
        assert_eq!(calls[0].1, vec![ipn(2)]);
        assert_eq!(calls[1].0, ClaAddress::Tcp(addr2));
        assert_eq!(calls[1].1, vec![ipn(3)]);
    }

    #[tokio::test]
    async fn peer_registration_error_does_not_abort() {
        let addr1: std::net::SocketAddr = "192.168.1.10:4556".parse().unwrap();
        let addr2: std::net::SocketAddr = "192.168.1.11:4556".parse().unwrap();

        let config = config::Config {
            address: None,
            peers: vec![
                config::PeerConfig {
                    node_id: ipn(2),
                    address: addr1,
                },
                config::PeerConfig {
                    node_id: ipn(3),
                    address: addr2,
                },
            ],
            ..Default::default()
        };

        let cla = Cla::new(&config).expect("CLA construction should succeed");

        let calls = Arc::new(AtomicUsize::new(0));
        let sink = FailingSink {
            calls: calls.clone(),
        };

        ClaTrait::on_register(&cla, Box::new(sink), &[ipn(1)]).await;

        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "both peers should be attempted even though the first failed"
        );
    }
}
