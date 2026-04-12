use hardy_bpa::cla::{Cla, ClaAddress, ForwardBundleResult, Result, Sink};
use hardy_bpa::{Bytes, async_trait};
use hardy_bpv7::eid::NodeId;

pub struct Cspcl {}

#[async_trait]
impl Cla for Cspcl {
    async fn on_register(&self, sink: Box<dyn Sink>, node_ids: &[NodeId]) {
        todo!()
    }
    async fn on_unregister(&self) {
        todo!()
    }
    async fn forward(
        &self,
        queue: Option<u32>,
        cla_addr: &ClaAddress,
        bundle: Bytes,
    ) -> Result<ForwardBundleResult> {
        todo!()
    }
}
