//! EH-403: `GraphSchema.AttachApproved` -- the one way a governed schema
//! candidate reaches a graph's schema sources.
//!
//! A source-authority candidate is approved by the governed ledger and spent
//! once by a graph node written in the attach's own commit; every other
//! candidate is approved by a lease.
//!
//! Before an ordinary attach is staged, the handler reads the named approval
//! lease from the request graph's native control-lease rows (tenant-bound: another
//! tenant's lease is simply not visible) and checks it with
//! [`medulla_catalog_types::graph_schema::approval::verify_schema_approval`] against the
//! exact candidate digest. Only then does the attach enter the ordinary
//! GraphSchema gateway commit (audit, CDC, ordering, full entering-schema
//! validation), under the reserved `approved:` key with an `Approved` origin
//! that records the lease id as provenance.

#[cfg(feature = "redb")]
use medulla_persistence::AsRedb;
use std::sync::Arc;

pub use medulla_catalog_types::graph_schema::shadow_branch::ShadowCheckSummary;
use medulla_catalog_types::graph_schema::GraphSchemaErrorCode;
use medulla_catalog_types::graph_schema::GraphSchemaOp;
use medulla_catalog_types::{
    graph_schema::approval::approved_candidate_digest,
    graph_schema::approval::verify_schema_approval,
    graph_schema::approval::SCHEMA_APPROVAL_ACTION, graph_schema::repair::RecordContract,
};
use medulla_mutation_types::governed_change::{
    governed_change_of_ref, APPROVAL_SPENT_NODE_PREFIX, SOURCE_AUTHORITY_KIND,
};
use medulla_graph_core::graph::{GraphCore, GraphSchemaSource, GraphSchemaSources};
use serde::{Deserialize, Serialize};

use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_server_mutation::mutation::MutationCtx;
use medulla_server_mutation::mutation::MutationPlan;

/// Verify the approval, then commit the attach through the gateway. A
/// source-authority attach spends its governed approval inside that same
/// commit (see [`spend_authority_approval`]), so there is no step before it.
pub(super) async fn handle_gateway(
    tenant_id: &str,
    ctx: &MutationCtx<'_>,
    plan: &MutationPlan,
    method: &Method,
    op: &GraphSchemaOp,
) -> Response {
    // A retry under the same request key replays the original receipt before
    // any re-verification, so it still replays after the approval window closes.
    if !already_committed(ctx).await {
        if let Err(error) = admit(tenant_id, ctx, op).await {
            return Response::err(ctx.req_id, error);
        }
    }
    let op = op.clone();
    let graph_name = ctx.graph_name.to_string();
    medulla_server_ops::api::ingest::graph::commit_gateway(ctx, plan, method, move |core| {
        super::apply(core, &graph_name, &op)
    })
    .await
}

/// Whether this request key already committed a batch on the graph: the retry
/// of an attach that landed. The commit below answers it with the stored
/// receipt through the batch idempotency. A failed lookup is not a replay, so
/// the approval is verified as for a first attempt.
async fn already_committed(ctx: &MutationCtx<'_>) -> bool {
    let Some(persistence) = ctx.persistence else {
        return false;
    };
    let batch_id = medulla_server_api::batch_compile::opaque_idempotency_key_for_context(
        "rpc",
        ctx.tenant_scope,
        ctx.graph_name,
        ctx.caller,
        ctx.idempotency_key,
    );
    let fname = medulla_kernel::persist::sanitize(ctx.graph_name);
    matches!(
        persistence.read_mutation_batch(&fname, &batch_id).await,
        Ok(Some(_))
    )
}

