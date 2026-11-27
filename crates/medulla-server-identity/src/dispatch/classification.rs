//! `Method::ClassificationClaim`: the served classification claim lifecycle
//! (MEDULLA-TYPED-PACKS-R652.3).
//!
//! The handler reads the verified tenant and principal from the request carrier
//! (never from the body), decides every refusal before the first write, and
//! commits through the ordinary graph gateway (`dispatch_graph_op`), so each
//! write is ACL-checked, durable, audited and CDC-emitted. Three ops:
//!
//! * `Propose` stores a claim in state `proposed` (one node per asset and
//!   category set). Every category must be a registered marking: a node
//!   `marking:<name>` in the request graph.
//! * `Promote` needs the steward or owner role and at least one evidence item
//!   (the claim's own plus the request's), moves the claim `proposed` to
//!   `promoted`, stamps the categories on the asset's `_markings` property, and
//!   proposes them downstream along `cat:lineageTo` edges with provenance
//!   `propagated` (`marking_propagation::propagate`).
//! * `Reject` moves a proposed claim to `rejected`; the same role is required.
//!
//! Each claim node records the marking set before and after the transition, the
//! promoting principal and the evidence list (R652.5); reads go through UQL over
//! those nodes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use medulla_core::stewardship::TargetScopes;
use medulla_decision_types::decision::knob_families::classification as classification_knobs;
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_identity_types::classification::{
    claim_id as derive_claim_id, ClaimRecord, ClaimState, ClassificationClaim,
    ClassificationClaimOp, ClassificationClaimResult,
};
use medulla_identity_types::tenancy::Marking;
use medulla_isolation::isolation::marking_propagation::{legal_move, propagate, LineageEdges};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_protocol_types::result_contract::ingestion::ClassificationClaim as ClassificationClaimResultContract;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_graph::dispatch::graph_client::GraphTarget;
use medulla_server_graph::dispatch::graph_pipeline::dispatch_graph_op;
use medulla_server_graph::dispatch::shared::typed_response;
use medulla_server_graph::dispatch::stewardship::holds_review_role;
use medulla_types::contract::ServerErrorCode;
use tokio::sync::RwLock;

/// The property that holds the markings stamped on an asset.
pub const MARKINGS_PROPERTY: &str = medulla_server_api::read_scope::MARKINGS_KEY;
/// The node id prefix of a registered marking.
pub const MARKING_NODE_PREFIX: &str = "marking:";
/// Edges fetched per page while building the lineage map.
const EDGE_PAGE: usize = 1000;

type Properties = serde_json::Map<String, serde_json::Value>;

/// Who asked and which graph holds the claims.
pub struct ClassificationTarget<'a> {
    /// The request graph.
    pub graph: &'a str,
    /// The request id.
    pub req_id: u64,
    /// The calling agent, when named.
    pub caller: Option<&'a str>,
    /// The verified request carrier.
    pub verified: &'a VerifiedRequestContext,
}

/// Serve one `ClassificationClaim` op.
pub async fn dispatch_classification(
    state: &Arc<RwLock<ServerState>>,
    target: ClassificationTarget<'_>,
    op: ClassificationClaimOp,
) -> Response {
    let result = serve(state, &target, op).await;
    typed_response::<ClassificationClaimResultContract>(target.req_id, result)
}

