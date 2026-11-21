//! Process compile behaviour (MEDULLA-CQ-ORCH-R700.2): load the process
//! source graph from the graph view and compile it.
//!
//! [`load_process_graph`] reads one process from the `:hasStep` and
//! `:handsOffTo` triple sets of the tenant view. The edge list holds both
//! sets: an edge whose source is the process node is a `:hasStep` edge, every
//! other edge is a `:handsOffTo` edge. The process node set is the targets of
//! `:hasStep` from the process, plus every node reached from them by
//! `:handsOffTo` edges; nothing else belongs to the process. Kinds and gates
//! are never read here: [`compile_process`] reads them through the class
//! annotations. [`compile_process_view`] is the composition of the loader,
//! `compile_process` and the unbound-step check; it is the one compile route
//! of the served `ProcessCompile` write.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use medulla_types::contract::Digest256;
use medulla_types::process_import::{
    CanonicalWorkflow, ImportError, KindAnnotation, LoweredFlow, ProcessSourceSystem, SourceGraph,
    SourceNode, StepKind, PROCESS_CYCLE_WITHOUT_GATE, PROCESS_TASK_UNBOUND, PROCESS_TOO_LARGE,
    PROCESS_UNKNOWN,
};

use crate::process_import::{canonical_json, compile_process};

/// One node of the tenant view that may belong to a process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewNode {
    pub id: String,
    pub class: String,
    pub label: String,
    pub role: Option<String>,
    pub reference: Option<String>,
}

/// One edge of the tenant view: a `:hasStep` edge or a `:handsOffTo` edge.
/// `loop_gate` is the `:loopGate` property of the edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewEdge {
    pub source: String,
    pub target: String,
    pub condition: Option<String>,
    pub default: bool,
    pub loop_gate: bool,
}

/// Why a process was not compiled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessCompilationError {
    /// The process node is not in the view.
    Unknown,
    /// A step other than an end step hands off to nothing.
    TaskUnbound { node_id: String },
    /// A cycle of hand-offs none of whose edges carries `:loopGate`.
    CycleWithoutGate { nodes: Vec<String> },
    /// More nodes than `process_compile_max_nodes`.
    TooLarge { nodes: u64, limit: u64 },
    /// The canonical compile refused the loaded graph.
    Import(ImportError),
}

impl From<ImportError> for ProcessCompilationError {
    fn from(error: ImportError) -> Self {
        Self::Import(error)
    }
}

/// Every refusal code of the process compile that is not an import code
/// (MEDULLA-CQ-ORCH-R700.4). The codes live in `medulla_types::process_import`
/// so the protocol registry can name them; an `Import` error keeps its own
/// `PROCESS_IMPORT_*` code.
pub const REFUSALS: &[&str] = medulla_types::process_import::COMPILE_REFUSALS;

impl ProcessCompilationError {
    /// The stable wire code of this refusal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unknown => PROCESS_UNKNOWN,
            Self::TaskUnbound { .. } => PROCESS_TASK_UNBOUND,
            Self::CycleWithoutGate { .. } => PROCESS_CYCLE_WITHOUT_GATE,
            Self::TooLarge { .. } => PROCESS_TOO_LARGE,
            Self::Import(error) => error.code(),
        }
    }
}

impl std::fmt::Display for ProcessCompilationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => write!(f, "the process is not visible to the caller's tenant"),
            Self::TaskUnbound { node_id } => {
                write!(f, "step {node_id} hands off to nothing and is not an end step")
            }
            Self::CycleWithoutGate { nodes } => write!(
                f,
                "hand-offs form a cycle with no loop gate through {}",
                nodes.join(", ")
            ),
            Self::TooLarge { nodes, limit } => {
                write!(f, "the process view holds {nodes} nodes, above the limit {limit}")
            }
            Self::Import(error) => write!(f, "{error}"),
        }
    }
}

/// A compiled process: the canonical workflow and the revision of the
/// process subgraph it was compiled from (MEDULLA-CQ-ORCH-R700.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledProcess {
    pub workflow: CanonicalWorkflow,
    /// SHA-256 of the canonical process subgraph, 64 lowercase hex characters.
    pub process_revision: String,
}