/// The approval check that must pass before the attach is staged. An ordinary
/// candidate is proven by its consumed approval lease; a source-authority
/// candidate is proven by the approved change in the governed ledger. Neither
/// reads whether the approval is spent: that is decided inside the commit, after
/// the batch idempotency replay, so a retried attach replays its receipt.
async fn admit(
    tenant_id: &str,
    ctx: &MutationCtx<'_>,
    op: &GraphSchemaOp,
) -> Result<(), String> {
    op.validate()?;
    let GraphSchemaOp::AttachApproved {
        source_id,
        contract,
        approval_lease_id,
        expected_digest,
        ..
    } = op
    else {
        return Err("attach_approved handler received another operation".to_string());
    };
    let candidate = approved_candidate_digest(source_id, contract);
    let now_ms = medulla_server_api::txn::now_ms();
    if contract.source_authority {
        #[cfg(feature = "security")]
        let now_ms = ctx.isolation.governed_now_ms();
        return verify_governed_change(
            ctx,
            tenant_id,
            approval_lease_id,
            source_id,
            &candidate,
            now_ms,
        );
    }
    let lease = read_lease(ctx, tenant_id, approval_lease_id).await?;
    if lease.is_none() && expected_digest.is_some() {
        return Err(format!(
            "{}: no approval lease of this tenant covers the selected candidate",
            GraphSchemaErrorCode::CandidateUnknown.as_str()
        ));
    }
    let executor = medulla_server_api::batch_compile::principal_fingerprint(
        ctx.caller
            .ok_or("ACCESS_DENIED: an approved attach requires a verified caller")?,
    )?;
    verify_schema_approval(
        lease.as_ref(),
        approval_action(contract),
        source_id,
        &candidate,
        now_ms,
        &executor,
    )
}

/// What the spent-approval node records.
#[derive(Serialize)]
struct ApprovalSpent<'a> {
    change_id: &'a str,
    source_id: &'a str,
    candidate_digest: &'a str,
}

/// Spend the governed approval of a source-authority attach
/// (MEDULLA-SRC-AUTH-R003.3): write the node `approval-spent:<change_id>` only
/// if it is absent. It runs inside the attach's own gateway commit, on the
/// authoritative staged image, so the spend and the attach are one graph commit:
/// a failed attach spends nothing, and a different request for the same change
/// finds the node and is refused "already used". The same request key never
/// reaches here a second time (the batch idempotency replays its receipt).
pub(super) fn spend_authority_approval(core: &GraphCore, op: &GraphSchemaOp) -> Result<(), String> {
    let GraphSchemaOp::AttachApproved {
        source_id,
        contract,
        approval_lease_id,
        ..
    } = op
    else {
        return Err("attach_approved handler received another operation".to_string());
    };
    if !contract.source_authority {
        return Ok(());
    }
    let change_id = governed_change_of_ref(approval_lease_id).ok_or_else(|| {
        format!(
            "{}: the approval reference names no governed change",
            GraphSchemaErrorCode::ApprovalMismatch.as_str()
        )
    })?;
    let candidate = approved_candidate_digest(source_id, contract);
    let properties = rmp_serde::to_vec_named(&ApprovalSpent {
        change_id,
        source_id,
        candidate_digest: &candidate,
    })
    .map_err(|error| format!("approval spent record encode failed: {error}"))?;
    if core.create_node_if_absent(
        format!("{APPROVAL_SPENT_NODE_PREFIX}{change_id}"),
        properties,
    ) {
        Ok(())
    } else {
        Err(format!(
            "{}: the approval was already used",
            GraphSchemaErrorCode::ApprovalRequired.as_str()
        ))
    }
}

