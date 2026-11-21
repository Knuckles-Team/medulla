//! Served order proposal staging and promotion (MEDULLA-FINANCE-PRIMITIVES-R015.1.1
//! to R015.1.3).
//!
//! `FinanceOrderStage` and `FinanceOrderPromote` are gateway-routed
//! (`Route::FinanceRecords`). [`stage`] admits one sealed `OrderProposalSnapshot`
//! revision: payload hash and canonical form, evidence, the revision chain with its
//! expected head, and the pinned policy limits through
//! `finance_order_limit_check`. [`promote`] rechecks the staged digest, the gate
//! receipts against the manifest digest and the tenant `TradingPolicy`, then writes
//! the promoted pointer, the `derivedFrom` history edges and the sealed
//! `OrderAdmissionReceipt` as the one `BatchUpdate`. A refusal writes nothing.
//! Admission is host review only; no order is created.

use super::*;

#[cfg(feature = "finance")]
use super::finance_order_limit_check::{
    payload_sha256, reasons_of, verify_proposal, Refusal, TENANT_SCOPE_KEY,
};
#[cfg(feature = "finance")]
use medulla_finance_market::market::digest;
#[cfg(feature = "finance")]
use medulla_finance_types::finance_order::{
    EvidenceRef, OrderAdmissionReceipt, OrderProposalSnapshot, OrderPromoteRequest,
    OrderPromoteResult, OrderStageResult, ORDER_LIMIT_BREACH, ORDER_PROMOTION_VETOED,
    PROMOTE_EVIDENCE_MISMATCH, PROMOTE_GATE_FAILED, PROMOTE_GATE_UNAVAILABLE, PROMOTE_HEAD_STALE,
    PROMOTE_RECEIPT_INCOMPLETE, PROMOTE_WRITE_FAILED, PROPOSAL_EVIDENCE_MISSING,
    PROPOSAL_HEAD_STALE, PROPOSAL_REVISION_CONFLICT,
};

#[cfg(feature = "finance")]
const SNAPSHOT_DOMAIN: &str = "finance.order_proposal_snapshot.v1";
#[cfg(feature = "finance")]
const RECEIPT_DOMAIN: &str = "finance.order_admission_receipt.v1";
#[cfg(feature = "finance")]
const MANIFEST_DOMAIN: &str = "finance.gate_manifest.v1";
#[cfg(feature = "finance")]
const DERIVED_FROM: &str = "derivedFrom";

#[cfg(feature = "finance")]
fn snapshot_node_id(proposal_id: &str, revision: u64) -> String {
    format!("finance:OrderProposalSnapshot:{proposal_id}:{revision}")
}
#[cfg(feature = "finance")]
fn head_node_id(proposal_id: &str) -> String {
    format!("finance:OrderProposalHead:{proposal_id}")
}
#[cfg(feature = "finance")]
fn pointer_node_id(proposal_id: &str) -> String {
    format!("finance:OrderPromotion:{proposal_id}")
}
#[cfg(feature = "finance")]
fn receipt_node_id(proposal_id: &str, revision: u64) -> String {
    format!("finance:OrderAdmissionReceipt:{proposal_id}:{revision}")
}

/// The digest of one snapshot: the seal of the sealed class.
#[cfg(feature = "finance")]
pub(super) fn snapshot_digest(snapshot: &OrderProposalSnapshot) -> String {
    digest::of_json(SNAPSHOT_DOMAIN, snapshot)
}

#[cfg(feature = "finance")]
type Props = serde_json::Map<String, serde_json::Value>;

#[cfg(feature = "finance")]
fn stored(core: &GraphCore, node_id: &str) -> Option<Props> {
    core.get_node_properties(node_id)
        .and_then(|blob| medulla_types::msgpack::decode_property_object(&blob).ok())
}

/// A stored node of another tenant is never read or overwritten.
#[cfg(feature = "finance")]
fn owned(core: &GraphCore, node_id: &str, scope: &str) -> Result<Option<Props>, Refusal> {
    match stored(core, node_id) {
        None => Ok(None),
        Some(props) if props.get(TENANT_SCOPE_KEY).and_then(|v| v.as_str()) == Some(scope) => {
            Ok(Some(props))
        }
        Some(_) => Err((
            "ACCESS_DENIED",
            format!("{node_id} belongs to another tenant"),
        )),
    }
}

#[cfg(feature = "finance")]
fn encode(operations: &[serde_json::Value]) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(operations)
        .map_err(|error| format!("INVALID_ARGUMENT: order proposal encoding failed: {error}"))
}

#[cfg(feature = "finance")]
fn refused((code, detail): Refusal) -> String {
    format!("{code}: {detail}")
}

