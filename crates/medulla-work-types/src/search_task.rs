//! Search-task synthesis records with shortcut-risk diagnostics
//! (MEDULLA-FEDERATED-QUERY-R702.1, R702.4).
//!
//! A [`SearchTask`] is a multi-hop question formulated from an
//! [`EvidenceGraph`]. A [`RiskReport`] lists every shortcut detector's value
//! and the nodes involved; [`RiskReport::check`] refuses a task whose value
//! for any detector exceeds its threshold, naming the detector.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// The record names a node or clue shape the graph does not support.
pub const SEARCH_TASK_INVALID: &str = "SEARCH_TASK_INVALID";
/// A detector value exceeded its threshold.
pub const SEARCH_TASK_SHORTCUT: &str = "SEARCH_TASK_SHORTCUT";

/// The four shortcut risks. Closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ShortcutDetector {
    /// One clue alone selects the answer.
    SingleClue,
    /// One clue's evidence also covers other clues.
    CoCoverage,
    /// The question exposes a constant that names the answer.
    ExposedConstant,
    /// The answer is reachable from prior knowledge alone.
    PriorKnowledge,
}

impl ShortcutDetector {
    pub const ALL: [Self; 4] = [
        Self::SingleClue,
        Self::CoCoverage,
        Self::ExposedConstant,
        Self::PriorKnowledge,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::SingleClue => "single_clue",
            Self::CoCoverage => "co_coverage",
            Self::ExposedConstant => "exposed_constant",
            Self::PriorKnowledge => "prior_knowledge",
        }
    }
}

/// A typed refusal naming the offending id or detector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[error("{code}: {subject}")]
pub struct SearchTaskRefusal {
    pub code: String,
    pub subject: String,
}

fn refuse(code: &str, subject: &str) -> SearchTaskRefusal {
    SearchTaskRefusal {
        code: code.to_string(),
        subject: subject.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct EvidenceNode {
    pub id: String,
    pub label: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct EvidenceEdge {
    pub from: String,
    pub to: String,
    pub relation: String,
}

/// The evidence subgraph a task is formulated from (MEDULLA-FEDERATED-QUERY-R702.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct EvidenceGraph {
    pub nodes: Vec<EvidenceNode>,
    pub edges: Vec<EvidenceEdge>,
}

impl EvidenceGraph {
    fn ids(&self) -> Result<BTreeSet<&str>, SearchTaskRefusal> {
        let mut ids = BTreeSet::new();
        for node in &self.nodes {
            if node.id.trim().is_empty() || !ids.insert(node.id.as_str()) {
                return Err(refuse(SEARCH_TASK_INVALID, &node.id));
            }
        }
        for edge in &self.edges {
            for end in [&edge.from, &edge.to] {
                if !ids.contains(end.as_str()) {
                    return Err(refuse(SEARCH_TASK_INVALID, end));
                }
            }
        }
        Ok(ids)
    }
}

/// One clue and the evidence nodes it rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Clue {
    pub text: String,
    pub evidence_nodes: Vec<String>,
}

/// A multi-hop search question (MEDULLA-FEDERATED-QUERY-R702.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SearchTask {
    pub question: String,
    pub answer_node: String,
    pub clues: Vec<Clue>,
    pub max_refinement_rounds: u32,
}

impl SearchTask {
    /// Refuse a task whose answer or clue evidence is not in `graph`, or that
    /// has fewer than two clues (a single-hop question is not a search task).
    pub fn validate(&self, graph: &EvidenceGraph) -> Result<(), SearchTaskRefusal> {
        let ids = graph.ids()?;
        if self.question.trim().is_empty() || self.clues.len() < 2 {
            return Err(refuse(SEARCH_TASK_INVALID, "clues"));
        }
        if !ids.contains(self.answer_node.as_str()) {
            return Err(refuse(SEARCH_TASK_INVALID, &self.answer_node));
        }
        for clue in &self.clues {
            if clue.evidence_nodes.is_empty() {
                return Err(refuse(SEARCH_TASK_INVALID, &clue.text));
            }
            if let Some(missing) = clue
                .evidence_nodes
                .iter()
                .find(|n| !ids.contains(n.as_str()))
            {
                return Err(refuse(SEARCH_TASK_INVALID, missing));
            }
        }
        Ok(())
    }
}

