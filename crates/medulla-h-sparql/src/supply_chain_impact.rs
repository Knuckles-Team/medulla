//! `Method::SupplyChainImpact` (MEDULLA-CQ-XSRC-R011): serve the supply-chain impact report.
//!
//! The handler builds the connected-source view from the caller's own owner-scoped foreign
//! catalog (`ForeignSourceCatalog::registry_for`) and the approved mappings of the request
//! graph's schema sources, marks every authority-row source outside it unconnected, plans the
//! question's class roots with `select_sources`, derives the impact rows from the nodes and
//! edges the caller may read, and assembles the typed report. A class whose authoritative
//! source is not connected is a typed abstention in the answer, never a silent fallback and
//! never an error. The method writes nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use medulla_federation::federation::ConnectedSources;
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_api::compute::compute_off_lock;
use medulla_server_api::state::ServerState;
use medulla_server_ops::api::query_txn::ForeignStateExt;
use medulla_source_types::supply_chain_report::{ImpactKind, ImpactRow};
use medulla_virtual_graph::source_selection::{
    connected_sources_from_turtle, fixture_from_schema_sources, CoverageFixture,
};
use medulla_virtual_graph::supply_chain_impact::supply_chain_impact;
use tokio::sync::RwLock;

/// Hop budget of the plan: service to component version to component to supplier is three
/// object properties.
const HOP_BUDGET: usize = 3;

/// Most service nodes one answer reads; a graph holding more answers the first page.
const MAX_SERVICES: usize = 1000;

/// The node property that names the source that served a component version.
const SOURCE_PROPERTY: &str = "source";

/// The local name of an IRI (`...#Service` and `.../Service` give `Service`).
fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

/// Resolve each requested root to a class the fixture names, by IRI or local name. A root
/// the schema does not name refuses `CQ_CLASS_UNKNOWN`.
fn resolve_roots(fixture: &CoverageFixture, roots: &[String]) -> Result<Vec<String>, String> {
    let known = fixture.class_names();
    roots
        .iter()
        .map(|root| {
            known
                .iter()
                .find(|class| class.as_str() == root || local_name(class) == root)
                .cloned()
                .ok_or_else(|| {
                    format!("CQ_CLASS_UNKNOWN: the graph's schema names no class {root}")
                })
        })
        .collect()
}