fn refuse(code: ServerErrorCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

fn state_illegal(from: &str, to: &str) -> String {
    refuse(
        ServerErrorCode::ClaimStateIllegal,
        format!("{from} to {to}"),
    )
}

fn evidence_missing() -> String {
    refuse(
        ServerErrorCode::ClaimEvidenceMissing,
        "promotion needs at least one evidence item",
    )
}

fn unknown_marking(name: &str) -> String {
    refuse(ServerErrorCode::ClaimUnknownMarking, name)
}

/// The lineage hop bound: `classification.propagation_depth_max` from the server's resolved
/// knob snapshot, the pinned decision policy, or the knob default when none is pinned
/// (`medulla_decision_types::decision::knob_families::classification`).
async fn propagation_depth_max(state: &Arc<RwLock<ServerState>>) -> u32 {
    let guard = state.read().await;
    let spec = &classification_knobs::KNOBS[classification_knobs::PROPAGATION_DEPTH_MAX];
    let value = match guard.pinned_policy.as_deref() {
        Some(policy) => policy.knob(spec),
        None => spec.default,
    };
    match value {
        KnobValue::Integer(depth) => u32::try_from(depth).unwrap_or(u32::MAX),
        _ => u32::MAX,
    }
}

async fn serve(
    state: &Arc<RwLock<ServerState>>,
    target: &ClassificationTarget<'_>,
    op: ClassificationClaimOp,
) -> Result<ClassificationClaimResult, String> {
    let tenant = target.verified.tenant();
    if tenant.trim().is_empty() {
        return Err(ServerErrorCode::TenantScopeRequired.as_str().to_string());
    }
    match op {
        ClassificationClaimOp::Propose { claim } => propose(state, target, claim).await,
        ClassificationClaimOp::Promote { claim_id, evidence } => {
            review(state, target, &claim_id, ClaimState::Promoted, evidence).await
        }
        ClassificationClaimOp::Reject { claim_id } => {
            review(state, target, &claim_id, ClaimState::Rejected, Vec::new()).await
        }
    }
}

/// The steward or owner check reads the stewardship store (MEDULLA-TYPED-PACKS-R802.3):
/// the verified principal must be an owner or steward governing the claim's asset.
async fn require_review_role(
    state: &Arc<RwLock<ServerState>>,
    target: &ClassificationTarget<'_>,
    asset: &str,
) -> Result<(), String> {
    let graph_target = GraphTarget {
        graph: target.graph,
        req_id: target.req_id,
        caller: target.caller,
        verified: target.verified,
    };
    let scopes = TargetScopes {
        asset: Some(asset.to_string()),
        concept_scheme: None,
        domain: None,
    };
    if holds_review_role(state, &graph_target, &scopes).await? {
        Ok(())
    } else {
        Err(refuse(
            ServerErrorCode::StewardRoleMissing,
            "the steward or owner role is required",
        ))
    }
}

// ── reads and writes through the graph gateway ─────────────────────────────

async fn read_node(
    state: &Arc<RwLock<ServerState>>,
    target: &ClassificationTarget<'_>,
    node_id: &str,
) -> Result<Option<Properties>, String> {
    let response = dispatch_graph_op(
        state,
        target.graph,
        target.req_id,
        target.caller,
        target.verified,
        Method::GetNodeProperties {
            node_id: node_id.to_string(),
        },
    )
    .await;
    if let Some(refusal) = response.refusal_text() {
        return Err(refusal);
    }
    match response.result {
        Some(ResultPayload::Raw(bytes)) => medulla_types::msgpack::decode_property_object(&bytes)
            .map(Some)
            .map_err(|_| "INVALID_ARGUMENT: the node properties are malformed".to_string()),
        Some(ResultPayload::Json(serde_json::Value::Object(object))) => Ok(Some(object)),
        _ => Ok(None),
    }
}

async fn write(
    state: &Arc<RwLock<ServerState>>,
    target: &ClassificationTarget<'_>,
    method: Method,
) -> Result<Response, String> {
    let response = dispatch_graph_op(
        state,
        target.graph,
        target.req_id,
        target.caller,
        target.verified,
        method,
    )
    .await;
    match response.refusal_text() {
        Some(refusal) => Err(refusal),
        None => Ok(response),
    }
}

fn encode(value: &Properties) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value)
        .map_err(|error| format!("INVALID_ARGUMENT: claim encoding failed: {error}"))
}

/// Whether a write answer means the write took effect: a failed
/// compare-and-set answers `false`.
fn took_effect(response: &Response) -> bool {
    !matches!(
        &response.result,
        Some(ResultPayload::Bool(false))
            | Some(ResultPayload::Json(serde_json::Value::Bool(false)))
    )
}

fn names(markings: &[Marking]) -> Vec<String> {
    markings.iter().map(|m| m.as_str().to_string()).collect()
}

/// The markings stamped on an asset, as committed.
fn stamped(properties: &Properties) -> Vec<Marking> {
    let mut found: Vec<Marking> = properties
        .get(MARKINGS_PROPERTY)
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().and_then(|name| Marking::new(name).ok()))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found.dedup();
    found
}

/// The stored record of one claim, when its node exists and decodes.
async fn stored_record(
    state: &Arc<RwLock<ServerState>>,
    target: &ClassificationTarget<'_>,
    claim_id: &str,
) -> Result<Option<ClaimRecord>, String> {
    let Some(properties) = read_node(state, target, claim_id).await? else {
        return Ok(None);
    };
    ClaimRecord::from_node_properties(&properties)
}

async fn create_claim_node(
    state: &Arc<RwLock<ServerState>>,
    target: &ClassificationTarget<'_>,
    record: &ClaimRecord,
) -> Result<(), String> {
    let properties = record.node_properties(target.verified.tenant())?;
    write(
        state,
        target,
        Method::CreateNodeIfAbsent {
            node_id: record.claim_id.clone(),
            properties_msgpack: encode(&properties)?,
        },
    )
    .await
    .map(drop)
}