/// A source-authority attach is approved by the governed ledger itself
/// (MEDULLA-SRC-AUTH-R003.3): the approval reference names a change, and that
/// change is `approved` and inside its window for `SOURCE_AUTHORITY_KIND`, the
/// same `source_id` and the same candidate digest. `SCHEMA_APPROVAL_MISMATCH`
/// when the reference names no change or the change names another kind, source
/// or digest; `SCHEMA_APPROVAL_REQUIRED` when the change is unknown, not
/// approved or past its window. The change must also have been proposed by the
/// attaching request's verified tenant (`tenant_id`): another tenant's change
/// answers byte-identically to an unknown change id, so the ledger is no
/// existence oracle across tenants.
#[cfg(feature = "security")]
fn verify_governed_change(
    ctx: &MutationCtx<'_>,
    tenant_id: &str,
    approval_ref: &str,
    source_id: &str,
    candidate: &str,
    now_ms: u64,
) -> Result<(), String> {
    use medulla_mutation_types::governed_change::GovernedStatus;
    let mismatch = |detail: &str| {
        format!(
            "{}: {detail}",
            GraphSchemaErrorCode::ApprovalMismatch.as_str()
        )
    };
    let change_id = governed_change_of_ref(approval_ref)
        .ok_or_else(|| mismatch("the approval reference names no governed change"))?;
    let unknown = || {
        format!(
            "{}: the governed change of this approval is unknown",
            GraphSchemaErrorCode::ApprovalRequired.as_str()
        )
    };
    let change = ctx
        .isolation
        .rbac()
        .governed()
        .get(change_id, tenant_id, now_ms)
        .ok_or_else(unknown)?;
    if change.status != GovernedStatus::Approved {
        return Err(format!(
            "{}: the governed change of this approval is not approved",
            GraphSchemaErrorCode::ApprovalRequired.as_str()
        ));
    }
    if change.kind != SOURCE_AUTHORITY_KIND
        || change.target != source_id
        || change.digest != candidate
    {
        return Err(mismatch(
            "the governed change does not name this kind, source and candidate digest",
        ));
    }
    Ok(())
}

/// Without the RBAC image no governed change can be proven, so a
/// source-authority attach is refused.
#[cfg(not(feature = "security"))]
fn verify_governed_change(
    _ctx: &MutationCtx<'_>,
    _tenant_id: &str,
    _approval_ref: &str,
    _source_id: &str,
    _candidate: &str,
    _now_ms: u64,
) -> Result<(), String> {
    Err(approval_store_unavailable())
}

/// The action the approval grant must name: a source-authority candidate (the
/// typed `RecordContract::source_authority`) is governed as
/// `SOURCE_AUTHORITY_KIND`, every other candidate as `SCHEMA_APPROVAL_ACTION`.
fn approval_action(contract: &RecordContract) -> &'static str {
    if contract.source_authority {
        SOURCE_AUTHORITY_KIND
    } else {
        SCHEMA_APPROVAL_ACTION
    }
}

/// `ValidateRepair`: render the candidate and run the entering-schema
/// validation against the request graph without installing anything.
pub(super) async fn handle_validate(
    ctx: &MutationCtx<'_>,
    plan: &MutationPlan,
    method: &Method,
    op: &GraphSchemaOp,
) -> Response {
    let op = op.clone();
    let graph_name = ctx.graph_name.to_string();
    medulla_server_ops::api::ingest::graph::commit_gateway(ctx, plan, method, move |core| {
        validate(core, &graph_name, &op)
    })
    .await
}

fn validate(
    core: &medulla_graph_core::graph::GraphCore,
    graph_name: &str,
    op: &GraphSchemaOp,
) -> Result<medulla_protocol_types::protocol::ResultPayload, String> {
    op.validate()?;
    let mut sources = (*core.schema_sources()).clone();
    super::check_expected_digest(&sources, op.expected_composed_digest())?;
    sources.attach_dynamic(op.source_token().to_string(), source(op, None)?)?;
    medulla_server_api::schema_compose::validate_entering_schema(&sources)?;
    let report = shadow_ingest(core, op)?;
    store_report(core, &report)?;
    let current = core.schema_sources().composed_digest().to_hex();
    medulla_protocol_types::protocol::ResultPayload::of::<
        medulla_protocol_types::result_contract::reasoning::GraphSchema,
    >(medulla_catalog_types::graph_schema::GraphSchemaCommitted {
        schema_version: medulla_catalog_types::graph_schema::GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
        graph: graph_name.to_string(),
        composed_digest: current,
        graph_version: core.version(),
        changed: false,
        edit_head: super::edits::head(core),
        ontology_diff: None,
        revision: None,
    })
}

