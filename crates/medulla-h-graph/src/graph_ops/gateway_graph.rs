use super::*;

#[path = "gateway_graph_routes.rs"]
mod gateway_graph_routes;

/// Decode a MessagePack-encoded JSON object blob (the `props_msgpack` /
/// `semantic_props_msgpack` wire fields) into a `serde_json` object map
/// (CONCEPT:MEDULLA-KG.memory.medulla-batch-decay-caller). A missing/undecodable/non-object blob yields an empty map, so
/// a caller may omit props entirely — the medulla-core primitive injects the structural
/// markers regardless. Mirrors the `CompareAndSetNodeFields` blob-decode discipline.
pub(super) fn decode_json_object(blob: &[u8]) -> serde_json::Map<String, serde_json::Value> {
    medulla_types::msgpack::decode_property_object(blob).unwrap_or_default()
}

// MEDULLA-ORCH-VOCAB-R801.3 — the versioned harness-component write attaches to
// the existing AddNode path at the pre-commit point in `try_handle_gateway`
// (beside the owner-id stamp), through
// `medulla_server::harness_admission::direct_add_node`: the method is mutated
// BEFORE both the durable commit and the `apply` closure clone it, so the
// stamped record is exactly what gets persisted and what the RAM projection
// applies. The ChangeEnvelope path runs the shared rules through
// `harness_admission::envelope_boundary` instead of mutating.

/// BUG-193: stamp the BUG-052/GOC-61 canonical `_owner_id` onto a brand-new
/// node's property blob when the caller did not already supply an ownership
/// key, so a genuinely natively-written row is no longer permanently unowned
/// (the gap BUG-193 names: `_owner`/`isolation.rs`'s own native key had no
/// production writer at all). Mutating `method` HERE — before either arm below
/// clones it for the durable-commit path AND the `apply` closure — keeps the
/// persisted blob and the applied RAM blob byte-identical, which is required
/// for crash-recovery replay to reproduce the same row. Scoped to the two
/// methods that hand the gateway a caller-supplied node property blob directly
/// (`AddNode`, `CreateNodeIfAbsent`); the coalescable batch surface
/// (`BatchUpdate`) and the staged Cypher/RDF/GraphQL write path are explicitly
/// OUT of scope for this lane — see `plans/graph-os-completion-program/
/// BUG-LEDGER.md#BUG-193` for the documented boundary. A `System`-role caller
/// (bootstrap/migration/internal maintenance) is exempt: it is not a real
/// per-agent owner and must not be stamped as one. An absent `caller`
/// (state-machine-authorized replicated apply) is likewise exempt — there is no
/// per-request identity to stamp with there. Pure extract-method from
/// `try_handle_gateway`'s preamble, byte-identical behaviour, no signature
/// change.
#[cfg(feature = "security")]
pub(super) fn stamp_owner_id_if_applicable(
    method: Method,
    caller: Option<&str>,
    isolation: &medulla_isolation::isolation::IsolationLayer,
) -> Method {
    // Flat guard clauses rather than nested `if`s: each exemption below returns
    // `method` untouched, which is byte-for-byte what the nested form did by
    // falling out of its `if` chain. Same conditions, same order, same result --
    // only the nesting depth (and so the cognitive complexity) differs.
    if !matches!(
        method,
        Method::AddNode { .. } | Method::CreateNodeIfAbsent { .. } | Method::Batch { .. }
    ) {
        return method;
    }
    // An absent `caller` (state-machine-authorized replicated apply) has no
    // per-request identity to stamp with.
    let Some(caller_id) = caller else {
        return method;
    };
    // A `System`-role caller is not a real per-agent owner.
    if isolation.is_system(caller_id) {
        return method;
    }
    if let Method::Batch { operations } = method {
        return Method::Batch {
            operations: operations
                .into_iter()
                .map(|operation| stamp_batch_operation(operation, caller_id))
                .collect(),
        };
    }
    let blob = match &method {
        Method::AddNode {
            properties_msgpack, ..
        }
        | Method::CreateNodeIfAbsent {
            properties_msgpack, ..
        } => properties_msgpack,
        _ => unreachable!("matched above"),
    };
    // Already owned (or unstampable) => leave the blob exactly as the caller sent it.
    let Some(stamped) = medulla_isolation::isolation::stamp_owner_id_if_absent(blob, caller_id)
    else {
        return method;
    };
    let mut method = method;
    match &mut method {
        Method::AddNode {
            properties_msgpack, ..
        }
        | Method::CreateNodeIfAbsent {
            properties_msgpack, ..
        } => *properties_msgpack = stamped,
        _ => unreachable!("matched above"),
    }
    method
}

/// A `Batch`'s created nodes carry the caller's owner id exactly as a top-level
/// `CreateNodeIfAbsent` does (MEDULLA-DURABLE-KERNEL-R871).
#[cfg(feature = "security")]
fn stamp_batch_operation(
    operation: medulla_query_types::wire::BatchOperation,
    caller_id: &str,
) -> medulla_query_types::wire::BatchOperation {
    use medulla_query_types::wire::BatchOperation;
    match operation {
        BatchOperation::CreateNodeIfAbsent {
            node_id,
            properties_msgpack,
        } => BatchOperation::CreateNodeIfAbsent {
            node_id,
            properties_msgpack: medulla_isolation::isolation::stamp_owner_id_if_absent(
                &properties_msgpack,
                caller_id,
            )
            .unwrap_or(properties_msgpack),
        },
        other => other,
    }
}

/// The node keys of a stored trust mark (MEDULLA-SRC-AUTH-R011.3). Only the mutation gateway
/// writes them.
#[cfg(feature = "security")]
const TRUST_MARK_KEY: &str = "trust_mark";
#[cfg(feature = "security")]
const MARKED_BY_KEY: &str = "marked_by";
#[cfg(feature = "security")]
const MARKED_AT_KEY: &str = "marked_at_generation";
#[cfg(feature = "security")]
const APPROVAL_KEY: &str = "trust_mark_approval_id";
/// The reserved key of an `AddNode` blob that names where its content came from.
#[cfg(feature = "security")]
const CONTENT_ORIGIN_KEY: &str = "content_origin";

#[cfg(feature = "security")]
fn trust_mark_reserved(detail: &str) -> String {
    format!(
        "{}: {detail}",
        medulla_virtual_graph::source_selection::TRUST_MARK_FIELDS_RESERVED
    )
}

/// MEDULLA-SRC-AUTH-R011.3: stamp the trust mark in the mutation gateway, beside the owner
/// stamp and before the harness admission.
///
/// `AddNode`: drop every caller-supplied mark key; a `content_origin` maps to a
/// [`ContentOrigin`](medulla_virtual_graph::source_selection::ContentOrigin) and the default
/// policy stamps `trust_mark`, `marked_by` and `marked_at_generation`; no `content_origin`
/// stamps nothing. `CompareAndSetNodeFields`: a mark change runs through `change_trust_mark`
/// against the node's current stored mark (an unmarked node counts as `untrusted`) and pins
/// that mark as a CAS condition, so a concurrent change fails the CAS instead of being
/// overwritten. A `System` caller and an absent caller (replicated apply) pass through.
#[cfg(feature = "security")]
pub(super) fn stamp_trust_mark_if_applicable(
    method: Method,
    core: &GraphCore,
    caller: Option<&str>,
    isolation: &medulla_isolation::isolation::IsolationLayer,
) -> Result<Method, String> {
    if !matches!(
        method,
        Method::AddNode { .. }
            | Method::CreateNodeIfAbsent { .. }
            | Method::CompareAndSetNodeFields { .. }
            | Method::Batch { .. }
    ) {
        return Ok(method);
    }
    let Some(caller_id) = caller else {
        return Ok(method);
    };
    if isolation.is_system(caller_id) {
        return Ok(method);
    }
    match method {
        Method::AddNode {
            node_id,
            properties_msgpack,
        } => stamp_add_node_trust_mark(core, node_id, properties_msgpack),
        Method::CreateNodeIfAbsent {
            node_id,
            properties_msgpack,
        } => Ok(Method::CreateNodeIfAbsent {
            node_id,
            properties_msgpack: stamp_properties_blob(core, properties_msgpack, true)?,
        }),
        Method::Batch { operations } => Ok(Method::Batch {
            operations: operations
                .into_iter()
                .map(|operation| stamp_batch_operation_trust_mark(core, operation))
                .collect::<Result<Vec<_>, String>>()?,
        }),
        Method::CompareAndSetNodeFields {
            node_id,
            conditions_msgpack,
            updates_msgpack,
        } => stamp_cas_trust_mark(core, node_id, conditions_msgpack, updates_msgpack),
        other => Ok(other),
    }
}

