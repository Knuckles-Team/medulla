//! The approval gate question (MEDULLA-BUSINESS-OPS-R004): which approval gate applies to an
//! operation, answered from the `:ApprovalRule` nodes of the graph.
//!
//! The graph is the authority. A caller asks for an operation (and an amount, when the operation
//! moves money) and the engine answers `none`, `single` or `two_person`, with the gate class, the
//! approver roles and the time the approval stays open. The caller holds no table of its own. An
//! operation that no rule covers answers `unresolved`: it is never read as `none`, so a caller
//! that cannot name a rule applies its strictest default.
//!
//! Pure data and decision; the served handler reads the rules the caller may see and calls
//! [`resolve_approval_gate`].

use std::collections::BTreeSet;

use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::lifecycle::sha256_hex;

/// The graph `type` of an approval rule node.
pub const APPROVAL_RULE_TYPE: &str = "ApprovalRule";
/// The rule property naming the operation (the action kind) the rule governs.
pub const RULE_ACTION_KIND: &str = "approvalActionKind";
/// The rule property holding the largest amount the rule covers, in minor units.
pub const RULE_CEILING_MINOR: &str = "approvalCeilingMinor";
/// The rule property holding the currency code of the ceiling.
pub const RULE_CURRENCY: &str = "approvalCurrency";
/// The rule property naming the role whose holder may approve.
pub const RULE_REQUIRED_ROLE: &str = "approvalRequiredRole";
/// The rule property holding how many distinct approvers the rule requires.
pub const RULE_QUORUM: &str = "approvalQuorum";
/// The rule property naming the gate class of the rule.
pub const RULE_GATE_CLASS: &str = "approvalGateClass";
/// The rule property holding how long an approval of the rule stays open, in milliseconds.
pub const RULE_EXPIRY_MS: &str = "approvalExpiryMs";

/// The request is out of bounds.
pub const APPROVAL_GATE_INVALID: &str = "APPROVAL_GATE_INVALID";

/// The stable refusal codes of the approval gate question.
pub const REFUSALS: &[RefusalCode] = &[RefusalCode::new(APPROVAL_GATE_INVALID)];

/// Longest operation, role or currency, in bytes.
pub const MAX_GATE_TEXT_BYTES: usize = 256;
/// Most rules one answer reads. A graph that holds more rules for one tenant answers from the
/// first of them in id order, which the handler bounds before it calls the resolver.
pub const MAX_APPROVAL_RULES: usize = 100_000;

/// A typed refusal of the approval gate question.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalGateRefusal {
    /// A request field is out of bounds.
    #[error("APPROVAL_GATE_INVALID: {0}")]
    Invalid(String),
}

impl ApprovalGateRefusal {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        APPROVAL_GATE_INVALID
    }
}

/// The approval gate that applies to an operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ApprovalGate {
    /// A rule applies and requires no approval.
    #[serde(rename = "none")]
    NoGate,
    /// A rule applies and requires one approver.
    #[serde(rename = "single")]
    Single,
    /// A rule applies and requires two distinct approvers.
    #[serde(rename = "two_person")]
    TwoPerson,
    /// No rule covers the operation. This is not `none`: the caller applies its strictest gate.
    #[serde(rename = "unresolved")]
    Unresolved,
}

impl ApprovalGate {
    /// The gate a quorum of approvers names: 0 none, 1 single, 2 or more two-person.
    pub fn of_quorum(quorum: u32) -> Self {
        match quorum {
            0 => Self::NoGate,
            1 => Self::Single,
            _ => Self::TwoPerson,
        }
    }
}

/// The class of an approval, which decides who is told and how long it may wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum GateClass {
    /// A filing with an authority.
    LegalFiling,
    /// Money leaving an account.
    MoneyMovement,
    /// Content going public.
    Publishing,
    /// A change to a production system.
    ProductionQa,
}

impl GateClass {
    /// Parse the stored wire name; an unknown name is `None`.
    pub fn parse(name: &str) -> Option<Self> {
        serde_json::from_value(Value::String(name.to_owned())).ok()
    }
}

