//! The `research_next` question family (MEDULLA-DECISION-ENGINE-R001.101, R705.3).
//!
//! Frontier sources are `Declared` options: `option_id` is the source id, the
//! numbers `p_useful`, `gain`, `cost` (Q32) and `depth` (whole links, Q32) are
//! the caller's claims, and the text `subtask` names the subtask that wants the
//! read. An option whose first classification is `spent` reports units already
//! spent (its `cost`). The pure `expand_frontier` kernel orders the reads by
//! expected value under the registry knobs `research.node_budget`,
//! `research.depth_limit` and `research.value_floor_micro`; reads are kept while
//! their cost fits the remaining spend budget (`budget_limit`, micro-units, less
//! what was spent). The answer is an uncalibrated advisory: one option per read,
//! scored by its expected value, and one option `stop` at `1 << 32`. Language
//! model calls stay outside medulla and nothing is written.

use std::collections::BTreeMap;

use medulla_decision_stats::decision::research_voi::{
    expand_frontier, q32_to_micro, FrontierLimits, FrontierStop, SourceNode,
};
use medulla_decision_types::decision::research_voi::{
    ResearchAnswer, ResearchRead, ResearchStop, SPENT_CLASSIFICATION, STOP_OPTION_ID,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionSafety, ScoredOption, StatisticalOutcome,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, QuantScaleTag, QuantisedValue, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::{Executed, ExecutionContext, FamilyExecuted};
use crate::decide::stat_support::invalid_parameter;

use super::kernel::claim;
use super::support::unsigned_param;
use super::DecisionFamily;

const KIND: &str = "research_next";
const BUDGET_PARAM: &str = "budget_limit";
const STOP_SCORE: i64 = 1 << 32;

fn number(option: &DeclaredOption, key: &str) -> Result<i64, String> {
    option
        .numbers
        .iter()
        .find(|n| n.key == key)
        .map(|n| n.q32)
        .ok_or_else(|| {
            invalid_parameter(format!(
                "{KIND} option {} declares no number {key}",
                option.option_id
            ))
        })
}

fn is_spent(option: &DeclaredOption) -> bool {
    option.classification.iter().next().map(String::as_str) == Some(SPENT_CLASSIFICATION)
}

/// The declared options: frontier nodes with their subtask and depth, and the
/// units already spent.
struct Parsed {
    nodes: BTreeMap<String, SourceNode>,
    subtasks: BTreeMap<String, String>,
    depths: BTreeMap<String, u8>,
    spent_micro: u64,
    limit_micro: u64,
}

fn parse(request: &DecideRequest) -> Result<Parsed, String> {
    if request.question.safety != QuestionSafety::Ordinary
        || request.head.is_some()
        || !request.belief_as_of.is_empty()
    {
        return Err(invalid_parameter(format!(
            "{KIND} needs ordinary safety, no head and no belief slices"
        )));
    }
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid_parameter(format!("{KIND} declares its candidates")));
    };
    if request.params.iter().any(|p| p.name != BUDGET_PARAM) {
        return Err(invalid_parameter(format!(
            "{KIND} takes one parameter, {BUDGET_PARAM}"
        )));
    }
    let limit_micro = unsigned_param(request, BUDGET_PARAM)
        .map_err(|_| invalid_parameter(format!("{KIND} parameter {BUDGET_PARAM} is invalid")))?
        .ok_or_else(|| invalid_parameter(format!("{KIND} needs the parameter {BUDGET_PARAM}")))?;
    let mut parsed = Parsed {
        nodes: BTreeMap::new(),
        subtasks: BTreeMap::new(),
        depths: BTreeMap::new(),
        spent_micro: 0,
        limit_micro,
    };
    for option in options.iter() {
        if option.option_id == STOP_OPTION_ID {
            return Err(invalid_parameter(format!(
                "{KIND} reserves the option id {STOP_OPTION_ID}"
            )));
        }
        if is_spent(option) {
            parsed.spent_micro = parsed
                .spent_micro
                .saturating_add(q32_to_micro(number(option, "cost")?));
            continue;
        }
        let depth = u8::try_from((number(option, "depth")? >> 32).clamp(0, i64::from(u8::MAX)))
            .unwrap_or(u8::MAX);
        parsed.depths.insert(option.option_id.clone(), depth);
        parsed.subtasks.insert(
            option.option_id.clone(),
            option
                .texts
                .iter()
                .find(|t| t.key == "subtask")
                .map(|t| t.text.clone())
                .unwrap_or_default(),
        );
        parsed.nodes.insert(
            option.option_id.clone(),
            SourceNode {
                source_id: option.option_id.clone(),
                p_useful_q32: number(option, "p_useful")?,
                gain_q32: number(option, "gain")?,
                cost_q32: number(option, "cost")?,
                links: Vec::new(),
            },
        );
    }
    Ok(parsed)
}