/// The origin a `content_origin` value names.
#[cfg(feature = "security")]
fn parse_content_origin(
    value: &serde_json::Value,
) -> Result<medulla_virtual_graph::source_selection::ContentOrigin, String> {
    use medulla_virtual_graph::source_selection::{ContentOrigin, CONTENT_ORIGIN_UNKNOWN};
    match value.as_str() {
        Some("tool_result") => Ok(ContentOrigin::ToolResult),
        Some("retrieved_document") => Ok(ContentOrigin::RetrievedDocument),
        Some("upload") => Ok(ContentOrigin::Upload),
        Some("source") => Ok(ContentOrigin::Source {
            approved_mapping: false,
        }),
        Some("derived") => Ok(ContentOrigin::Derived { inputs: Vec::new() }),
        _ => Err(format!(
            "{CONTENT_ORIGIN_UNKNOWN}: `{}` is not a known content origin",
            value.as_str().unwrap_or("<non-string>")
        )),
    }
}

#[cfg(feature = "security")]
fn stamp_add_node_trust_mark(
    core: &GraphCore,
    node_id: String,
    properties_msgpack: Vec<u8>,
) -> Result<Method, String> {
    Ok(Method::AddNode {
        node_id,
        properties_msgpack: stamp_properties_blob(core, properties_msgpack, false)?,
    })
}

/// The one stamp of a node-creating property blob. A top-level `AddNode` drops caller-set mark
/// keys; a batch operation (`refuse_forged`) refuses them with `TRUST_MARK_FIELDS_RESERVED`
/// (MEDULLA-SRC-AUTH-R011.3). A `content_origin` maps to an origin and the default policy
/// stamps the three keys; without one nothing is stamped.
#[cfg(feature = "security")]
fn stamp_properties_blob(
    core: &GraphCore,
    properties_msgpack: Vec<u8>,
    refuse_forged: bool,
) -> Result<Vec<u8>, String> {
    use medulla_virtual_graph::source_selection::{DefaultTrustMarkPolicy, StoredTrustMark};
    let Ok(mut props) = medulla_types::msgpack::decode_property_object(&properties_msgpack) else {
        return Ok(properties_msgpack);
    };
    let mut dropped = false;
    for key in [TRUST_MARK_KEY, MARKED_BY_KEY, MARKED_AT_KEY, APPROVAL_KEY] {
        if props.remove(key).is_some() {
            if refuse_forged {
                return Err(trust_mark_reserved(&format!(
                    "`{key}` is set by the engine, not by a batch write"
                )));
            }
            dropped = true;
        }
    }
    let origin = props
        .get(CONTENT_ORIGIN_KEY)
        .map(parse_content_origin)
        .transpose()?;
    if !dropped && origin.is_none() {
        return Ok(properties_msgpack);
    }
    if let Some(origin) = origin {
        let stamp = StoredTrustMark::stamp(&DefaultTrustMarkPolicy, &origin, None, core.version());
        props.insert(
            TRUST_MARK_KEY.into(),
            serde_json::to_value(stamp.mark).map_err(|error| error.to_string())?,
        );
        props.insert(MARKED_BY_KEY.into(), stamp.marked_by.into());
        props.insert(MARKED_AT_KEY.into(), stamp.marked_at_generation.into());
    }
    rmp_serde::to_vec_named(&serde_json::Value::Object(props)).map_err(|error| error.to_string())
}

/// A batch operation that writes node fields never carries a caller-set mark key
/// (MEDULLA-SRC-AUTH-R011.3): node creation is stamped like a top-level `AddNode`, and a
/// compare-and-set or claim whose updates name a mark key is refused. A mark changes only
/// through the top-level `CompareAndSetNodeFields` with its approval.
#[cfg(feature = "security")]
fn stamp_batch_operation_trust_mark(
    core: &GraphCore,
    operation: medulla_query_types::wire::BatchOperation,
) -> Result<medulla_query_types::wire::BatchOperation, String> {
    use medulla_query_types::wire::BatchOperation;
    let reserved = |updates_msgpack: &[u8]| -> Result<(), String> {
        let Ok(updates) = medulla_types::msgpack::decode_property_object(updates_msgpack) else {
            return Ok(());
        };
        for key in [TRUST_MARK_KEY, MARKED_BY_KEY, MARKED_AT_KEY, APPROVAL_KEY] {
            if updates.contains_key(key) {
                return Err(trust_mark_reserved(&format!(
                    "`{key}` is set by the engine, not by a batch write"
                )));
            }
        }
        Ok(())
    };
    match operation {
        BatchOperation::CreateNodeIfAbsent {
            node_id,
            properties_msgpack,
        } => Ok(BatchOperation::CreateNodeIfAbsent {
            node_id,
            properties_msgpack: stamp_properties_blob(core, properties_msgpack, true)?,
        }),
        BatchOperation::CompareAndSetNodeFields {
            node_id,
            conditions_msgpack,
            updates_msgpack,
        } => {
            reserved(&updates_msgpack)?;
            Ok(BatchOperation::CompareAndSetNodeFields {
                node_id,
                conditions_msgpack,
                updates_msgpack,
            })
        }
        BatchOperation::ClaimNext {
            label,
            updates_msgpack,
        } => {
            reserved(&updates_msgpack)?;
            Ok(BatchOperation::ClaimNext {
                label,
                updates_msgpack,
            })
        }
        other => Ok(other),
    }
}

/// The mark a node stores now; a node with no stored mark counts as `untrusted`, so a caller
/// cannot assert trust on unmarked content without an approval.
#[cfg(feature = "security")]
fn current_stored_trust_mark(
    core: &GraphCore,
    node_id: &str,
) -> (
    medulla_virtual_graph::source_selection::StoredTrustMark,
    serde_json::Value,
) {
    use medulla_virtual_graph::source_selection::{StoredTrustMark, TrustMark};
    let props = core
        .get_node_properties(node_id)
        .and_then(|blob| medulla_types::msgpack::decode_property_object(&blob).ok())
        .unwrap_or_default();
    let stored = props
        .get(TRUST_MARK_KEY)
        .and_then(|value| serde_json::from_value::<TrustMark>(value.clone()).ok());
    let pinned = props
        .get(TRUST_MARK_KEY)
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let current = StoredTrustMark {
        mark: stored.unwrap_or(TrustMark::Untrusted),
        marked_by: props
            .get(MARKED_BY_KEY)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unmarked_untrusted")
            .to_string(),
        marked_at_generation: props
            .get(MARKED_AT_KEY)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        approval_id: props
            .get(APPROVAL_KEY)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    };
    (current, pinned)
}