/// The question: which gate applies to `operation`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ApprovalGateRequest {
    /// The operation (the action kind of the rules) the plan is for.
    pub operation: String,
    /// The amount the plan moves, in minor units, when it moves money.
    #[serde(default)]
    pub amount_minor: Option<u64>,
    /// The currency code of the amount.
    #[serde(default)]
    pub currency: Option<String>,
}

/// The answer: the gate, the class, the approver roles and how long an approval stays open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ApprovalGateAnswer {
    /// The operation asked about.
    pub operation: String,
    /// The gate that applies; `unresolved` when no rule covers the operation.
    pub gate: ApprovalGate,
    /// The class of the gate, when the applying rule names one.
    pub gate_class: Option<GateClass>,
    /// The roles whose holders may approve, sorted.
    pub approver_roles: Vec<String>,
    /// How long an approval stays open, in milliseconds, when the applying rules bound it.
    pub expires_after_ms: Option<u64>,
    /// The ids of the rules the answer follows, sorted. Empty when `unresolved`.
    pub rule_ids: Vec<String>,
    /// The digest of the applying rules, so a caller can see the policy changed.
    pub policy_digest: String,
}

/// One approval rule read from the graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ApprovalRule {
    /// The node id of the rule.
    pub id: String,
    /// The operation the rule governs.
    pub action_kind: String,
    /// The largest amount the rule covers, in minor units; absent covers every amount.
    pub ceiling_minor: Option<u64>,
    /// The currency of the ceiling.
    pub currency: Option<String>,
    /// The role whose holder may approve.
    pub required_role: Option<String>,
    /// How many distinct approvers the rule requires; absent counts as one.
    pub quorum: Option<u32>,
    /// The class of the gate.
    pub gate_class: Option<GateClass>,
    /// How long an approval stays open, in milliseconds.
    pub expiry_ms: Option<u64>,
}

impl ApprovalRule {
    /// Read the rule a graph node holds. `None` when the node is not an approval rule, is stamped
    /// for another tenant (a rule with no tenant stamp is the graph's own), names no operation, or holds a value the rule model does not allow (an unknown
    /// gate class, a quorum that is not a number): a rule the engine cannot read is left out, so
    /// the operation answers `unresolved` rather than a looser gate.
    pub fn from_node(id: &str, node: &Value, tenant: &str) -> Option<Self> {
        let owned_elsewhere = node
            .get("tenant_id")
            .and_then(Value::as_str)
            .is_some_and(|owner| owner != tenant);
        if node.get("type").and_then(Value::as_str) != Some(APPROVAL_RULE_TYPE) || owned_elsewhere {
            return None;
        }
        let text = |key: &str| node.get(key).and_then(Value::as_str).map(str::to_owned);
        let number = |key: &str| -> Result<Option<u64>, ()> {
            match node.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(value) => value.as_u64().map(Some).ok_or(()),
            }
        };
        let action_kind = text(RULE_ACTION_KIND).filter(|kind| !kind.trim().is_empty())?;
        let gate_class = match text(RULE_GATE_CLASS) {
            None => None,
            Some(name) => Some(GateClass::parse(&name)?),
        };
        let quorum = match number(RULE_QUORUM).ok()? {
            None => None,
            Some(quorum) => Some(u32::try_from(quorum).ok()?),
        };
        let ceiling_minor = number(RULE_CEILING_MINOR).ok()?;
        let currency = text(RULE_CURRENCY);
        // The ceiling and the currency are owed only by a monetary rule; a monetary rule that
        // lacks either is not a rule the engine can read, exactly as the shape refuses it.
        if gate_class == Some(GateClass::MoneyMovement)
            && (ceiling_minor.is_none() || currency.as_deref().is_none_or(|c| c.trim().is_empty()))
        {
            return None;
        }
        Some(Self {
            id: id.to_owned(),
            action_kind,
            ceiling_minor,
            currency,
            required_role: text(RULE_REQUIRED_ROLE).filter(|role| !role.trim().is_empty()),
            quorum,
            gate_class,
            expiry_ms: number(RULE_EXPIRY_MS).ok()?,
        })
    }

    fn covers(&self, request: &ApprovalGateRequest) -> bool {
        if self.action_kind != request.operation {
            return false;
        }
        if let (Some(rule), Some(asked)) = (&self.currency, &request.currency) {
            if rule != asked {
                return false;
            }
        }
        self.ceiling_minor
            .is_none_or(|ceiling| request.amount_minor.unwrap_or(0) <= ceiling)
    }
}

