//! Memory scopes and per-session observation settings
//! (MEDULLA-DECISION-ENGINE-R651.9, MEDULLA-DECISION-ENGINE-R651.10).
//!
//! A scope is a named set of sessions owned by one principal. It only ever narrows what a reader
//! may see: [`resolve_scope_sessions`] is the one place that intersects a scope with the sessions
//! the reader's view already allows. Every type refuses unknown fields on the wire. The types live
//! in `medulla-work-types` (not `medulla-types`) because an [`ObservationConfig`] names a
//! [`ParticipantId`].

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::participant::ParticipantId;

/// Version of the serialized scope types.
pub const MEMORY_SCOPE_SCHEMA_VERSION: u32 = 1;

/// A named set of sessions owned by one principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MemoryScope {
    /// The scope's id, unique within the tenant.
    pub scope_id: String,
    /// The tenant the scope belongs to; the server stamps the verified tenant.
    pub tenant: String,
    /// The name shown for the scope.
    pub name: String,
    /// The principal that owns the scope; only it may change the scope.
    pub owner_principal: String,
    /// The sessions the scope names.
    pub session_ids: BTreeSet<String>,
    /// Principals other than the owner that may name the scope on a read. Only the owner changes
    /// the scope.
    #[serde(default)]
    pub shared_with: BTreeSet<String>,
    /// The record's schema version.
    pub schema_version: u32,
}

/// The optional request field that names a scope; a request carries only its id, and every path
/// resolves it through [`resolve_scope_sessions`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScopeRef {
    /// The scope to narrow to.
    pub scope_id: String,
}

/// Which perspectives exist for one participant in one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ObservationConfig {
    /// The session the setting applies to.
    pub session_id: String,
    /// The participant the setting belongs to.
    pub participant_id: ParticipantId,
    /// Whether the participant observes its own turns.
    pub observe_self: bool,
    /// Whether the participant observes every other member's turns.
    pub observe_others: bool,
}

impl ObservationConfig {
    /// The setting a participant has when none is stored: it observes itself and not the others.
    pub fn missing(session_id: &str, participant_id: ParticipantId) -> Self {
        Self {
            session_id: session_id.to_string(),
            participant_id,
            observe_self: true,
            observe_others: false,
        }
    }
}

/// The prefix every stored scope node id of `tenant_scope` starts with.
pub fn scope_node_prefix(tenant_scope: &str) -> String {
    format!("memory_scope:{tenant_scope}:")
}

/// Graph node id of a stored scope. It is keyed by the verified tenant, so two tenants may use the
/// same scope id and neither can reach the other's node.
pub fn scope_node_id(tenant_scope: &str, scope_id: &str) -> String {
    format!("{}{scope_id}", scope_node_prefix(tenant_scope))
}

/// Graph node id of a stored observation setting, keyed by tenant, session and participant.
pub fn observation_node_id(
    tenant_scope: &str,
    session_id: &str,
    participant_id: &ParticipantId,
) -> String {
    format!(
        "observation_config:{tenant_scope}:{session_id}:{}",
        participant_id.as_str()
    )
}

/// The sessions a reader sees through a scope: the intersection of the scope's sessions with the
/// sessions the reader's own view allows. The result never exceeds `visible`, and an empty
/// intersection stays empty.
pub fn resolve_scope_sessions(scope: &MemoryScope, visible: &BTreeSet<String>) -> BTreeSet<String> {
    scope.session_ids.intersection(visible).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(sessions: &[&str]) -> MemoryScope {
        MemoryScope {
            scope_id: "s".into(),
            tenant: "t".into(),
            name: "n".into(),
            owner_principal: "alice".into(),
            session_ids: sessions.iter().map(|s| s.to_string()).collect(),
            shared_with: BTreeSet::new(),
            schema_version: MEMORY_SCOPE_SCHEMA_VERSION,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.9
    #[test]
    fn a_scope_narrows_and_never_widens() {
        let visible: BTreeSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        let narrowed = resolve_scope_sessions(&scope(&["b", "c"]), &visible);
        assert_eq!(narrowed.into_iter().collect::<Vec<_>>(), vec!["b"]);
        assert!(resolve_scope_sessions(&scope(&["z"]), &visible).is_empty());
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.10
    #[test]
    fn a_missing_observation_setting_observes_self_only() {
        let config = ObservationConfig::missing("s1", ParticipantId::new("ada").unwrap());
        assert!(config.observe_self && !config.observe_others);
        assert_eq!(
            observation_node_id("t", "s1", &config.participant_id),
            "observation_config:t:s1:ada"
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.9
    #[test]
    fn node_ids_are_keyed_by_tenant() {
        assert_ne!(scope_node_id("t1", "team"), scope_node_id("t2", "team"));
        assert!(scope_node_id("t1", "team").starts_with(&scope_node_prefix("t1")));
        assert!(!scope_node_id("t2", "team").starts_with(&scope_node_prefix("t1")));
    }

    // spec: MEDULLA-DECISION-ENGINE-R651.9
    #[test]
    fn scope_never_exceeds_visible_sessions() {
        // A deterministic xorshift stream: random scope and visible sets over a small id universe,
        // so overlaps, subsets and disjoint sets all occur.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let pick = |bits: u64| -> BTreeSet<String> {
            (0..16).filter(|n| bits >> n & 1 == 1).map(|n| format!("s{n}")).collect()
        };
        for _ in 0..2_000 {
            let visible = pick(next());
            let named = pick(next());
            let scope = MemoryScope { session_ids: named.clone(), ..scope(&[]) };
            let resolved = resolve_scope_sessions(&scope, &visible);
            assert!(resolved.is_subset(&visible), "never exceeds visible");
            assert!(resolved.is_subset(&named), "never names a session the scope lacks");
            assert!(
                visible.intersection(&named).all(|session| resolved.contains(session)),
                "keeps every session both allow"
            );
        }
        let visible: BTreeSet<String> = ["a".to_string()].into();
        assert!(resolve_scope_sessions(&scope(&["z"]), &visible).is_empty());
        assert!(resolve_scope_sessions(&scope(&["a"]), &BTreeSet::new()).is_empty());
    }
}
