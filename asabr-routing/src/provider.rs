use std::sync::Mutex;
use std::thread::JoinHandle;

use async_trait::async_trait;
use hardy_bpa::routes::{self, LiveRoutingProvider};
use hardy_bpv7::eid::{Eid, IpnNodeId};
use tokio::sync::{mpsc, oneshot};
use tracing::debug;

use crate::{Config, Error, Topology, translate};

type RouteResult = Result<Option<u16>, Error>;

enum Command {
    Shutdown,
    Route {
        bundle: a_sabr::bundle::Bundle,
        now: f64,
        response: oneshot::Sender<RouteResult>,
    },
}

pub struct AsabrRoutingProvider {
    local_node_id: u16,
    tx: mpsc::UnboundedSender<Command>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl AsabrRoutingProvider {
    pub fn new(config: Config) -> Result<Self, Error> {
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<Result<u16, Error>>(0);
        let (tx, rx) = mpsc::unbounded_channel::<Command>();

        let handle = std::thread::Builder::new()
            .name("hardy-asabr-router".into())
            .spawn(move || worker_main(config, init_tx, rx))
            .map_err(Error::WorkerSpawn)?;

        let local_node_id = match init_rx.recv() {
            Ok(Ok(id)) => id,
            Ok(Err(err)) => {
                let _ = handle.join();
                return Err(err);
            }
            Err(_) => {
                let _ = handle.join();
                return Err(Error::WorkerInitFailed);
            }
        };

        Ok(Self {
            local_node_id,
            tx,
            handle: Mutex::new(Some(handle)),
        })
    }
}

impl Drop for AsabrRoutingProvider {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(handle) = self.handle.get_mut().ok().and_then(|h| h.take()) {
            let _ = handle.join();
        }
    }
}

fn worker_main(
    config: Config,
    init_tx: std::sync::mpsc::SyncSender<Result<u16, Error>>,
    mut rx: mpsc::UnboundedReceiver<Command>,
) {
    let topology = match Topology::load(&config) {
        Ok(topology) => topology,
        Err(err) => {
            let _ = init_tx.send(Err(err));
            return;
        }
    };
    let local_node_id = topology.local_node_id;
    let mut router = match topology.build_router() {
        Ok(router) => router,
        Err(err) => {
            let _ = init_tx.send(Err(err));
            return;
        }
    };
    if init_tx.send(Ok(local_node_id)).is_err() {
        return;
    }

    while let Some(command) = rx.blocking_recv() {
        match command {
            Command::Shutdown => break,
            Command::Route {
                bundle,
                now,
                response,
            } => {
                let result = route_one(router.as_mut(), local_node_id, bundle, now);
                let _ = response.send(result);
            }
        }
    }

    debug!("hardy-asabr-router worker thread exiting");
}

fn route_one(
    router: &mut crate::topology::AsabrRouter,
    local_node_id: u16,
    bundle: a_sabr::bundle::Bundle,
    now: f64,
) -> RouteResult {
    let destination = bundle.destinations[0];
    let output = router
        .route(local_node_id, &bundle, now, &[])
        .map_err(Error::Route)?;

    let Some(output) = output else {
        return Ok(None);
    };

    let Some((contact, _route)) = output.lazy_get_for_unicast(destination) else {
        return Ok(None);
    };

    let rx_node = contact.borrow().info.rx_node_id;
    Ok(Some(rx_node))
}

#[async_trait]
impl LiveRoutingProvider for AsabrRoutingProvider {
    async fn route(
        &self,
        bundle: &hardy_bpa::bundle::Bundle,
    ) -> routes::Result<Option<Eid>> {
        let translated = translate::translate_bundle(bundle, self.local_node_id)
            .map_err(|e| routes::Error::Internal(Box::new(e)))?;
        let now = translate::now_asabr_time();
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(Command::Route {
                bundle: translated,
                now,
                response: response_tx,
            })
            .map_err(|_| routes::Error::Disconnected)?;
        let result = response_rx
            .await
            .map_err(|_| routes::Error::Disconnected)?
            .map_err(|e| routes::Error::Internal(Box::new(e)))?;
        Ok(result.map(|node| {
            Eid::from(IpnNodeId {
                allocator_id: 0,
                node_number: u32::from(node),
            })
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_test_contact_plan() -> PathBuf {
        let now = time::OffsetDateTime::now_utc().unix_timestamp() as f64;
        let start = now - 60.0;
        let end = now + 3600.0;
        let plan = format!(
            "node 0 n0\n\
             node 1 n1\n\
             node 2 n2\n\
             node 3 n3\n\
             node 4 n4\n\
             node 5 n5\n\
             node 6 n6\n\
             contact 1 2 {start} {end} 10000 1\n\
             contact 2 3 {start} {end} 10000 1\n\
             contact 3 4 {start} {end} 10000 1\n\
             contact 4 5 {start} {end} 10000 1\n"
        );
        let mut path = std::env::temp_dir();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!(
            "hardy-asabr-test-{}-{stamp}.cp",
            std::process::id()
        ));
        std::fs::write(&path, plan).unwrap();
        path
    }

    fn make_bundle(destination: &str) -> hardy_bpa::bundle::Bundle {
        hardy_bpa::bundle::Bundle {
            bundle: hardy_bpv7::bundle::Bundle {
                id: hardy_bpv7::bundle::Id {
                    source: "ipn:1.1".parse().unwrap(),
                    timestamp: hardy_bpv7::creation_timestamp::CreationTimestamp::now(),
                    fragment_info: None,
                },
                flags: Default::default(),
                crc_type: Default::default(),
                destination: destination.parse().unwrap(),
                report_to: Default::default(),
                lifetime: core::time::Duration::from_secs(3600),
                previous_node: None,
                age: None,
                hop_count: None,
                blocks: Default::default(),
            },
            metadata: Default::default(),
        }
    }

    fn test_config(plan_path: PathBuf) -> Config {
        Config {
            protocol_id: "asabr".into(),
            router: "SpsnHybridParenting".into(),
            contact_plan_path: plan_path,
            local_node_id: "ipn:1.0".parse().unwrap(),
        }
    }

    #[tokio::test]
    async fn provider_returns_first_hop_eid() {
        let plan = write_test_contact_plan();
        let provider = AsabrRoutingProvider::new(test_config(plan)).unwrap();

        let bundle = make_bundle("ipn:0.5.7");
        let next_hop = provider.route(&bundle).await.unwrap();

        assert_eq!(next_hop, Some("ipn:0.2.0".parse().unwrap()));
    }

    #[tokio::test]
    async fn provider_returns_none_for_unreachable_destination() {
        let plan = write_test_contact_plan();
        let provider = AsabrRoutingProvider::new(test_config(plan)).unwrap();

        // Node 6 is declared in the plan but has no contacts, so it's unreachable
        // from local node 1.
        let bundle = make_bundle("ipn:0.6.7");
        let next_hop = provider.route(&bundle).await.unwrap();

        assert_eq!(next_hop, None);
    }

    #[tokio::test]
    async fn provider_surfaces_translation_error_for_dtn_destination() {
        let plan = write_test_contact_plan();
        let provider = AsabrRoutingProvider::new(test_config(plan)).unwrap();

        let bundle = make_bundle("dtn://mars/svc");
        match provider.route(&bundle).await {
            Err(routes::Error::Internal(_)) => {}
            other => panic!("expected Internal error, got {other:?}"),
        }
    }
}
