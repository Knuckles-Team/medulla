//! Durable run-scope bound (MEDULLA-IDENTITY-R008).
//!
//! A run's tool and data scope comes from a STORED policy node, never from plan
//! text: [`ScopeBound`] carries the policy IRI, the allowed tool IRIs and the
//! allowed graphs, and [`scope_within_bound`] checks a plan's tool list against
//! that bound in isolation from any scope field the plan body might carry.
//! Both halves are pure and fail-closed: a bound without a policy IRI is
//! refused at construction, and the subset check NAMES the first out-of-bound
//! tool instead of returning a bare boolean, so a refusal downstream can
//! audit which tool exactly crossed the line.

use std::collections::BTreeSet;

/// A refused [`ScopeBound`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeBoundError {
    /// The `policy_iri` was empty (or blank). A bound without a policy source
    /// has nothing to check against and is denied fail-closed.
    EmptyPolicyIri,
    /// The stored `:RunScopePolicy` node exists but cannot be read as a bound
    /// (MEDULLA-IDENTITY-R008.4): undecodable properties, or an `allowed_tools`
    /// / `allowed_graphs` that is not a list of strings.
    PolicyUnreadable {
        /// IRI of the malformed policy node.
        policy_iri: String,
        /// What made it unreadable.
        reason: String,
    },
}

impl std::fmt::Display for ScopeBoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyPolicyIri => {
                f.write_str("ScopeBound refused: policy_iri is empty (fail-closed)")
            }
            Self::PolicyUnreadable { policy_iri, reason } => write!(
                f,
                "{SCOPE_POLICY_UNREADABLE}: run-scope policy '{policy_iri}' is malformed: {reason}"
            ),
        }
    }
}

impl std::error::Error for ScopeBoundError {}

/// The durable scope a run may not exceed: the policy IRI it was read from,
/// the tool IRIs the policy allows, and the graphs the policy allows.
///
/// The `ScopeBound { policy_iri, allowed_tools, allowed_graphs }` contract of
/// MEDULLA-IDENTITY-R008.2: construction REFUSES an empty `policy_iri` with
/// [`ScopeBoundError::EmptyPolicyIri`]; an empty `allowed_tools` is legal and
/// simply means "no tools allowed".
///
/// External callers cannot bypass construction:
/// ```compile_fail,E0451
/// use medulla_isolation::isolation::ScopeBound;
/// let _ = ScopeBound {
///     policy_iri: String::new(),
///     allowed_tools: vec![],
///     allowed_graphs: vec![],
/// };
/// ```
/// Nor can they clear a validated policy IRI:
/// ```compile_fail,E0616
/// use medulla_isolation::isolation::ScopeBound;
/// let mut bound = ScopeBound::new("urn:policy:worker", vec![], vec![]).unwrap();
/// bound.policy_iri.clear();
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeBound {
    /// IRI of the stored policy node this bound was read from. Never empty.
    policy_iri: String,
    /// Tool IRIs the policy allows the run to name.
    pub allowed_tools: Vec<String>,
    /// Graph names the policy allows the run to touch.
    pub allowed_graphs: Vec<String>,
}

impl ScopeBound {
    /// Read the policy IRI without permitting mutation.
    pub fn policy_iri(&self) -> &str {
        &self.policy_iri
    }

    /// Build a bound, refusing an empty `policy_iri` fail-closed.
    pub fn new(
        policy_iri: &str,
        allowed_tools: Vec<String>,
        allowed_graphs: Vec<String>,
    ) -> Result<Self, ScopeBoundError> {
        if policy_iri.trim().is_empty() {
            return Err(ScopeBoundError::EmptyPolicyIri);
        }
        Ok(Self {
            policy_iri: policy_iri.to_string(),
            allowed_tools,
            allowed_graphs,
        })
    }
}

/// Pure subset check (MEDULLA-IDENTITY-R008.3): the FIRST plan tool IRI that is not
/// in `bound.allowed_tools`, or `None` when every planned tool is inside the
/// bound. Returning the named IRI (not a boolean) is what lets the admission
/// path refuse with `ScopeExceedsPolicy` naming the offending tool and the
/// bound's `policy_iri`.
pub fn scope_within_bound<'a>(plan_tools: &'a [String], bound: &ScopeBound) -> Option<&'a str> {
    let allowed: BTreeSet<&str> = bound.allowed_tools.iter().map(String::as_str).collect();
    plan_tools
        .iter()
        .find(|tool| !allowed.contains(tool.as_str()))
        .map(String::as_str)
}

