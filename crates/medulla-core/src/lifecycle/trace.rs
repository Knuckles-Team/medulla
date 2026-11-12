//! The lifecycle trace (MEDULLA-SDLC-R006.2): a breadth-first walk of the derived
//! edge set from one anchor, bounded by depth and node budget.

use std::collections::{BTreeSet, VecDeque};

use medulla_source_types::{
    lifecycle::LifecycleTraceAnswer, lifecycle::LifecycleTraceRequest, lifecycle::TraceEdge,
    lifecycle::TraceLimits, lifecycle::TraceRecord,
};

use super::store::LifecycleStore;

/// Walk the edge set from `request.anchor` in both edge directions, to
/// `request.depth` capped by `limits.max_depth`. Records are filtered by
/// `request.kinds` (empty keeps all); the walk still passes through filtered
/// records. `truncated` reports that `limits.node_budget` stopped the walk. An
/// unknown anchor yields an empty answer.
pub fn lifecycle_trace(
    store: &LifecycleStore,
    request: &LifecycleTraceRequest,
    limits: TraceLimits,
) -> LifecycleTraceAnswer {
    let mut answer = LifecycleTraceAnswer::default();
    if store.node_kind(&request.anchor).is_none() {
        return answer;
    }
    let depth = request.depth.min(limits.max_depth);
    let mut seen: BTreeSet<String> = BTreeSet::from([request.anchor.clone()]);
    let mut queue = VecDeque::from([(request.anchor.clone(), 0u8)]);
    let mut kept: BTreeSet<String> = BTreeSet::new();
    while let Some((id, at_depth)) = queue.pop_front() {
        let Some(kind) = store.node_kind(&id) else {
            continue;
        };
        if request.kinds.is_empty() || request.kinds.contains(&kind) {
            kept.insert(id.clone());
            answer.records.push(TraceRecord {
                id: id.clone(),
                kind,
            });
        }
        if at_depth >= depth {
            continue;
        }
        for edge in store.edges_of(&id) {
            let other = if edge.from == id {
                &edge.to
            } else {
                &edge.from
            };
            if seen.contains(other) {
                continue;
            }
            if seen.len() >= limits.node_budget {
                answer.truncated = true;
                continue;
            }
            seen.insert(other.clone());
            queue.push_back((other.clone(), at_depth + 1));
        }
    }
    answer.edges = store
        .edges()
        .filter(|e| kept.contains(&e.from) && kept.contains(&e.to))
        .map(|e| TraceEdge {
            kind: e.kind,
            from: e.from.clone(),
            to: e.to.clone(),
            rule: e.rule.clone(),
            rule_version: e.rule_version,
        })
        .collect();
    answer
}