/// What an admitted stage does: write, or answer an equal retry.
#[cfg(feature = "finance")]
enum Staged {
    Write(Method, OrderStageResult),
    Retry(OrderStageResult),
}

/// Every evidence reference must resolve to a record of the writer's tenant.
#[cfg(feature = "finance")]
fn check_evidence(core: &GraphCore, scope: &str, refs: &[EvidenceRef]) -> Result<(), Refusal> {
    if refs.is_empty() {
        return Err((
            PROPOSAL_EVIDENCE_MISSING,
            "a proposal cites at least one evidence record".to_owned(),
        ));
    }
    for evidence in refs {
        if owned(core, &evidence.record_id, scope)?.is_none() {
            return Err((
                PROPOSAL_EVIDENCE_MISSING,
                format!("evidence {} does not resolve", evidence.record_id),
            ));
        }
    }
    Ok(())
}

/// The revision chain: an equal retry answers, other bytes at a stored revision
/// conflict, and a new revision names the current head (or none for revision 1).
#[cfg(feature = "finance")]
fn check_chain(
    core: &GraphCore,
    scope: &str,
    snapshot: &OrderProposalSnapshot,
    expected_head_revision: Option<u64>,
    expected_head_digest: Option<&str>,
    digest_now: &str,
) -> Result<Option<()>, Refusal> {
    if snapshot.revision == 0 {
        return Err(("INVALID_ARGUMENT", "revision starts at 1".to_owned()));
    }
    let existing = owned(
        core,
        &snapshot_node_id(&snapshot.proposal_id, snapshot.revision),
        scope,
    )?;
    if let Some(props) = existing {
        return if props.get("snapshotDigest").and_then(|v| v.as_str()) == Some(digest_now) {
            Ok(Some(()))
        } else {
            Err((
                PROPOSAL_REVISION_CONFLICT,
                format!(
                    "revision {} of {} holds other bytes",
                    snapshot.revision, snapshot.proposal_id
                ),
            ))
        };
    }
    let head = owned(core, &head_node_id(&snapshot.proposal_id), scope)?;
    let head_revision = head
        .as_ref()
        .and_then(|props| props.get("headRevision"))
        .and_then(|v| v.as_u64());
    let head_digest = head
        .as_ref()
        .and_then(|props| props.get("headDigest"))
        .and_then(|v| v.as_str());
    let stale = |detail: String| Err((PROPOSAL_HEAD_STALE, detail));
    if snapshot.revision == 1 {
        if head_revision.is_some() || snapshot.predecessor_digest.is_some() {
            return stale("revision 1 has no predecessor and no head".to_owned());
        }
        if expected_head_revision.is_some() || expected_head_digest.is_some() {
            return stale("revision 1 expects no head".to_owned());
        }
        return Ok(None);
    }
    let previous = snapshot.revision - 1;
    if head_revision != Some(previous)
        || head_digest != snapshot.predecessor_digest.as_deref()
        || expected_head_revision != Some(previous)
        || expected_head_digest != head_digest
    {
        return stale(format!(
            "the head of {} is {head_revision:?}, revision {} names {previous}",
            snapshot.proposal_id, snapshot.revision
        ));
    }
    Ok(None)
}

