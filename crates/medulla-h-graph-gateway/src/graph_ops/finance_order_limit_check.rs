//! Served order limit verification (MEDULLA-FINANCE-PRIMITIVES-R010.2).
//!
//! [`check_order`] builds a `GuardedAction` from the order's instrument, signed
//! quantity and notional and runs `check_limits`, the one function the decision
//! limit stage also runs, against the risk limits of the pinned `TradingPolicy`.
//! The same verification is the read `Method::FinanceOrderLimitCheck` and the first
//! step of `Method::FinanceOrderStage`. Observations: `PositionSize` in scope
//! `<instrument_id>` carries the absolute signed quantity and `PositionSize` in scope
//! `<instrument_id>:notional` the absolute notional. Nothing is written.

use super::finance_accounting::literal;
use super::ops_context::GraphOpsContext;
use super::*;

use medulla_decision_outcome_types::decision::limit_guard::{check_limits, LimitBreach};
use medulla_finance_market::market::digest;
use medulla_finance_quant::leverage::LeveragePolicy;
use medulla_finance_types::finance_order::{
    FinanceOrder, LimitBreachReason, OrderLimitCheckResult, OrderProposalSnapshot, TradingPolicy,
    ORDER_PAYLOAD_SCHEMA_ID, ORDER_PAYLOAD_SCHEMA_REVISION, PROPOSAL_HASH_MISMATCH,
    TRADING_POLICY_MISSING,
};
use sha2::{Digest, Sha256};
use {
    medulla_decision_statistical_types::decision::limit_types::GuardedAction,
    medulla_decision_statistical_types::decision::limit_types::LimitMetric,
    medulla_decision_statistical_types::decision::limit_types::LimitObservation,
    medulla_decision_statistical_types::decision::limit_types::LimitRule,
};

pub const TENANT_SCOPE_KEY: &str = "finance_tenant_scope";

