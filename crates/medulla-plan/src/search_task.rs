//! Search-task synthesis with shortcut-risk detectors (MEDULLA-FEDERATED-QUERY-R702.2).
//!
//! `synthesize` builds a multi-hop clue chain and a templated question from an
//! `EvidenceGraph` — no model call — runs the four shortcut detectors, each a
//! bounded graph query linear in the subgraph, and refines the task (adds one
//! clue or masks one constant) for at most `max_refinement_rounds` rounds. The
//! result types are the existing `SearchTask`, `RiskFinding`, `RiskReport` and
//! `RiskThresholds` of `medulla-work-types` (MEDULLA-FEDERATED-QUERY-R702.1);
//! threshold refusal is the caller's `RiskReport::check`
//! (MEDULLA-FEDERATED-QUERY-R702.4), which this module never re-implements.
//!
//! The detector values:
//! * `single_clue` — fraction of clues whose evidence alone leaves at most
//!   one candidate answer (a neighbour of the clue's evidence that is not its
//!   own evidence).
//! * `co_coverage` — fraction of evidence nodes adjacent to at least two
//!   evidence nodes of OTHER clues (one node covers two clue chains).
//! * `exposed_constant` — the largest fraction of the answer-name tokens that
//!   one clue's text shares.
//! * `prior_knowledge` — the degree (edges, both directions) of the answer node.
//!
//! Refinement is bounded: it fixes `exposed_constant` by masking one constant
//! and `single_clue` by adding one clue, for at most `max_refinement_rounds`
//! rounds; `co_coverage` and `prior_knowledge` have no in-graph fix, so they
//! end the loop. Writes: none.

use std::collections::{BTreeMap, BTreeSet};

use medulla_work_types::search_task::{
    Clue, EvidenceGraph, RiskFinding, RiskReport, RiskThresholds, SearchTask, SearchTaskRefusal,
    ShortcutDetector, SEARCH_TASK_INVALID,
};

/// The output of one synthesis: the synthesized task, the complete detector
/// report and the refinement rounds used.
#[derive(Debug, Clone, PartialEq)]
pub struct Synthesis {
    pub task: SearchTask,
    pub report: RiskReport,
    pub rounds_used: u32,
}

fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

fn adjacency(graph: &EvidenceGraph) -> BTreeMap<String, BTreeSet<String>> {
    let mut adj: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for node in &graph.nodes {
        adj.entry(node.id.clone()).or_default();
    }
    for edge in &graph.edges {
        adj.entry(edge.from.clone())
            .or_default()
            .insert(edge.to.clone());
        adj.entry(edge.to.clone())
            .or_default()
            .insert(edge.from.clone());
    }
    adj
}

/// One clue and the evidence nodes it rests on.
fn clue_backing(clues: &[Clue]) -> BTreeMap<String, usize> {
    let mut backing = BTreeMap::new();
    for (index, clue) in clues.iter().enumerate() {
        for node in &clue.evidence_nodes {
            backing.entry(node.clone()).or_insert(index);
        }
    }
    backing
}

