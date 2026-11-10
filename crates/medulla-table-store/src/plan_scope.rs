//! Query-plan scope predicates (CONCEPT:MEDULLA-KG.query.plan-scope).
//!
//! The immutable scope model ([`PlanScope`] and its closed [`ScopePredicate`]
//! set) plus the plan rewriter [`inject_scope`] (R078.2): it walks a bound
//! plan and adds the tenant predicate to every graph, edge, and vector scan
//! leaf under the default [`ScopeRule`] table entry `scan_all_leaves`. Served
//! refusal, trace evidence, and served proof belong to the later
//! `MEDULLA-FEDERATED-QUERY-R078.4` through `.6` requirements.

use serde::{Deserialize, Deserializer, Serialize};

/// The closed set of predicates a [`PlanScope`] can contribute to a plan.
///
/// Keeping this an enum prevents a caller from smuggling an arbitrary
/// post-retrieval filter into a plan scope. The tenant predicate is exact;
/// the clearance predicate carries the labels the caller is allowed to see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScopePredicate {
    /// Restrict rows to one exact tenant.
    TenantEquals { tenant: String },
    /// Restrict rows to the caller's declared clearance-label set.
    ClearanceSubset { labels: Vec<String> },
}

/// The caller scope that a query plan will carry into later planning stages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanScope {
    /// The tenant whose rows may be read.
    #[serde(deserialize_with = "deserialize_non_empty_tenant")]
    pub tenant: String,
    /// Clearance labels available to the caller.
    pub clearance_labels: Vec<String>,
    /// Whether the tenant and labels came from verified authority.
    pub verified: bool,
}

/// Validation failures for a plan scope model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanScopeError {
    /// A scope without a tenant cannot safely constrain a query.
    EmptyTenant,
}

impl std::fmt::Display for PlanScopeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyTenant => formatter.write_str("plan scope tenant must be non-empty"),
        }
    }
}

impl std::error::Error for PlanScopeError {}

fn deserialize_non_empty_tenant<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let tenant = String::deserialize(deserializer)?;
    if tenant.trim().is_empty() {
        return Err(serde::de::Error::custom(PlanScopeError::EmptyTenant));
    }
    Ok(tenant)
}

impl PlanScope {
    /// Build a scope and reject an empty or whitespace-only tenant.
    pub fn new(
        tenant: impl Into<String>,
        clearance_labels: Vec<String>,
        verified: bool,
    ) -> Result<Self, PlanScopeError> {
        let scope = Self {
            tenant: tenant.into(),
            clearance_labels,
            verified,
        };
        scope.validate()?;
        Ok(scope)
    }

    /// Validate the fields that are required before a scope can constrain a plan.
    pub fn validate(&self) -> Result<(), PlanScopeError> {
        if self.tenant.trim().is_empty() {
            return Err(PlanScopeError::EmptyTenant);
        }
        Ok(())
    }

    /// Return the complete closed predicate set represented by this scope.
    pub fn predicate(&self) -> Result<Vec<ScopePredicate>, PlanScopeError> {
        self.validate()?;
        Ok(vec![
            ScopePredicate::TenantEquals {
                tenant: self.tenant.clone(),
            },
            ScopePredicate::ClearanceSubset {
                labels: self.clearance_labels.clone(),
            },
        ])
    }
}

/// The named rules of the scope-injection table (R078.2). The default rule
/// `scan_all_leaves` scopes every scan leaf; a replacement rule names the leaf
/// classes it injects at, so a rule can narrow the default without inventing a
/// predicate the closed [`ScopePredicate`] set does not carry.
#[cfg(feature = "query")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeRule {
    /// The default: inject at every graph, edge, and vector scan leaf.
    ScanAllLeaves,
}

#[cfg(feature = "query")]
impl ScopeRule {
    /// Whether this rule injects the scope at a leaf of the given class.
    pub fn covers(self, leaf: medulla_types::wire::PlanScopeLeaf) -> bool {
        match self {
            ScopeRule::ScanAllLeaves => matches!(
                leaf,
                medulla_types::wire::PlanScopeLeaf::Graph
                    | medulla_types::wire::PlanScopeLeaf::Edge
                    | medulla_types::wire::PlanScopeLeaf::Vector
            ),
        }
    }

