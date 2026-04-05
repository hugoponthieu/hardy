use std::sync::Arc;

use crate::config::Config;

pub mod cla;
pub mod config;
pub mod control;
pub mod listen;
pub mod peer;

struct ClaInner {
    sink: Arc<dyn hardy_bpa::cla::Sink>,
    cspcl_sender: cspcl_bindings::SessionSender,
    peers: Arc<peer::RuntimeState>,
}

pub struct Cla {
    _name: String,
    config: Config,
    inner: spin::once::Once<ClaInner>,
    tasks: Arc<hardy_async::TaskPool>,
}

impl Cla {
    pub fn new(name: String, config: Config) -> Self {
        Self {
            _name: name,
            config,
            inner: spin::once::Once::new(),
            tasks: Arc::new(hardy_async::TaskPool::new()),
        }
    }
}