#[cfg(feature = "security")]
fn stamp_cas_trust_mark(
    core: &GraphCore,
    node_id: String,
    conditions_msgpack: Vec<u8>,
    updates_msgpack: Vec<u8>,
) -> Result<Method, String> {
    use medulla_virtual_graph::source_selection::{change_trust_mark, TrustMark};
    let unchanged = |node_id: String, conditions_msgpack: Vec<u8>, updates_msgpack: Vec<u8>| {
        Method::CompareAndSetNodeFields {
            node_id,
            conditions_msgpack,
            updates_msgpack,
        }
    };
    let (Ok(mut updates), Ok(mut conditions)) = (
        medulla_types::msgpack::decode_property_object(&updates_msgpack),
        medulla_types::msgpack::decode_property_object(&conditions_msgpack),
    ) else {
        return Ok(unchanged(node_id, conditions_msgpack, updates_msgpack));
    };
    let reserved = [TRUST_MARK_KEY, MARKED_BY_KEY, MARKED_AT_KEY, APPROVAL_KEY];
    if !reserved.iter().any(|key| updates.contains_key(*key)) {
        return Ok(unchanged(node_id, conditions_msgpack, updates_msgpack));
    }
    let Some(requested) = updates.get(TRUST_MARK_KEY) else {
        return Err(trust_mark_reserved(
            "marked_by, marked_at_generation and trust_mark_approval_id are set only with trust_mark",
        ));
    };
    let requested: TrustMark = serde_json::from_value(requested.clone())
        .map_err(|_| trust_mark_reserved("trust_mark must be `trusted` or `untrusted`"))?;
    let approval_id = match updates.remove(APPROVAL_KEY) {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(id)) => Some(id),
        Some(_) => {
            return Err(trust_mark_reserved(
                "trust_mark_approval_id must be a string",
            ))
        }
    };
    let (current, pinned) = current_stored_trust_mark(core, &node_id);
    let stored = change_trust_mark(
        &node_id,
        &current,
        requested,
        approval_id.as_deref(),
        core.version(),
    )
    .map_err(|refusal| refusal.to_string())?;
    for key in reserved {
        updates.remove(key);
    }
    updates.insert(
        TRUST_MARK_KEY.into(),
        serde_json::to_value(stored.mark).map_err(|error| error.to_string())?,
    );
    updates.insert(MARKED_BY_KEY.into(), stored.marked_by.into());
    updates.insert(MARKED_AT_KEY.into(), stored.marked_at_generation.into());
    match stored.approval_id {
        Some(id) => {
            updates.insert(APPROVAL_KEY.into(), id.into());
        }
        // Clear an approval id a previous lowering left on the node.
        None if current.approval_id.is_some() => {
            updates.insert(APPROVAL_KEY.into(), serde_json::Value::Null);
        }
        None => {}
    }
    conditions.insert(TRUST_MARK_KEY.into(), pinned);
    Ok(unchanged(
        node_id,
        rmp_serde::to_vec_named(&serde_json::Value::Object(conditions))
            .map_err(|error| error.to_string())?,
        rmp_serde::to_vec_named(&serde_json::Value::Object(updates))
            .map_err(|error| error.to_string())?,
    ))
}