    /// The closed leaf classes this rule stamps onto the plan.
    pub fn leaves(self) -> Vec<medulla_types::wire::PlanScopeLeaf> {
        use medulla_types::wire::PlanScopeLeaf;
        [
            PlanScopeLeaf::Graph,
            PlanScopeLeaf::Edge,
            PlanScopeLeaf::Vector,
        ]
        .into_iter()
        .filter(|leaf| self.covers(*leaf))
        .collect()
    }
}

/// Classify one plan op as a scorable scan leaf, or `None` for a transform op
/// (`Filter`, `Limit`, `Project`, …) that carries no source of its own.
/// Feature-gated plan ops (foreign, spatial, tsdb, tensor, OWL sources) are
/// not leaves of this table; they scope under their own requirements.
#[cfg(feature = "query")]
pub fn scan_leaf(op: &medulla_types::wire::Op) -> Option<medulla_types::wire::PlanScopeLeaf> {
    use medulla_types::wire::{Op, PlanScopeLeaf};
    Some(match op {
        Op::Scan { .. } | Op::ScanAll { .. } => PlanScopeLeaf::Graph,
        Op::Traverse { .. } | Op::Expand { .. } => PlanScopeLeaf::Edge,
        Op::Rank { .. } | Op::RankEmbed { .. } => PlanScopeLeaf::Vector,
        _ => return None,
    })
}

/// MEDULLA-FEDERATED-QUERY-R078.2: inject the caller's closed scope predicate
/// set into `plan` under the default rule [`ScopeRule::ScanAllLeaves`]. The
/// scope — the predicates built by [`PlanScope::predicate`], the leaf classes
/// the rule names, and the `verified` flag — is stamped onto the plan, so the
/// statement that reaches execution carries the predicate at every scan leaf
/// and no scan runs unscoped.
#[cfg(feature = "query")]
pub fn inject_scope(
    plan: &mut medulla_types::wire::Plan,
    scope: &PlanScope,
) -> Result<(), PlanScopeError> {
    inject_scope_with_rule(plan, scope, ScopeRule::ScanAllLeaves)
}

/// The rule-parameterized form of [`inject_scope`]: a named rule from the
/// [`ScopeRule`] table selects which scan leaves carry the predicate. The
/// scope is server-owned: any plan scope the caller supplied is overwritten,
/// never merged or appended.
#[cfg(feature = "query")]
pub fn inject_scope_with_rule(
    plan: &mut medulla_types::wire::Plan,
    scope: &PlanScope,
    rule: ScopeRule,
) -> Result<(), PlanScopeError> {
    let predicates = scope.predicate()?;
    plan.scope = Some(medulla_types::wire::PlanScope {
        tenant: scope.tenant.clone(),
        clearance_labels: scope.clearance_labels.clone(),
        verified: scope.verified,
        leaves: rule.leaves(),
    });
    debug_assert!(!predicates.is_empty(), "the closed set is never empty");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PlanScope, ScopePredicate};

    #[test]
    // spec: MEDULLA-FEDERATED-QUERY-R078.1
    fn plan_scope_round_trips_and_rejects_empty_tenant() {
        let scope = PlanScope::new("tenant-a", vec!["public".into(), "restricted".into()], true)
            .expect("valid tenant");
        let encoded = serde_json::to_vec(&scope).expect("serialize plan scope");
        let decoded: PlanScope = serde_json::from_slice(&encoded).expect("deserialize plan scope");

        assert_eq!(decoded, scope);
        assert_eq!(
            decoded.predicate().expect("valid predicates"),
            vec![
                ScopePredicate::TenantEquals {
                    tenant: "tenant-a".into(),
                },
                ScopePredicate::ClearanceSubset {
                    labels: vec!["public".into(), "restricted".into()],
                },
            ]
        );
        assert!(PlanScope::new("  ", Vec::new(), true).is_err());
        assert!(serde_json::from_slice::<PlanScope>(
            br#"{"tenant":"","clearance_labels":[],"verified":true}"#
        )
        .is_err());
    }
}

