//! Ranked fallback for unbound steps (MEDULLA-CQ-ORCH-R704.3).
//!
//! A step the graph does not bind asks the `ToolRelevance` family with the step label and its
//! lane. The top tool binds when its relevance reaches the minimum score; otherwise the step
//! stays unbound and `PROCESS_STEP_UNBOUND` names it later (R704.6). Only a canonical step kind
//! and the step's label and lane are read.

use std::collections::BTreeSet;

use medulla_decision_types::decision::process_plan::{BindingSource, BindingTarget, StepBinding};
use medulla_decision_types::decision::tool_relevance::AssignedTool;
use medulla_types::process_import::{CanonicalStep, StepKind};

/// The default of `knob process_plan.min_tool_score`, in millionths (0.6).
pub const DEFAULT_MIN_TOOL_SCORE_MICROS: u64 = 600_000;

/// What the `ToolRelevance` family answers for one step: its ranked tools, best first.
pub type RelevanceAnswer = Vec<AssignedTool>;

/// Whether `relevance_q32` (a Q32 fraction) reaches `min_micros` millionths.
fn reaches(relevance_q32: i64, min_micros: u64) -> bool {
    i128::from(relevance_q32) * 1_000_000 >= i128::from(min_micros) << 32
}

/// Bind each agent step in `steps` that `explicit` does not bind to the top tool `relevance`
/// ranks for its label and lane, when that tool's relevance is at least `min_micros`
/// (`knob process_plan.min_tool_score`). `relevance(label, lane)` is the `ToolRelevance`
/// family's ranked answer. A step of any other kind has its own binding (R704.2) and is never
/// ranked. The result is in step order; each binding's source is `ranked` with the tool's
/// relevance as `score_q32`.
pub fn ranked_bindings(
    steps: &[CanonicalStep],
    explicit: &[StepBinding],
    min_micros: u64,
    relevance: &dyn Fn(&str, &str) -> RelevanceAnswer,
) -> Vec<StepBinding> {
    let bound: BTreeSet<&str> = explicit.iter().map(|b| b.step_id.as_str()).collect();
    let mut out = Vec::new();
    for step in steps {
        if step.kind != StepKind::Agent || bound.contains(step.id.as_str()) {
            continue;
        }
        let lane = step.role.as_deref().unwrap_or_default();
        let Some(top) = relevance(&step.label, lane).into_iter().min_by_key(|tool| tool.rank)
        else {
            continue;
        };
        if !reaches(top.relevance_q32, min_micros) {
            continue;
        }
        out.push(StepBinding {
            step_id: step.id.clone(),
            target: BindingTarget::Tool {
                server: top.server_id.clone(),
                tool: top.tool_id.clone(),
                contract_iri: top.tool_id.clone(),
            },
            source: BindingSource::Ranked {
                score_q32: top.relevance_q32,
            },
        });
    }
    out
}
