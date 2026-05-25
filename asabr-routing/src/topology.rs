use std::{
    fmt,
    path::{Path, PathBuf},
};

use a_sabr::{
    contact_manager::legacy::evl::EVLManager,
    contact_plan::{ContactPlan, asabr_file_lexer::FileLexer, from_asabr_lexer::ASABRContactPlan},
    node_manager::none::NoManagement,
    routing::{
        Router,
        aliases::{SpsnOptions, build_generic_router},
    },
};
use hardy_bpv7::eid::NodeId;
use tracing::debug;

use crate::{Config, Error};

type AsabrContactPlan = ContactPlan<NoManagement, EVLManager>;
pub type AsabrRouter = dyn Router<NoManagement, EVLManager>;

pub struct Topology {
    router: String,
    pub local_node_id: u16,
    pub contact_plan_path: PathBuf,
    contact_plan: AsabrContactPlan,
}

impl fmt::Debug for Topology {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Topology")
            .field("router", &self.router)
            .field("local_node_id", &self.local_node_id)
            .field("contact_plan_path", &self.contact_plan_path)
            .finish_non_exhaustive()
    }
}

impl Topology {
    pub fn load(config: &Config) -> Result<Self, Error> {
        let local_node_id = local_asabr_node_id(&config.local_node_id)?;
        let contact_plan = load_contact_plan(&config.contact_plan_path)?;

        debug!(
            router = %config.router,
            local_node_id,
            contact_plan_path = %config.contact_plan_path.display(),
            "loaded A-SABR topology scaffold"
        );

        Ok(Self {
            router: config.router.clone(),
            local_node_id,
            contact_plan_path: config.contact_plan_path.clone(),
            contact_plan,
        })
    }

    pub fn router(&self) -> &str {
        &self.router
    }

    pub fn build_router(self) -> Result<Box<AsabrRouter>, Error> {
        build_router(&self.router, self.contact_plan)
    }
}

pub(crate) fn local_asabr_node_id(node_id: &NodeId) -> Result<u16, Error> {
    match node_id {
        NodeId::Ipn(ipn) if ipn.allocator_id == 0 => {
            u16::try_from(ipn.node_number).map_err(|_| Error::NodeIdOutOfRange(ipn.node_number))
        }
        other => Err(Error::UnsupportedNodeId(other.clone())),
    }
}

fn load_contact_plan(path: &Path) -> Result<AsabrContactPlan, Error> {
    let path_string = path.to_string_lossy().into_owned();
    let mut lexer = FileLexer::new(&path_string).map_err(|source| Error::ContactPlanOpen {
        path: path.to_path_buf(),
        source,
    })?;

    ASABRContactPlan::parse::<NoManagement, EVLManager>(&mut lexer, None, None).map_err(|source| {
        Error::ContactPlanParse {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn build_router(router: &str, contact_plan: AsabrContactPlan) -> Result<Box<AsabrRouter>, Error> {
    build_generic_router::<NoManagement, EVLManager>(
        router,
        contact_plan,
        Some(SpsnOptions {
            check_priority: false,
            check_size: true,
            max_entries: 10,
        }),
    )
    .map_err(|source| Error::RouterBuild {
        router: router.to_string(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn rejects_non_ipn_local_node() {
        let config = Config {
            protocol_id: "asabr".into(),
            router: "SpsnHybridParenting".into(),
            contact_plan_path: PathBuf::from("tests/data/plan.cp"),
            local_node_id: "dtn://mars/".parse().unwrap(),
        };

        let error = Topology::load(&config).unwrap_err();

        assert!(matches!(error, Error::UnsupportedNodeId(_)));
    }

    #[test]
    fn rejects_local_node_sentinel() {
        let config = Config {
            protocol_id: "asabr".into(),
            router: "SpsnHybridParenting".into(),
            contact_plan_path: PathBuf::from("tests/data/plan.cp"),
            local_node_id: hardy_bpv7::eid::NodeId::LocalNode,
        };

        let error = Topology::load(&config).unwrap_err();

        assert!(matches!(
            error,
            Error::UnsupportedNodeId(hardy_bpv7::eid::NodeId::LocalNode)
        ));
    }

    #[test]
    fn rejects_ipn_node_above_u16_range() {
        let config = Config {
            protocol_id: "asabr".into(),
            router: "SpsnHybridParenting".into(),
            contact_plan_path: PathBuf::from("tests/data/plan.cp"),
            local_node_id: "ipn:70000.0".parse().unwrap(),
        };

        let error = Topology::load(&config).unwrap_err();

        assert!(matches!(error, Error::NodeIdOutOfRange(70000)));
    }

    #[test]
    fn rejects_ipn_allocator_namespace() {
        let config = Config {
            protocol_id: "asabr".into(),
            router: "SpsnHybridParenting".into(),
            contact_plan_path: PathBuf::from("tests/data/plan.cp"),
            local_node_id: "ipn:1.7.0".parse().unwrap(),
        };

        let error = Topology::load(&config).unwrap_err();

        assert!(matches!(error, Error::UnsupportedNodeId(_)));
    }
}