#[cfg(feature = "redb")]
async fn read_lease(
    ctx: &MutationCtx<'_>,
    tenant_id: &str,
    lease_id: &str,
) -> Result<Option<medulla_work_types::control_lease::ControlLeaseView>, String> {
    let backend = ctx
        .persistence
        .and_then(|backend| backend.as_redb())
        .ok_or_else(approval_store_unavailable)?;
    backend
        .read_control_lease(
            &medulla_kernel::persist::sanitize(ctx.graph_name),
            tenant_id,
            lease_id,
        )
        .await
}

/// Approval leases live only in the redb authority; without it no approval
/// can be proven, so the attach is refused.
#[cfg(not(feature = "redb"))]
async fn read_lease(
    _ctx: &MutationCtx<'_>,
    _tenant_id: &str,
    _lease_id: &str,
) -> Result<Option<medulla_work_types::control_lease::ControlLeaseView>, String> {
    Err(approval_store_unavailable())
}

fn approval_store_unavailable() -> String {
    format!(
        "{}: this engine build holds no native approval leases",
        medulla_catalog_types::graph_schema::GraphSchemaErrorCode::ApprovalRequired.as_str()
    )
}

/// The `Approved`-origin source a repair op installs (or, for
/// `ValidateRepair`, would install): EG renders the SHACL from the typed
/// contract (AUD-27).
pub(super) fn source(
    op: &GraphSchemaOp,
    shadow_report_digest: Option<String>,
) -> Result<GraphSchemaSource, String> {
    let (source_id, contract, approval) = match op {
        GraphSchemaOp::AttachApproved {
            source_id,
            contract,
            approval_lease_id,
            ..
        } => (source_id, contract, approval_lease_id.as_str()),
        GraphSchemaOp::ValidateRepair {
            source_id,
            contract,
            ..
        } => (source_id, contract, VALIDATION_ONLY),
        _ => return Err("approved schema source needs a repair operation".to_string()),
    };
    let name = source_id
        .strip_prefix(medulla_catalog_types::graph_schema::approval::APPROVED_SOURCE_PREFIX)
        .ok_or_else(|| "approved schema source id lacks its namespace".to_string())?;
    let shapes = medulla_catalog_types::graph_schema::repair::render_repair_shapes(name, contract);
    Ok(medulla_graph_core::graph::GraphSchemaSource::new(
        medulla_graph_core::graph::SchemaSourceOrigin::Approved {
            name: name.to_string(),
            approval_lease_id: approval.to_string(),
            shadow_report_digest,
        },
        Some(Arc::from(shapes)),
        None,
        0,
    )?
    .with_contract(contract.clone()))
}

/// The origin marker of a validated-but-never-installed candidate.
const VALIDATION_ONLY: &str = "validation-only";

/// Reserved node-id prefix of a stored shadow-ingest report. The report is a node of the
/// request graph written inside the `ValidateRepair` gateway commit, so it is audited,
/// replicated and replayed with the schema operation that produced it.
const SHADOW_REPORT_PREFIX: &str = "schema-shadow-report:";

/// What re-validating the graph's current nodes under a staged candidate found
/// (MEDULLA-DECISION-ENGINE-R102.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ShadowIngestReport {
    pub(super) candidate_digest: String,
    pub(super) nodes_ingested: u64,
    pub(super) violations: u64,
}

impl ShadowIngestReport {
    /// The counts of this report, with no candidate identity.
    fn summary(&self) -> ShadowCheckSummary {
        ShadowCheckSummary {
            nodes_checked: self.nodes_ingested,
            violations: self.violations,
        }
    }

    fn digest(&self) -> String {
        let canonical = serde_json::to_string(self).unwrap_or_default();
        medulla_types::contract::Digest256::sha256(canonical.as_bytes()).to_hex()
    }

    fn node_id(candidate_digest: &str) -> String {
        format!("{SHADOW_REPORT_PREFIX}{candidate_digest}")
    }
}

/// The counts of the shadow check `ValidateRepair` stored for `candidate_digest`, or `None`
/// when no check of that candidate is on the graph.
pub fn shadow_check_summary(core: &GraphCore, candidate_digest: &str) -> Option<ShadowCheckSummary> {
    core.get_node_properties(&ShadowIngestReport::node_id(candidate_digest))
        .and_then(|bytes| rmp_serde::from_slice::<ShadowIngestReport>(&bytes).ok())
        .filter(|report| report.candidate_digest == candidate_digest)
        .map(|report| report.summary())
}