/// SHA-256 of a request text as 64 lowercase hex characters (MEDULLA-FEDERATED-QUERY-R707.5).
#[cfg(all(feature = "sparql", feature = "shacl"))]
fn hex_sha256(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// `ApplyMutation` (SPARQL UPDATE): pure extract-method from `try_handle_gateway`'s
/// closure, byte-identical behaviour, no signature change.
fn apply_apply_mutation(
    core: &GraphCore,
    event_type: String,
    query: String,
) -> Result<ResultPayload, String> {
    #[cfg(feature = "sparql")]
    {
        let _ = event_type;
        struct SingleCoreStore(std::sync::Arc<GraphCore>);
        impl medulla_rdf::update::GraphStore for SingleCoreStore {
            fn core(&self, graph: Option<&str>) -> Option<std::sync::Arc<GraphCore>> {
                graph.is_none().then(|| self.0.clone())
            }
        }
        #[cfg(feature = "shacl")]
        {
            let update = medulla_rdf::update::parse_update(&query)
                .map_err(|error| format!("ApplyMutation: {error}"))?;
            if !medulla_rdf::update::referenced_named_graphs(&update).is_empty() {
                return Err(
                    "ApplyMutation: graph-scoped updates cannot address a named RDF graph"
                        .to_string(),
                );
            }
            // GraphStore requires an owned Arc. Clone the isolated
            // gateway image, execute there, then copy the successful
            // result back into that same staged image. The live core is
            // never exposed before the authoritative commit succeeds.
            let update_core =
                std::sync::Arc::new(GraphCore::from_snapshot(core.snapshot(), core.version())?);
            let store = SingleCoreStore(update_core.clone());
            let guard = medulla_server_api::icv_guard::CoreIcvGuard::single(update_core.as_ref());
            let report = medulla_rdf::update::execute(
                &update,
                &store,
                &medulla_rdf::sparql::Projection::raw(),
                &guard,
            )
            .map_err(|error| format!("ApplyMutation: {error}"))?;
            core.replace_snapshot(update_core.snapshot())?;
            ResultPayload::of::<medulla_protocol_types::result_contract::graph::ApplyMutation>(
                medulla_protocol_types::result_contract::transactions::SparqlUpdateReport {
                    operations: report.operations as u64,
                    inserted: report.inserted as u64,
                    deleted: report.deleted as u64,
                    updated_graphs: 1,
                    created_graphs: 0,
                    request_sha256: hex_sha256(&query),
                    batch_id: String::new(),
                },
            )
        }
        #[cfg(not(feature = "shacl"))]
        {
            Err("ApplyMutation requires the shacl integrity-guard feature".to_string())
        }
    }
    #[cfg(not(feature = "sparql"))]
    {
        let _ = (event_type, query, core);
        Err("ApplyMutation (SPARQL UPDATE) requires the `sparql` feature".to_string())
    }
}

/// `RunDatalogReasoning`: pure extract-method from `try_handle_gateway`'s closure,
/// byte-identical behaviour.
///
/// Gated on `reasoning`: `crate::reasoning` (see `lib.rs`, `#[cfg(feature =
/// "reasoning")] pub use medulla_reasoner::reasoning;`) exists only under that feature, and
/// this function's only caller is the `#[cfg(feature = "reasoning")]
/// Method::RunDatalogReasoning` arm in `try_handle_gateway` below. Same shape as
/// BUG-CX-104 / the `dispatch.rs` fix (commit `bc280437`): a function with no cfg of
/// its own reaching a cfg-gated module. Without this gate, `cargo check
/// --no-default-features --features server` fails E0433 on every `crate::reasoning::*`
/// call in this body, even though the function is unreachable in that build.
#[cfg(feature = "reasoning")]
struct DatalogReasoningInput {
    schema_digests: Vec<String>,
    subclass_relations: Vec<(String, String)>,
    subproperty_relations: Vec<(String, String)>,
    symmetric_properties: Vec<String>,
    transitive_properties: Vec<String>,
    inverse_properties: Vec<(String, String)>,
    domain_rules: Vec<(String, String)>,
    range_rules: Vec<(String, String)>,
    property_chains: Vec<(Vec<String>, String)>,
}

#[cfg(feature = "reasoning")]
impl DatalogReasoningInput {
    fn has_axioms(&self) -> bool {
        [
            self.subclass_relations.len(),
            self.subproperty_relations.len(),
            self.symmetric_properties.len(),
            self.transitive_properties.len(),
            self.inverse_properties.len(),
            self.domain_rules.len(),
            self.range_rules.len(),
            self.property_chains.len(),
        ]
        .into_iter()
        .any(|length| length != 0)
    }
}

#[cfg(feature = "reasoning")]
fn apply_run_datalog_reasoning(
    core: &GraphCore,
    input: DatalogReasoningInput,
) -> Result<ResultPayload, String> {
    let input = resolve_datalog_input(core, input)?;
    let (schema_digests, all_inferred) = collect_datalog_inferences(core, input)?;
    ResultPayload::of::<medulla_protocol_types::result_contract::reasoning::RunDatalogReasoning>(
        medulla_types::types::DatalogReasoningResult {
            schema_digests,
            inferred_count: all_inferred.len(),
            inferred_triples: all_inferred,
        },
    )
}

/// `RunTemporalRules` (MEDULLA-FEDERATED-QUERY-R006.1): temporal forward chaining
/// over the live bitemporal edges of the staged image. Every refusal returns
/// before any write, so a refused run commits nothing and the graph version
/// stays where it was. Each derived record becomes one `DerivedFact` node;
/// a stored fact the run derives again is left unchanged, and a stored fact of
/// the same rule the run no longer derives gets its retraction revision.
#[cfg(all(feature = "reasoning", feature = "query"))]
fn apply_run_temporal_rules(
    core: &GraphCore,
    text: &str,
    may_write_derived: bool,
) -> Result<ResultPayload, String> {
    use medulla_plan::rules::temporal::{derive, retraction, DeriveContext, TemporalRule};
    use medulla_query_types::derived_fact::RunTemporalRulesResult;

    let refusal = |refusal: medulla_plan::rules::temporal::DeriveRefusal| match refusal {
        medulla_plan::rules::temporal::DeriveRefusal::Coded(code, detail) => {
            format!("{}: {detail}", code.as_str())
        }
        medulla_plan::rules::temporal::DeriveRefusal::Invalid(detail) => format!(
            "{}: {detail}",
            medulla_types::contract::EngineErrorCode::InvalidArgument.as_str()
        ),
    };
    let rule = TemporalRule::parse(text).map_err(refusal)?;
    let graph_generation = core.version();
    let premises = temporal_premises(core, &rule.body)?;
    let derivation = derive(
        &rule,
        &premises,
        DeriveContext {
            may_write_derived,
            graph_generation,
        },
    )
    .map_err(refusal)?;
    let rule_digest = rule.digest().map_err(|error| {
        format!(
            "{}: {error}",
            medulla_types::contract::EngineErrorCode::InvalidArgument.as_str()
        )
    })?;

    let stored: Vec<medulla_query_types::derived_fact::DerivedFactRecord> = core
        .get_nodes_by_label(DERIVED_FACT_LABEL, 0)
        .into_iter()
        .filter_map(|(_, blob)| stored_derived_fact(&blob))
        .filter(|record| record.fact.rule_digest == rule_digest)
        .collect();
    let derived_ids: std::collections::BTreeSet<&str> = derivation
        .facts
        .iter()
        .map(|record| record.fact.fact_id.as_str())
        .collect();
    for record in &derivation.facts {
        let node_id = derived_fact_node_id(&record.fact.fact_id);
        if core.has_node(&node_id) {
            continue;
        }
        core.add_node(node_id, derived_fact_props(record)?);
    }
    for record in &stored {
        if record.tombstone || derived_ids.contains(record.fact.fact_id.as_str()) {
            continue;
        }
        let retracted = retraction(record);
        core.add_node(
            derived_fact_node_id(&retracted.fact.fact_id),
            derived_fact_props(&retracted)?,
        );
    }
    // The staged MutationBatch commits at the next graph version.
    let committed_version = graph_generation
        .checked_add(1)
        .ok_or_else(|| "RunTemporalRules: graph version overflow".to_string())?;
    ResultPayload::of::<medulla_protocol_types::result_contract::reasoning::RunTemporalRules>(
        RunTemporalRulesResult {
            rule_digest,
            graph_generation,
            committed_version,
            facts: derivation.facts,
            trace: derivation.trace,
        },
    )
}

/// A build without the query planner has no temporal derivation.
#[cfg(all(feature = "reasoning", not(feature = "query")))]
fn apply_run_temporal_rules(
    _core: &GraphCore,
    _text: &str,
    _may_write_derived: bool,
) -> Result<ResultPayload, String> {
    Err("RunTemporalRules requires the `query` feature".to_string())
}

/// Node label of a stored temporal rule-derived fact.
#[cfg(all(feature = "reasoning", feature = "query"))]
const DERIVED_FACT_LABEL: &str = "DerivedFact";

#[cfg(all(feature = "reasoning", feature = "query"))]
fn derived_fact_node_id(fact_id: &str) -> String {
    format!("derived:{fact_id}")
}

/// Every record field as a node property, beside the `DerivedFact` label.
#[cfg(all(feature = "reasoning", feature = "query"))]
fn derived_fact_props(
    record: &medulla_query_types::derived_fact::DerivedFactRecord,
) -> Result<Vec<u8>, String> {
    let fact = &record.fact;
    let props = serde_json::json!({
        "type": DERIVED_FACT_LABEL,
        "fact_id": fact.fact_id,
        "rule_digest": fact.rule_digest.to_hex(),
        "premise_ids": fact.premise_ids,
        "origin_kind": fact.origin_kind.as_str(),
        "origin_ref": fact.origin_ref,
        "graph_generation": fact.graph_generation,
        "valid_from": record.valid_from,
        "valid_to": record.valid_to,
        "revision": record.revision,
        "tombstone": record.tombstone,
    });
    rmp_serde::to_vec_named(&props)
        .map_err(|error| format!("RunTemporalRules: property encoding failed: {error}"))
}

/// Read a stored `DerivedFact` node back as its record.
#[cfg(all(feature = "reasoning", feature = "query"))]
fn stored_derived_fact(blob: &[u8]) -> Option<medulla_query_types::derived_fact::DerivedFactRecord> {
    let props = medulla_types::msgpack::decode_property_object(blob).ok()?;
    let value = serde_json::json!({
        "fact": {
            "fact_id": props.get("fact_id")?,
            "rule_digest": props.get("rule_digest")?,
            "premise_ids": props.get("premise_ids")?,
            "origin_kind": props.get("origin_kind")?,
            "origin_ref": props.get("origin_ref")?,
            "graph_generation": props.get("graph_generation")?,
        },
        "valid_from": props.get("valid_from")?,
        "valid_to": props.get("valid_to")?,
        "revision": props.get("revision")?,
        "tombstone": props.get("tombstone")?,
    });
    serde_json::from_value(value).ok()
}

/// The premises of one run: every live edge whose `relationship` is a body
/// predicate. An edge with `tx_to` set is closed history and not live. The
/// premise id is the edge id `<source>-><target>`; the subject is the source
/// node; `valid_to` is `valid_until`, or `i64::MAX` when it is absent.
#[cfg(all(feature = "reasoning", feature = "query"))]
fn temporal_premises(
    core: &GraphCore,
    body: &[String],
) -> Result<Vec<medulla_plan::rules::temporal::Premise>, String> {
    use medulla_plan::rules::temporal::{Premise, PremiseOrigin};
    use medulla_query_types::derived_fact::OriginKind;

    let malformed = |source: &str, target: &str, field: &str| {
        format!(
            "{}: edge `{source}->{target}` has no integer `{field}`",
            medulla_types::contract::ServerErrorCode::DerivedBoundMalformed.as_str()
        )
    };
    let mut premises = Vec::new();
    for (source, target, blob) in core.get_edges() {
        let Ok(props) = medulla_types::msgpack::decode_property_object(&blob) else {
            continue;
        };
        let Some(relationship) = props.get("relationship").and_then(|v| v.as_str()) else {
            continue;
        };
        if !body.iter().any(|atom| atom == relationship) {
            continue;
        }
        if props.get("tx_to").is_some_and(|v| !v.is_null()) {
            continue;
        }
        let valid_from = props
            .get("valid_from")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| malformed(&source, &target, "valid_from"))?;
        let valid_to = match props.get("valid_until") {
            None => i64::MAX,
            Some(value) if value.is_null() => i64::MAX,
            Some(value) => value
                .as_i64()
                .ok_or_else(|| malformed(&source, &target, "valid_until"))?,
        };
        let kind = match props.get("origin_kind").and_then(|v| v.as_str()) {
            Some("sentence") => Some(OriginKind::Sentence),
            Some("axiom") => Some(OriginKind::Axiom),
            _ => None,
        };
        let reference = props.get("origin_ref").and_then(|v| v.as_str());
        let origin = match (kind, reference) {
            (Some(kind), Some(reference)) => Some(PremiseOrigin {
                kind,
                reference: reference.to_string(),
            }),
            _ => None,
        };
        premises.push(Premise {
            id: format!("{source}->{target}"),
            predicate: relationship.to_string(),
            subject: source.clone(),
            valid_from,
            valid_to,
            origin,
        });
    }
    Ok(premises)
}

#[cfg(all(feature = "reasoning", feature = "owl", feature = "shacl"))]
fn resolve_datalog_input(
    core: &GraphCore,
    input: DatalogReasoningInput,
) -> Result<DatalogReasoningInput, String> {
    if input.has_axioms() {
        Ok(input)
    } else {
        datalog_input_from_graph_schema(core)
    }
}

#[cfg(all(feature = "reasoning", feature = "owl", not(feature = "shacl")))]
fn resolve_datalog_input(
    _core: &GraphCore,
    input: DatalogReasoningInput,
) -> Result<DatalogReasoningInput, String> {
    if input.has_axioms() {
        Ok(input)
    } else {
        Err("OWL_AUTHORITY_UNAVAILABLE: committed GraphSchema reasoning requires the shacl/owl-dl build".to_string())
    }
}

#[cfg(all(feature = "reasoning", not(feature = "owl")))]
fn resolve_datalog_input(
    _core: &GraphCore,
    input: DatalogReasoningInput,
) -> Result<DatalogReasoningInput, String> {
    Ok(input)
}

#[cfg(feature = "reasoning")]
type DatalogInferenceRows = (Vec<String>, Vec<std::collections::HashMap<String, String>>);