/// The text of a scalar node property.
fn text_property(properties: &serde_json::Map<String, serde_json::Value>, name: &str) -> String {
    properties
        .get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The relation an edge blob carries: `relationship`, else `type`, else `label`.
fn relation_of(blob: &[u8]) -> Option<String> {
    let value = medulla_types::msgpack::decode_property_value(blob).ok()?;
    let object = value.as_object()?;
    ["relationship", "type", "label"]
        .iter()
        .find_map(|key| object.get(*key).and_then(serde_json::Value::as_str))
        .map(|relation| local_name(relation).to_string())
}

/// One impact row per (service, component version, supplier, vulnerability) the caller may
/// read: the service `usesComponent` a version, the version is `supplyChainVersionOf` a
/// component, the component is `publishedBy` a supplier, and a vulnerability
/// `affectsComponent` the version. The row's source is the version node's `source`
/// property; a version with no source refuses the whole answer, it is never defaulted
/// (`CQ_XSRC_ITEM_WITHOUT_SOURCE`).
fn impact_rows(
    core: &GraphCore,
    read_authority: Option<&GraphReadAuthority>,
) -> Result<Vec<ImpactRow>, String> {
    let view = core.analysis_snapshot();
    let visible = |id: &str| read_authority.is_none_or(|authority| authority.node_visible(core, id));
    // relation local name -> source id -> target ids, from the snapshot's edges.
    let mut out: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for ((source, target), blobs) in &view.edge_properties {
        for blob in blobs {
            if let Some(relation) = relation_of(blob) {
                out.entry((relation, source.clone()))
                    .or_default()
                    .insert(target.clone());
            }
        }
    }
    let targets = |relation: &str, from: &str| -> Vec<String> {
        out.get(&(relation.to_string(), from.to_string()))
            .map(|ids| ids.iter().cloned().collect())
            .unwrap_or_default()
    };
    // version id -> vulnerability ids that affect it.
    let mut affecting: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for ((relation, from), versions) in &out {
        if relation == "affectsComponent" && visible(from) {
            for version in versions {
                affecting
                    .entry(version.clone())
                    .or_default()
                    .insert(from.clone());
            }
        }
    }
    let mut rows = Vec::new();
    for (service, _) in core.get_nodes_by_label("Service", MAX_SERVICES) {
        if !visible(&service) {
            continue;
        }
        for version in targets("usesComponent", &service) {
            if !visible(&version) {
                continue;
            }
            let blob = core.get_node_properties(&version).unwrap_or_default();
            let value = medulla_types::msgpack::decode_property_value(&blob).unwrap_or_default();
            let properties = value.as_object().cloned().unwrap_or_default();
            let source = text_property(&properties, SOURCE_PROPERTY);
            if source.trim().is_empty() {
                return Err(format!(
                    "CQ_XSRC_ITEM_WITHOUT_SOURCE: component version {version} names no source"
                ));
            }
            let suppliers: BTreeSet<String> = targets("supplyChainVersionOf", &version)
                .iter()
                .flat_map(|component| targets("publishedBy", component))
                .filter(|supplier| visible(supplier))
                .collect();
            let vulnerabilities: Vec<Option<String>> = match affecting.get(&version) {
                Some(ids) if !ids.is_empty() => ids.iter().cloned().map(Some).collect(),
                _ => vec![None],
            };
            for supplier in &suppliers {
                for vulnerability in &vulnerabilities {
                    rows.push(ImpactRow {
                        service: service.clone(),
                        supplier: supplier.clone(),
                        component_version: version.clone(),
                        vulnerability: vulnerability.clone(),
                        impact_kind: ImpactKind::RuntimeChain,
                        source: source.clone(),
                    });
                }
            }
        }
    }
    Ok(rows)
}

/// Serve one `SupplyChainImpact` for the request graph.
pub async fn handle(
    req_id: u64,
    state: &Arc<RwLock<ServerState>>,
    core: &GraphCore,
    read_authority: Option<&GraphReadAuthority>,
    roots: &[String],
) -> Response {
    if roots.is_empty() || roots.iter().any(|root| root.trim().is_empty()) {
        return Response::err(
            req_id,
            "PARAMETER_INVALID: roots must name at least one class and no blank class",
        );
    }
    let Some(carrier) = read_authority.and_then(GraphReadAuthority::carrier) else {
        return Response::err(
            req_id,
            "TENANT_DENIED: the supply-chain impact report needs a verified tenant caller",
        );
    };
    let sources = core.schema_sources();
    let parsed = compute_off_lock(req_id, move || {
        let fixture = fixture_from_schema_sources(&sources)?;
        let documents: Vec<&str> = sources
            .core
            .values()
            .chain(sources.dynamic.values())
            .filter_map(|source| source.ontology_ttl.as_deref())
            .collect();
        let approved = connected_sources_from_turtle(&documents)?;
        Ok::<_, String>((fixture, approved))
    })
    .await;
    let (fixture, approved) = match parsed {
        Err(response) => return response,
        Ok(Err(error)) => return Response::err(req_id, format!("PARAMETER_INVALID: {error}")),
        Ok(Ok(pair)) => pair,
    };
    let connected = {
        let state = state.read().await;
        let registry = state
            .foreign_sources()
            .registry_for(carrier, &state.isolation);
        ConnectedSources::from_registry(registry.registry(), &approved)
    };
    let absent: BTreeSet<String> = fixture
        .rows()
        .map(|row| row.source.clone())
        .filter(|source| !connected.contains(source))
        .collect();
    let fixture = absent
        .iter()
        .fold(fixture, |fixture, source| fixture.with_unconnected_source(source));
    let roots = match resolve_roots(&fixture, roots) {
        Ok(roots) => roots,
        Err(refusal) => return Response::err(req_id, refusal),
    };
    let rows = match impact_rows(core, read_authority) {
        Ok(rows) => rows,
        Err(refusal) => return Response::err(req_id, refusal),
    };
    match supply_chain_impact(&fixture, &roots, HOP_BUDGET, &rows) {
        Ok(report) => Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::query::SupplyChainImpact>(
                report,
            ),
        ),
        Err(error) => Response::err(req_id, format!("CQ_XSRC_ITEM_WITHOUT_SOURCE: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-CQ-XSRC-R011
    #[test]
    fn a_root_the_schema_does_not_name_is_refused_by_code() {
        let fixture = CoverageFixture::new().with_authority(
            "https://example.org/ns/Supplier",
            "erpnext-suppliers",
            "erpnext-mcp",
            1,
            1,
        );
        let resolved = resolve_roots(&fixture, &["Supplier".to_string()]).unwrap();
        assert_eq!(resolved, vec!["https://example.org/ns/Supplier".to_string()]);
        let refusal = resolve_roots(&fixture, &["Gadget".to_string()]).unwrap_err();
        assert!(refusal.starts_with("CQ_CLASS_UNKNOWN"));
    }
}
