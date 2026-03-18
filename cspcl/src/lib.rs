use cspcl::Cspcl;
use hardy_bpv7::eid::NodeId;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::Config;

pub mod cla;
pub mod config;
pub mod listen;

struct ClaInner {
    sink: Arc<dyn hardy_bpa::cla::Sink>,
    node_ids: Arc<[NodeId]>,
    cspcl: Arc<Mutex<Cspcl>>,
    peers: Arc<[config::PeerConfig]>,
}

pub struct Cla {
    name: String,
    config: Config,
    inner: spin::once::Once<ClaInner>,
    tasks: Arc<hardy_async::TaskPool>,
}

impl Cla {
    pub fn new(name: String, config: Config) -> Self {
        Self {
            name,
            config,
            inner: spin::once::Once::new(),
            tasks: Arc::new(hardy_async::TaskPool::new()),
        }
    }
}