#[cfg(feature = "reasoning")]
fn collect_datalog_inferences(
    core: &GraphCore,
    input: DatalogReasoningInput,
) -> Result<DatalogInferenceRows, String> {
    let DatalogReasoningInput {
        schema_digests,
        subclass_relations,
        subproperty_relations,
        symmetric_properties,
        transitive_properties,
        inverse_properties,
        domain_rules,
        range_rules,
        property_chains,
    } = input;
    let mut all_inferred = medulla_reasoner::reasoning::run_datalog_reasoning(
        core,
        subclass_relations,
        subproperty_relations,
        symmetric_properties,
        transitive_properties,
        inverse_properties,
    )?;
    append_optional_datalog_inferences(
        core,
        &mut all_inferred,
        domain_rules,
        range_rules,
        property_chains,
    );
    medulla_reasoner::reasoning::bind_inference_schema_digests(
        core,
        &mut all_inferred,
        &schema_digests,
    );
    Ok((schema_digests, all_inferred))
}

#[cfg(feature = "reasoning")]
fn append_optional_datalog_inferences(
    core: &GraphCore,
    all_inferred: &mut Vec<std::collections::HashMap<String, String>>,
    domain_rules: Vec<(String, String)>,
    range_rules: Vec<(String, String)>,
    property_chains: Vec<(Vec<String>, String)>,
) {
    if !domain_rules.is_empty() || !range_rules.is_empty() {
        all_inferred.extend(medulla_reasoner::reasoning::infer_domain_range(
            core,
            domain_rules,
            range_rules,
        ));
    }
    if !property_chains.is_empty() {
        all_inferred.extend(medulla_reasoner::reasoning::infer_property_chain_axioms(
            core,
            property_chains,
        ));
    }
}

/// Compile the current committed GraphSchema ontology union into the existing
/// mutation-safe Datalog materializer.  An empty `RunDatalogReasoning` request
/// therefore means "use committed authority", not "perform an empty no-op";
/// callers no longer parse copied TTL or maintain a second axiom registry.
#[cfg(all(feature = "reasoning", feature = "owl", feature = "shacl"))]
fn datalog_input_from_graph_schema(core: &GraphCore) -> Result<DatalogReasoningInput, String> {
    let sources = core.schema_sources();
    if sources.ontologies().next().is_none() {
        return Err("OWL_AUTHORITY_MISSING: composed GraphSchema has no ontology".to_string());
    }
    // Always reason over the validated composition. Parsing source documents
    // independently would let their local blank-node labels alias one another
    // and would bypass the conflict/import checks used at commit time.
    let composed = medulla_server_api::schema_compose::validate_and_compose(&sources)?;
    let ontology = medulla_reasoner::owl::parse_ontology(&composed.ontology);
    Ok(datalog_input_from_ontology(
        ontology,
        sources.composed_digest().to_hex(),
    ))
}

#[cfg(all(feature = "reasoning", feature = "owl", feature = "shacl"))]
fn datalog_input_from_ontology(
    ontology: medulla_reasoner::owl::Ontology,
    composed_digest: String,
) -> DatalogReasoningInput {
    // The native property graph stores relationship labels in UPPER_SNAKE,
    // while the RDF authority uses camelCase IRI local names. The canonical
    // projection lives with the native OWL parser and is shared by read-side
    // fact saturation and write-side materialization.
    let relationship = medulla_reasoner::owl::native_relationship_label;
    let subclass_relations = datalog_subclass_relations(&ontology);
    let (transitive_properties, property_chains) = datalog_property_chains(&ontology, relationship);
    DatalogReasoningInput {
        schema_digests: vec![composed_digest],
        subclass_relations,
        subproperty_relations: ontology
            .sub_roles
            .into_iter()
            .filter_map(|(sub, sup, _, _)| {
                let (sub, sup) = (relationship(&sub), relationship(&sup));
                (!sub.is_empty() && !sup.is_empty()).then_some((sub, sup))
            })
            .collect(),
        symmetric_properties: ontology
            .symmetric
            .into_iter()
            .map(|role| relationship(&role))
            .filter(|role| !role.is_empty())
            .collect(),
        transitive_properties,
        inverse_properties: ontology
            .inverses
            .into_iter()
            .filter_map(|(left, right)| {
                let (left, right) = (relationship(&left), relationship(&right));
                (!left.is_empty() && !right.is_empty()).then_some((left, right))
            })
            .collect(),
        domain_rules: ontology
            .domains
            .into_iter()
            .filter_map(|(role, class_iri)| {
                let role = relationship(&role);
                (!role.is_empty()).then_some((role, datalog_class(&class_iri)?))
            })
            .collect(),
        range_rules: ontology
            .ranges
            .into_iter()
            .filter_map(|(role, class_iri)| {
                let role = relationship(&role);
                (!role.is_empty()).then_some((role, datalog_class(&class_iri)?))
            })
            .collect(),
        property_chains,
    }
}

#[cfg(all(feature = "reasoning", feature = "owl", feature = "shacl"))]
fn datalog_subclass_relations(ontology: &medulla_reasoner::owl::Ontology) -> Vec<(String, String)> {
    let mut relations = Vec::new();
    for gci in &ontology.gcis {
        if let (
            [medulla_reasoner::owl::Concept::Named(sub)],
            medulla_reasoner::owl::Concept::Named(sup),
        ) = (gci.lhs.as_slice(), &gci.rhs)
        {
            if let (Some(sub), Some(sup)) = (datalog_class(sub), datalog_class(sup)) {
                relations.push((sub, sup));
            }
        }
    }
    relations
}

#[cfg(all(feature = "reasoning", feature = "owl", feature = "shacl"))]
fn datalog_property_chains(
    ontology: &medulla_reasoner::owl::Ontology,
    relationship: fn(&str) -> String,
) -> (Vec<String>, Vec<(Vec<String>, String)>) {
    let mut transitive_properties = Vec::new();
    let mut property_chains = Vec::new();
    for chain in &ontology.chains {
        let chain_roles: Vec<String> = chain.chain.iter().map(|role| relationship(role)).collect();
        let sup = relationship(&chain.sup);
        if sup.is_empty() || chain_roles.iter().any(String::is_empty) {
            continue;
        }
        if chain_roles.len() == 2 && chain_roles.iter().all(|role| role == &sup) {
            transitive_properties.push(sup);
        } else if !chain_roles.is_empty() {
            property_chains.push((chain_roles, sup));
        }
    }
    (transitive_properties, property_chains)
}

#[cfg(all(feature = "reasoning", feature = "owl", feature = "shacl"))]
fn datalog_class(value: &str) -> Option<String> {
    let value = value
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(value);
    let value = value
        .rsplit_once('#')
        .or_else(|| value.rsplit_once('/'))
        .map_or(value, |(_, local)| local);
    (!value.is_empty() && value.as_bytes()[0].is_ascii_alphabetic()).then(|| value.to_string())
}

/// `ClaimNext`: pure extract-method from `try_handle_gateway`'s closure,
/// byte-identical behaviour, no signature change.
fn apply_claim_next(
    core: &GraphCore,
    label: &str,
    updates_msgpack: &[u8],
) -> Result<ResultPayload, String> {
    let updates = match medulla_types::msgpack::decode_property_object(updates_msgpack) {
        Ok(m) => m,
        Err(_) => {
            return ResultPayload::of::<
                medulla_protocol_types::result_contract::coordination::ClaimNext,
            >(None)
        }
    };
    let claimed = core.claim_next_fields(label, &updates);
    ResultPayload::of::<medulla_protocol_types::result_contract::coordination::ClaimNext>(claimed)
}

/// `Batch`: the shared `medulla-core` application; a read fault aborts the staged commit, so
/// nothing the batch wrote is committed.
fn apply_batch(
    core: &GraphCore,
    operations: &[medulla_query_types::wire::BatchOperation],
) -> Result<ResultPayload, String> {
    let results = medulla_core::batch_apply::apply_batch_operations(core, operations)
        .map_err(|error| error.message())?;
    ResultPayload::of::<medulla_protocol_types::result_contract::coordination::Batch>(
        medulla_query_types::wire::BatchResult { results },
    )
}