/// The multi-hop clue chain: a deterministic depth-first walk from the answer;
/// every discovered node is one clue resting on itself, and the question is
/// templated from the chain — no model call.
fn clue_chain(
    graph: &EvidenceGraph,
    answer: &str,
    adj: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<Clue> {
    let labels: BTreeMap<String, String> = graph
        .nodes
        .iter()
        .map(|n| (n.id.clone(), n.label.clone()))
        .collect();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    visited.insert(answer.to_string());
    let mut clues = Vec::new();
    let mut stack = vec![answer.to_string()];
    while let Some(current) = stack.pop() {
        let neighbours = adj.get(&current).cloned().unwrap_or_default();
        let mut next: Vec<&String> = neighbours
            .iter()
            .filter(|n| !visited.contains(*n))
            .collect();
        next.sort();
        for node in next.iter().rev() {
            visited.insert((*node).clone());
            let node_label = labels
                .get(*node)
                .cloned()
                .unwrap_or_else(|| (*node).clone());
            let text = if current == answer {
                format!("the {node_label} adjacent to the sought entity")
            } else {
                let parent_label = labels
                    .get(&current)
                    .cloned()
                    .unwrap_or_else(|| current.clone());
                format!("the {node_label} linked to {parent_label}")
            };
            clues.push(Clue {
                text,
                evidence_nodes: vec![(*node).clone()],
            });
            stack.push((*node).clone());
        }
    }
    clues
}

/// Single-clue risk: the fraction of clues whose evidence alone leaves at most
/// one candidate answer — a neighbour of the clue's evidence that is not its
/// own evidence (MEDULLA-FEDERATED-QUERY-R702.2). Linear in the subgraph.
fn single_clue(clues: &[Clue], adj: &BTreeMap<String, BTreeSet<String>>) -> (f64, Vec<String>) {
    if clues.is_empty() {
        return (0.0, Vec::new());
    }
    let mut decisive: Vec<String> = Vec::new();
    for clue in clues {
        let mut candidates = BTreeSet::new();
        for node in &clue.evidence_nodes {
            for neighbour in adj.get(node).cloned().unwrap_or_default() {
                if !clue.evidence_nodes.contains(&neighbour) {
                    candidates.insert(neighbour);
                }
            }
        }
        if candidates.len() <= 1 {
            decisive.extend(clue.evidence_nodes.iter().cloned());
        }
    }
    (decisive.len() as f64 / clues.len() as f64, decisive)
}

/// Co-cover risk: the fraction of evidence nodes adjacent to at least two
/// evidence nodes of other clues (MEDULLA-FEDERATED-QUERY-R702.2). Linear in
/// the subgraph.
fn co_coverage(clues: &[Clue], adj: &BTreeMap<String, BTreeSet<String>>) -> (f64, Vec<String>) {
    let backing = clue_backing(clues);
    let mut covered: Vec<String> = Vec::new();
    for (node, clue) in &backing {
        let mut other_clue_nodes = 0;
        for neighbour in adj.get(node).cloned().unwrap_or_default() {
            if let Some(&other) = backing.get(&neighbour) {
                if other != *clue {
                    other_clue_nodes += 1;
                }
            }
        }
        if other_clue_nodes >= 2 {
            covered.push(node.clone());
        }
    }
    let value = if backing.is_empty() {
        0.0
    } else {
        covered.len() as f64 / backing.len() as f64
    };
    (value, covered)
}

/// Exposed-constant risk: the largest fraction of the answer-name tokens that
/// one clue's text shares (MEDULLA-FEDERATED-QUERY-R702.2). Linear in the
/// clue texts plus the answer name.
fn exposed_constant(graph: &EvidenceGraph, clues: &[Clue], answer: &str) -> (f64, Vec<String>) {
    let answer_label = graph
        .nodes
        .iter()
        .find(|n| n.id == answer)
        .map(|n| n.label.as_str())
        .unwrap_or("");
    let answer_tokens = tokens(answer_label);
    if answer_tokens.is_empty() {
        return (0.0, Vec::new());
    }
    let mut best: (f64, Vec<String>) = (0.0, Vec::new());
    for clue in clues {
        let clue_tokens = tokens(&clue.text);
        let shared = clue_tokens
            .iter()
            .filter(|t| answer_tokens.contains(*t))
            .count();
        let value = shared as f64 / answer_tokens.len() as f64;
        if value > best.0 {
            best = (value, clue.evidence_nodes.clone());
        }
    }
    best
}

/// Prior-knowledge risk: the degree of the answer node (MEDULLA-FEDERATED-QUERY-R702.2).
fn prior_knowledge(answer: &str, adj: &BTreeMap<String, BTreeSet<String>>) -> (f64, Vec<String>) {
    (
        adj.get(answer).cloned().unwrap_or_default().len() as f64,
        vec![answer.to_string()],
    )
}

fn evaluate(
    graph: &EvidenceGraph,
    clues: &[Clue],
    answer: &str,
    adj: &BTreeMap<String, BTreeSet<String>>,
) -> RiskReport {
    let (single, single_nodes) = single_clue(clues, adj);
    let (co, co_nodes) = co_coverage(clues, adj);
    let (exposed, exposed_nodes) = exposed_constant(graph, clues, answer);
    let (prior, prior_nodes) = prior_knowledge(answer, adj);
    RiskReport {
        findings: vec![
            RiskFinding {
                detector: ShortcutDetector::SingleClue,
                value: single,
                nodes: single_nodes,
            },
            RiskFinding {
                detector: ShortcutDetector::CoCoverage,
                value: co,
                nodes: co_nodes,
            },
            RiskFinding {
                detector: ShortcutDetector::ExposedConstant,
                value: exposed,
                nodes: exposed_nodes,
            },
            RiskFinding {
                detector: ShortcutDetector::PriorKnowledge,
                value: prior,
                nodes: prior_nodes,
            },
        ],
    }
}

fn over(report: &RiskReport, thresholds: &RiskThresholds, detector: ShortcutDetector) -> bool {
    let limit = match detector {
        ShortcutDetector::SingleClue => thresholds.single_clue,
        ShortcutDetector::CoCoverage => thresholds.co_coverage,
        ShortcutDetector::ExposedConstant => thresholds.exposed_constant,
        ShortcutDetector::PriorKnowledge => thresholds.prior_knowledge,
    };
    report
        .findings
        .iter()
        .find(|f| f.detector == detector)
        .is_some_and(|f| f.value > limit)
}

fn exposed_tokens(text: &str, answer_tokens: &BTreeSet<String>) -> u64 {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty() && answer_tokens.contains(&t.to_lowercase()))
        .count() as u64
}