impl ApprovalGateRequest {
    /// Check the bounds of the request.
    pub fn validate(&self) -> Result<(), ApprovalGateRefusal> {
        let bad = |field: &str| {
            Err(ApprovalGateRefusal::Invalid(format!("{field} is out of bounds")))
        };
        let text_ok = |value: &str| {
            !value.trim().is_empty()
                && value.len() <= MAX_GATE_TEXT_BYTES
                && !value.chars().any(char::is_control)
        };
        if !text_ok(&self.operation) {
            return bad("operation");
        }
        if self.currency.as_deref().is_some_and(|currency| !text_ok(currency)) {
            return bad("currency");
        }
        Ok(())
    }
}

/// `ApprovalGate` (MEDULLA-BUSINESS-OPS-R004): answer which gate applies, from `rules`.
///
/// A rule covers the request when it governs the operation, its currency does not differ and its
/// ceiling is not below the amount (a request with no amount counts as zero). The most specific
/// covering rules answer: those with the lowest ceiling, a rule with no ceiling being the least
/// specific. Among them the gate is the highest quorum, so two rules of one ceiling never loosen
/// each other. The class is that of the highest-quorum rule (the lowest rule id on a tie), the
/// roles are the union of their roles, and the expiry is the shortest. A rule with no quorum
/// counts as one approver; only an explicit quorum of zero is `none`.
pub fn resolve_approval_gate(
    request: &ApprovalGateRequest,
    rules: &[ApprovalRule],
) -> Result<ApprovalGateAnswer, ApprovalGateRefusal> {
    request.validate()?;
    let covering: Vec<&ApprovalRule> = rules.iter().filter(|rule| rule.covers(request)).collect();
    let tightest = covering
        .iter()
        .map(|rule| rule.ceiling_minor.unwrap_or(u64::MAX))
        .min();
    let mut selected: Vec<&ApprovalRule> = covering
        .into_iter()
        .filter(|rule| Some(rule.ceiling_minor.unwrap_or(u64::MAX)) == tightest)
        .collect();
    selected.sort_by(|left, right| left.id.cmp(&right.id));
    if selected.is_empty() {
        return Ok(ApprovalGateAnswer {
            operation: request.operation.clone(),
            gate: ApprovalGate::Unresolved,
            gate_class: None,
            approver_roles: Vec::new(),
            expires_after_ms: None,
            rule_ids: Vec::new(),
            policy_digest: policy_digest(&selected),
        });
    }
    let quorum = selected
        .iter()
        .map(|rule| rule.quorum.unwrap_or(1))
        .max()
        .unwrap_or(1);
    let gate_class = selected
        .iter()
        .filter(|rule| rule.quorum.unwrap_or(1) == quorum)
        .find_map(|rule| rule.gate_class);
    let roles: BTreeSet<String> = selected
        .iter()
        .filter_map(|rule| rule.required_role.clone())
        .collect();
    Ok(ApprovalGateAnswer {
        operation: request.operation.clone(),
        gate: ApprovalGate::of_quorum(quorum),
        gate_class,
        approver_roles: roles.into_iter().collect(),
        expires_after_ms: selected.iter().filter_map(|rule| rule.expiry_ms).min(),
        rule_ids: selected.iter().map(|rule| rule.id.clone()).collect(),
        policy_digest: policy_digest(&selected),
    })
}