/// `AddSceneObject`: pure extract-method from `try_handle_gateway`'s closure,
/// byte-identical behaviour, no signature change.
fn apply_add_scene_object(
    core: &GraphCore,
    pose_msgpack: &[u8],
    parent: Option<&str>,
) -> Result<ResultPayload, String> {
    let Some(pose) = decode_pose(pose_msgpack) else {
        return Err("AddSceneObject: undecodable pose_msgpack".to_string());
    };
    let id = core.add_scene_object(&pose, parent);
    Ok(ResultPayload::scalar::<
        medulla_protocol_types::result_contract::graph::AddSceneObject,
    >(id))
}

/// `SetPose`: pure extract-method from `try_handle_gateway`'s closure,
/// byte-identical behaviour, no signature change.
fn apply_set_pose(
    core: &GraphCore,
    node_id: &str,
    pose_msgpack: &[u8],
) -> Result<ResultPayload, String> {
    let Some(pose) = decode_pose(pose_msgpack) else {
        return Err("SetPose: undecodable pose_msgpack".to_string());
    };
    let ok = core.set_pose(node_id, &pose);
    Ok(ResultPayload::scalar::<
        medulla_protocol_types::result_contract::graph::SetPose,
    >(ok))
}

/// Where an `AddEmbedding` commit reports its vector to the drift monitors.
pub(super) struct EmbeddingWriteScope {
    pub(super) tenant: String,
    pub(super) graph: String,
    pub(super) shard: u32,
}

/// `AddEmbedding`: pure extract-method from `try_handle_gateway`'s closure. After the vector
/// commits it is reported to the installed drift monitors (MEDULLA-DECISION-ENGINE-R174.8.3).
pub(super) fn apply_add_embedding(
    core: &GraphCore,
    node_id: String,
    embedding: Vec<f32>,
    scope: &EmbeddingWriteScope,
) -> Result<ResultPayload, String> {
    let source_version = core.version();
    let written = embedding.clone();
    core.semantic_store
        .write()
        .add_embedding(node_id.clone(), embedding)
        .map_err(|error| error.to_string())?;
    // Content-derived indexes are unchanged by a vector-only mutation, but their
    // completeness manifest must advance with the graph version that
    // commit_finalize publishes.
    core.maintain_indexes_at(
        &medulla_graph_core::index::ChangeSet::new(),
        source_version.saturating_add(1),
        core.node_count(),
        core.edge_count(),
    );
    report_embedding_write(
        core,
        &node_id,
        &written,
        source_version.saturating_add(1),
        scope,
    );
    Ok(ResultPayload::String("ok".to_string()))
}

/// Feed the committed vector and the node's text tokens to the drift monitors.
fn report_embedding_write(
    core: &GraphCore,
    node_id: &str,
    vector: &[f32],
    version: u64,
    scope: &EmbeddingWriteScope,
) {
    let Some(signals) = medulla_retrieval::discover_signals::discover_signals() else {
        return;
    };
    let (name, description, node_type) = super::semantic::node_text(core, node_id);
    let tokens: Vec<String> = format!("{name} {description}")
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    signals.observe_write(&medulla_retrieval::discover_signals::WrittenVector {
        tenant: &scope.tenant,
        graph: &scope.graph,
        shard: scope.shard,
        node_id,
        node_type: &node_type,
        vector,
        tokens: &tokens,
        version,
    });
}

/// `SupersedeEdge`: pure extract-method from `try_handle_gateway`'s closure,
/// byte-identical behaviour.
struct SupersedeEdgeInput {
    source_id: String,
    target_id: String,
    properties_msgpack: Vec<u8>,
    prior_source: String,
    prior_target: String,
    prior_relationship: String,
    valid_at: u64,
    tx_now: u64,
}

fn apply_supersede_edge(
    core: &GraphCore,
    input: SupersedeEdgeInput,
) -> Result<ResultPayload, String> {
    core.check_invalidation_interval(
        &input.prior_source,
        &input.prior_target,
        &input.prior_relationship,
        input.valid_at,
    )?;
    match core.supersede_edge(
        input.source_id,
        input.target_id,
        input.properties_msgpack,
        &input.prior_source,
        &input.prior_target,
        &input.prior_relationship,
        input.valid_at,
        input.tx_now,
    ) {
        Ok(()) => Ok(ResultPayload::scalar::<
            medulla_protocol_types::result_contract::graph::SupersedeEdge,
        >("ok".to_string())),
        Err(e) => Err(e),
    }
}

/// `BatchUpdate`: pure extract-method from `try_handle_gateway`'s closure,
/// byte-identical behaviour, no signature change.
fn apply_batch_update(
    core: &GraphCore,
    operations_msgpack: &[u8],
) -> Result<ResultPayload, String> {
    let summary = medulla_graph_algos::algorithms::batch_update(core, operations_msgpack)?;
    medulla_protocol_types::result_contract::transactions::BatchUpdateReport::decode(&summary)
        .and_then(
            ResultPayload::of::<medulla_protocol_types::result_contract::transactions::BatchUpdate>,
        )
}

/// Decode a MessagePack-encoded `{translation,rotation,scale}` JSON blob into an
/// medulla-core [`medulla_graph_core::scene::Pose`] (CONCEPT:MEDULLA-KG.memory.medulla-batch-decay-caller/MEDULLA-087). `None` only if the blob
/// is not a decodable JSON object or a present sub-object is malformed (a bare `{}`
/// reads back as the identity pose, since translation/rotation default to identity
/// and scale to unit). Keeps the `medulla-types` wire crate free of the medulla-core scene
/// dependency — the Pose lives only handler-side.
fn decode_pose(blob: &[u8]) -> Option<medulla_graph_core::scene::Pose> {
    let val = medulla_types::msgpack::decode_property_value(blob).ok()?;
    medulla_graph_core::scene::Pose::from_json(&val)
}

pub(super) async fn try_handle(
    state: &Arc<RwLock<ServerState>>,
    tenant_id: &str,
    ctx: &MutationCtx<'_>,
    plan: &MutationPlan,
    method: &Method,
    known_digests: &std::collections::BTreeSet<String>,
) -> Option<Response> {
    gateway_graph_routes::try_handle(state, tenant_id, ctx, plan, method, known_digests).await
}

#[cfg(all(test, feature = "reasoning", feature = "owl", feature = "shacl"))]
mod graph_schema_reasoning_tests {
    use super::*;

    #[test]
    fn empty_materialization_request_compiles_the_committed_schema_authority() {
        let core = GraphCore::new();
        let input = datalog_input_from_graph_schema(&core).unwrap();

        assert_eq!(input.schema_digests.len(), 1);
        assert!(!input.subclass_relations.is_empty());
        assert!(!input.subproperty_relations.is_empty());
        assert!(!input.symmetric_properties.is_empty());
        assert!(!input.transitive_properties.is_empty());
        assert!(!input.inverse_properties.is_empty());
        assert!(!input.domain_rules.is_empty());
        assert!(!input.range_rules.is_empty());
        assert!(!input.property_chains.is_empty());
        assert!(input
            .transitive_properties
            .iter()
            .any(|property| property == "PART_OF"));
        assert!(input
            .subproperty_relations
            .iter()
            .any(|(child, parent)| child == "PART_OF" && parent == "DEPENDS_ON"));
    }
}

#[cfg(test)]
mod tests {
    use medulla_graph_core::graph::GraphCore;
    use medulla_protocol_types::protocol::Method;

