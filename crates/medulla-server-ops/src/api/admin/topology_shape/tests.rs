use medulla_agent_types::agent_fixtures::topology::facts;
use medulla_agent_types::agent_fixtures::topology::slot;
use medulla_agent_types::{
    agent_graph::AgentGraphEdge, agent_graph::AgentGraphNode, agent_graph::AgentGraphShape,
};
use medulla_types::contract::BoundedVec;

use super::*;

fn node(node_id: &str, kind: AgentGraphNodeKind) -> AgentGraphNode {
    AgentGraphNode {
        node_id: node_id.to_string(),
        kind,
        deps_contract: None,
        output_contract: None,
    }
}

fn agent(node_id: &str) -> AgentGraphNode {
    node(
        node_id,
        AgentGraphNodeKind::Agent {
            agent_id: format!("agent-{node_id}"),
            definition_digest: format!("sha256:{}", "1".repeat(64)),
        },
    )
}

fn edge(from: &str, to: &str) -> AgentGraphEdge {
    AgentGraphEdge {
        from: from.to_string(),
        to: to.to_string(),
        condition: None,
        kind: medulla_agent_types::agent_graph::AgentGraphEdgeKind::Feeds,
    }
}

/// lead → fanout → worker → join → end, carrying the fixture's fan-out facts.
fn fan_out_draft() -> AgentGraphDraft {
    AgentGraphDraft {
        graph_id: "graph:fan".to_string(),
        version: "1".to_string(),
        shape: AgentGraphShape {
            entry_node: "lead".to_string(),
            nodes: vec![
                agent("lead"),
                node("fan", AgentGraphNodeKind::Fanout),
                agent("worker"),
                node("join", AgentGraphNodeKind::Join),
                node("end", AgentGraphNodeKind::End),
            ],
            edges: vec![
                edge("lead", "fan"),
                edge("fan", "worker"),
                edge("worker", "join"),
                edge("join", "end"),
            ],
            max_iterations: 8,
        },
        tenant_id: "tenant-a".to_string(),
        actor_scope: "operator".to_string(),
        purpose_id: "agent-graph:publish".to_string(),
        policy_digest: format!("sha256:{}", "2".repeat(64)),
        synthesis_evidence: None,
        topology: Some(facts()),
    }
}

#[test]
fn a_well_formed_fan_out_template_passes() {
    assert_eq!(refusal(&fan_out_draft()), None);
    let mut plain = fan_out_draft();
    plain.topology = None;
    assert_eq!(
        refusal(&plain),
        None,
        "a graph without topology facts is not checked"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R108
#[test]
fn every_planted_shape_defect_is_refused_by_name() {
    type Plant = (&'static str, fn(&mut AgentGraphDraft));
    let plants: &[Plant] = &[
        ("agent nodes", |d| d.shape.nodes.push(agent("stray"))),
        ("Fanout node", |d| {
            d.shape.nodes.retain(|n| n.node_id != "join")
        }),
        ("more rounds", |d| {
            let facts = d.topology.as_mut().unwrap();
            let mut slots = facts.slots.as_slice().to_vec();
            slots[0].max_rounds = 2;
            facts.slots = BoundedVec::new(slots).unwrap();
        }),
        ("verifier slot", |d| {
            d.topology.as_mut().unwrap().stop = StopRule::VerifierPass { max_rounds: 1 };
        }),
        ("peer voters", |d| {
            d.topology.as_mut().unwrap().stop = StopRule::Quorum { k: 1, n: 2 };
        }),
        ("depth", |d| {
            d.shape.nodes.push(node(
                "child",
                AgentGraphNodeKind::Graph {
                    graph_id: "graph:child".to_string(),
                    shape_digest: format!("sha256:{}", "3".repeat(64)),
                },
            ));
        }),
        ("choices", |d| {
            let facts = d.topology.as_mut().unwrap();
            facts.slots = BoundedVec::new(vec![
                slot("lead", SlotRole::Parent, (1, 1), None),
                slot("worker", SlotRole::Child, (1, 12), None),
            ])
            .unwrap();
        }),
    ];
    for (needle, plant) in plants {
        let mut draft = fan_out_draft();
        plant(&mut draft);
        let refused = refusal(&draft).unwrap_or_else(|| panic!("{needle}: not refused"));
        assert!(refused.starts_with(TOPOLOGY_SHAPE_INVALID), "{refused}");
        assert!(refused.contains(needle), "{needle}: {refused}");
    }
}

// spec: MEDULLA-ORCH-VOCAB-R006.5
#[test]
fn feeds_cycle_and_misplaced_loop_back_refuse() {
    use medulla_agent_types::agent_graph::AgentGraphEdgeKind;

    let loop_back = |from: &str, to: &str| AgentGraphEdge {
        kind: AgentGraphEdgeKind::LoopsBack,
        ..edge(from, to)
    };

    // Unconditional feeds edges between two slots that form a cycle.
    let mut cyclic = fan_out_draft();
    cyclic.shape.edges.push(edge("lead", "worker"));
    cyclic.shape.edges.push(edge("worker", "lead"));
    let refused = refusal(&cyclic).expect("a feeds cycle is refused");
    assert!(refused.starts_with(TOPOLOGY_SHAPE_INVALID), "{refused}");
    assert!(refused.contains("cycle"), "{refused}");

    // The same back edge as a loop-back is not a cycle of feeds, but a
    // fan-out template is not a peer loop.
    let mut misplaced = fan_out_draft();
    misplaced.shape.edges.push(edge("lead", "worker"));
    misplaced.shape.edges.push(loop_back("worker", "lead"));
    let refused = refusal(&misplaced).expect("a loop-back outside a peer loop is refused");
    assert!(refused.starts_with(TOPOLOGY_SHAPE_INVALID), "{refused}");
    assert!(refused.contains("not a peer loop"), "{refused}");

    // A loop-back must join two slot nodes.
    let mut off_slot = fan_out_draft();
    off_slot.shape.edges.push(loop_back("worker", "fan"));
    let refused = refusal(&off_slot).expect("a loop-back to a non-slot is refused");
    assert!(refused.contains("does not join two slots"), "{refused}");

    // In a peer loop the same loop-back is admitted.
    let mut peer_loop = fan_out_draft();
    peer_loop.topology.as_mut().unwrap().class_iri =
        medulla_types::ontology_iri!("swarm#CritiqueLoop").to_string();
    peer_loop.shape.edges.push(edge("lead", "worker"));
    peer_loop.shape.edges.push(loop_back("worker", "lead"));
    assert_eq!(refusal(&peer_loop), None);
}