/// The digest of the rules an answer follows: `sha256:` over their canonical JSON in id order.
fn policy_digest(selected: &[&ApprovalRule]) -> String {
    format!(
        "sha256:{}",
        sha256_hex(serde_json::to_string(selected).unwrap_or_default().as_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(id: &str, kind: &str) -> ApprovalRule {
        ApprovalRule {
            id: id.to_owned(),
            action_kind: kind.to_owned(),
            ceiling_minor: None,
            currency: None,
            required_role: Some("owner".to_owned()),
            quorum: Some(1),
            gate_class: Some(GateClass::Publishing),
            expiry_ms: Some(86_400_000),
        }
    }

    fn ask(operation: &str, amount: Option<u64>) -> ApprovalGateRequest {
        ApprovalGateRequest {
            operation: operation.to_owned(),
            amount_minor: amount,
            currency: None,
        }
    }

    // spec: MEDULLA-BUSINESS-OPS-R004
    #[test]
    fn a_quorum_of_zero_one_and_two_is_none_single_and_two_person() {
        for (quorum, expected) in [
            (0, ApprovalGate::NoGate),
            (1, ApprovalGate::Single),
            (2, ApprovalGate::TwoPerson),
            (3, ApprovalGate::TwoPerson),
        ] {
            let mut only = rule("r1", "deploy.promotions.apply");
            only.quorum = Some(quorum);
            let answer = resolve_approval_gate(&ask("deploy.promotions.apply", None), &[only])
                .expect("answer");
            assert_eq!(answer.gate, expected, "quorum {quorum}");
            assert_eq!(answer.rule_ids, vec!["r1".to_owned()]);
        }
        assert_eq!(
            serde_json::to_value(ApprovalGate::TwoPerson).expect("json"),
            json!("two_person")
        );
        assert_eq!(serde_json::to_value(ApprovalGate::NoGate).expect("json"), json!("none"));
    }

    // spec: MEDULLA-BUSINESS-OPS-R004
    #[test]
    fn an_operation_no_rule_covers_is_unresolved_and_never_none() {
        let rules = [rule("r1", "deploy.promotions.apply")];
        let answer = resolve_approval_gate(&ask("money.transfer", None), &rules).expect("answer");
        assert_eq!(answer.gate, ApprovalGate::Unresolved);
        assert!(answer.rule_ids.is_empty() && answer.approver_roles.is_empty());
        assert_eq!(answer.gate_class, None);
        let empty = resolve_approval_gate(&ask("money.transfer", None), &[]).expect("answer");
        assert_eq!(empty.gate, ApprovalGate::Unresolved);
    }

    // spec: MEDULLA-BUSINESS-OPS-R004
    #[test]
    fn the_lowest_covering_ceiling_answers_and_a_larger_amount_tightens_the_gate() {
        let mut small = rule("small", "money.transfer");
        small.ceiling_minor = Some(10_000);
        small.quorum = Some(1);
        small.gate_class = Some(GateClass::MoneyMovement);
        let mut large = rule("large", "money.transfer");
        large.ceiling_minor = Some(1_000_000);
        large.quorum = Some(2);
        large.gate_class = Some(GateClass::MoneyMovement);
        let rules = [large, small];
        let few = resolve_approval_gate(&ask("money.transfer", Some(500)), &rules).expect("answer");
        assert_eq!((few.gate, few.rule_ids), (ApprovalGate::Single, vec!["small".to_owned()]));
        let many =
            resolve_approval_gate(&ask("money.transfer", Some(50_000)), &rules).expect("answer");
        assert_eq!(many.gate, ApprovalGate::TwoPerson);
        assert_eq!(many.gate_class, Some(GateClass::MoneyMovement));
        // Above every ceiling no rule covers the amount.
        let beyond =
            resolve_approval_gate(&ask("money.transfer", Some(2_000_000)), &rules).expect("answer");
        assert_eq!(beyond.gate, ApprovalGate::Unresolved);
    }

    // spec: MEDULLA-BUSINESS-OPS-R004
    #[test]
    fn rules_of_one_ceiling_merge_roles_and_never_loosen_each_other() {
        let mut first = rule("a", "deploy.promotions.apply");
        first.quorum = Some(1);
        first.required_role = Some("release-manager".to_owned());
        first.expiry_ms = Some(3_600_000);
        let mut second = rule("b", "deploy.promotions.apply");
        second.quorum = Some(2);
        second.required_role = Some("owner".to_owned());
        second.gate_class = Some(GateClass::ProductionQa);
        second.expiry_ms = None;
        let answer =
            resolve_approval_gate(&ask("deploy.promotions.apply", None), &[second, first])
                .expect("answer");
        assert_eq!(answer.gate, ApprovalGate::TwoPerson);
        assert_eq!(answer.gate_class, Some(GateClass::ProductionQa));
        assert_eq!(answer.approver_roles, vec!["owner".to_owned(), "release-manager".to_owned()]);
        assert_eq!(answer.expires_after_ms, Some(3_600_000));
        assert_eq!(answer.rule_ids, vec!["a".to_owned(), "b".to_owned()]);
        assert!(answer.policy_digest.starts_with("sha256:"));
    }

    // spec: MEDULLA-BUSINESS-OPS-R004
    #[test]
    fn a_rule_without_a_quorum_is_a_single_gate_and_a_foreign_currency_does_not_cover() {
        let mut bare = rule("r1", "money.transfer");
        bare.quorum = None;
        bare.currency = Some("USD".to_owned());
        let mut request = ask("money.transfer", Some(10));
        request.currency = Some("USD".to_owned());
        assert_eq!(
            resolve_approval_gate(&request, &[bare.clone()]).expect("answer").gate,
            ApprovalGate::Single
        );
        request.currency = Some("EUR".to_owned());
        assert_eq!(
            resolve_approval_gate(&request, &[bare]).expect("answer").gate,
            ApprovalGate::Unresolved
        );
    }

    // spec: MEDULLA-BUSINESS-OPS-R004
    #[test]
    fn a_node_reads_as_a_rule_only_for_its_tenant_and_only_when_well_formed() {
        let node = json!({
            "type": "ApprovalRule",
            "tenant_id": "acme",
            "approvalActionKind": "deploy.promotions.apply",
            "approvalQuorum": 2,
            "approvalRequiredRole": "owner",
            "approvalGateClass": "publishing",
            "approvalExpiryMs": 60_000,
        });
        let read = ApprovalRule::from_node("rule:1", &node, "acme").expect("a rule");
        assert_eq!(read.quorum, Some(2));
        assert_eq!(read.gate_class, Some(GateClass::Publishing));
        assert!(ApprovalRule::from_node("rule:1", &node, "globex").is_none());
        for (key, bad) in [
            ("approvalGateClass", json!("everything")),
            ("approvalQuorum", json!("two")),
            ("approvalQuorum", json!(-1)),
            ("approvalGateClass", json!("money_movement")),
            ("approvalActionKind", json!("  ")),
            ("type", json!("Policy")),
        ] {
            let mut broken = node.clone();
            broken[key] = bad;
            assert!(ApprovalRule::from_node("rule:1", &broken, "acme").is_none(), "{key}");
        }
    }

    // spec: MEDULLA-BUSINESS-OPS-R004
    #[test]
    fn a_monetary_rule_needs_its_ceiling_and_currency_and_any_other_rule_does_not() {
        let mut node = json!({
            "type": "ApprovalRule",
            "approvalActionKind": "deploy.promotions.apply",
            "approvalQuorum": 2,
            "approvalGateClass": "production_qa",
        });
        let plain = ApprovalRule::from_node("rule:1", &node, "acme").expect("non-monetary rule");
        assert_eq!((plain.ceiling_minor, plain.currency), (None, None));
        node["approvalGateClass"] = json!("money_movement");
        assert!(ApprovalRule::from_node("rule:1", &node, "acme").is_none(), "neither");
        node["approvalCeilingMinor"] = json!(5_000);
        assert!(ApprovalRule::from_node("rule:1", &node, "acme").is_none(), "no currency");
        node["approvalCurrency"] = json!("USD");
        let money = ApprovalRule::from_node("rule:1", &node, "acme").expect("complete rule");
        assert_eq!(money.ceiling_minor, Some(5_000));
        // A monetary rule left out answers unresolved, never a looser gate.
        node.as_object_mut().expect("object").remove("approvalCurrency");
        assert!(ApprovalRule::from_node("rule:1", &node, "acme").is_none());
    }

    // spec: MEDULLA-BUSINESS-OPS-R004
    #[test]
    fn a_blank_operation_refuses() {
        let refusal = resolve_approval_gate(&ask("  ", None), &[]).expect_err("refused");
        assert_eq!(refusal.code(), APPROVAL_GATE_INVALID);
    }
}
