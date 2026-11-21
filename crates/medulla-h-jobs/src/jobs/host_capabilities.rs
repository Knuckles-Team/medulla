//! Private implementation module for the analytics job handler: the heavy capabilities a host
//! declares in the graph (MEDULLA-PLACEMENT-R004.3).
//!
//! A host declares its heavy capabilities as one `placement:Host` node of the deployment-wide
//! commons graph: the node id is the host IRI and `placement:capabilities` is an array of
//! `HeavyCapability` names. The claim callers read the declaration at claim time; a worker
//! never asserts a heavy capability itself.

use std::collections::BTreeSet;
use std::sync::Arc;

use medulla_decision_types::decision::statistical::placement::{
    HeavyCapability, PLACEMENT_FACTS_INVALID,
};
use medulla_identity_types::tenancy::COMMONS_GRAPH;
use medulla_server_api::state::ServerState;
use tokio::sync::RwLock;

use super::helpers::opaque_worker_capability;

/// The most `placement:Host` nodes one declaration read scans.
const MAX_DECLARED_HOSTS: usize = 512;

/// The graph label of a host declaration.
fn host_class() -> &'static str {
    medulla_types::ontology_iri!("placement#Host")
}

/// The property that lists a host's heavy capabilities.
fn capabilities_property() -> &'static str {
    medulla_types::ontology_iri!("placement#capabilities")
}

/// The heavy capabilities `host_iri` declares in the commons graph: none for an absent graph or
/// node, a `PLACEMENT_FACTS_INVALID` refusal for a malformed declaration. An engine-internal
/// read of an engine-owned deployment fact: no caller authority applies and nothing read is
/// returned to a caller.
pub(crate) async fn declared_for(
    state: &Arc<RwLock<ServerState>>,
    host_iri: &str,
) -> Result<BTreeSet<HeavyCapability>, String> {
    let core = {
        let current = state.read().await;
        match current.registry.get(COMMONS_GRAPH) {
            Some(entry) => entry.core.clone(),
            None => return Ok(BTreeSet::new()),
        }
    };
    let invalid = |what: &str| format!("{PLACEMENT_FACTS_INVALID}: {host_iri}: {what}");
    let Some((_, properties)) = core
        .get_nodes_by_label(host_class(), MAX_DECLARED_HOSTS)
        .into_iter()
        .find(|(id, _)| id == host_iri)
    else {
        return Ok(BTreeSet::new());
    };
    let value = medulla_types::msgpack::decode_property_value(&properties)
        .map_err(|_| invalid("the properties are not a valid property value"))?;
    let names = value
        .as_object()
        .and_then(|node| node.get(capabilities_property()))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid("placement:capabilities must be an array"))?;
    let mut declared = BTreeSet::new();
    for name in names {
        let capability: HeavyCapability = serde_json::from_value(name.clone())
            .map_err(|_| invalid("placement:capabilities names an unknown capability"))?;
        declared.insert(capability);
    }
    Ok(declared)
}

/// The claim capability strings of the host the worker `worker_instance` registered on, when
/// the registry holds a live record of it owned by `principal`; none for an unregistered
/// worker. A read error refuses the claim rather than claiming without the declaration.
pub(crate) async fn registered_host_declaration(
    state: &Arc<RwLock<ServerState>>,
    principal: &str,
    worker_instance: &str,
) -> Result<Vec<String>, String> {
    let registry = state.read().await.worker_registry.clone();
    let host_iri = {
        let registry = registry.lock();
        let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
        match (
            registry.get_live(worker_instance, now_ms),
            registry.owner_of(worker_instance),
        ) {
            (Some(record), Some(owner)) if owner == principal => Some(record.host_iri.clone()),
            _ => None,
        }
    };
    match host_iri {
        Some(host_iri) => declared_for(state, &host_iri)
            .await
            .map(|declared| heavy_capability_strings(&declared)),
        None => Ok(Vec::new()),
    }
}

/// The snake-case name of a heavy capability, as a job names it in `required_capabilities`.
pub(crate) fn heavy_capability_name(capability: HeavyCapability) -> &'static str {
    match capability {
        HeavyCapability::Gpu => "gpu",
        HeavyCapability::QuantumSimulation => "quantum_simulation",
        HeavyCapability::LargeMemory => "large_memory",
        HeavyCapability::Backtest => "backtest",
        HeavyCapability::VideoTranscode => "video_transcode",
    }
}

/// The five heavy capability names.
pub(crate) const HEAVY_CAPABILITY_NAMES: [&str; 5] = [
    "gpu",
    "quantum_simulation",
    "large_memory",
    "backtest",
    "video_transcode",
];

/// The claim capability string of each declared heavy capability: the same mapping a job that
/// names the capability in `required_capabilities` is indexed under.
pub(crate) fn heavy_capability_strings(set: &BTreeSet<HeavyCapability>) -> Vec<String> {
    set.iter()
        .map(|capability| opaque_worker_capability(heavy_capability_name(*capability)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn state_with_commons(nodes: &[(&str, serde_json::Value)]) -> Arc<RwLock<ServerState>> {
        let state = Arc::new(RwLock::new(medulla_server::seams_install::new_state_for_test(
            "host-capabilities-test-secret",
            ServerState::test_isolation("host-capabilities-test-issuer"),
        )));
        {
            let mut guard = state.write().await;
            guard
                .registry
                .create_graph(
                    COMMONS_GRAPH,
                    medulla_protocol_types::protocol::GraphType::Global,
                    None,
                )
                .unwrap();
            let core = guard.registry.get(COMMONS_GRAPH).unwrap().core.clone();
            for (iri, capabilities) in nodes {
                let props = serde_json::json!({
                    "type": host_class(),
                    capabilities_property(): capabilities,
                });
                core.add_node(iri.to_string(), rmp_serde::to_vec_named(&props).unwrap());
            }
        }
        state
    }

    // spec: MEDULLA-PLACEMENT-R004.3
    #[tokio::test]
    async fn declared_for_reads_the_commons_host_node() {
        let state = state_with_commons(&[
            ("host:alpha", serde_json::json!(["backtest"])),
            ("host:odd", serde_json::json!(["teleport"])),
        ])
        .await;
        assert_eq!(
            declared_for(&state, "host:alpha").await.unwrap(),
            BTreeSet::from([HeavyCapability::Backtest])
        );
        assert!(declared_for(&state, "host:absent").await.unwrap().is_empty());
        let refused = declared_for(&state, "host:odd").await.unwrap_err();
        assert!(refused.starts_with(PLACEMENT_FACTS_INVALID), "{refused}");
        assert_eq!(
            heavy_capability_strings(&BTreeSet::from([HeavyCapability::Backtest])),
            [opaque_worker_capability("backtest")]
        );
    }
}
