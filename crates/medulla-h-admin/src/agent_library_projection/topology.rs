//! A graph template's topology as `swarm:` facts (MEDULLA-ORCH-VOCAB-R006.5).
//!
//! [`project_topology`] is the one mapping from the topology of a published
//! [`AgentGraphEntry`] to triples. The entry's IRI is the subject of the
//! template triples and the prefix of each slot IRI, so replacing or removing
//! one entry's triples never touches another entry's. The mapping is linear in
//! slots plus edges.

use std::collections::BTreeMap;

use medulla_agent_types::agent_graph::{AgentGraphEdgeKind, AgentGraphEntry};
use medulla_agent_types::agent_topology::SlotRole;
use medulla_rdf::oxrdf::{Literal, NamedNode, Triple};

use super::map::segment;

const SWARM: &str = medulla_types::ontology_iri!("swarm#");
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

fn swarm(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{SWARM}{local}"))
}

fn role_term(role: SlotRole) -> &'static str {
    match role {
        SlotRole::Parent => "Parent",
        SlotRole::Child => "Child",
        SlotRole::Peer => "Peer",
        SlotRole::Aggregator => "Aggregator",
        SlotRole::Verifier => "Verifier",
    }
}

/// The IRI of one slot of the template `entry_iri`.
pub fn slot_iri(entry_iri: &str, node_id: &str) -> String {
    format!("{entry_iri}/slot/{}", segment(node_id))
}