/// Refusal code of a malformed stored policy node (MEDULLA-IDENTITY-R008.4).
pub const SCOPE_POLICY_UNREADABLE: &str = "SCOPE_POLICY_UNREADABLE";
/// Refusal code of a plan naming a tool outside its bound (MEDULLA-IDENTITY-R008.5).
pub const SCOPE_EXCEEDS_POLICY: &str = "SCOPE_EXCEEDS_POLICY";
/// Ontology class of the stored policy node.
pub const RUN_SCOPE_POLICY_CLASS: &str = "RunScopePolicy";

/// Node id of a principal's stored run-scope policy:
/// `urn:policy:run-scope:<principal_persistence_id>`.
pub fn run_scope_policy_iri(principal_persistence_id: &str) -> String {
    format!("urn:policy:run-scope:{principal_persistence_id}")
}

fn string_list(
    props: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    policy_iri: &str,
) -> Result<Vec<String>, ScopeBoundError> {
    let unreadable = |reason: String| ScopeBoundError::PolicyUnreadable {
        policy_iri: policy_iri.to_string(),
        reason,
    };
    match props.get(key) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| unreadable(format!("'{key}' holds a non-string entry")))
            })
            .collect(),
        Some(_) => Err(unreadable(format!("'{key}' is not a list"))),
    }
}

/// Read a principal's bound from its stored `:RunScopePolicy` node in the
/// verified tenant's graph (MEDULLA-IDENTITY-R008.4). Only the stored node is
/// read; no request field can widen it. A principal with no policy node gets
/// the EMPTY bound (no tools, no graphs), so every tool is out of bound
/// (fail closed). A node that exists but is malformed is refused with
/// [`ScopeBoundError::PolicyUnreadable`].
pub fn load_scope_bound(
    core: &medulla_graph_core::graph::GraphCore,
    principal_persistence_id: &str,
) -> Result<ScopeBound, ScopeBoundError> {
    let policy_iri = run_scope_policy_iri(principal_persistence_id);
    let Some(bytes) = core.get_node_properties(&policy_iri) else {
        return ScopeBound::new(&policy_iri, Vec::new(), Vec::new());
    };
    let unreadable = |reason: String| ScopeBoundError::PolicyUnreadable {
        policy_iri: policy_iri.clone(),
        reason,
    };
    let value: serde_json::Value = rmp_serde::from_slice(&bytes)
        .map_err(|error| unreadable(format!("undecodable properties: {error}")))?;
    let props = value
        .as_object()
        .ok_or_else(|| unreadable("properties are not a map".to_string()))?;
    if !medulla_graph_core::graph::props_carry_label(props, RUN_SCOPE_POLICY_CLASS) {
        return Err(unreadable(format!(
            "node is not of class :{RUN_SCOPE_POLICY_CLASS}"
        )));
    }
    let tools = string_list(props, "allowed_tools", &policy_iri)?;
    let graphs = string_list(props, "allowed_graphs", &policy_iri)?;
    ScopeBound::new(&policy_iri, tools, graphs)
}

/// A plan refused by [`enforce_scope`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeRefusal {
    /// The stored bound could not be read (`SCOPE_POLICY_UNREADABLE`).
    Unreadable(ScopeBoundError),
    /// The plan names a tool outside the bound (`SCOPE_EXCEEDS_POLICY`).
    ExceedsPolicy {
        /// The first out-of-bound tool IRI.
        tool_iri: String,
        /// The bound's policy IRI.
        policy_iri: String,
    },
}

impl ScopeRefusal {
    /// The wire refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unreadable(_) => SCOPE_POLICY_UNREADABLE,
            Self::ExceedsPolicy { .. } => SCOPE_EXCEEDS_POLICY,
        }
    }
}

impl std::fmt::Display for ScopeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(error) => write!(f, "{error}"),
            Self::ExceedsPolicy {
                tool_iri,
                policy_iri,
            } => write!(
                f,
                "{SCOPE_EXCEEDS_POLICY}: tool '{tool_iri}' is outside run-scope policy '{policy_iri}'"
            ),
        }
    }
}

impl std::error::Error for ScopeRefusal {}

/// Audit class of a refused out-of-bound plan (MEDULLA-IDENTITY-R008.5).
pub const SCOPE_EXCEEDS_POLICY_AUDIT_CLASS: &str = "scope_exceeds_policy";

