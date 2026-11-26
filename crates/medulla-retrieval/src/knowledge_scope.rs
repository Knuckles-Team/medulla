//! Agent knowledge scopes (MEDULLA-FEDERATED-QUERY-R704.1, R704.2).
//!
//! A scope narrows what an agent's retrieval may read: named knowledge sets,
//! containers, source kinds and a time floor. It is a prefilter applied before
//! candidate truncation, so recall inside the scope is not lost to candidates
//! outside it. A scope only narrows: an empty scope is the caller's access
//! alone.

use serde::{Deserialize, Serialize};

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

/// Keep the top `k` scored candidates inside `scope` (MEDULLA-FEDERATED-QUERY-R704.2).
/// `candidates` are what the caller may already read; the scope filters them
/// before truncation, ties by id.
pub fn scoped_top_k(
    scope: &KnowledgeScope,
    candidates: Vec<(ScopedCandidate, f64)>,
    k: usize,
) -> Vec<(ScopedCandidate, f64)> {
    let mut inside: Vec<_> = candidates
        .into_iter()
        .filter(|(c, _)| scope.contains(c))
        .collect();
    inside.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.id.cmp(&b.0.id)));
    inside.truncate(k);
    inside
}

/// The property of an agent definition node that holds its knowledge scope
/// (MEDULLA-ORCH-VOCAB-R808): a JSON object, or its text.
pub const KNOWLEDGE_SCOPE_PROPERTY: &str = "knowledgeScope";
/// The edge relationship from a source record to its container (MEDULLA-INGEST-WORKER-R904.1).
const IN_CONTAINER: &str = "inContainer";

fn node_props(
    core: &medulla_graph_core::graph::GraphCore,
    id: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let blob = core.get_node_properties(id)?;
    medulla_types::msgpack::decode_property_object(&blob).ok()
}

/// The knowledge scope the agent definition node `agent_ref` declares (MEDULLA-FEDERATED-QUERY-R704.3).
/// `Ok(None)` when the agent has no node in this view, declares no scope, or declares an empty
/// one: the caller's access alone then applies. A malformed scope refuses.
pub fn agent_scope(
    core: &medulla_graph_core::graph::GraphCore,
    agent_ref: &str,
) -> Result<Option<KnowledgeScope>, String> {
    use serde_json::Value;
    let Some(node) = node_props(core, agent_ref) else {
        return Ok(None);
    };
    let Some(raw) = node.get(KNOWLEDGE_SCOPE_PROPERTY) else {
        return Ok(None);
    };
    let mut object = match raw {
        Value::String(text) => serde_json::from_str::<Value>(text),
        other => Ok(other.clone()),
    }
    .map_err(|e| format!("INVALID_ARGUMENT: knowledgeScope of {agent_ref} is not JSON: {e}"))?;
    if let Value::Object(fields) = &mut object {
        fields.insert("agent_ref".into(), Value::String(agent_ref.to_string()));
    }
    let scope: KnowledgeScope = serde_json::from_value(object)
        .map_err(|e| format!("INVALID_ARGUMENT: knowledgeScope of {agent_ref} is malformed: {e}"))?;
    Ok((!scope.is_empty()).then_some(scope))
}

/// A scope resolved against one graph view: its container references expanded to subtrees
/// through the container hierarchy (MEDULLA-INGEST-WORKER-R904.2), and each record's container.
pub struct ResolvedScope {
    scope: KnowledgeScope,
    container_of: std::collections::BTreeMap<String, String>,
}

impl ResolvedScope {
    /// Resolve `scope` over `core`. A container reference that is not a container node stays
    /// as named; one that is expands to itself and every descendant.
    pub fn resolve(core: &medulla_graph_core::graph::GraphCore, scope: &KnowledgeScope) -> Self {
        let mut scope = scope.clone();
        if !scope.container_refs.is_empty() {
            let mut expanded = std::collections::BTreeSet::new();
            for reference in &scope.container_refs {
                match medulla_core::source_hierarchy::subtree(
                    core,
                    reference,
                    usize::MAX,
                ) {
                    Ok(entries) => expanded.extend(entries.into_iter().map(|e| e.node_id)),
                    Err(_) => {
                        expanded.insert(reference.clone());
                    }
                }
            }
            scope.container_refs = expanded.into_iter().collect();
        }
        let mut container_of = std::collections::BTreeMap::new();
        if !scope.container_refs.is_empty() {
            for (source, target, blob) in core.get_edges() {
                let relationship = medulla_types::msgpack::decode_property_object(&blob)
                    .ok()
                    .and_then(|p| p.get("relationship")?.as_str().map(str::to_string));
                if relationship.as_deref() == Some(IN_CONTAINER) {
                    container_of.entry(source).or_insert(target);
                }
            }
        }
        Self { scope, container_of }
    }

