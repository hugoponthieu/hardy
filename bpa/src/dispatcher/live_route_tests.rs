use super::*;
use crate::routes;
use hardy_bpv7::eid::{Eid, IpnNodeId, NodeId};
use std::sync::atomic::{AtomicUsize, Ordering};

struct MockLiveRoutingProvider {
    calls: AtomicUsize,
    response: Option<Eid>,
    fail: bool,
}

#[async_trait]
impl routes::LiveRoutingProvider for MockLiveRoutingProvider {
    async fn route(&self, _bundle: &bundle::Bundle) -> routes::Result<Option<Eid>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.fail {
            Err(routes::Error::Disconnected)
        } else {
            Ok(self.response.clone())
        }
    }
}

fn make_bundle(destination: &str) -> bundle::Bundle {
    bundle::Bundle {
        bundle: hardy_bpv7::bundle::Bundle {
            id: hardy_bpv7::bundle::Id {
                source: "ipn:0.99.1".parse().unwrap(),
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

async fn make_dispatcher_with_live_provider(
    provider: MockLiveRoutingProvider,
) -> (Arc<Dispatcher>, Arc<rib::Rib>, Arc<MockLiveRoutingProvider>) {
    let node_ids = Arc::new(node_ids::NodeIds {
        ipn: Some(IpnNodeId {
            allocator_id: 0,
            node_number: 1,
        }),
        dtn: None,
    });
    let store = Arc::new(storage::Store::new(
        core::num::NonZeroUsize::new(16).unwrap(),
        Arc::new(storage::MetadataMemStorage::new(&Default::default())),
        Arc::new(storage::BundleMemStorage::new(&Default::default())),
    ));
    let rib = rib::RibBuilder::new()
        .build(node_ids.clone(), store.clone())
        .await
        .unwrap();
    let filter_engine = Arc::new(filter::FilterEngine::new());
    let keys_registry = Arc::new(keys::registry::Registry::new());
    let provider = Arc::new(provider);

    let (dispatcher, _start) = Dispatcher::new_inner(
        false,
        core::num::NonZeroUsize::new(16).unwrap(),
        core::num::NonZeroUsize::new(1).unwrap(),
        node_ids,
        store,
        rib.clone(),
        keys_registry,
        filter_engine,
        Some(provider.clone() as Arc<dyn routes::LiveRoutingProvider>),
    );

    (dispatcher, rib, provider)
}

fn ipn_node(node_number: u32) -> NodeId {
    NodeId::Ipn(IpnNodeId {
        allocator_id: 0,
        node_number,
    })
}

fn make_local_service(service_number: u32) -> Arc<services::registry::Service> {
    Arc::new(services::registry::Service {
        service: services::registry::ServiceImpl::LowLevel(Arc::new(
            crate::services::tests::NullService,
        )),
        service_id: hardy_bpv7::eid::Service::Ipn(service_number),
    })
}

#[tokio::test]
async fn live_provider_is_consulted_before_rib_fallback() {
    let (dispatcher, rib, provider) = make_dispatcher_with_live_provider(MockLiveRoutingProvider {
        calls: AtomicUsize::new(0),
        response: Some("ipn:0.2.0".parse().unwrap()),
        fail: false,
    })
    .await;
    rib.add_forward(ipn_node(2), 42).await;
    rib.add_forward(ipn_node(9), 7).await;

    let mut bundle = make_bundle("ipn:0.9.7");
    let result = dispatcher.lookup_route(&mut bundle).await;

    assert!(matches!(result, Some(rib::FindResult::Forward(42))));
    assert_eq!(
        bundle.metadata.read_only.next_hop,
        Some("ipn:0.2.0".parse().unwrap())
    );
    assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn live_provider_error_falls_back_to_rib() {
    let (dispatcher, rib, provider) = make_dispatcher_with_live_provider(MockLiveRoutingProvider {
        calls: AtomicUsize::new(0),
        response: None,
        fail: true,
    })
    .await;
    rib.add_forward(ipn_node(9), 7).await;

    let mut bundle = make_bundle("ipn:0.9.7");
    let result = dispatcher.lookup_route(&mut bundle).await;

    assert!(matches!(result, Some(rib::FindResult::Forward(7))));
    assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn live_provider_is_not_consulted_for_local_admin_route() {
    let (dispatcher, _rib, provider) =
        make_dispatcher_with_live_provider(MockLiveRoutingProvider {
            calls: AtomicUsize::new(0),
            response: Some("ipn:0.2.0".parse().unwrap()),
            fail: false,
        })
        .await;

    let mut bundle = make_bundle("ipn:0.1.0");
    let result = dispatcher.lookup_route(&mut bundle).await;

    assert!(matches!(result, Some(rib::FindResult::AdminEndpoint)));
    assert_eq!(bundle.metadata.read_only.next_hop, None);
    assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn live_provider_is_not_consulted_for_local_service_route() {
    let (dispatcher, rib, provider) = make_dispatcher_with_live_provider(MockLiveRoutingProvider {
        calls: AtomicUsize::new(0),
        response: Some("ipn:0.2.0".parse().unwrap()),
        fail: false,
    })
    .await;
    rib.add_service("ipn:0.1.42".parse().unwrap(), make_local_service(42))
        .await;

    let mut bundle = make_bundle("ipn:0.1.42");
    let result = dispatcher.lookup_route(&mut bundle).await;

    assert!(matches!(result, Some(rib::FindResult::Deliver(_))));
    assert_eq!(bundle.metadata.read_only.next_hop, None);
    assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn live_provider_ok_none_falls_back_to_rib() {
    let (dispatcher, rib, provider) = make_dispatcher_with_live_provider(MockLiveRoutingProvider {
        calls: AtomicUsize::new(0),
        response: None,
        fail: false,
    })
    .await;
    rib.add_forward(ipn_node(9), 7).await;

    let mut bundle = make_bundle("ipn:0.9.7");
    let result = dispatcher.lookup_route(&mut bundle).await;

    assert!(matches!(result, Some(rib::FindResult::Forward(7))));
    assert_eq!(
        bundle.metadata.read_only.next_hop,
        Some("ipn:0.9.7".parse().unwrap())
    );
    assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn live_provider_unresolved_next_hop_falls_back_to_rib() {
    let (dispatcher, rib, provider) = make_dispatcher_with_live_provider(MockLiveRoutingProvider {
        calls: AtomicUsize::new(0),
        response: Some("ipn:0.200.0".parse().unwrap()),
        fail: false,
    })
    .await;
    rib.add_forward(ipn_node(9), 7).await;

    let mut bundle = make_bundle("ipn:0.9.7");
    let result = dispatcher.lookup_route(&mut bundle).await;

    assert!(matches!(result, Some(rib::FindResult::Forward(7))));
    assert_eq!(
        bundle.metadata.read_only.next_hop,
        Some("ipn:0.9.7".parse().unwrap())
    );
    assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
}
