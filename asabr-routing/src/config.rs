use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub protocol_id: String,
    pub router: String,
    pub contact_plan_path: PathBuf,
    pub local_node_id: hardy_bpv7::eid::NodeId,
}
