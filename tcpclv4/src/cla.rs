use super::*;
use hardy_bpa::async_trait;

impl Cla {
    fn start_background_tasks(&self) {
        self.start_listeners();
        self.start_static_peers();
    }

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

    fn start_static_peers(&self) {
        let ctx = self
            .connection_context()
            .trace_expect("start_static_peers called before registration");

        for remote_addr in self.peers.iter().copied() {
            let ctx = ctx.clone();
            let tasks = self.tasks.clone();
            let reconnect_delay = self.reconnect_delay;

            self.tasks.spawn(async move {
                loop {
                    if !ctx.registry.has_session(&remote_addr) {
                        let connector = connect::Connector {
                            tasks: tasks.clone(),
                            ctx: ctx.clone(),
                        };

                        match connector.connect(&remote_addr).await {
                            Ok(()) => {
                                info!("Connected to static TCPCL peer at {remote_addr}");
                            }
                            Err(transport::Error::Timeout) => {
                                debug!("Timed out connecting to static TCPCL peer at {remote_addr}");
                            }
                            Err(e) => {
                                debug!("Failed to connect to static TCPCL peer at {remote_addr}: {e}");
                            }
                        }
                    }

                    tokio::select! {
                        _ = ctx.task_cancel_token.cancelled() => break,
                        _ = ctx.session_cancel_token.cancelled() => break,
                        _ = tokio::time::sleep(reconnect_delay) => {}
                    }
                }
            });
        }
    }
}

#[async_trait]
impl hardy_bpa::cla::Cla for Cla {
    #[cfg_attr(feature = "instrument", instrument(skip(self, sink)))]
    async fn on_register(&self, sink: Box<dyn hardy_bpa::cla::Sink>, node_ids: &[NodeId]) {
        // Store sink and node_ids in single atomic operation
        self.inner.call_once(|| Inner {
            sink: sink.into(),
            node_ids: node_ids.into(),
        });

        // Start listeners and configured outbound sessions now that we have a sink
        self.start_background_tasks();
    }

    #[cfg_attr(feature = "instrument", instrument(skip(self)))]
    async fn on_unregister(&self) {
        // Cancel sessions first so they exit promptly when channels close
        self.session_cancel_token.cancel();

        // Shutdown all pooled connections (drops tx senders)
        self.registry.shutdown().await;

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
