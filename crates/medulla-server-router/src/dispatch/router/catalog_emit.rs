//! Engine-held records reach the metadata catalog through the one emitter
//! (MEDULLA-TYPED-PACKS-R108.3).
//!
//! An Agent Library publish, a workflow registration and a graph creation each hold a
//! record the engine owns. After the record is durable, `emit_asset` turns it into the
//! catalog triples that describe it and the triples are committed as one served
//! `AddTriples` on the graph the request was issued on, so the catalog write guard of
//! `R108.4` and the ranking index of `R108.16` see them like any other asset. Connector
//! asset nodes stay with the arbora batch; no family keeps its own index.

use super::*;

/// Emit one record's catalog nodes into `graph`. A refusal text means the record is durable
/// but its catalog nodes are not; replaying the same request emits them again (the triples
/// are idempotent).
#[cfg(feature = "rdf")]
pub(super) async fn commit_asset(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
    graph: &str,
    record: medulla_catalog::emit::AssetRecord<'_>,
) -> Result<(), String> {
    let evidence = asset_evidence(state, req, verified_context, graph, &record).await;
    let links = medulla_catalog::emit::AssetLinks {
        owner: evidence.owner.as_deref(),
        steward: evidence.steward.as_deref(),
        graph_revision: evidence.graph_revision,
        changed_at_ms: Some(evidence.changed_at_ms),
        ..medulla_catalog::emit::AssetLinks::default()
    };
    let triples = medulla_catalog::emit::emit_asset(record, &links)
        .map_err(|error| error.to_string())?;
    let response = dispatch_graph_op(
        state,
        graph,
        req.id,
        req.agent_id.as_deref(),
        verified_context,
        Method::AddTriples {
            turtle: String::new(),
            ntriples: medulla_catalog::emit::to_ntriples(&triples),
        },
    )
    .await;
    match response.refusal_text() {
        Some(refusal) => Err(format!("CATALOG_EMIT_FAILED: {refusal}")),
        None => Ok(()),
    }
}

/// Answer `response` unchanged when the catalog nodes committed, else the emit refusal.
#[cfg(feature = "rdf")]
pub(super) fn after_emit(response: Response, req_id: u64, emitted: Result<(), String>) -> Response {
    match emitted {
        Ok(()) => response,
        Err(refusal) => Response::err(req_id, refusal),
    }
}

/// What a node carries beyond its source record id: who owns and stewards it, the graph
/// revision it was catalogued from and when (MEDULLA-TYPED-PACKS-R108.5).
#[cfg(feature = "rdf")]
struct AssetEvidence {
    owner: Option<String>,
    steward: Option<String>,
    graph_revision: Option<u64>,
    changed_at_ms: u64,
}

/// The principal as an IRI: an IRI stays as it is, anything else is minted under the
/// principal namespace.
#[cfg(feature = "rdf")]
fn principal_iri(principal: &str) -> String {
    if principal.contains(':') && !principal.starts_with("principal:") {
        principal.to_string()
    } else {
        format!("urn:medulla:principal:{principal}")
    }
}

/// Read the evidence of one record. The node is a projection, so a stewardship or graph read
/// that fails leaves that field off the node (and the node flagged `owner_missing`); it never
/// fails the record's own write.
#[cfg(feature = "rdf")]
async fn asset_evidence(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified_context: &VerifiedRequestContext,
    graph: &str,
    record: &medulla_catalog::emit::AssetRecord<'_>,
) -> AssetEvidence {
    use medulla_catalog_types::stewardship::StewardRole;
    use medulla_server_graph::dispatch::graph_client::GraphTarget;

    let changed_at_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let mut evidence = AssetEvidence {
        owner: None,
        steward: None,
        graph_revision: None,
        changed_at_ms,
    };
    medulla_server_graph::dispatch::shared::ensure_graph_resident(state, graph).await;
    {
        let current = medulla_server_graph::dispatch::shared::timed_read(state).await;
        if let Ok((core, _)) = medulla_server_graph::dispatch::shared::authorized_graph_read(
            &current,
            graph,
            verified_context,
            &format!("GRAPH_NOT_FOUND: {graph}"),
        ) {
            evidence.graph_revision = Some(core.version());
        }
    }
    let Ok(probe) = medulla_catalog::emit::emit_asset(
        record.clone(),
        &medulla_catalog::emit::AssetLinks::default(),
    ) else {
        return evidence;
    };
    let Some(subject) = probe.first().map(|triple| triple.subject.clone()) else {
        return evidence;
    };
    let target = GraphTarget {
        graph,
        req_id: req.id,
        caller: req.agent_id.as_deref(),
        verified: verified_context,
    };
    if let Ok(assignments) =
        medulla_server_graph::dispatch::stewardship::load_assignments(state, &target).await
    {
        let scopes = medulla_core::stewardship::TargetScopes {
            asset: Some(subject),
            concept_scheme: None,
            domain: None,
        };
        let resolved = medulla_core::stewardship::resolve(&assignments, &scopes);
        let first = |role: StewardRole| {
            resolved
                .iter()
                .find(|assignment| assignment.role == role)
                .map(|assignment| principal_iri(&assignment.principal))
        };
        evidence.owner = first(StewardRole::Owner);
        evidence.steward = first(StewardRole::Steward);
    }
    // A product names its owner when it is registered; the stewardship store answers first.
    if evidence.owner.is_none() {
        if let medulla_catalog::emit::AssetRecord::Product(product) = record {
            evidence.owner = Some(principal_iri(product.owner));
        }
    }
    evidence
}