// ── Propose ────────────────────────────────────────────────────────────────

async fn propose(
    state: &Arc<RwLock<ServerState>>,
    target: &ClassificationTarget<'_>,
    claim: ClassificationClaim,
) -> Result<ClassificationClaimResult, String> {
    claim.validate()?;
    if claim.state != ClaimState::Proposed {
        return Err(state_illegal(
            claim.state.as_str(),
            ClaimState::Proposed.as_str(),
        ));
    }
    for category in &claim.categories {
        let node = format!("{MARKING_NODE_PREFIX}{}", category.as_str());
        if read_node(state, target, &node).await?.is_none() {
            return Err(unknown_marking(category.as_str()));
        }
    }
    let claim_id = claim.claim_id();
    if let Some(stored) = stored_record(state, target, &claim_id).await? {
        // A repeated proposal answers the stored claim; a decided one cannot be proposed again.
        return match stored.claim.state {
            ClaimState::Proposed => Ok(ClassificationClaimResult {
                record: stored,
                propagated: Vec::new(),
            }),
            decided => Err(state_illegal(
                decided.as_str(),
                ClaimState::Proposed.as_str(),
            )),
        };
    }
    let current = match read_node(state, target, &claim.asset).await? {
        Some(properties) => stamped(&properties),
        None => Vec::new(),
    };
    let record = ClaimRecord {
        claim_id,
        claim,
        before_markings: current.clone(),
        after_markings: current,
        promoted_by: None,
    };
    create_claim_node(state, target, &record).await?;
    Ok(ClassificationClaimResult {
        record,
        propagated: Vec::new(),
    })
}

// ── Promote and Reject ─────────────────────────────────────────────────────

async fn review(
    state: &Arc<RwLock<ServerState>>,
    target: &ClassificationTarget<'_>,
    claim_id: &str,
    to: ClaimState,
    request_evidence: Vec<String>,
) -> Result<ClassificationClaimResult, String> {
    let mut record = stored_record(state, target, claim_id)
        .await?
        .ok_or_else(|| format!("INVALID_ARGUMENT: claim '{claim_id}' does not exist"))?;
    require_review_role(state, target, &record.claim.asset).await?;
    if !legal_move(record.claim.state, to) {
        return Err(state_illegal(record.claim.state.as_str(), to.as_str()));
    }
    let next = to;
    for item in request_evidence {
        if !record.claim.evidence.contains(&item) {
            record.claim.evidence.push(item);
        }
    }
    if next == ClaimState::Promoted && record.claim.evidence.is_empty() {
        return Err(evidence_missing());
    }
    let asset_properties = read_node(state, target, &record.claim.asset).await?;
    if next == ClaimState::Promoted && asset_properties.is_none() {
        return Err(format!(
            "INVALID_ARGUMENT: asset '{}' does not exist",
            record.claim.asset
        ));
    }
    let before = asset_properties.as_ref().map(stamped).unwrap_or_default();
    let mut after = before.clone();
    if next == ClaimState::Promoted {
        after.extend(record.claim.categories.iter().cloned());
        after.sort();
        after.dedup();
    }
    let principal = target.verified.principal().to_string();
    record.claim.state = next;
    record.before_markings = before.clone();
    record.after_markings = after.clone();
    record.promoted_by = (next == ClaimState::Promoted).then_some(principal);

    // Everything that can refuse has been decided. The claim node moves first:
    // its compare-and-set on the state is what makes a second reviewer lose.
    let mut conditions = Properties::new();
    conditions.insert("state".to_string(), ClaimState::Proposed.as_str().into());
    let updates = record.node_properties(target.verified.tenant())?;
    let committed = write(
        state,
        target,
        Method::CompareAndSetNodeFields {
            node_id: claim_id.to_string(),
            conditions_msgpack: encode(&conditions)?,
            updates_msgpack: encode(&updates)?,
        },
    )
    .await?;
    if !took_effect(&committed) {
        return Err(state_illegal("decided", next.as_str()));
    }
    if next != ClaimState::Promoted {
        return Ok(ClassificationClaimResult {
            record,
            propagated: Vec::new(),
        });
    }

    stamp_asset(
        state,
        target,
        &record.claim.asset,
        asset_properties.as_ref(),
        &after,
    )
    .await?;
    let propagated = propagate_downstream(state, target, &record).await?;
    Ok(ClassificationClaimResult { record, propagated })
}

