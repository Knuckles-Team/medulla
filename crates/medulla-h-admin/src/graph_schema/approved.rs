//! EH-403: `GraphSchema.AttachApproved` -- the one way a governed schema
//! candidate reaches a graph's schema sources.
//!
//! Before anything is staged, the handler reads the named approval lease from
//! the request graph's native control-lease rows (tenant-bound: another
//! tenant's lease is simply not visible) and checks it with
//! [`medulla_catalog_types::graph_schema::approval::verify_schema_approval`] against the
//! exact candidate digest. Only then does the attach enter the ordinary
//! GraphSchema gateway commit (audit, CDC, ordering, full entering-schema
//! validation), under the reserved `approved:` key with an `Approved` origin
//! that records the lease id as provenance.

#[cfg(feature = "redb")]
use medulla_persistence::AsRedb;
use std::sync::Arc;

use medulla_catalog_types::graph_schema::GraphSchemaErrorCode;
use medulla_catalog_types::graph_schema::GraphSchemaOp;
use medulla_catalog_types::{
    graph_schema::approval::approved_candidate_digest,
    graph_schema::approval::verify_schema_approval,
};
use medulla_graph_core::graph::{GraphCore, GraphSchemaSource, GraphSchemaSources};
use serde::{Deserialize, Serialize};

use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_server_mutation::mutation::MutationCtx;
use medulla_server_mutation::mutation::MutationPlan;

/// Verify the approval, then commit the attach through the gateway.
pub(super) async fn handle_gateway(
    tenant_id: &str,
    ctx: &MutationCtx<'_>,
    plan: &MutationPlan,
    method: &Method,
    op: &GraphSchemaOp,
) -> Response {
    if let Err(error) = admit(tenant_id, ctx, op).await {
        return Response::err(ctx.req_id, error);
    }
    let op = op.clone();
    let graph_name = ctx.graph_name.to_string();
    medulla_server_ops::api::ingest::graph::commit_gateway(ctx, plan, method, move |core| {
        super::apply(core, &graph_name, &op)
    })
    .await
}

/// The approval check that must pass before the attach is staged.
async fn admit(tenant_id: &str, ctx: &MutationCtx<'_>, op: &GraphSchemaOp) -> Result<(), String> {
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
    let lease = read_lease(ctx, tenant_id, approval_lease_id).await?;
    if lease.is_none() && expected_digest.is_some() {
        return Err(format!(
            "{}: no approval lease of this tenant covers the selected candidate",
            GraphSchemaErrorCode::CandidateUnknown.as_str()
        ));
    }
    verify_schema_approval(
        lease.as_ref(),
        source_id,
        &candidate,
        medulla_server_api::txn::now_ms(),
    )
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
    medulla_graph_core::graph::GraphSchemaSource::new(
        medulla_graph_core::graph::SchemaSourceOrigin::Approved {
            name: name.to_string(),
            approval_lease_id: approval.to_string(),
            shadow_report_digest,
        },
        Some(Arc::from(shapes)),
        None,
        0,
    )
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
    fn digest(&self) -> String {
        let canonical = serde_json::to_string(self).unwrap_or_default();
        medulla_types::contract::Digest256::sha256(canonical.as_bytes()).to_hex()
    }

    fn node_id(candidate_digest: &str) -> String {
        format!("{SHADOW_REPORT_PREFIX}{candidate_digest}")
    }
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

/// Stage the candidate on a clone of the schema sources, then re-validate up to
/// `knob schema_repair.shadow_sample_nodes` current nodes of the request graph against
/// the candidate's record contract. Report and control-lease nodes are not data
/// and are skipped.
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
    let cap = medulla_server_api::knob_config::knobs().schema_repair_shadow_sample_nodes;
    let mut ids = core.node_ids();
    ids.retain(|id| !id.starts_with(SHADOW_REPORT_PREFIX));
    ids.sort();
    let (mut nodes_ingested, mut violations) = (0u64, 0u64);
    for id in ids
        .into_iter()
        .take(usize::try_from(cap).unwrap_or(usize::MAX))
    {
        let Some(properties) = core.get_node_properties(&id) else {
            continue;
        };
        if is_control_lease(&properties) {
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
