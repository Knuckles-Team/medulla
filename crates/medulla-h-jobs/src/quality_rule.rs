//! The stored shape of a quality rule and the write of `QualityRulePut`
//! (MEDULLA-UNIFIED-DATA-PLANE-R762.3).
//!
//! A rule lives on one node private to the verified tenant, keyed by the tenant and the
//! rule id, so two tenants naming the same rule never share a node. The node carries the
//! canonical rule, its change-request state (`RuleState`: a put by a caller is approved
//! by that caller; a rule proposed from a profile enters as proposed,
//! MEDULLA-UNIFIED-DATA-PLANE-R762.7) and its consecutive failure counter, which the
//! `QualityEvaluate` job updates. A put compiles the rule first, so a rule that cannot be
//! evaluated is refused and writes nothing.

use medulla_catalog_types::quality::{
    InvalidQualityRule, QualityRule, QualityRuleReceipt, RuleState,
};
use medulla_types::contract::Digest256;
use serde_json::{json, Value};

/// The node type of a stored quality rule.
pub const QUALITY_RULE_LABEL: &str = "QualityRule";
/// The property holding the canonical JSON of the [`QualityRule`].
pub const RULE_BODY_PROPERTY: &str = "rule";
/// The property holding the rule's [`RuleState`].
pub const RULE_STATE_PROPERTY: &str = "state";
/// The property holding the rule's consecutive failure count.
pub const RULE_FAILURES_PROPERTY: &str = "consecutive_failures";

/// A node id private to one tenant: `<prefix>:` plus 32 hex characters of SHA-256 over
/// the tenant and the parts. Two tenants naming the same rule, score or column never
/// share a node.
pub fn tenant_node_id(prefix: &str, tenant: &str, parts: &[&str]) -> String {
    let mut key = String::from(tenant);
    for part in parts {
        key.push('\0');
        key.push_str(part);
    }
    let digest = Digest256::sha256(key.as_bytes()).to_hex();
    format!("{prefix}:{}", &digest[..32])
}

/// The node id of `rule_id` in `tenant`.
pub fn quality_rule_node_id(tenant: &str, rule_id: &str) -> String {
    tenant_node_id("quality_rule", tenant, &[rule_id])
}

/// One `QualityRulePut`: validate and compile `rule`, then build the one `upsert_node`
/// operation that stores it approved by `agent` in `tenant`, with its failure counter at
/// zero. Refuses `QUALITY_RULE_INVALID` (blank id, target or expression, or an expression
/// that does not compile) and `QUALITY_THRESHOLD_INVALID` (threshold outside 0 to 1).
pub fn put_quality_rule(
    rule: &QualityRule,
    tenant: &str,
    agent: &str,
) -> Result<(Vec<Value>, QualityRuleReceipt), InvalidQualityRule> {
    medulla_core::quality::compile_rule(rule)?;
    let body = serde_json::to_string(rule)
        .map_err(|_| InvalidQualityRule::RuleInvalid("the rule cannot be encoded".to_string()))?;
    let state = RuleState::initial()
        .approve(agent)
        .map_err(|_| InvalidQualityRule::RuleInvalid("rule state refused".to_string()))?;
    let node_id = quality_rule_node_id(tenant, &rule.id);
    let properties = json!({
        "type": QUALITY_RULE_LABEL,
        "rule_id": rule.id,
        "target": rule.target,
        "dimension": rule.dimension,
        "threshold": rule.threshold,
        "severity": rule.severity,
        RULE_BODY_PROPERTY: body,
        RULE_STATE_PROPERTY: state,
        RULE_FAILURES_PROPERTY: 0,
        "tenant_id": tenant,
        "_owner": agent,
    });
    let operations = vec![json!({"op": "upsert_node", "id": node_id, "properties": properties})];
    Ok((
        operations,
        QualityRuleReceipt {
            rule_id: rule.id.clone(),
            node_id,
            state,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_catalog_types::quality::{QualityDimension, QualitySeverity};

    fn rule(expression: &str, threshold: f64) -> QualityRule {
        QualityRule {
            id: "orders-email".into(),
            target: "orders".into(),
            dimension: QualityDimension::Completeness,
            expression: expression.into(),
            threshold,
            severity: QualitySeverity::Critical,
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R762.3
    #[test]
    fn a_put_stores_one_approved_rule_private_to_the_tenant() {
        let (operations, receipt) =
            put_quality_rule(&rule("email IS NOT NULL", 0.9), "acme", "steward").unwrap();
        assert_eq!(operations.len(), 1);
        let properties = &operations[0]["properties"];
        assert_eq!(properties["type"], QUALITY_RULE_LABEL);
        assert_eq!(properties["tenant_id"], "acme");
        assert_eq!(properties[RULE_FAILURES_PROPERTY], 0);
        let stored: QualityRule =
            serde_json::from_str(properties[RULE_BODY_PROPERTY].as_str().unwrap()).unwrap();
        assert_eq!(stored, rule("email IS NOT NULL", 0.9));
        assert_eq!(
            receipt.state,
            RuleState::Approved {
                approver: "steward".into()
            }
        );
        assert_eq!(
            receipt.node_id,
            quality_rule_node_id("acme", "orders-email")
        );
        assert_ne!(
            receipt.node_id,
            quality_rule_node_id("globex", "orders-email")
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R762.3
    #[test]
    fn tenant_node_ids_never_collide_across_tenants() {
        let a = tenant_node_id("quality_rule", "acme", &["r1"]);
        assert_ne!(a, tenant_node_id("quality_rule", "globex", &["r1"]));
        assert_eq!(a, quality_rule_node_id("acme", "r1"));
        assert!(a.starts_with("quality_rule:") && a.len() == "quality_rule:".len() + 32);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R762.3
    #[test]
    fn an_invalid_rule_writes_nothing() {
        let refused = put_quality_rule(&rule("email IS NOT NULL; DROP", 0.9), "acme", "s");
        assert_eq!(refused.unwrap_err().code(), "QUALITY_RULE_INVALID");
        let refused = put_quality_rule(&rule("email IS NOT NULL", 1.5), "acme", "s");
        assert_eq!(refused.unwrap_err().code(), "QUALITY_THRESHOLD_INVALID");
    }
}