/// A finance refusal: its closed wire code and the detail.
pub type Refusal = (&'static str, String);

/// The domain of a trading policy digest.
const POLICY_DIGEST_DOMAIN: &str = "finance.trading_policy.v1";

/// `sha256:<hex>` over `bytes` as stored.
pub fn payload_sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// The pinned policy with its parsed limits.
pub struct PinnedPolicy {
    pub policy: TradingPolicy,
    pub digest: String,
    pub risk_limits: Vec<LimitRule>,
    pub leverage_limits: Vec<LeveragePolicy>,
}

/// The order a proposal carries. The payload hash must match its bytes, the schema
/// must be the one medulla validates, and the bytes must be that schema's canonical
/// form (`serde_json::to_vec` of [`FinanceOrder`]).
pub fn order_of(snapshot: &OrderProposalSnapshot) -> Result<FinanceOrder, Refusal> {
    let bytes = &snapshot.canonical_payload_bytes;
    if payload_sha256(bytes) != snapshot.payload_sha256 {
        return Err((
            PROPOSAL_HASH_MISMATCH,
            "payload_sha256 is not the SHA-256 of the payload bytes".to_owned(),
        ));
    }
    if snapshot.payload_schema_id != ORDER_PAYLOAD_SCHEMA_ID
        || snapshot.payload_schema_revision != ORDER_PAYLOAD_SCHEMA_REVISION
    {
        return Err((
            "INVALID_ARGUMENT",
            format!(
                "payload schema {}@{} is not {ORDER_PAYLOAD_SCHEMA_ID}@{ORDER_PAYLOAD_SCHEMA_REVISION}",
                snapshot.payload_schema_id, snapshot.payload_schema_revision
            ),
        ));
    }
    let order: FinanceOrder = serde_json::from_slice(bytes).map_err(|error| {
        (
            PROPOSAL_HASH_MISMATCH,
            format!("the payload is not a {ORDER_PAYLOAD_SCHEMA_ID} order: {error}"),
        )
    })?;
    if serde_json::to_vec(&order).ok().as_deref() != Some(bytes.as_slice()) {
        return Err((
            PROPOSAL_HASH_MISMATCH,
            "the payload bytes are not the canonical form of the declared schema".to_owned(),
        ));
    }
    Ok(order)
}

/// The tenant's `TradingPolicy` record `policy_id`, or `TRADING_POLICY_MISSING`.
pub fn read_policy(
    core: &GraphCore,
    scope: &str,
    policy_id: &str,
) -> Result<PinnedPolicy, Refusal> {
    let missing = |detail: String| (TRADING_POLICY_MISSING, detail);
    let node_id = format!("finance:TradingPolicy:{policy_id}");
    let props = core
        .get_node_properties(&node_id)
        .and_then(|blob| medulla_types::msgpack::decode_property_object(&blob).ok())
        .filter(|props| props.get(TENANT_SCOPE_KEY).and_then(|v| v.as_str()) == Some(scope))
        .ok_or_else(|| missing(format!("no trading policy {policy_id} for the caller")))?;
    let fields = props
        .get("fields")
        .and_then(|fields| fields.as_object())
        .ok_or_else(|| missing(format!("trading policy {policy_id} has no fields")))?;
    let text = |name: &str| {
        fields
            .get(name)
            .and_then(|term| term.as_str())
            .and_then(literal)
            .ok_or_else(|| missing(format!("trading policy {policy_id} has no {name}")))
    };
    let json = |name: &str| -> Result<serde_json::Value, Refusal> {
        serde_json::from_str(&text(name)?).map_err(|_| {
            missing(format!(
                "trading policy {policy_id} has an unreadable {name}"
            ))
        })
    };
    let revision = text("tradingPolicyRevision")?.parse::<u64>().map_err(|_| {
        missing(format!(
            "trading policy {policy_id} has a non-numeric revision"
        ))
    })?;
    let leverage_limits = json("tradingPolicyLeverageLimits")?;
    let risk_limits = json("tradingPolicyRiskLimits")?;
    let designated_approvers: Vec<String> = serde_json::from_value(json("tradingPolicyApprovers")?)
        .map_err(|_| {
            missing(format!(
                "trading policy {policy_id} has unreadable approvers"
            ))
        })?;
    let parsed_risk: Vec<LimitRule> =
        serde_json::from_value(risk_limits.clone()).map_err(|_| {
            missing(format!(
                "trading policy {policy_id} has unreadable risk limits"
            ))
        })?;
    let parsed_leverage: Vec<LeveragePolicy> = serde_json::from_value(leverage_limits.clone())
        .map_err(|_| {
            missing(format!(
                "trading policy {policy_id} has unreadable leverage limits"
            ))
        })?;
    let tenant_id = props
        .get("tenant")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();
    let policy = TradingPolicy {
        tenant_id,
        revision,
        leverage_limits,
        risk_limits,
        designated_approvers,
    };
    Ok(PinnedPolicy {
        digest: digest::of_json(POLICY_DIGEST_DOMAIN, &policy),
        policy,
        risk_limits: parsed_risk,
        leverage_limits: parsed_leverage,
    })
}

/// The proposed action as the limit guard sees it.
fn guarded_action(order: &FinanceOrder, proposal_id: &str) -> GuardedAction {
    GuardedAction {
        signal_iri: format!("finance:order:{proposal_id}"),
        observations: vec![
            LimitObservation {
                metric: LimitMetric::PositionSize,
                scope: order.instrument_id.clone(),
                value: order.signed_quantity.saturating_abs(),
            },
            LimitObservation {
                metric: LimitMetric::PositionSize,
                scope: format!("{}:notional", order.instrument_id),
                value: order.notional.saturating_abs(),
            },
        ],
        requested_bounds: Vec::new(),
    }
}

/// Every breach of the pinned risk limits by `order`, in rule order.
pub fn check_order(
    order: &FinanceOrder,
    proposal_id: &str,
    rules: &[LimitRule],
) -> Vec<LimitBreach> {
    check_limits(&guarded_action(order, proposal_id), rules)
}

/// The pinned policy of a proposal: its record must exist at the proposal's pinned
/// revision and digest.
pub fn pinned_policy_of(
    core: &GraphCore,
    scope: &str,
    snapshot: &OrderProposalSnapshot,
) -> Result<PinnedPolicy, Refusal> {
    let pinned = read_policy(core, scope, &snapshot.policy_id)?;
    if pinned.policy.revision != snapshot.policy_revision || pinned.digest != snapshot.policy_digest
    {
        return Err((
            TRADING_POLICY_MISSING,
            format!(
                "trading policy {} is not at the pinned revision and digest",
                snapshot.policy_id
            ),
        ));
    }
    Ok(pinned)
}

/// The verification both the read and the stage run: the order, the pinned policy
/// and the breaches.
pub fn verify_proposal(
    core: &GraphCore,
    scope: &str,
    snapshot: &OrderProposalSnapshot,
) -> Result<(FinanceOrder, PinnedPolicy, Vec<LimitBreach>), Refusal> {
    if medulla_server_api::carrier::tenant_scope_of(&snapshot.tenant_id) != scope {
        return Err((
            "ACCESS_DENIED",
            "a proposal for another tenant cannot be checked".to_owned(),
        ));
    }
    let order = order_of(snapshot)?;
    let pinned = pinned_policy_of(core, scope, snapshot)?;
    let breaches = check_order(&order, &snapshot.proposal_id, &pinned.risk_limits);
    Ok((order, pinned, breaches))
}

/// The `Abstained` reasons of a breach list, one per breach in rule order.
pub fn reasons_of(breaches: &[LimitBreach]) -> Vec<LimitBreachReason> {
    breaches
        .iter()
        .map(|breach| LimitBreachReason {
            rule_id: breach.rule_id.clone(),
            observed: breach.observed,
            bound: breach.bound,
            signal_iri: breach.signal_iri.clone(),
        })
        .collect()
}

fn handle_limit_check(ctx: GraphOpsContext<'_>, proposal: &OrderProposalSnapshot) -> Response {
    let Some(scope) = ctx
        .read_authority
        .carrier()
        .map(|carrier| carrier.tenant_scope().to_owned())
    else {
        return Response::err(ctx.req_id, "ACCESS_DENIED: no verified tenant");
    };
    match verify_proposal(ctx.core, &scope, proposal) {
        Ok((_, _, breaches)) => {
            let result = OrderLimitCheckResult {
                outcome: if breaches.is_empty() {
                    "Passed"
                } else {
                    "Abstained"
                }
                .to_owned(),
                reasons: reasons_of(&breaches),
            };
            Response::ok(
                ctx.req_id,
                ResultPayload::of_ref::<
                    medulla_protocol_types::result_contract::graph::FinanceOrderLimitCheck,
                >(&result),
            )
        }
        Err((code, detail)) => Response::err(ctx.req_id, format!("{code}: {detail}")),
    }
}

/// Route `FinanceOrderLimitCheck` from the graph terminal read chain.
pub async fn handle_check(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    match &method {
        Method::FinanceOrderLimitCheck { proposal } => {
            ControlFlow::Break(handle_limit_check(ctx, proposal))
        }
        _ => ControlFlow::Continue(method),
    }
}