/// One detector's value and the nodes that produced it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RiskFinding {
    pub detector: ShortcutDetector,
    pub value: f64,
    pub nodes: Vec<String>,
}

/// Per-detector thresholds a task must stay within.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RiskThresholds {
    pub single_clue: f64,
    pub co_coverage: f64,
    pub exposed_constant: f64,
    pub prior_knowledge: f64,
}

impl RiskThresholds {
    fn get(&self, detector: ShortcutDetector) -> f64 {
        match detector {
            ShortcutDetector::SingleClue => self.single_clue,
            ShortcutDetector::CoCoverage => self.co_coverage,
            ShortcutDetector::ExposedConstant => self.exposed_constant,
            ShortcutDetector::PriorKnowledge => self.prior_knowledge,
        }
    }

    /// Whether a detector's value breaches its threshold. The exposed-constant
    /// and prior-knowledge detectors flag a value at least the threshold
    /// (MEDULLA-FEDERATED-QUERY-R702.2: "shares at least", "degree is at
    /// least"); the single-clue and co-coverage detectors flag a value beyond
    /// it.
    pub fn breached(&self, detector: ShortcutDetector, value: f64) -> bool {
        let limit = self.get(detector);
        match detector {
            ShortcutDetector::ExposedConstant | ShortcutDetector::PriorKnowledge => value >= limit,
            ShortcutDetector::SingleClue | ShortcutDetector::CoCoverage => value > limit,
        }
    }
}

/// Every detector's value for one task (MEDULLA-FEDERATED-QUERY-R702.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RiskReport {
    pub findings: Vec<RiskFinding>,
}

impl RiskReport {
    /// Refuse a report that omits a detector, repeats one or carries a
    /// non-finite value, then refuse the task if any value exceeds its
    /// threshold (MEDULLA-FEDERATED-QUERY-R702.4).
    pub fn check(&self, thresholds: &RiskThresholds) -> Result<(), SearchTaskRefusal> {
        let detectors: BTreeSet<ShortcutDetector> =
            self.findings.iter().map(|f| f.detector).collect();
        if detectors.len() != self.findings.len() || detectors.len() != ShortcutDetector::ALL.len()
        {
            return Err(refuse(SEARCH_TASK_INVALID, "findings"));
        }
        if let Some(bad) = self.findings.iter().find(|f| !f.value.is_finite()) {
            return Err(refuse(SEARCH_TASK_INVALID, bad.detector.name()));
        }
        match self.first_over(thresholds) {
            Some(finding) => Err(refuse(SEARCH_TASK_SHORTCUT, finding.detector.name())),
            None => Ok(()),
        }
    }

    /// The first finding whose value exceeds its threshold.
    fn first_over(&self, thresholds: &RiskThresholds) -> Option<&RiskFinding> {
        self.findings
            .iter()
            .find(|f| thresholds.breached(f.detector, f.value))
    }

    /// The typed rejection of a task over a threshold: the shortcut code, the
    /// detector that tripped and this complete report with the thresholds it was
    /// judged against, so a rejected task shows every detector value and node id
    /// exactly like an accepted one (MEDULLA-FEDERATED-QUERY-R702.5). `None` when
    /// no value is over its threshold.
    pub fn rejection(&self, thresholds: &RiskThresholds) -> Option<SearchTaskRejection> {
        self.first_over(thresholds)
            .map(|finding| SearchTaskRejection {
                code: SEARCH_TASK_SHORTCUT.to_string(),
                detector: finding.detector,
                report: self.clone(),
                thresholds: thresholds.clone(),
            })
    }
}

