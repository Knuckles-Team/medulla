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

/// The node property that names the mapping that produced a component version's facts
/// (MEDULLA-CQ-XSRC-R012.1).
const MAPPING_PROPERTY: &str = "mappingRef";

/// The node property that holds the time a component version's source was read, in
/// milliseconds since the epoch (MEDULLA-CQ-XSRC-R012.1).
const READ_AT_PROPERTY: &str = "readAtMs";

/// The relation of an approved entity link: `ApproveEntityLink` is the only writer
/// (MEDULLA-CQ-XSRC-R004.5), so the edge existing is the approval.
const SAME_AS: &str = "owl:sameAs";

/// The relationship of the curated contract-to-service edge (MEDULLA-SUPPLY-VOCAB-R005.9).
const COVERS_SERVICE: &str = "coversService";

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

/// `None` for a blank text, so a missing provenance property is absent, never an empty string.
fn non_blank(text: String) -> Option<String> {
    (!text.trim().is_empty()).then_some(text)
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

/// The snapshot's edges as (relation local name, source id) -> target ids.
fn outgoing_edges(core: &GraphCore) -> BTreeMap<(String, String), BTreeSet<String>> {
    let view = core.analysis_snapshot();
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
    out
}

/// The `(contract, service)` pairs of the asked contracts' `coversService` edges
/// (MEDULLA-SUPPLY-VOCAB-R005.9), read from the caller's row-projected view: a contract or a
/// service the caller may not read is absent, never counted. A contract id that names no node
/// the caller may read is `PARAMETER_INVALID`, the same answer for a denied and a missing node.
fn contract_covers(
    core: &GraphCore,
    read_authority: Option<&GraphReadAuthority>,
    contracts: &[String],
) -> Result<Vec<(String, String)>, String> {
    let visible =
        |id: &str| read_authority.is_none_or(|authority| authority.node_visible(core, id));
    let out = outgoing_edges(core);
    let mut covers = Vec::new();
    for contract in contracts {
        if !core.has_node(contract) || !visible(contract) {
            return Err(format!(
                "PARAMETER_INVALID: contract {contract} names no node the caller may read"
            ));
        }
        if let Some(services) = out.get(&(COVERS_SERVICE.to_string(), contract.clone())) {
            covers.extend(
                services
                    .iter()
                    .filter(|service| visible(service))
                    .map(|service| (contract.clone(), service.clone())),
            );
        }
    }
    Ok(covers)
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
    let visible =
        |id: &str| read_authority.is_none_or(|authority| authority.node_visible(core, id));
    let out = outgoing_edges(core);
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
    // target id -> the ids that link to it by an approved `owl:sameAs` edge.
    let mut same_as_inbound: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for ((relation, from), linked) in &out {
        if relation == SAME_AS {
            for target in linked {
                same_as_inbound
                    .entry(target.clone())
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
            let mapping = non_blank(text_property(&properties, MAPPING_PROPERTY));
            let read_at_ms = properties
                .get(READ_AT_PROPERTY)
                .and_then(serde_json::Value::as_u64);
            // A publisher joins the records of another source only across an approved
            // `owl:sameAs` edge, read in either direction (MEDULLA-CQ-XSRC-R012.1); an
            // unapproved pair has no edge, so it adds no row.
            let suppliers: BTreeSet<String> = targets("supplyChainVersionOf", &version)
                .iter()
                .flat_map(|component| targets("publishedBy", component))
                .flat_map(|supplier| {
                    let mut linked = targets(SAME_AS, &supplier);
                    linked.extend(
                        same_as_inbound
                            .get(&supplier)
                            .into_iter()
                            .flat_map(|ids| ids.iter().cloned()),
                    );
                    linked.push(supplier);
                    linked
                })
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
                        mapping: mapping.clone(),
                        read_at_ms,
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
    contracts: &[String],
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
    let fixture = absent.iter().fold(fixture, |fixture, source| {
        fixture.with_unconnected_source(source)
    });
    let roots = match resolve_roots(&fixture, roots) {
        Ok(roots) => roots,
        Err(refusal) => return Response::err(req_id, refusal),
    };
    let rows = match impact_rows(core, read_authority) {
        Ok(rows) => rows,
        Err(refusal) => return Response::err(req_id, refusal),
    };
    let covers = match contract_covers(core, read_authority, contracts) {
        Ok(covers) => covers,
        Err(refusal) => return Response::err(req_id, refusal),
    };
    match supply_chain_impact(&fixture, &roots, HOP_BUDGET, &rows, contracts, &covers) {
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
        assert_eq!(
            resolved,
            vec!["https://example.org/ns/Supplier".to_string()]
        );
        let refusal = resolve_roots(&fixture, &["Gadget".to_string()]).unwrap_err();
        assert!(refusal.starts_with("CQ_CLASS_UNKNOWN"));
    }

    fn node(core: &GraphCore, id: &str, kind: &str) {
        let properties = serde_json::json!({ "type": kind });
        core.add_node(
            id.to_string(),
            rmp_serde::to_vec_named(&properties).expect("encode node"),
        );
    }

    fn link(core: &GraphCore, from: &str, relation: &str, to: &str) {
        let properties = serde_json::json!({ "relationship": relation });
        core.add_edge(
            from.to_string(),
            to.to_string(),
            rmp_serde::to_vec_named(&properties).expect("encode edge"),
        )
        .expect("add edge");
    }

    fn version_node(core: &GraphCore, id: &str, source: &str) {
        let properties = serde_json::json!({
            "type": "ComponentVersion",
            "source": source,
            "mappingRef": "vm:leanix-components",
            "readAtMs": 1_700_000_000_123_u64,
        });
        core.add_node(
            id.to_string(),
            rmp_serde::to_vec_named(&properties).expect("encode node"),
        );
    }

    // spec: MEDULLA-CQ-XSRC-R012.1
    #[test]
    fn impact_rows_carry_provenance_and_join_only_across_an_approved_link() {
        let core = GraphCore::new();
        node(&core, "svc-a", "Service");
        version_node(&core, "ver-1", "leanix-mcp");
        node(&core, "cmp-1", "SoftwareComponent");
        node(&core, "leanix:fs-1", "Supplier");
        node(&core, "erpnext:SUP-1", "Supplier");
        link(&core, "svc-a", "usesComponent", "ver-1");
        link(&core, "ver-1", "supplyChainVersionOf", "cmp-1");
        link(&core, "cmp-1", "publishedBy", "leanix:fs-1");

        // Before the approval no edge exists, so the other source's record joins no row.
        let before = impact_rows(&core, None).expect("rows read");
        let suppliers: Vec<&str> = before.iter().map(|row| row.supplier.as_str()).collect();
        assert_eq!(suppliers, ["leanix:fs-1"]);

        // The approved link joins the second connector's supplier, in either direction.
        link(&core, "erpnext:SUP-1", "owl:sameAs", "leanix:fs-1");
        let after = impact_rows(&core, None).expect("rows read");
        let suppliers: Vec<&str> = after.iter().map(|row| row.supplier.as_str()).collect();
        assert_eq!(suppliers, ["erpnext:SUP-1", "leanix:fs-1"]);
        for row in &after {
            assert_eq!(row.source, "leanix-mcp");
            assert_eq!(row.mapping.as_deref(), Some("vm:leanix-components"));
            assert_eq!(row.read_at_ms, Some(1_700_000_000_123));
        }
    }

    // spec: MEDULLA-SUPPLY-VOCAB-R005.9
    #[test]
    fn contract_covers_reads_only_the_curated_edges_of_readable_contracts() {
        let core = GraphCore::new();
        node(&core, "contract-1", "CustomerContract");
        node(&core, "contract-2", "CustomerContract");
        node(&core, "svc-a", "Service");
        node(&core, "svc-b", "Service");
        link(&core, "contract-1", "coversService", "svc-b");
        link(&core, "contract-1", "coversService", "svc-a");
        link(&core, "contract-2", "mentions", "svc-a");

        let covers = contract_covers(
            &core,
            None,
            &["contract-1".to_string(), "contract-2".to_string()],
        )
        .expect("both contracts name a node");
        assert_eq!(
            covers,
            vec![
                ("contract-1".to_string(), "svc-a".to_string()),
                ("contract-1".to_string(), "svc-b".to_string()),
            ],
            "only coversService edges count; contract-2 has none"
        );
        let refusal = contract_covers(&core, None, &["contract-9".to_string()]).unwrap_err();
        assert!(refusal.starts_with("PARAMETER_INVALID"), "{refusal}");
    }
}