    /// What the scope tests on node `id`.
    pub fn candidate(
        &self,
        core: &medulla_graph_core::graph::GraphCore,
        id: &str,
    ) -> ScopedCandidate {
        let props = node_props(core, id).unwrap_or_default();
        let text = |key: &str| props.get(key).and_then(|v| v.as_str()).map(str::to_string);
        let timestamp = ["updated_at", "created_at"].iter().find_map(|key| {
            let value = props.get(*key)?;
            value
                .as_u64()
                .or_else(|| value.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
        });
        ScopedCandidate {
            id: id.to_string(),
            knowledge_set: text("knowledge_set"),
            container_ref: self.container_of.get(id).cloned(),
            source_kind: text("source_kind").or_else(|| text("source")).unwrap_or_default(),
            timestamp,
        }
    }

    /// Whether node `id` lies inside the scope.
    pub fn admits(&self, core: &medulla_graph_core::graph::GraphCore, id: &str) -> bool {
        self.scope.contains(&self.candidate(core, id))
    }

    /// Every node of `core` inside the scope: the prefilter of a retrieval that truncates.
    pub fn node_ids(
        &self,
        core: &medulla_graph_core::graph::GraphCore,
    ) -> std::collections::BTreeSet<String> {
        core.node_ids()
            .into_iter()
            .filter(|id| self.admits(core, id))
            .collect()
    }

    /// Keep the top `k` of `scored` inside the scope, by [`scoped_top_k`].
    pub fn top_k(
        &self,
        core: &medulla_graph_core::graph::GraphCore,
        scored: Vec<(String, f32)>,
        k: usize,
    ) -> Vec<(String, f32)> {
        let candidates = scored
            .into_iter()
            .map(|(id, score)| (self.candidate(core, &id), f64::from(score)))
            .collect();
        scoped_top_k(&self.scope, candidates, k)
            .into_iter()
            .map(|(candidate, score)| (candidate.id, score as f32))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> KnowledgeScope {
        KnowledgeScope {
            agent_ref: "agent:support".into(),
            knowledge_sets: vec!["ks:runbooks".into()],
            container_refs: vec![],
            source_kinds: vec!["confluence".into()],
            time_floor: Some(1_000),
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R704.1
    #[test]
    fn scope_round_trips() {
        let s = scope();
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<KnowledgeScope>(&json).unwrap(), s);
        let minimal: KnowledgeScope = serde_json::from_str(r#"{"agent_ref":"a"}"#).unwrap();
        assert!(minimal.is_empty());
        assert!(
            serde_json::from_str::<KnowledgeScope>(r#"{"agent_ref":"a","widen":true}"#).is_err()
        );
    }

    fn candidate(id: &str, set: &str, kind: &str, ts: u64) -> ScopedCandidate {
        ScopedCandidate {
            id: id.into(),
            knowledge_set: Some(set.into()),
            container_ref: None,
            source_kind: kind.into(),
            timestamp: Some(ts),
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R704.2
    #[test]
    fn scope_prefilters_before_truncation() {
        // The three best-scoring candidates are all out of scope; post-filtering a
        // top-3 would return nothing, prefiltering keeps both in-scope hits.
        let candidates = vec![
            (candidate("x1", "ks:hr", "confluence", 2_000), 0.99),
            (candidate("x2", "ks:runbooks", "slack", 2_000), 0.98),
            (candidate("x3", "ks:runbooks", "confluence", 10), 0.97),
            (candidate("in1", "ks:runbooks", "confluence", 2_000), 0.50),
            (candidate("in2", "ks:runbooks", "confluence", 3_000), 0.40),
        ];
        let kept: Vec<_> = scoped_top_k(&scope(), candidates.clone(), 3)
            .into_iter()
            .map(|(c, _)| c.id)
            .collect();
        assert_eq!(kept, vec!["in1", "in2"]);
        // An empty scope is the caller's access alone.
        let open = KnowledgeScope {
            agent_ref: "a".into(),
            ..Default::default()
        };
        assert_eq!(scoped_top_k(&open, candidates, 3).len(), 3);
    }
}