/// Version of the `SynthesizeSearchTask` request and result shapes.
pub const SEARCH_TASK_SCHEMA_VERSION: u16 = 1;

/// One served synthesis (MEDULLA-FEDERATED-QUERY-R702.3): the task is built
/// around `answer_node` from the evidence the caller may read in `graph`.
/// `tenant_id` must be the verified tenant of the call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SynthesizeSearchTaskRequest {
    pub schema_version: u16,
    pub tenant_id: String,
    pub graph: String,
    pub answer_node: String,
}

/// An accepted task with the complete detector report and the thresholds it
/// was judged against. The engine stores nothing: the caller turns accepted
/// tasks into pinned query-set items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SynthesizeSearchTaskResult {
    pub task: SearchTask,
    pub report: RiskReport,
    pub thresholds: RiskThresholds,
    pub rounds_used: u32,
    /// Evidence nodes the caller could read and the task was built from.
    pub evidence_nodes: u64,
}

/// A task refused for a shortcut: the stable refusal code, the detector that
/// tripped and the complete report (MEDULLA-FEDERATED-QUERY-R702.5). It is a
/// typed value of the served answer, never text inside an error message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SearchTaskRejection {
    pub code: String,
    pub detector: ShortcutDetector,
    pub report: RiskReport,
    pub thresholds: RiskThresholds,
}