/// The revision of a loaded process subgraph: the digest of the canonical
/// JSON of its process id, nodes and flows, which `load_process_graph` sorts.
pub fn process_revision(graph: &SourceGraph) -> String {
    let value = serde_json::json!({
        "process": graph.model_id,
        "name": graph.name,
        "nodes": graph.nodes,
        "flows": graph.edges,
    });
    let mut text = String::new();
    canonical_json(&value, &mut text);
    Digest256::sha256(text.as_bytes()).to_hex()
}

/// Strongly connected components, iterative Tarjan: the component index of
/// every node, in `O(nodes + edges)`.
fn components(count: usize, adjacency: &[Vec<usize>]) -> Vec<usize> {
    const UNSEEN: usize = usize::MAX;
    let mut index = vec![UNSEEN; count];
    let mut low = vec![0usize; count];
    let mut on_stack = vec![false; count];
    let mut component = vec![UNSEEN; count];
    let mut stack: Vec<usize> = Vec::new();
    let mut next_index = 0usize;
    let mut next_component = 0usize;
    for root in 0..count {
        if index[root] != UNSEEN {
            continue;
        }
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = next_index;
        low[root] = next_index;
        next_index += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&(node, cursor)) = work.last() {
            if let Some(&target) = adjacency[node].get(cursor) {
                if let Some(top) = work.last_mut() {
                    top.1 += 1;
                }
                if index[target] == UNSEEN {
                    index[target] = next_index;
                    low[target] = next_index;
                    next_index += 1;
                    stack.push(target);
                    on_stack[target] = true;
                    work.push((target, 0));
                } else if on_stack[target] {
                    low[node] = low[node].min(index[target]);
                }
            } else {
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    low[parent] = low[parent].min(low[node]);
                }
                if low[node] == index[node] {
                    while let Some(member) = stack.pop() {
                        on_stack[member] = false;
                        component[member] = next_component;
                        if member == node {
                            break;
                        }
                    }
                    next_component += 1;
                }
            }
        }
    }
    component
}

/// Load the source graph of the process `process_iri` from the view.
pub fn load_process_graph(
    process_iri: &str,
    nodes: &[ViewNode],
    edges: &[ViewEdge],
    max_nodes: u64,
) -> Result<SourceGraph, ProcessCompilationError> {
    if nodes.len() as u64 > max_nodes {
        return Err(ProcessCompilationError::TooLarge {
            nodes: nodes.len() as u64,
            limit: max_nodes,
        });
    }
    let by_id: HashMap<&str, &ViewNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let process = by_id
        .get(process_iri)
        .ok_or(ProcessCompilationError::Unknown)?;

    // The closure: `:hasStep` targets, then every `:handsOffTo` successor.
    let mut hands_off: HashMap<&str, Vec<&ViewEdge>> = HashMap::new();
    let mut members: BTreeSet<&str> = BTreeSet::new();
    let mut frontier: Vec<&str> = Vec::new();
    for edge in edges {
        if edge.source == process_iri {
            if by_id.contains_key(edge.target.as_str()) && members.insert(edge.target.as_str()) {
                frontier.push(edge.target.as_str());
            }
        } else {
            hands_off.entry(edge.source.as_str()).or_default().push(edge);
        }
    }
    while let Some(node) = frontier.pop() {
        for edge in hands_off.get(node).into_iter().flatten() {
            if by_id.contains_key(edge.target.as_str()) && members.insert(edge.target.as_str()) {
                frontier.push(edge.target.as_str());
            }
        }
    }

    let ids: Vec<&str> = members.iter().copied().collect();
    let position: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut flows: Vec<&ViewEdge> = edges
        .iter()
        .filter(|e| {
            e.source != process_iri
                && position.contains_key(e.source.as_str())
                && position.contains_key(e.target.as_str())
        })
        .collect();
    flows.sort_by(|a, b| (&a.source, &a.target).cmp(&(&b.source, &b.target)));

    // Cycle rule: a component of more than one node, or a self edge, passes
    // only when one of its own edges is a loop gate.
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); ids.len()];
    for edge in &flows {
        adjacency[position[edge.source.as_str()]].push(position[edge.target.as_str()]);
    }
    let component = components(ids.len(), &adjacency);
    let mut size: BTreeMap<usize, usize> = BTreeMap::new();
    for c in &component {
        *size.entry(*c).or_default() += 1;
    }
    let mut cyclic: BTreeMap<usize, bool> = BTreeMap::new();
    for edge in &flows {
        let (s, t) = (position[edge.source.as_str()], position[edge.target.as_str()]);
        if component[s] == component[t] && (s == t || size[&component[s]] > 1) {
            let gated = cyclic.entry(component[s]).or_insert(false);
            *gated |= edge.loop_gate;
        }
    }
    let ungated: BTreeSet<usize> = cyclic
        .into_iter()
        .filter_map(|(c, gated)| (!gated).then_some(c))
        .collect();
    if !ungated.is_empty() {
        let mut offending: Vec<Vec<String>> = ungated
            .iter()
            .map(|c| {
                ids.iter()
                    .enumerate()
                    .filter(|(i, _)| component[*i] == *c)
                    .map(|(_, id)| (*id).to_string())
                    .collect()
            })
            .collect();
        offending.sort();
        let nodes = offending.swap_remove(0);
        return Err(ProcessCompilationError::CycleWithoutGate { nodes });
    }

    Ok(SourceGraph {
        name: process.label.clone(),
        system: ProcessSourceSystem::Bpmn20,
        model_id: process_iri.to_string(),
        nodes: ids
            .iter()
            .map(|id| {
                let node = by_id[id];
                SourceNode {
                    element_id: node.id.clone(),
                    class: node.class.clone(),
                    label: node.label.clone(),
                    role: node.role.clone(),
                    reference: node.reference.clone(),
                }
            })
            .collect(),
        edges: flows
            .iter()
            .map(|e| LoweredFlow {
                source: e.source.clone(),
                target: e.target.clone(),
                condition: e.condition.clone(),
                default: e.default,
            })
            .collect(),
        annex: Vec::new(),
    })
}