/// Whether `value` has one of the JSON types a field allows.
fn has_allowed_type(
    value: &serde_json::Value,
    types: &[medulla_catalog_types::graph_schema::repair::JsonType],
) -> bool {
    use medulla_catalog_types::graph_schema::repair::JsonType;
    types.iter().any(|kind| match kind {
        JsonType::Null => value.is_null(),
        JsonType::Boolean => value.is_boolean(),
        JsonType::Integer => value.is_i64() || value.is_u64(),
        JsonType::Number => value.is_number(),
        JsonType::String => value.is_string(),
        JsonType::Array => value.is_array(),
        JsonType::Object => value.is_object(),
    })
}

/// Whether one node's properties break the candidate's record contract.
fn breaks_contract(
    properties: &[u8],
    contract: &medulla_catalog_types::graph_schema::repair::RecordContract,
) -> bool {
    let Ok(serde_json::Value::Object(record)) =
        rmp_serde::from_slice::<serde_json::Value>(properties)
    else {
        return true;
    };
    contract
        .fields
        .iter()
        .any(|(name, field)| match record.get(name) {
            None => field.required,
            Some(value) => !has_allowed_type(value, &field.types),
        })
}

/// `rdf:type`, the relationship of a typing edge.
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// Whether the node `id` is a record of `class`, the class the candidate's
/// shapes target: a `type` property naming the class, or an `rdf:type` edge to
/// the class node. Bookkeeping rows (approval leases, edit-chain and report
/// nodes) are never of that class, so the shadow ingest leaves them out.
fn is_target_record(core: &GraphCore, id: &str, properties: &[u8], class: &str) -> bool {
    let typed = matches!(
        rmp_serde::from_slice::<serde_json::Value>(properties),
        Ok(serde_json::Value::Object(row))
            if row.get("type").and_then(serde_json::Value::as_str) == Some(class)
    );
    typed
        || core
            .get_edge_properties(id, &format!("<{class}>"))
            .iter()
            .any(|edge| {
                medulla_types::msgpack::decode_edge_relationship(edge).as_deref() == Some(RDF_TYPE)
            })
}

/// Stage the candidate on a clone of the schema sources, then re-validate up to
/// `knob schema_repair.shadow_sample_nodes` current nodes of the request graph against
/// the candidate's record contract. Only records of the class the candidate's
/// shapes target are sampled; bookkeeping nodes are not data.
pub(super) fn shadow_ingest(
    core: &GraphCore,
    op: &GraphSchemaOp,
) -> Result<ShadowIngestReport, String> {
    let (source_id, contract) = match op {
        GraphSchemaOp::AttachApproved {
            source_id,
            contract,
            ..
        }
        | GraphSchemaOp::ValidateRepair {
            source_id,
            contract,
            ..
        } => (source_id, contract),
        _ => return Err("shadow ingest needs a repair operation".to_string()),
    };
    let mut staged: GraphSchemaSources = (*core.schema_sources()).clone();
    staged.attach_dynamic(source_id.clone(), source(op, None)?)?;
    let name = source_id
        .strip_prefix(medulla_catalog_types::graph_schema::approval::APPROVED_SOURCE_PREFIX)
        .ok_or_else(|| "approved schema source id lacks its namespace".to_string())?;
    let class = medulla_catalog_types::graph_schema::repair::repair_record_class(name);
    let cap = medulla_server_api::knob_config::knobs().schema_repair_shadow_sample_nodes;
    let mut ids = core.node_ids();
    ids.retain(|id| !id.starts_with(SHADOW_REPORT_PREFIX));
    ids.sort();
    let (mut nodes_ingested, mut violations) = (0u64, 0u64);
    for id in ids {
        if nodes_ingested >= cap {
            break;
        }
        let Some(properties) = core.get_node_properties(&id) else {
            continue;
        };
        if is_control_lease(&properties) || !is_target_record(core, &id, &properties, &class) {
            continue;
        }
        nodes_ingested += 1;
        if breaks_contract(&properties, contract) {
            violations += 1;
        }
    }
    Ok(ShadowIngestReport {
        candidate_digest: approved_candidate_digest(source_id, contract),
        nodes_ingested,
        violations,
    })
}