/// The served answer of `SynthesizeSearchTask`: an accepted task or the typed
/// rejection of a risky one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SynthesizeSearchTaskAnswer {
    Accepted(SynthesizeSearchTaskResult),
    Rejected(SearchTaskRejection),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> EvidenceGraph {
        let node = |id: &str| EvidenceNode {
            id: id.into(),
            label: "Entity".into(),
            text: format!("text {id}"),
        };
        EvidenceGraph {
            nodes: vec![node("a"), node("b"), node("ans")],
            edges: vec![EvidenceEdge {
                from: "a".into(),
                to: "ans".into(),
                relation: "founded".into(),
            }],
        }
    }

    fn task() -> SearchTask {
        SearchTask {
            question: "Which company did the founder of a start in b?".into(),
            answer_node: "ans".into(),
            clues: vec![
                Clue {
                    text: "founded by a".into(),
                    evidence_nodes: vec!["a".into()],
                },
                Clue {
                    text: "located in b".into(),
                    evidence_nodes: vec!["b".into()],
                },
            ],
            max_refinement_rounds: 2,
        }
    }

    fn report(single: f64) -> RiskReport {
        RiskReport {
            findings: ShortcutDetector::ALL
                .iter()
                .map(|d| RiskFinding {
                    detector: *d,
                    value: if *d == ShortcutDetector::SingleClue {
                        single
                    } else {
                        0.1
                    },
                    nodes: vec!["a".into()],
                })
                .collect(),
        }
    }

    const GOLDEN: &str = r#"{"detector":"co_coverage","value":0.25,"nodes":["a","b"]}"#;

    // spec: MEDULLA-FEDERATED-QUERY-R702.1
    #[test]
    fn records_round_trip_and_reject_invalid_shapes() {
        let finding: RiskFinding = serde_json::from_str(GOLDEN).unwrap();
        assert_eq!(serde_json::to_string(&finding).unwrap(), GOLDEN);
        for value in [
            serde_json::to_value(graph()).unwrap(),
            serde_json::to_value(task()).unwrap(),
            serde_json::to_value(report(0.2)).unwrap(),
        ] {
            let text = value.to_string();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&text).unwrap(),
                value
            );
        }
        assert_eq!(
            serde_json::from_value::<SearchTask>(serde_json::to_value(task()).unwrap()).unwrap(),
            task()
        );
        assert!(serde_json::from_str::<ShortcutDetector>("\"lucky_guess\"").is_err());

        task().validate(&graph()).unwrap();
        let mut dangling = graph();
        dangling.edges[0].to = "ghost".into();
        assert_eq!(task().validate(&dangling).unwrap_err().subject, "ghost");
        let mut single_hop = task();
        single_hop.clues.pop();
        assert_eq!(
            single_hop.validate(&graph()).unwrap_err().code,
            SEARCH_TASK_INVALID
        );
        let mut unknown = task();
        unknown.clues[1].evidence_nodes = vec!["zz".into()];
        assert_eq!(unknown.validate(&graph()).unwrap_err().subject, "zz");
    }

    // spec: MEDULLA-FEDERATED-QUERY-R702.2
    #[test]
    fn exposed_constant_and_prior_knowledge_breach_at_exactly_the_threshold() {
        let thresholds = RiskThresholds {
            single_clue: 0.5,
            co_coverage: 0.5,
            exposed_constant: 0.5,
            prior_knowledge: 50.0,
        };
        assert!(thresholds.breached(ShortcutDetector::ExposedConstant, 0.5));
        assert!(!thresholds.breached(ShortcutDetector::ExposedConstant, 0.4999));
        assert!(thresholds.breached(ShortcutDetector::PriorKnowledge, 50.0));
        assert!(!thresholds.breached(ShortcutDetector::PriorKnowledge, 49.0));
        assert!(!thresholds.breached(ShortcutDetector::SingleClue, 0.5));
        assert!(thresholds.breached(ShortcutDetector::SingleClue, 0.5001));
    }

    // spec: MEDULLA-FEDERATED-QUERY-R702.5
    #[test]
    fn rejection_carries_the_complete_report() {
        let thresholds = RiskThresholds {
            single_clue: 0.5,
            co_coverage: 0.5,
            exposed_constant: 0.5,
            prior_knowledge: 0.5,
        };
        let rejected = report(0.9);
        let rejection = rejected.rejection(&thresholds).unwrap();
        assert_eq!(rejection.code, SEARCH_TASK_SHORTCUT);
        assert_eq!(rejection.detector, ShortcutDetector::SingleClue);
        assert_eq!(rejection.report, rejected);
        assert_eq!(rejection.thresholds, thresholds);
        assert_eq!(rejection.report.findings.len(), ShortcutDetector::ALL.len());
        assert!(rejection
            .report
            .findings
            .iter()
            .all(|f| !f.nodes.is_empty()));
        let answer = SynthesizeSearchTaskAnswer::Rejected(rejection);
        let wire = serde_json::to_value(&answer).unwrap();
        assert_eq!(wire["outcome"], "rejected");
        let back: SynthesizeSearchTaskAnswer = serde_json::from_value(wire).unwrap();
        assert_eq!(back, answer);
        assert!(report(0.1).rejection(&thresholds).is_none());
    }
    // spec: MEDULLA-FEDERATED-QUERY-R702.4
    #[test]
    fn task_over_any_threshold_is_refused_with_the_detector_name() {
        let thresholds = RiskThresholds {
            single_clue: 0.5,
            co_coverage: 0.5,
            exposed_constant: 0.0,
            prior_knowledge: 0.5,
        };
        let mut lenient = thresholds.clone();
        lenient.exposed_constant = 0.2;
        report(0.2).check(&lenient).unwrap();
        let refusal = report(0.9).check(&lenient).unwrap_err();
        assert_eq!(refusal.code, SEARCH_TASK_SHORTCUT);
        assert_eq!(refusal.subject, "single_clue");
        let refusal = report(0.2).check(&thresholds).unwrap_err();
        assert_eq!(refusal.subject, "exposed_constant");

        let mut partial = report(0.2);
        partial.findings.pop();
        assert_eq!(
            partial.check(&lenient).unwrap_err().code,
            SEARCH_TASK_INVALID
        );
        let mut nan = report(0.2);
        nan.findings[0].value = f64::NAN;
        assert_eq!(nan.check(&lenient).unwrap_err().code, SEARCH_TASK_INVALID);
    }
}