fn stop_reason(stop: FrontierStop) -> ResearchStop {
    match stop {
        FrontierStop::Exhausted => ResearchStop::Exhausted,
        FrontierStop::NodeBudget => ResearchStop::NodeBudget,
        FrontierStop::ValueFloor => ResearchStop::ValueFloor,
    }
}

fn q32(value: i64) -> QuantisedValue {
    QuantisedValue {
        scale: QuantScaleTag::Q32,
        value,
    }
}

/// The family of the `research_next` question kind.
pub struct ResearchVoiFamily;

impl DecisionFamily for ResearchVoiFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        parse(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(parse(request).and_then(|parsed| {
            let knobs = medulla_server_api::knob_config::knobs();
            let limits = FrontierLimits {
                node_budget: knobs.research_node_budget,
                depth_limit: u8::try_from(knobs.research_depth_limit).unwrap_or(u8::MAX),
                value_floor_micro: knobs.research_value_floor_micro,
            };
            let seeds: Vec<String> = parsed.nodes.keys().cloned().collect();
            let walk = expand_frontier(&seeds, &parsed.nodes, &limits);
            let mut remaining = parsed.limit_micro.saturating_sub(parsed.spent_micro);
            let mut stop = stop_reason(walk.stop);
            let mut reads = Vec::new();
            for read in walk.reads {
                let cost_micro = parsed
                    .nodes
                    .get(&read.source_id)
                    .map_or(0, |n| q32_to_micro(n.cost_q32));
                if cost_micro > remaining {
                    stop = ResearchStop::Budget;
                    break;
                }
                remaining -= cost_micro;
                reads.push(ResearchRead {
                    subtask: parsed
                        .subtasks
                        .get(&read.source_id)
                        .cloned()
                        .unwrap_or_default(),
                    depth: parsed.depths.get(&read.source_id).copied().unwrap_or(0),
                    expected_value_q32: read.expected_value_q32,
                    cost_micro,
                    source_id: read.source_id,
                });
            }
            let mut scores: Vec<ScoredOption> = reads
                .iter()
                .map(|read| ScoredOption {
                    option_id: read.source_id.clone(),
                    score: q32(read.expected_value_q32),
                    probability: None,
                })
                .collect();
            scores.push(ScoredOption {
                option_id: STOP_OPTION_ID.to_string(),
                score: q32(STOP_SCORE),
                probability: None,
            });
            let scores = BoundedVec::new(scores)
                .map_err(|detail| invalid_parameter(format!("{KIND}: {detail}")))?;
            let field = "params.budget_limit";
            let premises = vec![
                claim(
                    KIND,
                    format!("research:budget_limit={}", parsed.limit_micro),
                    field,
                ),
                claim(
                    KIND,
                    format!("research:spent={}", parsed.spent_micro),
                    "candidates.spent",
                ),
            ];
            Ok(Executed::from_family(
                StatisticalOutcome::Advisory {
                    scores,
                    calibrated: false,
                },
                Some(FamilyExecuted {
                    answer: Some(FamilyAnswer::Research(Box::new(ResearchAnswer {
                        reads,
                        stop,
                        spent_micro: parsed.spent_micro,
                        remaining_micro: remaining,
                    }))),
                    premises,
                    resolution: None,
                    evidence: Some(EvidenceClass::Claim),
                }),
            ))
        }))
    }

    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(ResolutionKind::Statistical)
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Claim)
    }
}
