use hardy_bpv7::eid::NodeId;

use crate::Error;

pub(crate) fn translate_bundle(
    bundle: &hardy_bpa::bundle::Bundle,
    local_node_id: u16,
) -> Result<a_sabr::bundle::Bundle, Error> {
    let destination = bundle
        .bundle
        .destination
        .to_node_id()
        .map_err(Error::InvalidDestinationNodeId)?;

    let destination = match destination {
        NodeId::Ipn(ipn) if ipn.allocator_id == 0 => u16::try_from(ipn.node_number)
            .map_err(|_| Error::NodeIdOutOfRange(ipn.node_number))?,
        other => return Err(Error::UnsupportedNodeId(other)),
    };

    Ok(a_sabr::bundle::Bundle {
        source: local_node_id,
        destinations: vec![destination],
        priority: 0,
        size: 1.0,
        expiration: bundle.expiry().unix_timestamp() as f64,
    })
}

pub(crate) fn now_asabr_time() -> f64 {
    time::OffsetDateTime::now_utc().unix_timestamp() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bundle(destination: &str) -> hardy_bpa::bundle::Bundle {
        hardy_bpa::bundle::Bundle {
            bundle: hardy_bpv7::bundle::Bundle {
                id: hardy_bpv7::bundle::Id {
                    source: "ipn:0.1.1".parse().unwrap(),
                    timestamp: hardy_bpv7::creation_timestamp::CreationTimestamp::now(),
                    fragment_info: None,
                },
                flags: Default::default(),
                crc_type: Default::default(),
                destination: destination.parse().unwrap(),
                report_to: Default::default(),
                lifetime: core::time::Duration::from_secs(60),
                previous_node: None,
                age: None,
                hop_count: None,
                blocks: Default::default(),
            },
            metadata: Default::default(),
        }
    }

    #[test]
    fn translates_ipn_bundle_to_asabr_bundle() {
        let bundle = make_bundle("ipn:0.8.7");

        let translated = translate_bundle(&bundle, 1).unwrap();

        assert_eq!(translated.source, 1);
        assert_eq!(translated.destinations, vec![8]);
        assert_eq!(translated.priority, 0);
        assert_eq!(translated.size, 1.0);
    }

    #[test]
    fn translator_rejects_dtn_destination() {
        let bundle = make_bundle("dtn://mars/svc");

        match translate_bundle(&bundle, 1) {
            Err(Error::UnsupportedNodeId(_)) => {}
            Err(other) => panic!("expected UnsupportedNodeId, got {other:?}"),
            Ok(_) => panic!("expected translation error, got Ok"),
        }
    }

    #[test]
    fn translator_rejects_ipn_destination_above_u16_range() {
        let bundle = make_bundle("ipn:0.70000.7");

        match translate_bundle(&bundle, 1) {
            Err(Error::NodeIdOutOfRange(70000)) => {}
            Err(other) => panic!("expected NodeIdOutOfRange(70000), got {other:?}"),
            Ok(_) => panic!("expected translation error, got Ok"),
        }
    }
}