/// Whether a node is a control-lease row (an approval lives in the request
/// graph beside the data it governs); a lease is not data.
fn is_control_lease(properties: &[u8]) -> bool {
    rmp_serde::from_slice::<serde_json::Value>(properties)
        .ok()
        .and_then(|record| {
            record
                .get("node_type")
                .and_then(serde_json::Value::as_str)
                .map(|node_type| {
                    node_type == medulla_work_types::control_lease::CONTROL_LEASE_NODE_TYPE
                })
        })
        .unwrap_or(false)
}

/// Store the report as the control row of its candidate (replacing an earlier one).
fn store_report(core: &GraphCore, report: &ShadowIngestReport) -> Result<(), String> {
    let properties = rmp_serde::to_vec_named(report)
        .map_err(|error| format!("shadow ingest report encode failed: {error}"))?;
    core.add_node(
        ShadowIngestReport::node_id(&report.candidate_digest),
        properties,
    );
    Ok(())
}

/// The stored report of `op`'s candidate, or the refusal that keeps it from activating.
fn require_report(core: &GraphCore, op: &GraphSchemaOp) -> Result<ShadowIngestReport, String> {
    let GraphSchemaOp::AttachApproved {
        source_id,
        contract,
        ..
    } = op
    else {
        return Err("attach_approved handler received another operation".to_string());
    };
    let candidate = approved_candidate_digest(source_id, contract);
    let stored = core
        .get_node_properties(&ShadowIngestReport::node_id(&candidate))
        .and_then(|bytes| rmp_serde::from_slice::<ShadowIngestReport>(&bytes).ok())
        .filter(|report| report.candidate_digest == candidate && report.nodes_ingested > 0);
    let Some(report) = stored else {
        return Err(format!(
            "{}: run ValidateRepair against current data before AttachApproved",
            GraphSchemaErrorCode::RepairShadowMissing.as_str()
        ));
    };
    if report.violations > 0 {
        return Err(format!(
            "{}: the shadow ingest found {} violating nodes",
            GraphSchemaErrorCode::RepairShadowViolations.as_str(),
            report.violations
        ));
    }
    Ok(report)
}

/// The source `AttachApproved` installs, once the shadow ingest, the operator's expected
/// digest and the drift rule all pass. Each refusal changes no schema
/// (MEDULLA-DECISION-ENGINE-R102.2, MEDULLA-TYPED-PACKS-R808).
pub(super) fn admitted_source(
    core: &GraphCore,
    sources: &GraphSchemaSources,
    op: &GraphSchemaOp,
) -> Result<GraphSchemaSource, String> {
    let GraphSchemaOp::AttachApproved {
        source_id,
        expected_digest,
        ..
    } = op
    else {
        return Err("attach_approved handler received another operation".to_string());
    };
    let candidate = source(op, None)?;
    if let Some(expected) = expected_digest {
        let actual = candidate
            .shapes_sha256
            .or(candidate.ontology_sha256)
            .map(|digest| digest.to_hex())
            .unwrap_or_default();
        if *expected != actual {
            return Err(format!(
                "{}: the selected digest is not the candidate's document digest",
                GraphSchemaErrorCode::DigestMismatch.as_str()
            ));
        }
        let drifted = sources.dynamic.get(source_id).is_some_and(|current| {
            current.shapes_sha256.or(current.ontology_sha256)
                != candidate.shapes_sha256.or(candidate.ontology_sha256)
        });
        if drifted {
            return Err(format!(
                "{}: a different candidate is already attached under {source_id}",
                GraphSchemaErrorCode::SourceDrift.as_str()
            ));
        }
    }
    let report = require_report(core, op)?;
    source(op, Some(report.digest()))
}

#[cfg(all(test, feature = "redb"))]
mod tests;
