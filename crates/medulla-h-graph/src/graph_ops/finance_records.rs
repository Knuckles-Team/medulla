//! Served finance-v1 records (MEDULLA-FINANCE-PRIMITIVES-R004.3.2).
//!
//! `FinanceRecordPut` is gateway-routed (`Route::FinanceRecords`): the declared
//! schema version is admitted first (`admit_finance_record_version`), then the
//! kind policy (`FINANCE_RECORD_POLICY`) refuses a derived kind, the record's
//! tenant must be the verified request tenant, the record must conform to its
//! `<Kind>Shape` in `finance-v1.shapes.ttl`, and the writer's expected head must
//! match the stored one. The admitted record then commits as the one `AddNode`
//! upsert through the same coalescable gateway `Method::AddNode` uses, so the
//! durable log, audit and CDC see an ordinary node write.
//!
//! The five strategy research kinds (R751.754) are create-only: their `fields` hold
//! JSON text, the handler computes the fused scores with
//! `medulla_finance::research::fuse_scores`, resolves every evidence node of the
//! writer's tenant, and commits the record node plus one `derivedFrom` edge per
//! evidence node as one `BatchUpdate`. Each write emits the span
//! `medulla.finance.record_write`.
//!
//! `FinanceRecordGet` is a terminal read over the RLS-projected core: the stored
//! record node, refused with `ACCESS_DENIED` when it belongs to another tenant.

use super::*;

use super::terminal::GraphOpsContext;

/// The node property naming the opaque verified tenant scope of the writer.
const TENANT_SCOPE_KEY: &str = "finance_tenant_scope";
/// The node property holding the record head (a per-record revision counter).
#[cfg(all(feature = "finance", feature = "shacl"))]
const HEAD_KEY: &str = "finance_head";

#[cfg(all(feature = "finance", feature = "shacl"))]
const FINANCE_SHAPES: &str = include_str!("../../../medulla-core/ontology/finance-v1.shapes.ttl");

#[cfg(all(feature = "finance", feature = "shacl"))]
const TURTLE_PREFIXES: &str = "@prefix : <http://knuckles.team/kg#> .\n\
     @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n";

/// A finance refusal is its closed `ServerErrorCode` wire code, then the detail.
#[cfg(all(feature = "finance", feature = "shacl"))]
fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

#[cfg(all(feature = "finance", feature = "shacl"))]
fn is_local_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Each field value must be exactly one Turtle object term, so a value can never
/// smuggle extra triples into the validated document.
#[cfg(all(feature = "finance", feature = "shacl"))]
fn single_object_term(value: &str) -> bool {
    let probe = format!("{TURTLE_PREFIXES}<urn:medulla:probe> <urn:medulla:probe> {value} .\n");
    matches!(medulla_rdf::mapping::parse_turtle(&probe), Ok(triples) if triples.len() == 1)
}

/// Validate a record against its `<Kind>Shape`.
#[cfg(all(feature = "finance", feature = "shacl"))]
pub(super) fn check_record_shape(
    kind: medulla_types::wire::FinanceRecordKind,
    record: &medulla_types::wire::FinanceRecord,
) -> Result<(), String> {
    use medulla_types::wire::finance_record_refusal::SHAPE_INVALID;
    if !is_local_name(&record.record_id) {
        return Err(refuse(
            SHAPE_INVALID,
            "record_id must be a non-empty [A-Za-z0-9_] name",
        ));
    }
    let mut data = format!(
        "{TURTLE_PREFIXES}<urn:medulla:finance:{}> a :{}",
        record.record_id,
        kind.class_name()
    );
    for (name, value) in &record.fields {
        if !is_local_name(name) || !single_object_term(value) {
            return Err(refuse(
                SHAPE_INVALID,
                format!("field '{name}' is not one finance property term"),
            ));
        }
        data.push_str(&format!(" ;\n    :{name} {value}"));
    }
    data.push_str(" .\n");
    let report = medulla_shacl::validate_turtle(FINANCE_SHAPES, &data)
        .map_err(|error| refuse(SHAPE_INVALID, error))?;
    if report.conforms {
        Ok(())
    } else {
        Err(refuse(
            SHAPE_INVALID,
            format!(
                "{} violates {}Shape ({} result(s))",
                record.record_id,
                kind.class_name(),
                report.results.len()
            ),
        ))
    }
}