/// The canonical audit line of a scope refusal: audit class, tenant, tool IRI
/// and policy IRI. The served path digests it into the audit-chain entry it
/// writes before the refusal returns.
pub fn scope_refusal_audit_line(refusal: &ScopeRefusal, tenant: &str) -> String {
    match refusal {
        ScopeRefusal::ExceedsPolicy {
            tool_iri,
            policy_iri,
        } => format!(
            "{SCOPE_EXCEEDS_POLICY_AUDIT_CLASS}|tenant={tenant}|tool={tool_iri}|policy={policy_iri}"
        ),
        ScopeRefusal::Unreadable(error) => {
            format!("{SCOPE_EXCEEDS_POLICY_AUDIT_CLASS}|tenant={tenant}|unreadable={error}")
        }
    }
}

/// Tool-set admission (MEDULLA-IDENTITY-R008.1): load the principal's stored
/// bound and refuse the first plan tool outside it. Returns the bound the
/// plan was admitted under.
pub fn enforce_scope(
    core: &medulla_graph_core::graph::GraphCore,
    principal_persistence_id: &str,
    plan_tools: &[String],
) -> Result<ScopeBound, ScopeRefusal> {
    let bound =
        load_scope_bound(core, principal_persistence_id).map_err(ScopeRefusal::Unreadable)?;
    if let Some(tool) = scope_within_bound(plan_tools, &bound) {
        return Err(ScopeRefusal::ExceedsPolicy {
            tool_iri: tool.to_string(),
            policy_iri: bound.policy_iri().to_string(),
        });
    }
    Ok(bound)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-IDENTITY-R008.2
    #[test]
    fn empty_policy_iri_is_refused() {
        for policy_iri in ["", "   "] {
            assert_eq!(
                ScopeBound::new(policy_iri, vec![], vec![]),
                Err(ScopeBoundError::EmptyPolicyIri),
                "an empty policy_iri must be refused fail-closed"
            );
        }
        assert!(matches!(
            ScopeBound::new("", vec![], vec![]),
            Err(ScopeBoundError::EmptyPolicyIri)
        ));
    }

    // spec: MEDULLA-IDENTITY-R008.3
    #[test]
    fn first_excess_tool_is_named() {
        let bound = ScopeBound::new(
            "http://ex.org/policy/worker",
            vec!["http://ex.org/tools/query".to_string()],
            vec!["__commons__".to_string()],
        )
        .unwrap_or_else(|e| panic!("a non-empty policy_iri must construct: {e}"));
        let plan = vec![
            "http://ex.org/tools/query".to_string(),
            "http://ex.org/tools/write".to_string(),
            "http://ex.org/tools/erase".to_string(),
        ];
        assert_eq!(
            scope_within_bound(&plan, &bound),
            Some("http://ex.org/tools/write"),
            "the FIRST out-of-bound tool must be named, not just detected"
        );
    }

    // spec: MEDULLA-IDENTITY-R008.2
    #[test]
    fn valid_bound_round_trips_its_fields() {
        let bound = ScopeBound::new(
            "http://ex.org/policy/worker",
            vec!["http://ex.org/tools/query".to_string()],
            vec!["__commons__".to_string(), "agent:worker".to_string()],
        )
        .expect("a non-empty policy_iri must construct");
        assert_eq!(bound.policy_iri(), "http://ex.org/policy/worker");
        assert_eq!(
            bound.allowed_tools,
            vec!["http://ex.org/tools/query".to_string()]
        );
        assert_eq!(
            bound.allowed_graphs,
            vec!["__commons__".to_string(), "agent:worker".to_string()]
        );
    }

    // spec: MEDULLA-IDENTITY-R008.3
    #[test]
    fn in_bound_plan_returns_none() {
        let bound = ScopeBound::new(
            "http://ex.org/policy/worker",
            vec![
                "http://ex.org/tools/query".to_string(),
                "http://ex.org/tools/insert".to_string(),
            ],
            vec!["__commons__".to_string()],
        )
        .unwrap_or_else(|e| panic!("a non-empty policy_iri must construct: {e}"));
        let plan = vec![
            "http://ex.org/tools/query".to_string(),
            "http://ex.org/tools/insert".to_string(),
        ];
        assert_eq!(
            scope_within_bound(&plan, &bound),
            None,
            "a fully in-bound plan must pass quietly"
        );
        assert_eq!(scope_within_bound(&[], &bound), None);
    }
}
