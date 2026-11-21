//! Process compile behaviour (MEDULLA-CQ-ORCH-R700.2).

use medulla_core::process_compile::{
    compile_process_view, load_process_graph, ProcessCompilationError, ViewEdge, ViewNode,
};
use medulla_types::process_import::{KindAnnotation, StepKind};

const PROCESS: &str = "urn:process:p";

const ANNOTATIONS: &[KindAnnotation] = &[
    KindAnnotation {
        class: "human",
        kind: Some(StepKind::Human),
        gate: None,
    },
    KindAnnotation {
        class: "end",
        kind: Some(StepKind::End),
        gate: None,
    },
];

fn node(id: &str, class: &str) -> ViewNode {
    ViewNode {
        id: id.into(),
        class: class.into(),
        label: id.into(),
        role: None,
        reference: None,
    }
}

fn edge(source: &str, target: &str, loop_gate: bool) -> ViewEdge {
    ViewEdge {
        source: source.into(),
        target: target.into(),
        condition: None,
        default: false,
        loop_gate,
    }
}

/// A process of `n` nodes: `:hasStep` to the first node, a `:handsOffTo` chain
/// through the rest, an end step last, and two nodes of other processes.
fn chain(n: usize) -> (Vec<ViewNode>, Vec<ViewEdge>) {
    let mut nodes = vec![node(PROCESS, "process")];
    let mut edges = vec![edge(PROCESS, "n00000000", false)];
    for i in 0..n {
        let class = if i + 1 == n { "end" } else { "human" };
        nodes.push(node(&format!("n{i:08}"), class));
        if i + 1 < n {
            edges.push(edge(&format!("n{i:08}"), &format!("n{:08}", i + 1), false));
        }
    }
    nodes.push(node("other-a", "human"));
    nodes.push(node("other-b", "end"));
    edges.push(edge("other-a", "other-b", false));
    (nodes, edges)
}

// spec: MEDULLA-CQ-ORCH-R700.2
#[test]
fn load_is_linear_and_deterministic() {
    for n in [10usize, 1_000, 100_000] {
        let (nodes, edges) = chain(n);
        let graph = load_process_graph(PROCESS, &nodes, &edges, 1_000_000).expect("loads");
        assert_eq!(graph.nodes.len(), n, "the :hasStep targets plus the closure");
        assert!(graph.nodes.iter().all(|x| x.element_id.starts_with('n')));
        assert_eq!(graph.edges.len(), n - 1);
        assert_eq!(graph.model_id, PROCESS);

        let first = compile_process_view(PROCESS, &nodes, &edges, 1_000_000, ANNOTATIONS)
            .expect("compiles");
        let mut reversed = (nodes.clone(), edges.clone());
        reversed.0.reverse();
        reversed.1.reverse();
        let second =
            compile_process_view(PROCESS, &reversed.0, &reversed.1, 1_000_000, ANNOTATIONS)
                .expect("compiles again");
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
        assert_eq!(first.steps.len(), n);
    }
}

// spec: MEDULLA-CQ-ORCH-R700.2
#[test]
fn cycle_needs_a_loop_gate_and_size_and_unbound_are_refused() {
    let (mut nodes, mut edges) = chain(4);
    edges.push(edge("n00000002", "n00000001", false));
    assert_eq!(
        load_process_graph(PROCESS, &nodes, &edges, 1_000),
        Err(ProcessCompilationError::CycleWithoutGate {
            nodes: vec!["n00000001".into(), "n00000002".into()]
        })
    );
    edges.last_mut().unwrap().loop_gate = true;
    assert!(load_process_graph(PROCESS, &nodes, &edges, 1_000).is_ok());

    edges.push(edge("n00000000", "n00000000", false));
    assert!(matches!(
        load_process_graph(PROCESS, &nodes, &edges, 1_000),
        Err(ProcessCompilationError::CycleWithoutGate { .. })
    ));
    edges.pop();

    assert!(matches!(
        load_process_graph(PROCESS, &nodes, &edges, 3),
        Err(ProcessCompilationError::TooLarge { limit: 3, .. })
    ));
    assert_eq!(
        load_process_graph("urn:process:missing", &nodes, &edges, 1_000),
        Err(ProcessCompilationError::Unknown)
    );

    // The last step is not an end step and hands off to nothing.
    nodes.retain(|n| n.id != "n00000003");
    edges.retain(|e| e.target != "n00000003" && e.target != "n00000001" || e.source == PROCESS);
    edges.push(edge("n00000000", "n00000001", false));
    assert_eq!(
        compile_process_view(PROCESS, &nodes, &edges, 1_000, ANNOTATIONS)
            .map(|_| ()),
        Err(ProcessCompilationError::TaskUnbound {
            node_id: "n00000002".into()
        })
    );
}