#[cfg(all(feature = "finance", feature = "shacl"))]
fn stored_props(
    core: &GraphCore,
    node_id: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    core.get_node_properties(node_id)
        .and_then(|blob| medulla_types::msgpack::decode_property_object(&blob).ok())
}

/// Admit one `FinanceRecordPut` and return the `AddNode` upsert it commits as,
/// with the new head.
#[cfg(all(feature = "finance", feature = "shacl"))]
pub(super) fn admit_put(
    core: &GraphCore,
    tenant_scope: &str,
    method: &Method,
) -> Result<(Method, u64), String> {
    use medulla_types::wire::{
        admit_finance_record_version, finance_record_refusal as code, FinanceRecordPolicy,
    };
    let Method::FinanceRecordPut {
        kind,
        schema_version,
        record,
        expected_head,
    } = method
    else {
        unreachable!("admit_put is only called for FinanceRecordPut");
    };
    admit_finance_record_version(schema_version)
        .map_err(|refusal| refuse(code::VERSION_UNKNOWN, refusal.reason))?;
    if kind.policy() == FinanceRecordPolicy::Derived {
        return Err(refuse(
            code::DERIVED_READONLY,
            format!("{} records are derived by the engine", kind.class_name()),
        ));
    }
    // Compared by the opaque carrier scope, not the raw name: replicated apply
    // runs under a context that carries only the leader's tenant scope
    // (`VerifiedRequestContext::replicated_mutation`), so a raw comparison
    // refused every replicated put on apply. On the leader the two agree,
    // because `tenant_scope` is the scope of the verified tenant.
    if medulla_server_api::carrier::tenant_scope_of(&record.tenant) != tenant_scope {
        return Err(format!(
            "ACCESS_DENIED: a {} record for another tenant cannot be written",
            kind.class_name()
        ));
    }
    if kind.is_research() {
        return admit_research(
            core,
            tenant_scope,
            *kind,
            schema_version,
            record,
            *expected_head,
        );
    }
    check_record_shape(*kind, record)?;
    let node_id = kind.node_id(&record.record_id);
    let stored = stored_props(core, &node_id);
    if let Some(props) = &stored {
        if props.get(TENANT_SCOPE_KEY).and_then(|v| v.as_str()) != Some(tenant_scope) {
            return Err(format!(
                "ACCESS_DENIED: {node_id} belongs to another tenant"
            ));
        }
    }
    let current_head = stored
        .as_ref()
        .and_then(|props| props.get(HEAD_KEY))
        .and_then(|v| v.as_u64());
    if current_head != *expected_head {
        return Err(refuse(
            code::HEAD_CONFLICT,
            format!("{node_id} head is {current_head:?}, the writer expected {expected_head:?}"),
        ));
    }
    let head = current_head.unwrap_or(0) + 1;
    let props = serde_json::json!({
        "type": kind.class_name(),
        "finance_kind": kind.class_name(),
        "record_id": record.record_id,
        "schema_version": schema_version,
        "tenant": record.tenant,
        TENANT_SCOPE_KEY: tenant_scope,
        HEAD_KEY: head,
        "fields": record.fields,
    });
    let properties_msgpack = rmp_serde::to_vec_named(&props)
        .map_err(|error| format!("INVALID_ARGUMENT: finance record encoding failed: {error}"))?;
    Ok((
        Method::AddNode {
            node_id,
            properties_msgpack,
        },
        head,
    ))
}