/// Build the task, run the four detectors, and refine for at most
/// `max_refinement_rounds` rounds (MEDULLA-FEDERATED-QUERY-R702.2). Refinement
/// masks one constant when `exposed_constant` is over threshold and adds one
/// clue when `single_clue` is over threshold; `co_coverage` and
/// `prior_knowledge` have no in-graph fix. The report is complete for every
/// outcome so the caller's `RiskReport::check` can refuse by detector name
/// (MEDULLA-FEDERATED-QUERY-R702.4).
pub fn synthesize(
    graph: &EvidenceGraph,
    answer: &str,
    thresholds: &RiskThresholds,
    max_refinement_rounds: u32,
) -> Result<Synthesis, SearchTaskRefusal> {
    if graph.nodes.iter().all(|n| n.id != answer) {
        return Err(SearchTaskRefusal {
            code: SEARCH_TASK_INVALID.to_string(),
            subject: answer.to_string(),
        });
    }
    let adj = adjacency(graph);
    let clues = clue_chain(graph, answer, &adj);
    let question = {
        let answer_label = graph
            .nodes
            .iter()
            .find(|n| n.id == answer)
            .map(|n| n.label.as_str())
            .unwrap_or(answer);
        let steps = clues
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        format!("Which {answer_label} satisfies every step: {steps}?")
    };
    let mut task = SearchTask {
        question,
        answer_node: answer.to_string(),
        clues,
        max_refinement_rounds,
    };
    let mut report = evaluate(graph, &task.clues, answer, &adj);
    let mut rounds_used = 0u32;
    loop {
        if rounds_used >= max_refinement_rounds {
            break;
        }
        let detector = match ShortcutDetector::ALL
            .iter()
            .find(|d| over(&report, thresholds, **d))
        {
            Some(d) => *d,
            None => break,
        };
        match detector {
            ShortcutDetector::ExposedConstant => {
                // Mask one constant: the most exposed clue loses the
                // answer-name tokens from its text.
                let answer_tokens = tokens(
                    graph
                        .nodes
                        .iter()
                        .find(|n| n.id == answer)
                        .map(|n| n.label.as_str())
                        .unwrap_or(""),
                );
                let index = task
                    .clues
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| exposed_tokens(&c.text, &answer_tokens) > 0)
                    .max_by_key(|(_, c)| exposed_tokens(&c.text, &answer_tokens))
                    .map(|(i, _)| i);
                match index {
                    Some(i) => {
                        task.clues[i].text = task.clues[i]
                            .text
                            .split(' ')
                            .map(|word| {
                                if !word.is_empty() && answer_tokens.contains(&word.to_lowercase())
                                {
                                    "•".to_string()
                                } else {
                                    word.to_string()
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" ");
                        report = evaluate(graph, &task.clues, answer, &adj);
                        rounds_used += 1;
                    }
                    None => break,
                }
            }
            ShortcutDetector::SingleClue => {
                // Add one clue: an un-backed, non-answer node adjacent to a
                // backed one (the strongest such first, then by id), else the
                // strongest un-backed node.
                let backed = clue_backing(&task.clues)
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>();
                let unbacked: Vec<&String> = graph
                    .nodes
                    .iter()
                    .map(|n| &n.id)
                    .filter(|id| id.as_str() != answer && !backed.contains(*id))
                    .collect();
                if unbacked.is_empty() {
                    break;
                }
                let degree = |id: &str| adj.get(id).cloned().unwrap_or_default().len();
                let mut extension: Vec<(String, Option<String>)> = unbacked
                    .iter()
                    .filter_map(|id| {
                        adj.get(*id)
                            .cloned()
                            .unwrap_or_default()
                            .iter()
                            .find(|p| backed.contains(*p))
                            .map(|p| ((*id).clone(), Some((*p).clone())))
                    })
                    .collect();
                if extension.is_empty() {
                    let mut fallback = unbacked;
                    fallback.sort_by(|a, b| degree(b).cmp(&degree(a)).then(a.cmp(b)));
                    extension = vec![(fallback[0].clone(), None)];
                }
                extension
                    .sort_by(|a, b| degree(&b.0).cmp(&degree(&a.0)).then_with(|| a.0.cmp(&b.0)));
                let (node, parent) = extension.remove(0);
                let labels: BTreeMap<String, String> = graph
                    .nodes
                    .iter()
                    .map(|n| (n.id.clone(), n.label.clone()))
                    .collect();
                let node_label = labels.get(&node).cloned().unwrap_or_else(|| node.clone());
                let text = match parent {
                    Some(p) => {
                        let parent_label = labels.get(&p).cloned().unwrap_or_else(|| p.clone());
                        format!("the {node_label} linked to {parent_label}")
                    }
                    None => format!("the {node_label} with {} links", degree(&node)),
                };
                task.clues.push(Clue {
                    text,
                    evidence_nodes: vec![node],
                });
                report = evaluate(graph, &task.clues, answer, &adj);
                rounds_used += 1;
            }
            // Co-cover and prior-knowledge risks are properties of the graph
            // itself; refinement cannot fix them, so the loop ends.
            ShortcutDetector::CoCoverage | ShortcutDetector::PriorKnowledge => break,
        }
    }
    task.validate(graph)?;
    Ok(Synthesis {
        task,
        report,
        rounds_used,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_work_types::search_task::EvidenceNode;

    fn node(id: &str, label: &str) -> EvidenceNode {
        EvidenceNode {
            id: id.into(),
            label: label.into(),
            text: format!("text of {id}"),
        }
    }

    fn edge(from: &str, to: &str) -> medulla_work_types::search_task::EvidenceEdge {
        medulla_work_types::search_task::EvidenceEdge {
            from: from.into(),
            to: to.into(),
            relation: "linked".into(),
        }
    }

    fn thresholds(single: f64, co: f64, exposed: f64, prior: f64) -> RiskThresholds {
        RiskThresholds {
            single_clue: single,
            co_coverage: co,
            exposed_constant: exposed,
            prior_knowledge: prior,
        }
    }

    fn finding(report: &RiskReport, detector: ShortcutDetector) -> &RiskFinding {
        report
            .findings
            .iter()
            .find(|f| f.detector == detector)
            .expect("every detector is reported")
    }

    // spec: MEDULLA-FEDERATED-QUERY-R702.2
    #[test]
    fn each_detector_threshold_boundary() {
        // Single-clue: in the chain a—e1—e2 only the end clue leaves at most
        // one candidate, so the value is 1/2; it passes at the threshold and
        // refuses just below it, naming the detector.
        let chain = EvidenceGraph {
            nodes: vec![node("a", "hub"), node("e1", "one"), node("e2", "two")],
            edges: vec![edge("a", "e1"), edge("e1", "e2")],
        };
        let out = synthesize(&chain, "a", &thresholds(0.5, 1.0, 1.0, 100.0), 0).expect("synthesis");
        assert_eq!(
            finding(&out.report, ShortcutDetector::SingleClue).value,
            0.5
        );
        out.report
            .check(&thresholds(0.5, 1.0, 1.0, 100.0))
            .expect("a value equal to the threshold passes");
        let refusal = out
            .report
            .check(&thresholds(0.499, 1.0, 1.0, 100.0))
            .unwrap_err();
        assert_eq!(
            (refusal.code.as_str(), refusal.subject.as_str()),
            ("SEARCH_TASK_SHORTCUT", "single_clue")
        );
        assert_eq!(out.rounds_used, 0);

        // Co-cover: each evidence node is adjacent to the two other clue
        // nodes, so the value is 1.0.
        let co = EvidenceGraph {
            nodes: vec![
                node("a", "hub"),
                node("e1", "one"),
                node("e2", "two"),
                node("e3", "three"),
            ],
            edges: vec![
                edge("a", "e1"),
                edge("a", "e2"),
                edge("a", "e3"),
                edge("e1", "e2"),
                edge("e2", "e3"),
                edge("e1", "e3"),
            ],
        };
        let out = synthesize(&co, "a", &thresholds(1.0, 1.0, 1.0, 100.0), 0).expect("synthesis");
        assert_eq!(
            finding(&out.report, ShortcutDetector::CoCoverage).value,
            1.0
        );
        out.report
            .check(&thresholds(1.0, 1.0, 1.0, 100.0))
            .expect("a value equal to the threshold passes");
        let refusal = out
            .report
            .check(&thresholds(1.0, 0.999, 1.0, 100.0))
            .unwrap_err();
        assert_eq!(
            (refusal.code.as_str(), refusal.subject.as_str()),
            ("SEARCH_TASK_SHORTCUT", "co_coverage")
        );

        // Exposed constant: the answer name is two tokens and one clue shares
        // exactly one, so the value is 0.5.
        let exposed = EvidenceGraph {
            nodes: vec![
                node("a", "alpha beta"),
                node("e1", "node one"),
                node("e2", "alpha gamma"),
            ],
            edges: vec![edge("a", "e1"), edge("e1", "e2")],
        };
        let out =
            synthesize(&exposed, "a", &thresholds(1.0, 1.0, 0.5, 100.0), 0).expect("synthesis");
        assert_eq!(
            finding(&out.report, ShortcutDetector::ExposedConstant).value,
            0.5
        );
        out.report
            .check(&thresholds(1.0, 1.0, 0.5, 100.0))
            .expect("a value equal to the threshold passes");
        let refusal = out
            .report
            .check(&thresholds(1.0, 1.0, 0.4999, 100.0))
            .unwrap_err();
        assert_eq!(
            (refusal.code.as_str(), refusal.subject.as_str()),
            ("SEARCH_TASK_SHORTCUT", "exposed_constant")
        );

        // Prior knowledge: the answer has degree five, so the value is 5.0.
        let prior = EvidenceGraph {
            nodes: vec![
                node("a", "hub"),
                node("n1", "n1"),
                node("n2", "n2"),
                node("n3", "n3"),
                node("n4", "n4"),
                node("n5", "n5"),
            ],
            edges: vec![
                edge("a", "n1"),
                edge("a", "n2"),
                edge("a", "n3"),
                edge("a", "n4"),
                edge("a", "n5"),
            ],
        };
        let out = synthesize(&prior, "a", &thresholds(1.0, 1.0, 1.0, 5.0), 0).expect("synthesis");
        assert_eq!(
            finding(&out.report, ShortcutDetector::PriorKnowledge).value,
            5.0
        );
        out.report
            .check(&thresholds(1.0, 1.0, 1.0, 5.0))
            .expect("a value equal to the threshold passes");
        let refusal = out
            .report
            .check(&thresholds(1.0, 1.0, 1.0, 4.9))
            .unwrap_err();
        assert_eq!(
            (refusal.code.as_str(), refusal.subject.as_str()),
            ("SEARCH_TASK_SHORTCUT", "prior_knowledge")
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R702.2
    #[test]
    fn detector_complexity_on_10_1000_100000_items() {
        use std::time::Instant;
        let mut times = Vec::new();
        for size in [10, 1_000, 100_000] {
            // A chain of `size` nodes: the answer n000000 followed by n000001….
            let nodes = (0..size)
                .map(|i| node(&format!("n{i:06}"), &format!("label {i}")))
                .collect::<Vec<_>>();
            let edges = (0..size - 1)
                .map(|i| {
                    let next = i + 1;
                    edge(&format!("n{i:06}"), &format!("n{next:06}"))
                })
                .collect::<Vec<_>>();
            let graph = EvidenceGraph { nodes, edges };
            let lenient = thresholds(1.0, 1.0, 1.0, f64::MAX);
            let started = Instant::now();
            let out = synthesize(&graph, "n000000", &lenient, 0).expect("synthesis");
            times.push(started.elapsed());
            assert_eq!(out.report.findings.len(), 4);
            assert_eq!(out.task.answer_node, "n000000");
            assert_eq!(out.task.clues.len(), size - 1);
        }
        // A linear pass grows at most linearly: 100x the items must not cost
        // the 10,000x of a quadratic one; the loose cap tolerates constant
        // overhead at the small sizes.
        assert!(
            times[2].as_secs_f64() < 500.0 * times[1].as_secs_f64() + 2.0,
            "the detector pass went superlinear: {times:?}"
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R702.2
    #[test]
    fn refinement_masks_an_exposed_constant() {
        // The end clue exposes half of the two-token answer name; one masking
        // round removes it, after which the report is clean at the thresholds.
        let graph = EvidenceGraph {
            nodes: vec![
                node("a", "alpha beta"),
                node("e1", "node one"),
                node("e2", "alpha gamma"),
            ],
            edges: vec![edge("a", "e1"), edge("e1", "e2")],
        };
        let thresholds = thresholds(1.0, 1.0, 0.4999, 100.0);
        let out = synthesize(&graph, "a", &thresholds, 2).expect("synthesis");
        assert_eq!(out.rounds_used, 1);
        assert!(out.task.clues.iter().any(|c| c.text.contains('•')));
        assert!(out.task.clues.iter().all(|c| {
            !c.text
                .split(|ch: char| !ch.is_alphanumeric())
                .any(|t| !t.is_empty() && t.to_lowercase() == "alpha")
        }));
        out.report
            .check(&thresholds)
            .expect("refined task passes every detector");
    }

    // spec: MEDULLA-FEDERATED-QUERY-R702.2
    #[test]
    fn refinement_adds_a_clue_for_single_clue_risk() {
        // Two of the three chain clues are decisive (2/3 over the 0.5
        // threshold); adding the disjoint node `free` — which carries two
        // non-evidence neighbours — dilutes the fraction to 1/2, which
        // passes at the threshold.
        let graph = EvidenceGraph {
            nodes: vec![
                node("a", "hub"),
                node("e1", "one"),
                node("e2", "two"),
                node("e3", "three"),
                node("free", "free"),
                node("g", "g"),
                node("h", "h"),
            ],
            edges: vec![
                edge("a", "e1"),
                edge("e1", "e2"),
                edge("e1", "e3"),
                edge("free", "g"),
                edge("free", "h"),
            ],
        };
        let thresholds = thresholds(0.5, 1.0, 1.0, 100.0);
        let out = synthesize(&graph, "a", &thresholds, 2).expect("synthesis");
        assert_eq!(out.rounds_used, 1);
        assert!(out
            .task
            .clues
            .iter()
            .any(|c| c.evidence_nodes == vec!["free"]));
        assert_eq!(
            finding(&out.report, ShortcutDetector::SingleClue).value,
            0.5
        );
        out.report
            .check(&thresholds)
            .expect("refined task passes every detector");
    }
}
