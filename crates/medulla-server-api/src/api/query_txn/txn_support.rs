//! Transaction helpers moved below the handler crates so wire can reach them (MEDULLA-CONTRACT-R056.8).

#[cfg(feature = "sparql")]
use medulla_types::protocol::Method;
#[cfg(feature = "graphql")]
use medulla_types::protocol::ResultPayload;
#[cfg(feature = "graphql")]
use crate::access::CarrierAuthority;
#[cfg(feature = "graphql")]
use crate::server::ServerState;
#[cfg(feature = "graphql")]
use std::sync::Arc;
#[cfg(feature = "graphql")]
use tokio::sync::RwLock;

/// Lower a stream of RDF triples to graph-native `AddNode`/`AddEdge` methods
/// (CONCEPT:MEDULLA-KG.txn.extended-cross-modal/362), mirroring the canonical `medulla_rdf::mapping::load_triples`
/// property-graph projection so the durable rows match the in-memory model:
///   * literal object  → a property `{predicate: literal-cell}` on the subject node;
///   * resource object → subject + object nodes + an edge `{"relationship": predicate}`,
///     and (for `rdf:type`) the subject's `type` label.
///
/// The SAME lowered `Vec<Method>` is applied both durably (`apply_method_rows`) and
/// in-memory (`apply_staged`), so the two are identical by construction.
/// Returns `(methods, schema_refs)` — `schema_refs` (BUG A3, 2026-08-12) is
/// [`medulla_rdf::mapping::LoweredTripleGraph::schema_refs`] passed through
/// unchanged: one entry per schema-defining triple occurrence this batch
/// contributed. This lowering produces GENERIC `AddNode`/`AddEdge` methods
/// with no room to carry a "this is schema" signal on the wire, so a caller
/// that auto-commits immediately (the pgwire off-txn `SPARQL UPDATE` seam,
/// `WireSession::stage_or_commit_owl`) MUST mark these on the live core
/// itself right after the methods commit — see that function's doc for why
/// a staged (explicit multi-statement transaction) caller does not yet.
#[cfg(feature = "sparql")]
pub fn triples_to_methods(
    triples: &[medulla_rdf::oxrdf::Triple],
) -> Result<(Vec<Method>, Vec<String>), String> {
    let lowered = medulla_rdf::mapping::lower_triples(triples.iter().cloned())?;
    let mut methods = Vec::with_capacity(lowered.nodes.len() + lowered.edges.len());
    for (id, blob) in lowered.nodes {
        methods.push(Method::AddNode {
            node_id: id,
            properties_msgpack: blob,
        });
    }
    for (source, target, blob) in lowered.edges {
        methods.push(Method::AddEdge {
            source_id: source,
            target_id: target,
            properties_msgpack: blob,
        });
    }
    Ok((methods, lowered.schema_refs))
}

/// Returns `(methods, schema_refs)` — see [`triples_to_methods`]'s doc.
#[cfg(feature = "sparql")]
pub fn construct_view_to_methods(
    snap: &medulla_core::graph::GraphView,
    sparql: &str,
) -> Result<(Vec<Method>, Vec<String>), String> {
    let proj = medulla_rdf::sparql::Projection::from_wire("", "");
    let dataset = medulla_rdf::sparql::Dataset::new(snap, Vec::new());
    let triples = match medulla_rdf::sparql::execute(&dataset, sparql, &proj, None) {
        Ok(medulla_rdf::sparql::QueryOutcome::Graph(t)) => t,
        Ok(_) => return Err("SPARQL CONSTRUCT/DESCRIBE query required".to_string()),
        Err(e) => return Err(e),
    };
    triples_to_methods(&triples)
}

/// Lower a SPARQL UPDATE's `INSERT DATA` triples to graph-native `AddNode`/`AddEdge`
/// methods (CONCEPT:MEDULLA-KG.txn.isolation-ryow-begin-set), reusing the SAME `triples_to_methods` lowering as the OWL
/// axiom path. Used by the pgwire cross-modal txn seam's `SPARQL UPDATE` verb.
/// Returns `(methods, schema_refs)` — see [`triples_to_methods`]'s doc.
#[cfg(feature = "sparql")]
pub fn sparql_update_to_methods(update_str: &str) -> Result<(Vec<Method>, Vec<String>), String> {
    let triples = medulla_rdf::update::insert_data_triples(update_str)?;
    triples_to_methods(&triples)
}

/// Identity for a GraphQL durable cross-modal commit: the request's target
/// graph/core, the in-memory staging registry, the transaction id, and the
/// verified carrier authority. `prepare_graphql_cross_modal` and
/// `commit_graphql_cross_modal` are two phases of the SAME commit and take
/// this exact tuple — bundled once so the two phases can't drift on which
/// fields identify a commit.
#[cfg(feature = "graphql")]
#[derive(Clone, Copy)]
pub struct GraphQlCrossModalCommit<'a> {
    pub state: &'a Arc<RwLock<ServerState>>,
    pub request_id: u64,
    pub graph_name: &'a str,
    pub core: &'a medulla_core::graph::GraphCore,
    pub registry: &'a medulla_graphql::CrossModalTxnRegistry,
    pub txn_id: &'a str,
    pub authority: &'a CarrierAuthority,
}

/// Abort after a volatile lifecycle effect and before its durable terminal
/// receipt is written.  This is an explicit fault-window hook for restart
/// testing; it is inert unless a request id is armed in the environment.
/// Keeping the hook at this boundary exercises the real signed dispatch path
/// without creating another replay or idempotency authority.
pub fn fault_after_txn_lifecycle_effect(req_id: u64) {
    let Ok(armed) = std::env::var("MEDULLA_LIFECYCLE_EFFECT_FAULT_REQUEST_ID") else {
        return;
    };
    if armed.parse::<u64>().ok() == Some(req_id) {
        eprintln!("MEDULLA_LIFECYCLE_EFFECT_FAULT_REQUEST_ID armed for request {req_id}");
        std::process::abort();
    }
}

/// Opaque handle to an admitted GraphQL lifecycle operation. The transaction handler crate
/// produces it from `begin_graphql_lifecycle` and consumes it in `finish_graphql_lifecycle`;
/// callers below it only carry it between the two calls.
#[cfg(feature = "graphql")]
pub struct LifecycleToken(pub Box<dyn std::any::Any + Send>);

/// Admission result for a GraphQL-native begin/stage/read/rollback operation: either the
/// terminal result of an exact replay, or the token the caller finishes after executing.
#[cfg(feature = "graphql")]
pub enum GraphQlLifecycleAdmission {
    /// The operation already reached a terminal receipt; return this result.
    Replayed(ResultPayload),
    /// The operation is new; execute it, then finish it with this token.
    Execute(LifecycleToken),
}
