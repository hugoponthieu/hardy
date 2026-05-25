use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported node id for A-SABR v1: {0}")]
    UnsupportedNodeId(hardy_bpv7::eid::NodeId),
    #[error("ipn node number {0} exceeds A-SABR NodeID range")]
    NodeIdOutOfRange(u32),
    #[error("bundle destination is not a concrete node id: {0}")]
    InvalidDestinationNodeId(hardy_bpv7::eid::Error),
    #[error("failed to open contact plan '{path}': {source}")]
    ContactPlanOpen {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse contact plan '{path}': {source}")]
    ContactPlanParse {
        path: PathBuf,
        source: a_sabr::errors::ASABRError,
    },
    #[error("failed to build router '{router}': {source}")]
    RouterBuild {
        router: String,
        source: a_sabr::errors::ASABRError,
    },
}