/// MEDULLA-FINANCE-PRIMITIVES-R004.3.2.9: the leader's checks before it proposes
/// the replicated `FinanceRecordPut`. They are exactly the admission of
/// [`admit_put`] (version gate, kind policy with `RECORD_DERIVED_READONLY` first,
/// tenant, shape, expected head) over the leader's applied core, so a put that
/// fails any of them proposes nothing and writes nothing. Replica apply runs the
/// same admission again through the one gateway path before its one upsert.
/// Compiled with the same features as its `graph_ops` re-export.
#[cfg(any(
    feature = "query",
    feature = "cypher",
    feature = "graphql",
    feature = "rdf"
))]
pub fn precheck_replicated_put(
    core: &GraphCore,
    tenant_scope: &str,
    method: &Method,
) -> Result<(), String> {
    #[cfg(all(feature = "finance", feature = "shacl"))]
    {
        admit_put(core, tenant_scope, method).map(|_| ())
    }
    #[cfg(not(all(feature = "finance", feature = "shacl")))]
    {
        let _ = (core, tenant_scope, method);
        Err(
            "METHOD_NOT_YET_SERVED: FinanceRecordPut needs the finance and shacl features"
                .to_string(),
        )
    }
}

/// The `Route::FinanceRecords` entry: admit the put, then commit it as the
/// `AddNode` upsert through the coalescable gateway.
pub(super) async fn handle_record(
    ctx: &MutationCtx<'_>,
    tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    if matches!(method, Method::FinanceAlertRaise { .. }) {
        return super::finance_alerts::raise(ctx, tenant_id, method).await;
    }
    if matches!(method, Method::FinanceFactorRecord { .. }) {
        return super::finance_factors::record(ctx, tenant_id, method).await;
    }
    let Method::FinanceRecordPut { kind, record, .. } = method else {
        return None;
    };
    #[cfg(all(feature = "finance", feature = "shacl"))]
    {
        use tracing::Instrument;
        // MEDULLA-FINANCE-PRIMITIVES-R751.755: one span per record write.
        let span = tracing::info_span!(
            "medulla.finance.record_write",
            tenant = tenant_id,
            record_class = kind.class_name(),
            record_id = record.record_id.as_str(),
            outcome = tracing::field::Empty,
        );
        let response = commit_put(ctx, method).instrument(span.clone()).await;
        span.record(
            "outcome",
            if response.error.is_none() {
                "committed"
            } else {
                "refused"
            },
        );
        Some(response)
    }
    #[cfg(not(all(feature = "finance", feature = "shacl")))]
    {
        let _ = (tenant_id, kind, record);
        Some(Response::err(
            ctx.req_id,
            "METHOD_NOT_YET_SERVED: FinanceRecordPut needs the finance and shacl features",
        ))
    }
}

/// MEDULLA-FINANCE-PRIMITIVES-R011.4: a `FinanceReceipt` written through
/// `CreateNodeIfAbsent` must carry a non-empty `idempotencyKey` (the
/// `FinanceReceiptShape` rule); an empty or missing key refuses
/// `RECEIPT_KEY_INVALID` and writes nothing. Any other node is not this rule's
/// concern.
pub(super) fn refuse_invalid_receipt(req_id: u64, method: &Method) -> Option<Response> {
    let Method::CreateNodeIfAbsent {
        properties_msgpack, ..
    } = method
    else {
        return None;
    };
    let props = medulla_types::msgpack::decode_property_object(properties_msgpack).ok()?;
    let is_receipt = [props.get("type"), props.get("finance_kind")]
        .into_iter()
        .flatten()
        .any(|class| class.as_str() == Some("FinanceReceipt"));
    if !is_receipt {
        return None;
    }
    let key_present = props
        .get("idempotencyKey")
        .and_then(|value| value.as_str())
        .is_some_and(|key| !key.trim().is_empty());
    if key_present {
        return None;
    }
    Some(Response::err(
        req_id,
        "RECEIPT_KEY_INVALID: a FinanceReceipt needs a non-empty idempotencyKey",
    ))
}