/// The `swarm:` triples of the topology of `entry`, or none when it has no
/// topology facts. Per slot: `a swarm:Slot`, `swarm:nodeId`, `swarm:role` and,
/// when set, `swarm:requiresAgentKind`. Per unconditional `Feeds` edge between
/// two slots: `swarm:feeds`. Per `LoopsBack` edge between two slots:
/// `swarm:loopsBackTo` and never `swarm:feeds`. An edge to or from a node that
/// is not a slot writes nothing.
pub fn project_topology(entry_iri: &str, entry: &AgentGraphEntry) -> Vec<Triple> {
    let Some(facts) = &entry.topology else {
        return Vec::new();
    };
    let template = NamedNode::new_unchecked(entry_iri);
    let mut triples = vec![
        Triple::new(
            template.clone(),
            NamedNode::new_unchecked(RDF_TYPE),
            swarm("TopologyTemplate"),
        ),
        Triple::new(
            template.clone(),
            swarm("class"),
            NamedNode::new_unchecked(facts.class_iri.as_str()),
        ),
    ];
    let mut slots: BTreeMap<&str, NamedNode> = BTreeMap::new();
    for slot in facts.slots.iter() {
        let iri = NamedNode::new_unchecked(slot_iri(entry_iri, &slot.node_id));
        triples.push(Triple::new(template.clone(), swarm("hasSlot"), iri.clone()));
        triples.push(Triple::new(
            iri.clone(),
            NamedNode::new_unchecked(RDF_TYPE),
            swarm("Slot"),
        ));
        triples.push(Triple::new(
            iri.clone(),
            swarm("nodeId"),
            Literal::new_simple_literal(slot.node_id.as_str()),
        ));
        triples.push(Triple::new(
            iri.clone(),
            swarm("role"),
            swarm(role_term(slot.role)),
        ));
        if let Some(kind) = &slot.required_agent_kind {
            triples.push(Triple::new(
                iri.clone(),
                swarm("requiresAgentKind"),
                NamedNode::new_unchecked(kind.as_str()),
            ));
        }
        slots.insert(slot.node_id.as_str(), iri);
    }
    for edge in &entry.shape.edges {
        let (Some(from), Some(to)) = (slots.get(edge.from.as_str()), slots.get(edge.to.as_str()))
        else {
            continue;
        };
        match edge.kind {
            AgentGraphEdgeKind::Feeds if edge.condition.is_none() => {
                triples.push(Triple::new(from.clone(), swarm("feeds"), to.clone()));
            }
            AgentGraphEdgeKind::Feeds => {}
            AgentGraphEdgeKind::LoopsBack => {
                triples.push(Triple::new(from.clone(), swarm("loopsBackTo"), to.clone()));
            }
        }
    }
    triples
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_agent_types::agent_fixtures::topology::{facts, slot};
    use medulla_agent_types::agent_graph::{
        AgentGraphEdge, AgentGraphNode, AgentGraphNodeKind, AgentGraphShape,
    };
    use medulla_agent_types::agent_library::AgentLibraryLifecycle;
    use medulla_types::contract::BoundedVec;

    const ENTRY: &str = "https://knuckles-team.github.io/ontology/core#agentGraph/t/g";

    fn agent(node_id: &str) -> AgentGraphNode {
        AgentGraphNode {
            node_id: node_id.to_string(),
            kind: AgentGraphNodeKind::Agent {
                agent_id: format!("agent-{node_id}"),
                definition_digest: format!("sha256:{}", "1".repeat(64)),
            },
            deps_contract: None,
            output_contract: None,
        }
    }

    fn edge(from: &str, to: &str, kind: AgentGraphEdgeKind) -> AgentGraphEdge {
        AgentGraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            condition: None,
            kind,
        }
    }

    fn entry(edges: Vec<AgentGraphEdge>) -> AgentGraphEntry {
        let mut topology = facts();
        let mut slots = topology.slots.as_slice().to_vec();
        slots[1].required_agent_kind =
            Some("https://knuckles-team.github.io/ontology/core#Researcher".to_string());
        slots[0] = slot("lead", SlotRole::Peer, (1, 1), Some(100));
        topology.slots = BoundedVec::new(slots).unwrap();
        AgentGraphEntry {
            schema_version: 1,
            graph_id: "g".to_string(),
            version: "1".to_string(),
            shape: AgentGraphShape {
                entry_node: "lead".to_string(),
                nodes: vec![agent("lead"), agent("worker")],
                edges,
                max_iterations: 4,
            },
            tenant_id: "t".to_string(),
            actor_scope: "operator".to_string(),
            purpose_id: "agent-graph:publish".to_string(),
            policy_digest: format!("sha256:{}", "2".repeat(64)),
            synthesis_evidence: None,
            topology: Some(topology),
            composed_work_ceiling: 0,
            entry_revision: 1,
            lifecycle: AgentLibraryLifecycle::Published,
            shape_digest: format!("sha256:{}", "3".repeat(64)),
            definition_digest: format!("sha256:{}", "4".repeat(64)),
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    fn has(triples: &[Triple], subject: &str, predicate: &str, object: &str) -> bool {
        triples.iter().any(|t| {
            t.subject.to_string() == format!("<{subject}>")
                && t.predicate.as_str() == format!("{SWARM}{predicate}")
                && t.object.to_string() == format!("<{object}>")
        })
    }

    // spec: MEDULLA-ORCH-VOCAB-R006.5
    #[test]
    fn feeds_and_loop_back_edges() {
        let lead = slot_iri(ENTRY, "lead");
        let worker = slot_iri(ENTRY, "worker");
        let triples = project_topology(
            ENTRY,
            &entry(vec![
                edge("lead", "worker", AgentGraphEdgeKind::Feeds),
                edge("worker", "lead", AgentGraphEdgeKind::LoopsBack),
                // An edge to a node that is not a slot writes nothing.
                edge("worker", "end", AgentGraphEdgeKind::Feeds),
            ]),
        );
        assert!(has(&triples, ENTRY, "hasSlot", &lead));
        assert!(has(&triples, ENTRY, "hasSlot", &worker));
        assert!(has(&triples, &lead, "feeds", &worker));
        assert!(has(&triples, &worker, "loopsBackTo", &lead));
        assert!(
            !has(&triples, &worker, "feeds", &lead),
            "a loop-back edge never writes feeds"
        );
        assert_eq!(
            triples
                .iter()
                .filter(|t| t.predicate.as_str() == format!("{SWARM}feeds"))
                .count(),
            1
        );
        assert!(has(
            &triples,
            &worker,
            "requiresAgentKind",
            "https://knuckles-team.github.io/ontology/core#Researcher"
        ));
        assert!(!triples
            .iter()
            .any(|t| t.subject.to_string() == format!("<{lead}>")
                && t.predicate.as_str() == format!("{SWARM}requiresAgentKind")));
        assert!(has(&triples, &lead, "role", &format!("{SWARM}Peer")));

        let mut bare = entry(Vec::new());
        bare.topology = None;
        assert!(project_topology(ENTRY, &bare).is_empty());
    }
}