#[cfg(feature = "finance")]
fn admit_stage(core: &GraphCore, scope: &str, method: &Method) -> Result<Staged, String> {
    let Method::FinanceOrderStage {
        snapshot,
        expected_head_revision,
        expected_head_digest,
    } = method
    else {
        unreachable!("admit_stage is only called for FinanceOrderStage");
    };
    let digest_now = snapshot_digest(snapshot);
    // Tenant, payload hash and canonical form, the pinned policy and its limits.
    let (_, _, breaches) = verify_proposal(core, scope, snapshot).map_err(refused)?;
    check_evidence(core, scope, &snapshot.evidence_refs).map_err(refused)?;
    let result = OrderStageResult {
        proposal_id: snapshot.proposal_id.clone(),
        revision: snapshot.revision,
        predecessor_digest: snapshot.predecessor_digest.clone(),
        snapshot_digest: digest_now.clone(),
        state: "staged".to_owned(),
    };
    let retry = check_chain(
        core,
        scope,
        snapshot,
        *expected_head_revision,
        expected_head_digest.as_deref(),
        &digest_now,
    )
    .map_err(refused)?;
    if retry.is_some() {
        return Ok(Staged::Retry(result));
    }
    if !breaches.is_empty() {
        let detail = reasons_of(&breaches)
            .iter()
            .map(|r| format!("{} observed {} over {}", r.rule_id, r.observed, r.bound))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("{ORDER_LIMIT_BREACH}: {detail}"));
    }
    let mut node = serde_json::json!({
        "type": "OrderProposalSnapshot",
        "finance_kind": "OrderProposalSnapshot",
        "tenant": snapshot.tenant_id,
        TENANT_SCOPE_KEY: scope,
        "proposalId": snapshot.proposal_id,
        "revision": snapshot.revision,
        "snapshotDigest": digest_now,
        "decisionId": snapshot.decision_id,
        "decisionDigest": snapshot.decision_digest,
        "policyId": snapshot.policy_id,
        "policyRevision": snapshot.policy_revision,
        "policyDigest": snapshot.policy_digest,
        "payloadSha256": snapshot.payload_sha256,
        "snapshot": snapshot,
    });
    if let Some(predecessor) = &snapshot.predecessor_digest {
        node["predecessorDigest"] = serde_json::json!(predecessor);
    }
    let operations = vec![
        serde_json::json!({
            "op": "upsert_node",
            "id": snapshot_node_id(&snapshot.proposal_id, snapshot.revision),
            "properties": node,
        }),
        serde_json::json!({
            "op": "upsert_node",
            "id": head_node_id(&snapshot.proposal_id),
            "properties": {
                "type": "OrderProposalHead",
                "finance_kind": "OrderProposalHead",
                "tenant": snapshot.tenant_id,
                TENANT_SCOPE_KEY: scope,
                "proposalId": snapshot.proposal_id,
                "headRevision": snapshot.revision,
                "headDigest": result.snapshot_digest,
            },
        }),
    ];
    Ok(Staged::Write(
        Method::BatchUpdate {
            operations_msgpack: encode(&operations)?,
        },
        result,
    ))
}

/// The staged revision a promotion names, re-read from the store: its stored
/// snapshot must re-seal to the stored digest and the request's digest, and its
/// payload must hash to the bound hash.
#[cfg(feature = "finance")]
fn staged_candidate(
    core: &GraphCore,
    scope: &str,
    request: &OrderPromoteRequest,
) -> Result<(OrderProposalSnapshot, String), Refusal> {
    let mismatch = |detail: String| (PROMOTE_EVIDENCE_MISMATCH, detail);
    let node_id = snapshot_node_id(&request.proposal_id, request.candidate_revision);
    let props = owned(core, &node_id, scope)?
        .ok_or_else(|| mismatch(format!("{node_id} is not staged")))?;
    let snapshot: OrderProposalSnapshot = props
        .get("snapshot")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| mismatch(format!("{node_id} has an unreadable snapshot")))?;
    let digest_now = snapshot_digest(&snapshot);
    if props.get("snapshotDigest").and_then(|v| v.as_str()) != Some(digest_now.as_str())
        || request.snapshot_digest != digest_now
    {
        return Err(mismatch(
            "the staged digest is not the requested snapshot digest".to_owned(),
        ));
    }
    if payload_sha256(&snapshot.canonical_payload_bytes) != snapshot.payload_sha256 {
        return Err(mismatch(
            "the staged payload does not hash to its bound SHA-256".to_owned(),
        ));
    }
    Ok((snapshot, digest_now))
}

/// The promoted head and the staged head, checked against the request.
#[cfg(feature = "finance")]
fn check_heads(
    core: &GraphCore,
    scope: &str,
    request: &OrderPromoteRequest,
) -> Result<Option<u64>, Refusal> {
    let promoted = owned(core, &pointer_node_id(&request.proposal_id), scope)?
        .and_then(|props| props.get("promotedRevision").and_then(|v| v.as_u64()));
    if promoted != request.expected_promoted_revision {
        return Err((
            PROMOTE_HEAD_STALE,
            format!(
                "the promoted head is {promoted:?}, the request expected {:?}",
                request.expected_promoted_revision
            ),
        ));
    }
    let staged = owned(core, &head_node_id(&request.proposal_id), scope)?
        .and_then(|props| props.get("headRevision").and_then(|v| v.as_u64()));
    if staged != Some(request.candidate_revision) || promoted >= Some(request.candidate_revision) {
        return Err((
            PROMOTE_HEAD_STALE,
            format!(
                "revision {} is not the newest staged revision ({staged:?})",
                request.candidate_revision
            ),
        ));
    }
    Ok(promoted)
}

