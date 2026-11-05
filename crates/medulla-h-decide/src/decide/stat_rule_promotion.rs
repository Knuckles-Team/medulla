//! The `DecisionLog` `PromoteRule` attach point (MEDULLA-DECISION-ENGINE-R129.1).
//!
//! A candidate guardrail rule reaches the invariant gate over the caller
//! tenant's published `RuleInvariant` components, read in invariant-IRI
//! order. Another tenant's invariants never enter the gate.

use medulla_agent_types::agent_guardrail::{
    check_rule_against_invariants, GuardrailRule, RulePromotionVerdict,
};
use medulla_types::contract::BoundedVec;

use super::stat_executor::ExecutionContext;

/// Run `rule` through the invariant gate of the caller's tenant.
pub(super) fn promote(
    ctx: &ExecutionContext,
    rule: &GuardrailRule,
    _resolution_ids: &BoundedVec<String, 64>,
) -> Result<RulePromotionVerdict, String> {
    let invariants = ctx.store.published_rule_invariants(ctx.tenant_id)?;
    Ok(check_rule_against_invariants(rule, &invariants))
}
