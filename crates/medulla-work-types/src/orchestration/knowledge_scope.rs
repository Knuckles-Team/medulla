//! The knowledge scope of an agent definition (MEDULLA-ORCH-VOCAB-R808).
//!
//! A scope narrows what an agent's retrieval may read: named knowledge sets, containers,
//! source kinds and a time floor. It lives with the agent definition vocabulary so the write
//! side (the definition node admitted through the mutation gateway) and the read side
//! (`medulla-retrieval`, MEDULLA-FEDERATED-QUERY-R704) share one type and one parser. A scope
//! only narrows: an empty scope is the caller's access alone.

use serde::{Deserialize, Serialize};

/// The property of an agent definition node that holds its knowledge scope: a JSON object,
/// or its text.
pub const KNOWLEDGE_SCOPE_PROPERTY: &str = "knowledgeScope";

/// The refusal code of a scope that names a knowledge set the caller's view does not hold.
/// A foreign tenant's set and an absent one answer byte-identically (no existence oracle).
pub const SCOPE_SET_UNKNOWN: &str = "SCOPE_SET_UNKNOWN";

/// An agent's knowledge scope (MEDULLA-FEDERATED-QUERY-R704.1). Empty lists do not
/// constrain that dimension.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeScope {
    pub agent_ref: String,
    #[serde(default)]
    pub knowledge_sets: Vec<String>,
    #[serde(default)]
    pub container_refs: Vec<String>,
    #[serde(default)]
    pub source_kinds: Vec<String>,
    /// Unix seconds; older candidates are out of scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_floor: Option<u64>,
}

/// What the scope tests on one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedCandidate {
    pub id: String,
    pub knowledge_set: Option<String>,
    pub container_ref: Option<String>,
    pub source_kind: String,
    pub timestamp: Option<u64>,
}

fn admits(allowed: &[String], value: Option<&str>) -> bool {
    allowed.is_empty() || value.is_some_and(|v| allowed.iter().any(|a| a == v))
}

impl KnowledgeScope {
    /// Whether the scope places no constraint.
    pub fn is_empty(&self) -> bool {
        self.knowledge_sets.is_empty()
            && self.container_refs.is_empty()
            && self.source_kinds.is_empty()
            && self.time_floor.is_none()
    }

    /// Whether `candidate` lies inside the scope.
    pub fn contains(&self, candidate: &ScopedCandidate) -> bool {
        admits(&self.knowledge_sets, candidate.knowledge_set.as_deref())
            && admits(&self.container_refs, candidate.container_ref.as_deref())
            && admits(&self.source_kinds, Some(candidate.source_kind.as_str()))
            && self
                .time_floor
                .is_none_or(|floor| candidate.timestamp.is_some_and(|t| t >= floor))
    }
}

/// Parse the value of the `knowledgeScope` property of the definition `agent_ref`: a JSON
/// object, or its text. The `agent_ref` field is the node's own id, never the stored value's.
/// A value that is not JSON or does not decode as a scope is `INVALID_ARGUMENT: ...`.
pub fn parse_knowledge_scope(
    agent_ref: &str,
    raw: &serde_json::Value,
) -> Result<KnowledgeScope, String> {
    use serde_json::Value;
    let mut object = match raw {
        Value::String(text) => serde_json::from_str::<Value>(text),
        other => Ok(other.clone()),
    }
    .map_err(|e| format!("INVALID_ARGUMENT: knowledgeScope of {agent_ref} is not JSON: {e}"))?;
    if let Value::Object(fields) = &mut object {
        fields.insert("agent_ref".into(), Value::String(agent_ref.to_string()));
    }
    serde_json::from_value(object)
        .map_err(|e| format!("INVALID_ARGUMENT: knowledgeScope of {agent_ref} is malformed: {e}"))
}

/// The first knowledge set of `scope` that `holds` does not report as a node of the caller's
/// view, as the refusal text `SCOPE_SET_UNKNOWN: knowledge set <id> is unknown`. One code and
/// one detail whether the set is absent or another tenant's.
pub fn unknown_set(scope: &KnowledgeScope, holds: impl Fn(&str) -> bool) -> Option<String> {
    scope
        .knowledge_sets
        .iter()
        .find(|set| !holds(set))
        .map(|set| format!("{SCOPE_SET_UNKNOWN}: knowledge set {set} is unknown"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-ORCH-VOCAB-R808
    #[test]
    fn scope_parses_from_object_or_text_and_names_unknown_sets() {
        let object = serde_json::json!({"knowledge_sets": ["set:a", "set:b"], "time_floor": 5});
        let from_object = parse_knowledge_scope("agent:x", &object).unwrap();
        let from_text =
            parse_knowledge_scope("agent:x", &serde_json::Value::String(object.to_string()))
                .unwrap();
        assert_eq!(from_object, from_text);
        assert_eq!(from_object.agent_ref, "agent:x");
        assert!(!from_object.is_empty());
        let refused = unknown_set(&from_object, |set| set == "set:a").unwrap();
        assert_eq!(refused, "SCOPE_SET_UNKNOWN: knowledge set set:b is unknown");
        assert!(unknown_set(&from_object, |_| true).is_none());
        let malformed = parse_knowledge_scope("agent:x", &serde_json::json!({"nope": 1}));
        assert!(malformed.unwrap_err().starts_with("INVALID_ARGUMENT"));
        let not_json = parse_knowledge_scope("agent:x", &serde_json::json!("{"));
        assert!(not_json.unwrap_err().starts_with("INVALID_ARGUMENT"));
    }
}
