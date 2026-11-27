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
//!
//! A subtask declares its own budget through an option whose first
//! classification is `subtask` (text `subtask`, number `budget`); spent options
//! name the subtask that consumed them. A declared subtask with nothing left
//! abstains `RESEARCH_NO_BUDGET`, a frontier node deeper than
//! `research.depth_limit` abstains `RESEARCH_DEPTH_EXCEEDED` (R705.4). The
//! sealed record carries the spent units and the remaining budget per subtask
//! as claim premises (R705.5).

use std::collections::BTreeMap;

use medulla_decision_types::decision::research_voi::{
    ResearchAnswer, ResearchRead, ResearchStop, RESEARCH_DEPTH_EXCEEDED, RESEARCH_NO_BUDGET,
    SPENT_CLASSIFICATION, STOP_OPTION_ID, SUBTASK_CLASSIFICATION,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, QuestionSafety, ScoredOption, StatisticalOutcome,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, QuantScaleTag, QuantisedValue, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;
use medulla_types::evidence_class::EvidenceClass;
use medulla_types::refusal_code::RefusalCode;
use {
    medulla_decision_stats_records::decision::research_voi::expand_frontier,
    medulla_decision_stats_records::decision::research_voi::q32_to_micro,
    medulla_decision_stats_records::decision::research_voi::FrontierLimits,
    medulla_decision_stats_records::decision::research_voi::FrontierStop,
    medulla_decision_stats_records::decision::research_voi::SourceNode,
};

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

fn first_class(option: &DeclaredOption) -> Option<&str> {
    option.classification.iter().next().map(String::as_str)
}

fn text<'a>(option: &'a DeclaredOption, key: &str) -> Option<&'a str> {
    option
        .texts
        .iter()
        .find(|t| t.key == key)
        .map(|t| t.text.as_str())
}

/// The subtask a spent or budget option names; both must name one.
fn named_subtask(option: &DeclaredOption) -> Result<String, String> {
    text(option, "subtask")
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            invalid_parameter(format!(
                "{KIND} option {} names no subtask",
                option.option_id
            ))
        })
}

/// The declared options: frontier nodes with their subtask and depth, and the
/// units already spent.
struct Parsed {
    nodes: BTreeMap<String, SourceNode>,
    subtasks: BTreeMap<String, String>,
    depths: BTreeMap<String, u8>,
    spent_micro: u64,
    limit_micro: u64,
    /// Micro-units spent per subtask, as reported by the caller.
    spent_by_subtask: BTreeMap<String, u64>,
    /// Declared per-subtask budgets, micro-units.
    budgets: BTreeMap<String, u64>,
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
        spent_by_subtask: BTreeMap::new(),
        budgets: BTreeMap::new(),
    };
    for option in options.iter() {
        if option.option_id == STOP_OPTION_ID {
            return Err(invalid_parameter(format!(
                "{KIND} reserves the option id {STOP_OPTION_ID}"
            )));
        }
        if first_class(option) == Some(SPENT_CLASSIFICATION) {
            let units = q32_to_micro(number(option, "cost")?);
            parsed.spent_micro = parsed.spent_micro.saturating_add(units);
            let held = parsed
                .spent_by_subtask
                .entry(named_subtask(option)?)
                .or_default();
            *held = held.saturating_add(units);
            continue;
        }
        if first_class(option) == Some(SUBTASK_CLASSIFICATION) {
            let name = named_subtask(option)?;
            let budget = q32_to_micro(number(option, "budget")?);
            if parsed.budgets.insert(name.clone(), budget).is_some() {
                return Err(invalid_parameter(format!(
                    "{KIND} declares the budget of subtask {name} twice"
                )));
            }
            continue;
        }
        let depth = u8::try_from((number(option, "depth")? >> 32).clamp(0, i64::from(u8::MAX)))
            .unwrap_or(u8::MAX);
        parsed.depths.insert(option.option_id.clone(), depth);
        parsed.subtasks.insert(
            option.option_id.clone(),
            text(option, "subtask").unwrap_or_default().to_string(),
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

/// An abstention of the family: a sealed record with the registered code.
fn abstained(code: RefusalCode, detail: String) -> Result<Executed, String> {
    let reasons = BoundedVec::new(vec![AbstainReason::Family {
        family: QuestionKind::ResearchNext,
        code,
        detail,
    }])
    .map_err(|detail| invalid_parameter(format!("{KIND}: {detail}")))?;
    Ok(Executed::from_family(
        StatisticalOutcome::Abstained { reasons },
        None,
    ))
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
            if let Some((source_id, depth)) = parsed
                .depths
                .iter()
                .find(|(_, depth)| u64::from(**depth) > knobs.research_depth_limit)
            {
                return abstained(
                    RESEARCH_DEPTH_EXCEEDED,
                    format!(
                        "source {source_id} lies {depth} links deep, past the depth limit {}",
                        knobs.research_depth_limit
                    ),
                );
            }
            if let Some((subtask, _)) = parsed.budgets.iter().find(|(subtask, budget)| {
                parsed.spent_by_subtask.get(*subtask).copied().unwrap_or(0) >= **budget
            }) {
                return abstained(
                    RESEARCH_NO_BUDGET,
                    format!("subtask {subtask} has no remaining budget"),
                );
            }
            let seeds: Vec<String> = parsed.nodes.keys().cloned().collect();
            let walk = expand_frontier(&seeds, &parsed.nodes, &limits);
            let mut remaining = parsed.limit_micro.saturating_sub(parsed.spent_micro);
            let mut subtask_left: BTreeMap<String, u64> = parsed
                .budgets
                .iter()
                .map(|(subtask, budget)| {
                    let spent = parsed.spent_by_subtask.get(subtask).copied().unwrap_or(0);
                    (subtask.clone(), budget.saturating_sub(spent))
                })
                .collect();
            let mut stop = stop_reason(walk.stop);
            let mut reads = Vec::new();
            for read in walk.reads {
                let cost_micro = parsed
                    .nodes
                    .get(&read.source_id)
                    .map_or(0, |n| q32_to_micro(n.cost_q32));
                let subtask = parsed
                    .subtasks
                    .get(&read.source_id)
                    .cloned()
                    .unwrap_or_default();
                let subtask_fits = subtask_left
                    .get(&subtask)
                    .is_none_or(|left| cost_micro <= *left);
                if cost_micro > remaining || !subtask_fits {
                    stop = ResearchStop::Budget;
                    break;
                }
                remaining -= cost_micro;
                if let Some(left) = subtask_left.get_mut(&subtask) {
                    *left -= cost_micro;
                }
                reads.push(ResearchRead {
                    subtask,
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
            let mut premises = vec![
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
            // The ledger per subtask (R705.5): what each subtask consumed, and
            // what each declared subtask budget has left after the reads.
            for (subtask, units) in &parsed.spent_by_subtask {
                premises.push(claim(
                    KIND,
                    format!("spent:{subtask}={units}"),
                    "candidates.spent",
                ));
            }
            for (subtask, left) in &subtask_left {
                premises.push(claim(
                    KIND,
                    format!("remaining:{subtask}={left}"),
                    "candidates.subtask",
                ));
            }
            Ok(Executed::from_family(
                StatisticalOutcome::advisory(scores, false),
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

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        match outcome {
            StatisticalOutcome::Abstained { .. } => Some(ResolutionKind::Abstention),
            _ => Some(ResolutionKind::Statistical),
        }
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        match outcome {
            StatisticalOutcome::Abstained { .. } => None,
            _ => Some(EvidenceClass::Claim),
        }
    }
}