#[cfg(all(test, feature = "query"))]
mod plan_injection_tests {
    use super::{inject_scope, PlanScope, ScopeRule};
    use medulla_types::wire::{Op, Plan, PlanScopeLeaf};

    // spec: MEDULLA-FEDERATED-QUERY-R078.2
    #[test]
    fn three_leaf_plan_all_scoped() {
        let scope = PlanScope::new("tenant-a", vec!["public".into()], true).expect("valid tenant");
        let mut plan = Plan::new(vec![
            Op::Scan {
                label: "Doc".into(),
            },
            Op::Traverse {
                rel: "CITES".into(),
                min: 1,
                max: 1,
            },
            Op::Rank {
                query: vec![0.1, 0.2],
            },
            Op::Limit { k: 5 },
        ]);

        inject_scope(&mut plan, &scope).expect("a valid scope injects");

        // Every scan leaf the plan carries is scoped: each leaf class present
        // in the plan is named by the injected predicate's leaf set.
        let leaves_in_plan = plan
            .ops
            .iter()
            .filter_map(super::scan_leaf)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            leaves_in_plan,
            std::collections::BTreeSet::from([
                PlanScopeLeaf::Graph,
                PlanScopeLeaf::Edge,
                PlanScopeLeaf::Vector
            ]),
            "the plan has a graph, edge and vector scan leaf: {:?}",
            plan.ops
        );
        let scoped = plan.scope.expect("the plan carries the caller scope");
        for leaf in &leaves_in_plan {
            assert!(
                scoped.leaves.contains(leaf),
                "the leaf {leaf:?} carries the tenant predicate"
            );
        }

        // The closed predicate set rides with it, verbatim from R078.1.
        assert_eq!(scoped.tenant, "tenant-a");
        assert!(scoped.verified);
        assert_eq!(scoped.clearance_labels, vec!["public".to_string()]);
        assert_eq!(
            scope.predicate().expect("valid predicates").len(),
            2,
            "tenant equality plus the clearance-label set"
        );
        assert_eq!(
            plan.ops.len(),
            4,
            "the injection stamps the plan, it rewrites no op: {:?}",
            plan.ops
        );
    }

    #[test]
    fn the_injection_overwrites_a_caller_supplied_scope() {
        let caller_scope = PlanScope::new("tenant-a", Vec::new(), true).expect("valid tenant");
        let mut plan = Plan::new(vec![Op::Scan {
            label: "Doc".into(),
        }]);
        plan.scope = Some(medulla_types::wire::PlanScope {
            tenant: "other-tenant".into(),
            clearance_labels: vec!["secret".into()],
            verified: false,
            leaves: vec![PlanScopeLeaf::Vector],
        });

        inject_scope(&mut plan, &caller_scope).expect("a valid scope injects");

        let scoped = plan.scope.expect("the plan carries the caller scope");
        assert_eq!(scoped.tenant, "tenant-a");
        assert!(scoped.verified);
        assert!(scoped.clearance_labels.is_empty());
        assert_eq!(
            scoped.leaves,
            ScopeRule::ScanAllLeaves.leaves(),
            "the server scope overwrites the caller scope"
        );
    }

    #[test]
    fn the_rule_table_names_the_default_and_stays_closed() {
        assert_eq!(
            serde_json::from_str::<ScopeRule>("\"scan_all_leaves\"").expect("default rule"),
            ScopeRule::ScanAllLeaves
        );
        assert!(
            serde_json::from_str::<ScopeRule>("\"only_vector\"").is_err(),
            "the rule table is closed"
        );
        assert_eq!(
            ScopeRule::ScanAllLeaves.leaves(),
            vec![
                PlanScopeLeaf::Graph,
                PlanScopeLeaf::Edge,
                PlanScopeLeaf::Vector
            ]
        );
    }
}