/// Every manifest gate needs a complete, passing receipt bound to the candidate.
#[cfg(feature = "finance")]
fn check_gates(request: &OrderPromoteRequest, candidate_digest: &str) -> Result<Vec<String>, Refusal> {
    if request.manifest_gate_ids.is_empty()
        || request.required_gate_manifest_digest
            != digest::of_json(MANIFEST_DOMAIN, &request.manifest_gate_ids)
    {
        return Err((
            PROMOTE_GATE_UNAVAILABLE,
            "no gate manifest matches the required manifest digest".to_owned(),
        ));
    }
    let mut digests = Vec::new();
    let mut failed = None;
    for gate_id in &request.manifest_gate_ids {
        let Some(receipt) = request.gate_receipts.iter().find(|r| &r.gate_id == gate_id) else {
            return Err((
                PROMOTE_GATE_UNAVAILABLE,
                format!("gate {gate_id} has no receipt"),
            ));
        };
        if receipt.gate_revision == 0
            || receipt.evidence_digest.is_empty()
            || receipt.candidate_digest != candidate_digest
        {
            return Err((
                PROMOTE_RECEIPT_INCOMPLETE,
                format!("the receipt of gate {gate_id} is incomplete or bound to another candidate"),
            ));
        }
        if !receipt.pass_fail && failed.is_none() {
            failed = Some(gate_id.clone());
        }
        digests.push(digest::of_json("finance.gate_receipt.v1", receipt));
    }
    match failed {
        Some(gate_id) => Err((PROMOTE_GATE_FAILED, format!("gate {gate_id} did not pass"))),
        None => Ok(digests),
    }
}

/// The per-instrument leverage and designated-approver policy of R010.1, never
/// re-implemented here.
#[cfg(feature = "finance")]
fn check_approver(
    pinned: &super::finance_order_limit_check::PinnedPolicy,
    instrument_id: &str,
    approver: &str,
) -> Result<(), Refusal> {
    use medulla_finance_quant::leverage::{live_order_eligibility, OrderMode};
    let designated = pinned
        .policy
        .designated_approvers
        .iter()
        .any(|designated| designated == approver);
    let policy = pinned
        .leverage_limits
        .iter()
        .find(|policy| policy.instrument_id == instrument_id);
    live_order_eligibility(OrderMode::Live, instrument_id, approver, false, false, policy)
        .map_err(|refusal| (ORDER_PROMOTION_VETOED, format!("{refusal:?}")))?;
    if designated {
        Ok(())
    } else {
        Err((
            ORDER_PROMOTION_VETOED,
            format!("{approver} is not a designated approver of the tenant policy"),
        ))
    }
}

#[cfg(feature = "finance")]
fn admit_promote(
    core: &GraphCore,
    scope: &str,
    method: &Method,
) -> Result<(Method, OrderPromoteResult), String> {
    let Method::FinanceOrderPromote { request } = method else {
        unreachable!("admit_promote is only called for FinanceOrderPromote");
    };
    if medulla_server_api::carrier::tenant_scope_of(&request.tenant_id) != scope {
        return Err(refused((
            "ACCESS_DENIED",
            "a proposal for another tenant cannot be promoted".to_owned(),
        )));
    }
    if let Some(reason) = &request.veto_reason {
        return Err(refused((ORDER_PROMOTION_VETOED, reason.clone())));
    }
    let (snapshot, candidate_digest) = staged_candidate(core, scope, request).map_err(refused)?;
    let promoted = check_heads(core, scope, request).map_err(refused)?;
    let gate_digests = check_gates(request, &candidate_digest).map_err(refused)?;
    let (order, pinned, breaches) = verify_proposal(core, scope, &snapshot).map_err(refused)?;
    if !breaches.is_empty() {
        return Err(refused((
            ORDER_LIMIT_BREACH,
            format!("{} limit(s) breached at promotion", breaches.len()),
        )));
    }
    check_approver(&pinned, &order.instrument_id, &request.approver).map_err(refused)?;
    let receipt = OrderAdmissionReceipt {
        proposal_id: snapshot.proposal_id.clone(),
        revision: snapshot.revision,
        snapshot_digest: candidate_digest.clone(),
        decision_digest: snapshot.decision_digest.clone(),
        policy_id: snapshot.policy_id.clone(),
        policy_revision: snapshot.policy_revision,
        policy_digest: snapshot.policy_digest.clone(),
        gate_manifest_digest: request.required_gate_manifest_digest.clone(),
        gate_receipt_digests: gate_digests,
        transaction_revision: core.ledger_len() as u64 + 1,
    };
    let receipt_digest = digest::of_json(RECEIPT_DOMAIN, &receipt);
    let receipt_id = receipt_node_id(&receipt.proposal_id, receipt.revision);
    let snapshot_id = snapshot_node_id(&receipt.proposal_id, receipt.revision);
    let mut operations = vec![
        serde_json::json!({
            "op": "upsert_node",
            "id": receipt_id,
            "properties": {
                "type": "OrderAdmissionReceipt",
                "finance_kind": "OrderAdmissionReceipt",
                "tenant": snapshot.tenant_id,
                TENANT_SCOPE_KEY: scope,
                "proposalId": receipt.proposal_id,
                "revision": receipt.revision,
                "receiptDigest": receipt_digest,
                "receipt": receipt,
            },
        }),
        serde_json::json!({
            "op": "upsert_node",
            "id": pointer_node_id(&receipt.proposal_id),
            "properties": {
                "type": "OrderPromotionPointer",
                "finance_kind": "OrderPromotionPointer",
                "tenant": snapshot.tenant_id,
                TENANT_SCOPE_KEY: scope,
                "proposalId": receipt.proposal_id,
                "promotedRevision": receipt.revision,
                "promotedDigest": candidate_digest,
            },
        }),
        serde_json::json!({
            "op": "upsert_edge",
            "source": receipt_id,
            "target": snapshot_id,
            "properties": {"relationship": DERIVED_FROM},
        }),
    ];
    if let Some(previous) = promoted {
        operations.push(serde_json::json!({
            "op": "upsert_edge",
            "source": snapshot_id,
            "target": snapshot_node_id(&receipt.proposal_id, previous),
            "properties": {"relationship": DERIVED_FROM},
        }));
    }
    let result = OrderPromoteResult {
        receipt_id,
        receipt_digest,
        receipt,
    };
    Ok((
        Method::BatchUpdate {
            operations_msgpack: encode(&operations)?,
        },
        result,
    ))
}