    fn hex(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    fn step(index: u64, digest: &str) -> serde_json::Value {
        serde_json::json!({
            "step_index": index,
            "kind": "tool_call",
            "model_request_digest": hex('a'),
            "tool_args_digest": digest,
            "result_digest": hex('c'),
        })
    }

    fn trace_write(steps: Vec<serde_json::Value>) -> Method {
        let props = serde_json::json!({
            "node_type": "RunTrace",
            "terminationReason": "final_answer",
            "steps": steps,
        });
        Method::AddNode {
            node_id: "trace-steps".into(),
            properties_msgpack: rmp_serde::to_vec_named(&props).unwrap(),
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R013.3
    #[test]
    fn add_run_trace_checks_step_digests() {
        let core = GraphCore::new();
        let valid = trace_write(vec![step(0, &hex('b')), step(1, &hex('d'))]);
        let admitted =
            crate::harness_admission::direct_add_node(&valid, &core, 7, Some("alice")).unwrap();
        let (
            Method::AddNode {
                properties_msgpack: admitted,
                ..
            },
            Method::AddNode {
                properties_msgpack: sent,
                ..
            },
        ) = (admitted, valid)
        else {
            panic!("AddNode came back");
        };
        // R013.5: the steps pass through in order, each stamped with the writer.
        let strip = |blob: &[u8]| {
            let mut props = medulla_types::msgpack::decode_property_object(blob).unwrap();
            let steps = props["steps"].as_array_mut().unwrap();
            let actors: Vec<_> = steps
                .iter_mut()
                .map(|step| step.as_object_mut().unwrap().remove("recordedBy"))
                .collect();
            (props, actors)
        };
        let ((admitted_props, actors), (sent_props, _)) = (strip(&admitted), strip(&sent));
        assert_eq!(
            admitted_props, sent_props,
            "a valid trace passes through in order"
        );
        assert!(actors
            .iter()
            .all(|a| a.as_ref().and_then(|v| v.as_str()) == Some("alice")));

        let bad_digest = trace_write(vec![step(0, "not-a-digest")]);
        let refusal =
            crate::harness_admission::direct_add_node(&bad_digest, &core, 7, None).unwrap_err();
        assert!(refusal.starts_with("STEP_DIGEST_INVALID"), "{refusal}");

        let gap = trace_write(vec![step(0, &hex('b')), step(2, &hex('b'))]);
        let refusal = crate::harness_admission::direct_add_node(&gap, &core, 7, None).unwrap_err();
        assert!(refusal.starts_with("STEP_ORDER_INVALID"), "{refusal}");
        assert!(!core.has_node("trace-steps"));
    }

    #[cfg(feature = "security")]
    fn blob(value: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&value).unwrap()
    }

    #[cfg(feature = "security")]
    fn props(blob: &[u8]) -> serde_json::Map<String, serde_json::Value> {
        medulla_types::msgpack::decode_property_object(blob).unwrap()
    }

    #[cfg(feature = "security")]
    fn add_node(properties: serde_json::Value) -> Method {
        Method::AddNode {
            node_id: "doc-1".into(),
            properties_msgpack: blob(properties),
        }
    }

    // spec: MEDULLA-SRC-AUTH-R011.3
    #[cfg(feature = "security")]
    #[test]
    fn add_node_stamps_trust_mark() {
        let core = GraphCore::new();
        let isolation = medulla_isolation::isolation::IsolationLayer::new();
        let stamp = |method| {
            super::stamp_trust_mark_if_applicable(method, &core, Some("agent-1"), &isolation)
        };

        // A tool result is stamped untrusted; the forged keys the caller sent are dropped.
        let Method::AddNode {
            properties_msgpack, ..
        } = stamp(add_node(serde_json::json!({
            "node_type": "Doc",
            "content_origin": "tool_result",
            "trust_mark": "trusted",
            "marked_by": "forged",
            "trust_mark_approval_id": "forged-approval",
        })))
        .unwrap()
        else {
            panic!("AddNode came back");
        };
        let stamped = props(&properties_msgpack);
        assert_eq!(stamped["trust_mark"], "untrusted");
        assert_eq!(stamped["marked_by"], "tool_result_untrusted");
        assert_eq!(stamped["marked_at_generation"], core.version());
        assert!(!stamped.contains_key("trust_mark_approval_id"));

        // Without a content origin nothing is stamped, and a forged mark is still dropped.
        let Method::AddNode {
            properties_msgpack, ..
        } = stamp(add_node(serde_json::json!({
            "node_type": "Doc",
            "trust_mark": "trusted",
        })))
        .unwrap()
        else {
            panic!("AddNode came back");
        };
        assert!(!props(&properties_msgpack).contains_key("trust_mark"));

        // An unknown origin is refused.
        let refusal = stamp(add_node(serde_json::json!({"content_origin": "telepathy"})))
            .unwrap_err();
        assert!(refusal.starts_with("CONTENT_ORIGIN_UNKNOWN"), "{refusal}");

        // An absent caller (replicated apply) passes through unchanged.
        let forged = serde_json::json!({"content_origin": "upload", "trust_mark": "x"});
        let Method::AddNode {
            properties_msgpack, ..
        } = super::stamp_trust_mark_if_applicable(add_node(forged.clone()), &core, None, &isolation)
            .unwrap()
        else {
            panic!("AddNode came back");
        };
        assert_eq!(properties_msgpack, blob(forged));
    }

    // spec: MEDULLA-SRC-AUTH-R011.3
    #[cfg(feature = "security")]
    #[test]
    fn batch_writes_refuse_caller_set_trust_mark() {
        use medulla_query_types::wire::BatchOperation;
        let core = GraphCore::new();
        let isolation = medulla_isolation::isolation::IsolationLayer::new();
        let stamp = |operation| {
            super::stamp_trust_mark_if_applicable(
                Method::Batch {
                    operations: vec![operation],
                },
                &core,
                Some("agent-1"),
                &isolation,
            )
        };
        let create = |properties| BatchOperation::CreateNodeIfAbsent {
            node_id: "doc-1".into(),
            properties_msgpack: blob(properties),
        };

        // A batch create carrying a mark is refused; one with an origin is stamped.
        let refusal = stamp(create(serde_json::json!({"trust_mark": "trusted"}))).unwrap_err();
        assert!(refusal.starts_with("TRUST_MARK_FIELDS_RESERVED"), "{refusal}");
        let Method::Batch { operations } =
            stamp(create(serde_json::json!({"content_origin": "upload"}))).unwrap()
        else {
            panic!("Batch came back");
        };
        let BatchOperation::CreateNodeIfAbsent {
            properties_msgpack, ..
        } = &operations[0]
        else {
            panic!("CreateNodeIfAbsent came back");
        };
        assert_eq!(props(properties_msgpack)["trust_mark"], "untrusted");

        // A batch compare-and-set or claim that names a mark key is refused.
        let refusal = stamp(BatchOperation::CompareAndSetNodeFields {
            node_id: "doc-1".into(),
            conditions_msgpack: blob(serde_json::json!({})),
            updates_msgpack: blob(serde_json::json!({"trust_mark": "trusted"})),
        })
        .unwrap_err();
        assert!(refusal.starts_with("TRUST_MARK_FIELDS_RESERVED"), "{refusal}");
        let refusal = stamp(BatchOperation::ClaimNext {
            label: "Task".into(),
            updates_msgpack: blob(serde_json::json!({"marked_by": "me"})),
        })
        .unwrap_err();
        assert!(refusal.starts_with("TRUST_MARK_FIELDS_RESERVED"), "{refusal}");
    }

    #[cfg(feature = "security")]
    fn cas(updates: serde_json::Value) -> Method {
        Method::CompareAndSetNodeFields {
            node_id: "doc-1".into(),
            conditions_msgpack: blob(serde_json::json!({"status": "open"})),
            updates_msgpack: blob(updates),
        }
    }

    // spec: MEDULLA-SRC-AUTH-R011.3
    #[cfg(feature = "security")]
    #[test]
    fn cas_lowering_needs_approval_and_pins_current_mark() {
        let core = GraphCore::new();
        core.add_node(
            "doc-1".into(),
            blob(serde_json::json!({
                "node_type": "Doc",
                "trust_mark": "untrusted",
                "marked_by": "tool_result_untrusted",
                "marked_at_generation": 1,
            })),
        );
        let isolation = medulla_isolation::isolation::IsolationLayer::new();
        let stamp = |method| {
            super::stamp_trust_mark_if_applicable(method, &core, Some("agent-1"), &isolation)
        };

        // A write that touches no mark key passes through byte for byte.
        let plain = serde_json::json!({"status": "closed"});
        let Method::CompareAndSetNodeFields {
            updates_msgpack, ..
        } = stamp(cas(plain.clone())).unwrap()
        else {
            panic!("CompareAndSetNodeFields came back");
        };
        assert_eq!(updates_msgpack, blob(plain));

        // A lone reserved field without the mark is refused.
        let refusal = stamp(cas(serde_json::json!({"marked_by": "me"}))).unwrap_err();
        assert!(refusal.starts_with("TRUST_MARK_FIELDS_RESERVED"), "{refusal}");

        // Lowering without an approval id is refused.
        let refusal = stamp(cas(serde_json::json!({"trust_mark": "trusted"}))).unwrap_err();
        assert!(
            refusal.starts_with("TRUST_MARK_DOWNGRADE_REFUSED"),
            "{refusal}"
        );

        // With an approval id the updates carry the approved mark and the CAS pins the
        // current mark.
        let Method::CompareAndSetNodeFields {
            conditions_msgpack,
            updates_msgpack,
            ..
        } = stamp(cas(serde_json::json!({
            "trust_mark": "trusted",
            "trust_mark_approval_id": "approval-7",
        })))
        .unwrap()
        else {
            panic!("CompareAndSetNodeFields came back");
        };
        let updates = props(&updates_msgpack);
        assert_eq!(updates["trust_mark"], "trusted");
        assert_eq!(updates["marked_by"], "operator_approval");
        assert_eq!(updates["trust_mark_approval_id"], "approval-7");
        let conditions = props(&conditions_msgpack);
        assert_eq!(conditions["trust_mark"], "untrusted");
        assert_eq!(conditions["status"], "open");

        // An unmarked node counts as untrusted: the same request is refused, and the pinned
        // condition is null.
        core.add_node("doc-1".into(), blob(serde_json::json!({"node_type": "Doc"})));
        let refusal = stamp(cas(serde_json::json!({"trust_mark": "trusted"}))).unwrap_err();
        assert!(
            refusal.starts_with("TRUST_MARK_DOWNGRADE_REFUSED"),
            "{refusal}"
        );
        let Method::CompareAndSetNodeFields {
            conditions_msgpack, ..
        } = stamp(cas(serde_json::json!({"trust_mark": "untrusted"}))).unwrap()
        else {
            panic!("CompareAndSetNodeFields came back");
        };
        assert!(props(&conditions_msgpack)["trust_mark"].is_null());
    }

    struct Index;

    impl medulla_server_api::walkthrough_readers::RepositoryIndexReader for Index {
        fn current_revision(&self, tenant: &str, repository_id: &str) -> Option<String> {
            (tenant == "tenant-a" && repository_id == "repo-1").then(|| "rev-2".to_string())
        }

        fn snapshot_contains(
            &self,
            tenant: &str,
            repository_id: &str,
            repository_revision: &str,
            entity_id: &str,
            source_revision: &str,
            source_digest: &medulla_types::contract::Digest256,
        ) -> bool {
            tenant == "tenant-a"
                && repository_id == "repo-1"
                && repository_revision == "rev-2"
                && entity_id == "entity-1"
                && source_revision == "rev-2"
                && *source_digest == medulla_types::contract::Digest256::sha256(b"source")
        }
    }

    struct Blobs;

    impl medulla_server_api::walkthrough_readers::BlobReader for Blobs {
        fn read(&self, owner_scope: &str, artifact_ref: &str) -> Option<Vec<u8>> {
            (owner_scope == "owner-a" && artifact_ref == "blob:1").then(|| b"artifact".to_vec())
        }
    }

    fn walkthrough() -> serde_json::Value {
        use medulla_types::contract::Digest256;
        serde_json::json!({
            "run_id": "run-1",
            "repository_id": "repo-1",
            "repository_revision": "rev-2",
            "graph_snapshot_revision": 7,
            "query_digest": Digest256::sha256(b"query"),
            "ordered_sections": [{
                "ordinal": 0,
                "source_refs": [{
                    "entity_id": "entity-1",
                    "source_revision": "rev-2",
                    "source_digest": Digest256::sha256(b"source"),
                    "path": "src/lib.rs",
                    "start_line": 1,
                    "end_line": 3,
                }],
                "content": {"mode": "referenced", "digest": Digest256::sha256(b"content")},
            }],
            "interaction_edges": [],
            "artifact_ref": "blob:1",
            "artifact_digest": Digest256::sha256(b"artifact"),
            "execution_notes": [{
                "run_id": "run-1",
                "step_index": 0,
                "span_id": "span-1",
                "kind": "tool_call",
                "result_digest": Digest256::sha256(b"result"),
                "source_refs": [],
            }],
        })
    }

    fn walkthrough_write(walkthrough: serde_json::Value) -> Method {
        let props = serde_json::json!({
            "node_type": "RunTrace",
            "terminationReason": "final_answer",
            "steps": [step(0, &hex('b'))],
            "walkthrough": walkthrough,
        });
        Method::AddNode {
            node_id: "trace-walkthrough".into(),
            properties_msgpack: rmp_serde::to_vec_named(&props).unwrap(),
        }
    }

    /// The handler's way: resolve through the readers, then admit with the result.
    fn admit(write: &Method, tenant: &str) -> Result<Method, String> {
        use medulla_server_api::walkthrough_readers::WalkthroughReaders;
        let readers = WalkthroughReaders::new(std::sync::Arc::new(Index), std::sync::Arc::new(Blobs));
        let checks =
            crate::harness_admission::resolve_walkthrough_checks(
            write,
            tenant,
            if tenant == "tenant-a" { "owner-a" } else { "owner-b" },
            Some(&readers),
        );
        if let crate::harness_admission::WalkthroughChecks::Refused(refusal) = checks {
            return Err(refusal.code().as_str().to_string());
        }
        crate::harness_admission::direct_add_node_checked(
            write,
            &GraphCore::new(),
            7,
            None,
            checks,
        )
    }

    // spec: MEDULLA-ORCH-VOCAB-R013.7.2
    #[test]
    fn add_run_trace_rejects_stale_walkthrough_source_or_artifact() {
        // The valid manifest is admitted.
        admit(&walkthrough_write(walkthrough()), "tenant-a").expect("a valid walkthrough");

        // A stale or missing repository revision.
        let mut stale = walkthrough();
        stale["repository_revision"] = "rev-1".into();
        let refusal = admit(&walkthrough_write(stale), "tenant-a").unwrap_err();
        assert!(refusal.starts_with("WALKTHROUGH_REVISION_STALE"), "{refusal}");

        // A cross-tenant manifest reads as absent.
        let refusal = admit(&walkthrough_write(walkthrough()), "tenant-b").unwrap_err();
        assert!(refusal.starts_with("WALKTHROUGH_REVISION_STALE"), "{refusal}");

        // A ref outside the pinned snapshot.
        let mut outside = walkthrough();
        outside["ordered_sections"][0]["source_refs"][0]["entity_id"] = "entity-9".into();
        let refusal = admit(&walkthrough_write(outside), "tenant-a").unwrap_err();
        assert!(
            refusal.starts_with("WALKTHROUGH_REF_OUTSIDE_SNAPSHOT"),
            "{refusal}"
        );

        // An unresolved or tampered artifact.
        let mut tampered = walkthrough();
        tampered["artifact_digest"] = serde_json::to_value(
            medulla_types::contract::Digest256::sha256(b"other"),
        )
        .unwrap();
        let refusal = admit(&walkthrough_write(tampered), "tenant-a").unwrap_err();
        assert!(
            refusal.starts_with("WALKTHROUGH_ARTIFACT_UNRESOLVED"),
            "{refusal}"
        );
        let mut unresolved = walkthrough();
        unresolved["artifact_ref"] = "blob:missing".into();
        let refusal = admit(&walkthrough_write(unresolved), "tenant-a").unwrap_err();
        assert!(
            refusal.starts_with("WALKTHROUGH_ARTIFACT_UNRESOLVED"),
            "{refusal}"
        );

        // A section with no source ref.
        let mut uncited = walkthrough();
        uncited["ordered_sections"][0]["source_refs"] = serde_json::json!([]);
        let refusal = admit(&walkthrough_write(uncited), "tenant-a").unwrap_err();
        assert!(refusal.starts_with("WALKTHROUGH_SECTION_UNCITED"), "{refusal}");

        // A duplicate or gapped note index.
        let mut gapped = walkthrough();
        gapped["execution_notes"][0]["step_index"] = 2.into();
        let refusal = admit(&walkthrough_write(gapped), "tenant-a").unwrap_err();
        assert!(refusal.starts_with("WALKTHROUGH_NOTE_ORDER_INVALID"), "{refusal}");

        // A span outside the run.
        let mut other_run = walkthrough();
        other_run["execution_notes"][0]["run_id"] = "run-2".into();
        let refusal = admit(&walkthrough_write(other_run), "tenant-a").unwrap_err();
        assert!(refusal.starts_with("WALKTHROUGH_SPAN_OUTSIDE_RUN"), "{refusal}");
    }
}