/// Stamp `markings` on the asset, conditioned on the markings read.
async fn stamp_asset(
    state: &Arc<RwLock<ServerState>>,
    target: &ClassificationTarget<'_>,
    asset: &str,
    current: Option<&Properties>,
    markings: &[Marking],
) -> Result<(), String> {
    let mut conditions = Properties::new();
    conditions.insert(
        MARKINGS_PROPERTY.to_string(),
        current
            .and_then(|properties| properties.get(MARKINGS_PROPERTY))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    let mut updates = Properties::new();
    updates.insert(
        MARKINGS_PROPERTY.to_string(),
        serde_json::json!(names(markings)),
    );
    let response = write(
        state,
        target,
        Method::CompareAndSetNodeFields {
            node_id: asset.to_string(),
            conditions_msgpack: encode(&conditions)?,
            updates_msgpack: encode(&updates)?,
        },
    )
    .await?;
    if took_effect(&response) {
        Ok(())
    } else {
        Err(format!(
            "INVALID_ARGUMENT: the markings of '{asset}' changed since they were read"
        ))
    }
}

// ── Propagation ────────────────────────────────────────────────────────────

fn is_lineage_to(relationship: &str) -> bool {
    matches!(relationship, "lineageTo" | "cat:lineageTo") || relationship.ends_with("#lineageTo")
}

fn is_derived_from(relationship: &str) -> bool {
    matches!(relationship, "derivedFrom" | ":derivedFrom") || relationship.ends_with("#derivedFrom")
}

/// The lineage map of the request graph: `cat:lineageTo` edges as written, and
/// `derivedFrom` edges reversed (`cat:lineageTo` is their inverse).
async fn read_lineage(
    state: &Arc<RwLock<ServerState>>,
    target: &ClassificationTarget<'_>,
) -> Result<LineageEdges, String> {
    let mut lineage: LineageEdges = BTreeMap::new();
    let mut after: Option<(String, String, u32)> = None;
    loop {
        let response = dispatch_graph_op(
            state,
            target.graph,
            target.req_id,
            target.caller,
            target.verified,
            Method::GetEdgesPage {
                after: after.clone(),
                limit: EDGE_PAGE,
            },
        )
        .await;
        if let Some(refusal) = response.refusal_text() {
            return Err(refusal);
        }
        let page: Vec<(String, String, u32, Vec<u8>)> = match response.result {
            Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes)
                .map_err(|_| "INVALID_ARGUMENT: the edge page is malformed".to_string())?,
            _ => Vec::new(),
        };
        let Some(last) = page.last() else {
            return Ok(lineage);
        };
        after = Some((last.0.clone(), last.1.clone(), last.2));
        let full = page.len() >= EDGE_PAGE;
        for (source, destination, _, properties) in &page {
            let relationship = medulla_types::msgpack::decode_property_object(properties)
                .ok()
                .and_then(|map| {
                    map.get("relationship")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                });
            match relationship.as_deref() {
                Some(name) if is_lineage_to(name) => {
                    lineage
                        .entry(source.clone())
                        .or_default()
                        .insert(destination.clone());
                }
                Some(name) if is_derived_from(name) => {
                    lineage
                        .entry(destination.clone())
                        .or_default()
                        .insert(source.clone());
                }
                _ => {}
            }
        }
        if !full {
            return Ok(lineage);
        }
    }
}

/// Propose the promoted claim's markings to every asset downstream of it
/// (`marking_propagation::propagate`). An asset that already holds a claim for the same
/// markings keeps it, so a repeated promotion adds nothing.
async fn propagate_downstream(
    state: &Arc<RwLock<ServerState>>,
    target: &ClassificationTarget<'_>,
    promoted: &ClaimRecord,
) -> Result<Vec<ClaimRecord>, String> {
    let lineage = read_lineage(state, target).await?;
    let depth_max = propagation_depth_max(state).await;
    let evidence = vec![format!("propagated_from:{}", promoted.claim_id)];
    let proposed = propagate(
        &lineage,
        &promoted.claim.asset,
        &promoted.claim.categories,
        &evidence,
        depth_max,
        &BTreeSet::new(),
    );
    let mut created = Vec::new();
    for claim in proposed {
        let id = derive_claim_id(&claim.asset, &claim.categories);
        if stored_record(state, target, &id).await?.is_some() {
            continue;
        }
        // The propagated claim goes through the same `Propose` path as a caller's claim.
        created.push(propose(state, target, claim).await?.record);
    }
    Ok(created)
}