/// Load the process and compile it: the one compile route of `ProcessCompile`.
/// A step other than an end step with no outgoing flow is `TaskUnbound`.
pub fn compile_process_view(
    process_iri: &str,
    nodes: &[ViewNode],
    edges: &[ViewEdge],
    max_nodes: u64,
    annotations: &[KindAnnotation],
) -> Result<CompiledProcess, ProcessCompilationError> {
    let graph = load_process_graph(process_iri, nodes, edges, max_nodes)?;
    let process_revision = process_revision(&graph);
    let workflow = compile_process(&graph, annotations)?;
    let with_outgoing: BTreeSet<&str> = workflow.flows.iter().map(|f| f.source.as_str()).collect();
    for step in &workflow.steps {
        if step.kind != StepKind::End && !with_outgoing.contains(step.id.as_str()) {
            let node_id = workflow
                .source
                .elements
                .get(&step.id)
                .cloned()
                .unwrap_or_else(|| step.id.clone());
            return Err(ProcessCompilationError::TaskUnbound { node_id });
        }
    }
    Ok(CompiledProcess {
        workflow,
        process_revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-CQ-ORCH-R700.4
    #[test]
    fn codes_are_unique_and_stable() {
        let errors = [
            ProcessCompilationError::Unknown,
            ProcessCompilationError::TaskUnbound {
                node_id: "n".into(),
            },
            ProcessCompilationError::CycleWithoutGate {
                nodes: vec!["a".into(), "b".into()],
            },
            ProcessCompilationError::TooLarge { nodes: 3, limit: 2 },
        ];
        let codes: Vec<&str> = errors.iter().map(ProcessCompilationError::code).collect();
        assert_eq!(codes, &REFUSALS[..4]);
        assert_eq!(REFUSALS[4], medulla_types::process_import::WORKFLOW_VERSION_CONFLICT);
        let mut unique = REFUSALS.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), REFUSALS.len());
        let import = ProcessCompilationError::Import(ImportError::NotCanonical {
            reason: "schema".into(),
        });
        assert_eq!(import.code(), "PROCESS_IMPORT_NOT_CANONICAL");
    }
}
