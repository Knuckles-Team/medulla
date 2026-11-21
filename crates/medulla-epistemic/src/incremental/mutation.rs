use std::collections::BTreeSet;

use medulla_graph_core::graph::GraphView;
use medulla_protocol_types::protocol::Method;
use medulla_types::materialization_status_machine::{
    MaterializationEvent, MaterializationStatus,
};

use super::{
    opaque_identity, refused, IncrementalDelta, IncrementalReasoningIndex,
    ProjectionInvalidationKind, ProjectionPosition,
};

impl IncrementalReasoningIndex {
    /// One-time bootstrap when no projection snapshot exists. Unsupported or corrupt
    /// persisted snapshots are rejected by the current reader. Steady-state updates
    /// are exclusively change-driven through [`Self::apply_batch`]. A machine refusal
    /// while registering a materialization fails the bootstrap.
    pub fn from_graph_view(view: &GraphView) -> Result<Self, String> {
        let mut index = Self::default();
        for node_id in view.node_properties.keys() {
            index.register_from_graph_view(view, node_id)?;
        }
        // A `(source, target)` pair may carry several parallel property entries —
        // e.g. a provenance `GENERATED_BY` edge AND a separate epistemic `SUPPORTS`
        // edge to the same node (see the read/write model note on
        // `GraphCore::edge_properties`). Registering only `versions.last()` silently
        // dropped whichever relationship was NOT the most recently written for that
        // pair. `add_edge` already decodes and dispatches by each entry's own
        // `relationship`, so visiting every version is the correct, non-lossy read.
        for ((source, target), versions) in &view.edge_properties {
            for properties in versions {
                index.add_edge(source, target, properties)?;
            }
        }
        Ok(index)
    }

    /// Apply one committed batch exactly once. Replaying the same outbox lease is
    /// harmless; cursor acknowledgement happens only after this state is durable.
    pub fn apply_batch(
        &mut self,
        position: ProjectionPosition,
        methods: &[Method],
    ) -> Result<IncrementalDelta, String> {
        let Some(position) = self.prepare_position(position)? else {
            return Ok(IncrementalDelta::default());
        };
        let mut delta = IncrementalDelta::default();
        for method in methods {
            apply_method(self, method, &mut delta)?;
        }
        self.position = Some(position);
        Ok(delta)
    }

    pub fn causes_of(&self, source: &str) -> BTreeSet<String> {
        self.causal_out
            .get(&opaque_identity(source))
            .cloned()
            .unwrap_or_default()
    }

    pub fn conflicts_with(&self, node: &str) -> BTreeSet<String> {
        self.conflicts
            .get(&opaque_identity(node))
            .cloned()
            .unwrap_or_default()
    }

    /// The opaque refs of every stale materialization, in key order.
    pub fn stale_materialization_refs(&self) -> Vec<String> {
        self.materializations
            .iter()
            .filter(|(_, status)| **status == MaterializationStatus::Stale)
            .map(|(node_ref, _)| node_ref.clone())
            .collect()
    }
}

fn apply_method(
    index: &mut IncrementalReasoningIndex,
    method: &Method,
    delta: &mut IncrementalDelta,
) -> Result<(), String> {
    match method {
        Method::AddNode {
            node_id,
            properties_msgpack,
        } => apply_add_node(index, node_id, properties_msgpack, delta),
        Method::RemoveNode { node_id } => apply_remove_node(index, node_id, delta),
        Method::CompareAndSetNodeFields { node_id, .. } => index.stale_dependents(node_id, delta),
        Method::AddEdge {
            source_id,
            target_id,
            properties_msgpack,
        } => apply_add_edge(index, source_id, target_id, properties_msgpack, delta),
        Method::RemoveEdge {
            source_id,
            target_id,
            relationship,
        } => apply_remove_edge(index, source_id, target_id, relationship.is_some(), delta),
        Method::ApplyMutation { event_type, query } => {
            apply_invalidation(index, event_type, query, delta)
        }
        _ => Ok(()),
    }
}

fn apply_add_node(
    index: &mut IncrementalReasoningIndex,
    node_id: &str,
    properties: &[u8],
    delta: &mut IncrementalDelta,
) -> Result<(), String> {
    index.stale_dependents(node_id, delta)?;
    index.register_materialization(node_id, properties)
}

fn apply_remove_node(
    index: &mut IncrementalReasoningIndex,
    node_id: &str,
    delta: &mut IncrementalDelta,
) -> Result<(), String> {
    let node_ref = opaque_identity(node_id);
    index.remove_incident(node_id, delta)?;
    index.materialization_deps.remove(&node_ref);
    index.remove_materialization_generator(&node_ref);
    // Removing a materialization retracts it; a node that is not one has no status to write.
    index
        .fire(&node_ref, MaterializationEvent::Retract)
        .map_err(refused)?;
    index.stale_dependents(node_id, delta)
}

fn apply_add_edge(
    index: &mut IncrementalReasoningIndex,
    source_id: &str,
    target_id: &str,
    properties: &[u8],
    delta: &mut IncrementalDelta,
) -> Result<(), String> {
    if index.add_edge(source_id, target_id, properties)? {
        delta.edge_changes += 1;
        index.stale_dependents(&format!("{source_id}->{target_id}"), delta)?;
        index.stale_dependents(source_id, delta)?;
        index.stale_dependents(target_id, delta)?;
    }
    Ok(())
}

fn apply_remove_edge(
    index: &mut IncrementalReasoningIndex,
    source_id: &str,
    target_id: &str,
    typed: bool,
    delta: &mut IncrementalDelta,
) -> Result<(), String> {
    // The reasoning index keeps one relation per endpoint pair and no edge type, so a
    // type-scoped removal never drops it: its dependents go stale instead.
    if !typed && index.remove_edge(source_id, target_id)? {
        delta.edge_changes += 1;
    }
    index.stale_dependents(&format!("{source_id}->{target_id}"), delta)?;
    index.stale_dependents(source_id, delta)?;
    index.stale_dependents(target_id, delta)
}

fn apply_invalidation(
    index: &mut IncrementalReasoningIndex,
    event_type: &str,
    query: &str,
    delta: &mut IncrementalDelta,
) -> Result<(), String> {
    let kind = match event_type {
        "policy_changed" => Some(ProjectionInvalidationKind::PolicyChanged),
        "model_retired" => Some(ProjectionInvalidationKind::ModelRetired),
        "ontology_evolved" => Some(ProjectionInvalidationKind::OntologyEvolved),
        _ => None,
    };
    if let Some(kind) = kind {
        delta.newly_stale.extend(index.invalidate(kind, query)?);
    }
    Ok(())
}