/// Admit one put, then commit it: the one `AddNode` upsert for a finance record,
/// the one `BatchUpdate` (record node plus `derivedFrom` edges) for a research
/// record, both through the gateway as one atomic mutation.
#[cfg(all(feature = "finance", feature = "shacl"))]
async fn commit_put(ctx: &MutationCtx<'_>, method: &Method) -> Response {
    let (write, head) = match admit_put(ctx.core, ctx.tenant_scope, method) {
        Ok(admitted) => admitted,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let plan = MutationPlan::for_method(&write);
    if let Method::BatchUpdate { operations_msgpack } = &write {
        let operations = operations_msgpack.clone();
        return medulla_server_ops::api::ingest::graph::commit_gateway(
            ctx,
            &plan,
            &write,
            move |core| {
                medulla_graph_algos::algorithms::batch_update(core, &operations)
                    .map(|_| ResultPayload::Count(head))
            },
        )
        .await;
    }
    let owned = write.clone();
    super::gateway::commit_gateway_coalescable(ctx, &plan, &write, move |core| {
        medulla_server_mutation::mutation::apply_coalescable_write(core, &owned)
            .map(|_| ResultPayload::Count(head))
    })
    .await
}

// ── strategy research records (MEDULLA-FINANCE-PRIMITIVES-R751.754) ──────────

/// The node property naming the kernel version that computed a research record.
#[cfg(all(feature = "finance", feature = "shacl"))]
const KERNEL_VERSION_KEY: &str = "research_kernel_version";

/// The edge label from a research record to each evidence node it rests on.
#[cfg(all(feature = "finance", feature = "shacl"))]
const DERIVED_FROM: &str = "derivedFrom";

#[cfg(all(feature = "finance", feature = "shacl"))]
fn research_refuse(error: medulla_finance::research::ResearchRecordError) -> String {
    refuse(error.code(), error.detail())
}

#[cfg(all(feature = "finance", feature = "shacl"))]
fn shape_invalid(detail: impl std::fmt::Display) -> String {
    refuse(
        medulla_types::wire::finance_record_refusal::SHAPE_INVALID,
        detail,
    )
}

/// A Turtle string literal for one value.
#[cfg(all(feature = "finance", feature = "shacl"))]
fn turtle_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

/// A Turtle `xsd:decimal` literal for one score.
#[cfg(all(feature = "finance", feature = "shacl"))]
fn turtle_decimal(value: f64) -> String {
    format!("\"{value}\"^^xsd:decimal")
}

/// Validate one subject's scalar properties against `<class>Shape`.
#[cfg(all(feature = "finance", feature = "shacl"))]
fn check_scalars(class: &str, subject: &str, terms: &[(&str, String)]) -> Result<(), String> {
    let mut data = format!("{TURTLE_PREFIXES}<urn:medulla:finance:{subject}> a :{class}");
    for (name, term) in terms {
        data.push_str(&format!(" ;\n    :{name} {term}"));
    }
    data.push_str(" .\n");
    let report = medulla_shacl::validate_turtle(FINANCE_SHAPES, &data).map_err(shape_invalid)?;
    if report.conforms {
        Ok(())
    } else {
        Err(shape_invalid(format!(
            "{subject} violates {class}Shape ({} result(s))",
            report.results.len()
        )))
    }
}

/// Resolve every evidence id to a node of the writer's tenant. Returns the ids
/// sorted and de-duplicated, the targets of the `derivedFrom` edges.
#[cfg(all(feature = "finance", feature = "shacl"))]
fn resolve_evidence<'a>(
    core: &GraphCore,
    tenant_scope: &str,
    refs: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<String>, String> {
    use medulla_finance::research::ResearchRecordError as Error;
    let mut ids = std::collections::BTreeSet::new();
    for id in refs {
        if id.is_empty() {
            return Err(research_refuse(Error::ScoreEvidenceMissing(
                "an evidence reference is empty".to_string(),
            )));
        }
        let Some(props) = stored_props(core, id) else {
            return Err(research_refuse(Error::ScoreEvidenceMissing(format!(
                "evidence node {id} does not resolve"
            ))));
        };
        let owner = props
            .get(TENANT_SCOPE_KEY)
            .or_else(|| props.get("tenant_scope"))
            .and_then(|v| v.as_str());
        if owner.is_some_and(|owner| owner != tenant_scope) {
            return Err(research_refuse(Error::RecordTenantMismatch(format!(
                "evidence node {id} belongs to another tenant"
            ))));
        }
        ids.insert(id.to_owned());
    }
    Ok(ids.into_iter().collect())
}

/// One research record after validation: the JSON it stores, the Turtle scalars
/// its shape checks and the evidence ids it cites.
#[cfg(all(feature = "finance", feature = "shacl"))]
type ShapeCheck = (&'static str, String, Vec<(&'static str, String)>);