#[cfg(feature = "finance")]
async fn commit_write(
    ctx: &MutationCtx<'_>,
    write: Method,
    payload: Result<ResultPayload, String>,
    write_failed_code: Option<&'static str>,
) -> Response {
    let payload = match payload {
        Ok(payload) => payload,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let Method::BatchUpdate { operations_msgpack } = &write else {
        unreachable!("admission builds a BatchUpdate");
    };
    let operations = operations_msgpack.clone();
    let plan = MutationPlan::for_method(&write);
    medulla_server_ops::api::ingest::graph::commit_gateway(ctx, &plan, &write, move |core| {
        medulla_graph_algos::algorithms::batch_update(core, &operations)
            .map(|_| payload)
            .map_err(|error| match write_failed_code {
                Some(code) => format!("{code}: {error}"),
                None => error.to_string(),
            })
    })
    .await
}

/// `FinanceOrderProposal.Stage`.
#[cfg(feature = "finance")]
async fn stage(ctx: &MutationCtx<'_>, method: &Method) -> Response {
    use medulla_protocol_types::result_contract::graph::FinanceOrderStage;
    match admit_stage(ctx.core, ctx.tenant_scope, method) {
        Err(error) => Response::err(ctx.req_id, error),
        Ok(Staged::Retry(result)) => {
            Response::ok(ctx.req_id, ResultPayload::of::<FinanceOrderStage>(result))
        }
        Ok(Staged::Write(write, result)) => {
            commit_write(
                ctx,
                write,
                ResultPayload::of::<FinanceOrderStage>(result),
                None,
            )
            .await
        }
    }
}

/// `FinanceOrderProposal.Promote` and its admission receipt.
#[cfg(feature = "finance")]
async fn promote(ctx: &MutationCtx<'_>, method: &Method) -> Response {
    use medulla_protocol_types::result_contract::graph::FinanceOrderPromote;
    match admit_promote(ctx.core, ctx.tenant_scope, method) {
        Err(error) => Response::err(ctx.req_id, error),
        Ok((write, result)) => {
            commit_write(
                ctx,
                write,
                ResultPayload::of::<FinanceOrderPromote>(result),
                Some(PROMOTE_WRITE_FAILED),
            )
            .await
        }
    }
}

/// The `Route::FinanceRecords` entry for the order proposal methods.
pub(super) async fn record(
    ctx: &MutationCtx<'_>,
    tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    let _ = tenant_id;
    if !matches!(
        method,
        Method::FinanceOrderStage { .. } | Method::FinanceOrderPromote { .. }
    ) {
        return None;
    }
    #[cfg(feature = "finance")]
    {
        Some(match method {
            Method::FinanceOrderStage { .. } => stage(ctx, method).await,
            _ => promote(ctx, method).await,
        })
    }
    #[cfg(not(feature = "finance"))]
    {
        Some(Response::err(
            ctx.req_id,
            "METHOD_NOT_YET_SERVED: the order proposal methods need the finance feature",
        ))
    }
}