#[cfg(all(feature = "finance", feature = "shacl"))]
struct ResearchRecord {
    stored: serde_json::Value,
    checks: Vec<ShapeCheck>,
    evidence: Vec<String>,
}

#[cfg(all(feature = "finance", feature = "shacl"))]
fn parse_research<T: serde::de::DeserializeOwned>(
    fields: &serde_json::Map<String, serde_json::Value>,
) -> Result<T, String> {
    serde_json::from_value(serde_json::Value::Object(fields.clone())).map_err(shape_invalid)
}

#[cfg(all(feature = "finance", feature = "shacl"))]
fn to_json<T: serde::Serialize>(value: &T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(shape_invalid)
}

/// Validate one research put and build its record. Reads the graph for evidence
/// and debate order; computes the fused scores.
#[cfg(all(feature = "finance", feature = "shacl"))]
fn build_research(
    core: &GraphCore,
    tenant_scope: &str,
    kind: medulla_types::wire::FinanceRecordKind,
    record: &medulla_types::wire::FinanceRecord,
) -> Result<ResearchRecord, String> {
    use medulla_finance::research::{fuse_scores, ResearchRecordError as Error};
    use medulla_types::finance_research as types;
    use medulla_types::wire::FinanceRecordKind as Kind;

    let mut fields = serde_json::Map::new();
    for (name, text) in &record.fields {
        let value = serde_json::from_str(text)
            .map_err(|_| shape_invalid(format!("field '{name}' is not JSON text")))?;
        fields.insert(name.clone(), value);
    }
    let id = record.record_id.as_str();
    let version = (
        "researchKernelVersion",
        turtle_string(types::RESEARCH_KERNEL_VERSION),
    );
    match kind {
        Kind::DebateRound => {
            let round: types::DebateRound = parse_research(&fields)?;
            let local = |name: &str| {
                is_local_name(name)
                    .then_some(())
                    .ok_or_else(|| shape_invalid("debateId must be a [A-Za-z0-9_] name"))
            };
            local(&round.debate_id)?;
            if round.round_index == 0 || id != format!("{}_r{}", round.debate_id, round.round_index)
            {
                return Err(shape_invalid(
                    "record_id must be <debateId>_r<roundIndex> with a roundIndex of 1 or more",
                ));
            }
            if round.round_index > 1 {
                let prior =
                    kind.node_id(&format!("{}_r{}", round.debate_id, round.round_index - 1));
                let owned = stored_props(core, &prior).is_some_and(|props| {
                    props.get(TENANT_SCOPE_KEY).and_then(|v| v.as_str()) == Some(tenant_scope)
                });
                if !owned {
                    return Err(research_refuse(Error::DebateRoundOutOfOrder(format!(
                        "round {} follows round {}, which is not stored",
                        round.round_index,
                        round.round_index - 1
                    ))));
                }
            }
            let evidence = resolve_evidence(
                core,
                tenant_scope,
                round.evidence_refs.iter().map(String::as_str),
            )?;
            let side = to_json(&round.side)?;
            let terms = vec![
                ("debateId", turtle_string(&round.debate_id)),
                ("roundIndex", round.round_index.to_string()),
                (
                    "debateSide",
                    turtle_string(side.as_str().unwrap_or_default()),
                ),
                ("debateArgument", turtle_string(&round.argument)),
            ];
            Ok(ResearchRecord {
                stored: to_json(&round)?,
                checks: vec![("DebateRound", id.to_owned(), terms)],
                evidence,
            })
        }
        Kind::TradeJournalEntry => {
            let entry: types::TradeJournalEntry = parse_research(&fields)?;
            let evidence = resolve_evidence(
                core,
                tenant_scope,
                entry
                    .bias_audits
                    .iter()
                    .flat_map(|audit| audit.evidence_refs.iter().map(String::as_str)),
            )?;
            let mut terms = vec![
                ("journalId", turtle_string(&entry.journal_id)),
                (
                    "entryAt",
                    format!("{}^^xsd:dateTime", turtle_string(&entry.entry_at)),
                ),
                ("journalRationale", turtle_string(&entry.rationale)),
            ];
            if let Some(run) = &entry.strategy_run_ref {
                terms.push(("strategyRunRef", turtle_string(run)));
            }
            let mut checks = vec![("TradeJournalEntry", id.to_owned(), terms)];
            for (index, audit) in entry.bias_audits.iter().enumerate() {
                let bias = to_json(&audit.bias_kind)?;
                checks.push((
                    "BiasAudit",
                    format!("{id}_bias{index}"),
                    vec![
                        ("biasKind", turtle_string(bias.as_str().unwrap_or_default())),
                        ("biasFinding", turtle_string(&audit.finding)),
                    ],
                ));
            }
            Ok(ResearchRecord {
                stored: to_json(&entry)?,
                checks,
                evidence,
            })
        }
        Kind::SentimentFusion => {
            let input: types::SentimentFusionInput = parse_research(&fields)?;
            let score = fuse_scores(&input.components, -1.0, 1.0).map_err(research_refuse)?;
            let evidence = resolve_evidence(
                core,
                tenant_scope,
                input.components.iter().map(|c| c.evidence_ref.as_str()),
            )?;
            let terms = vec![
                ("subjectListing", turtle_string(&input.subject_listing)),
                ("sentimentScore", turtle_decimal(score)),
                version,
            ];
            let stored = types::SentimentFusion {
                subject_listing: input.subject_listing,
                score,
                components: input.components,
            };
            Ok(ResearchRecord {
                stored: to_json(&stored)?,
                checks: vec![("SentimentFusion", id.to_owned(), terms)],
                evidence,
            })
        }
        Kind::GeopoliticalRisk => {
            let input: types::GeopoliticalRiskInput = parse_research(&fields)?;
            let score = fuse_scores(&input.components, 0.0, 1.0).map_err(research_refuse)?;
            let evidence = resolve_evidence(
                core,
                tenant_scope,
                input.components.iter().map(|c| c.evidence_ref.as_str()),
            )?;
            let terms = vec![
                ("geoRegion", turtle_string(&input.region)),
                ("geoRiskScore", turtle_decimal(score)),
                version,
            ];
            let stored = types::GeopoliticalRisk {
                region: input.region,
                score,
                components: input.components,
            };
            Ok(ResearchRecord {
                stored: to_json(&stored)?,
                checks: vec![("GeopoliticalRisk", id.to_owned(), terms)],
                evidence,
            })
        }
        Kind::StrategyCard => {
            if !fields.contains_key("licenceClass") {
                return Err(research_refuse(Error::StrategyCardLicenceMissing(
                    "a strategy card needs a licenceClass".to_string(),
                )));
            }
            let card: types::StrategyCard = parse_research(&fields)?;
            let licence = to_json(&card.licence_class)?;
            let terms = vec![
                ("cardId", turtle_string(&card.card_id)),
                ("strategySpecRef", turtle_string(&card.strategy_spec_ref)),
                (
                    "licenceClass",
                    turtle_string(licence.as_str().unwrap_or_default()),
                ),
                ("cardSummary", turtle_string(&card.summary)),
            ];
            Ok(ResearchRecord {
                stored: to_json(&card)?,
                checks: vec![("StrategyCard", id.to_owned(), terms)],
                evidence: Vec::new(),
            })
        }
        _ => unreachable!("build_research is only called for research kinds"),
    }
}

/// Admit one research `FinanceRecordPut` and return the single `BatchUpdate`
/// (the record node and its `derivedFrom` edges) it commits as, with the head.
#[cfg(all(feature = "finance", feature = "shacl"))]
fn admit_research(
    core: &GraphCore,
    tenant_scope: &str,
    kind: medulla_types::wire::FinanceRecordKind,
    schema_version: &str,
    record: &medulla_types::wire::FinanceRecord,
    expected_head: Option<u64>,
) -> Result<(Method, u64), String> {
    use medulla_finance::research::ResearchRecordError as Error;
    use medulla_types::wire::finance_record_refusal as code;
    if !is_local_name(&record.record_id) {
        return Err(shape_invalid(
            "record_id must be a non-empty [A-Za-z0-9_] name",
        ));
    }
    let node_id = kind.node_id(&record.record_id);
    if let Some(props) = stored_props(core, &node_id) {
        if props.get(TENANT_SCOPE_KEY).and_then(|v| v.as_str()) != Some(tenant_scope) {
            return Err(format!(
                "ACCESS_DENIED: {node_id} belongs to another tenant"
            ));
        }
        return Err(research_refuse(Error::RecordImmutable(format!(
            "{node_id} is stored and cannot be rewritten"
        ))));
    }
    if expected_head.is_some() {
        return Err(refuse(
            code::HEAD_CONFLICT,
            format!("{node_id} is new; the writer expected head {expected_head:?}"),
        ));
    }
    let built = build_research(core, tenant_scope, kind, record)?;
    for (class, subject, terms) in &built.checks {
        check_scalars(class, subject, terms)?;
    }
    let head = 1_u64;
    let props = serde_json::json!({
        "type": kind.class_name(),
        "finance_kind": kind.class_name(),
        "record_id": record.record_id,
        "schema_version": schema_version,
        "tenant": record.tenant,
        TENANT_SCOPE_KEY: tenant_scope,
        HEAD_KEY: head,
        KERNEL_VERSION_KEY: medulla_types::finance_research::RESEARCH_KERNEL_VERSION,
        "evidence_ids": built.evidence,
        "fields": built.stored,
    });
    let mut operations = vec![serde_json::json!({
        "op": "add_node",
        "id": node_id,
        "properties": props,
    })];
    for evidence in &built.evidence {
        operations.push(serde_json::json!({
            "op": "add_edge",
            "source": node_id,
            "target": evidence,
            "properties": {"relationship": DERIVED_FROM},
        }));
    }
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|error| format!("INVALID_ARGUMENT: finance record encoding failed: {error}"))?;
    Ok((Method::BatchUpdate { operations_msgpack }, head))
}

/// `FinanceRecordGet` over the RLS-projected core (`terminal.rs`).
pub(super) async fn handle_record_get(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::FinanceRecordGet { kind, record_id } = method else {
        return ControlFlow::Continue(method);
    };
    let node_id = kind.node_id(&record_id);
    let props = ctx.core.get_node_properties(&node_id);
    if let Some(blob) = &props {
        let owner_scope = medulla_types::msgpack::decode_property_object(blob)
            .ok()
            .and_then(|props| {
                props
                    .get(TENANT_SCOPE_KEY)
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            });
        let caller_scope = ctx
            .read_authority
            .carrier()
            .map(|carrier| carrier.tenant_scope().to_owned());
        if owner_scope.is_none() || owner_scope != caller_scope {
            return ControlFlow::Break(Response::err(
                ctx.req_id,
                format!("ACCESS_DENIED: {node_id} belongs to another tenant"),
            ));
        }
    }
    ControlFlow::Break(Response::ok(
        ctx.req_id,
        ResultPayload::of_encoded_or_null::<
            medulla_protocol_types::result_contract::graph::FinanceRecordGet,
        >(props),
    ))
}
