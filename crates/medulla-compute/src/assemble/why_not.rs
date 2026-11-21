//! Why-not explanations (DECIDE-LAYER-DESIGN §7.4, EH-008).
//!
//! An explanation is not prose: it is a re-solve with the excluded option
//! FORCED in, recorded as the objective that forcing costs or as the fact
//! that forcing it leaves no feasible answer. Each costs one bounded solve,
//! so they are budgeted: at most `limit` per slot, taken in order of the
//! option's own objective weight (cheapest first, ties by candidate order).
//! Options step 1b removed are not re-solved -- their violation is already in
//! the record's `eliminated` list.

use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_decision_types::decision::statistical::log::{
    OptionExplanation, WhyNotReason, WhyNotVerdict,
};
use medulla_decision_types::decision::{CandidateFacts, DecisionInputs};
use medulla_decision_types::{decision::Violation, decision::WhyNot};
use medulla_decision_types::{
    solve::ConstraintBody, solve::ConstraintSpec, solve::SolveStatus, solve::VarId,
};

use super::agent::slot_name;
use super::conclude::slot_label;
use super::model::var_name;
use super::search::{search, SolveContext};
use super::{agent, conclude, model, topology, AssembleError};
use medulla_decision_policy::{solve::solve, solve::Model};

/// The slot order explanations are produced in.
const SLOTS: [AgentComponentKind; 6] = [
    AgentComponentKind::ModelProfile,
    AgentComponentKind::SystemPrompt,
    AgentComponentKind::Tool,
    AgentComponentKind::Toolset,
    AgentComponentKind::Skill,
    AgentComponentKind::Ontology,
];

/// Most explanations one record carries in total.
const MAX_WHY_NOT: usize = 64;

pub(super) fn explain(
    context: &SolveContext<'_>,
    model: &Model,
    selected: &[bool],
    limit: u8,
) -> Vec<WhyNot> {
    let mut out = Vec::new();
    for kind in SLOTS {
        let mut excluded: Vec<VarId> = (0..context.built.vars.len().min(selected.len()))
            .map(|position| VarId(position as u32))
            .filter(|var| !selected[var.index()] && context.kind_of(*var) == kind)
            .collect();
        excluded.sort_by_key(|var| (model.weights()[var.index()], *var));
        for var in excluded.into_iter().take(usize::from(limit)) {
            if out.len() >= MAX_WHY_NOT {
                return out;
            }
            out.push(forced(context, var, kind).0);
        }
    }
    out
}

/// Re-solve with `var` forced in; the flag is true when the forced solve ran
/// out of its node budget before it proved anything.
fn forced(context: &SolveContext<'_>, var: VarId, kind: AgentComponentKind) -> (WhyNot, bool) {
    let placed = context.built.vars[var.index()];
    let mut spec = context.built.spec.clone();
    spec.constraints.push(ConstraintSpec {
        label: format!("why-not:{}", var_name(placed)),
        body: ConstraintBody::Fix { var, value: true },
    });
    let certificate = Model::try_from(spec).map(|model| solve(&model, &context.config));
    let mut exhausted = false;
    let (forced_objective, violation) = match certificate {
        Ok(certificate) => match certificate.outcome {
            SolveStatus::Infeasible { .. }
            | SolveStatus::InfeasibleByDeterministicSearch { .. } => {
                (None, Some(Violation::InfeasibleWhenForced))
            }
            SolveStatus::Optimal
            | SolveStatus::OptimalByDeterministicSearch { .. }
            | SolveStatus::FeasibleWithGap { .. }
            | SolveStatus::BudgetExhausted => {
                exhausted = matches!(certificate.outcome, SolveStatus::BudgetExhausted);
                (
                    certificate.incumbent.map(|incumbent| incumbent.objective),
                    None,
                )
            }
        },
        Err(_) => (None, None),
    };
    let why_not = WhyNot {
        component_id: context.candidate(var).component_id.clone(),
        slot: slot_label(context.template, placed.slot, slot_name(kind)),
        forced_objective,
        violation,
    };
    (why_not, exhausted)
}

/// Why one named option is not in the assembly the inputs decide
/// (MEDULLA-DECISION-ENGINE-R870.3): the ladder is re-run on the stored inputs,
/// and the option is forced in by the same re-solve `explain` uses. `clock`
/// reads milliseconds; the compute crate reads no wall clock itself. `None`
/// when the inputs hold no such candidate.
pub fn explain_option(
    inputs: &DecisionInputs,
    component_id: &str,
    budget_ms: u32,
    clock: &dyn Fn() -> u64,
) -> Result<Option<OptionExplanation>, AssembleError> {
    let started = clock();
    let Some(position) = inputs
        .candidates
        .as_slice()
        .iter()
        .position(|candidate| candidate.component_id == component_id)
    else {
        return Ok(None);
    };
    let used = |reasons: Vec<WhyNotReason>, verdict: WhyNotVerdict, exhausted: bool| {
        let spent = clock().saturating_sub(started);
        Some(OptionExplanation {
            verdict,
            reasons,
            budget_used_ms: spent.min(u64::from(u32::MAX)) as u32,
            budget_exhausted: exhausted || spent >= u64::from(budget_ms),
        })
    };
    let ladder = match conclude::prepare(inputs)? {
        conclude::Prepared::Ready(ladder) => ladder,
        conclude::Prepared::Done(decided) => {
            return Ok(match decided.eliminated.iter().find(|e| e.component_id == component_id) {
                Some(elimination) => used(
                    vec![eliminated_reason(&elimination.violation)],
                    WhyNotVerdict::Eliminated,
                    false,
                ),
                None => used(
                    vec![WhyNotReason {
                        kind: "abstained".to_string(),
                        detail: "the decision abstained before any option was solved".to_string(),
                    }],
                    WhyNotVerdict::Unresolved,
                    false,
                ),
            });
        }
    };
    if let Some(elimination) = ladder
        .screened
        .eliminated
        .iter()
        .find(|e| e.component_id == component_id)
    {
        return Ok(used(
            vec![eliminated_reason(&elimination.violation)],
            WhyNotVerdict::Eliminated,
            false,
        ));
    }
    let unresolved = |kind: &str, detail: &str| {
        used(
            vec![WhyNotReason {
                kind: kind.to_string(),
                detail: detail.to_string(),
            }],
            WhyNotVerdict::Unresolved,
            false,
        )
    };
    if !inputs.templates.is_empty()
        || !matches!(topology::pose(inputs, None), topology::Posed::Plain)
    {
        return Ok(unresolved(
            "topology",
            "a template or topology assembly is not re-solved one option at a time",
        ));
    }
    let built = match model::build(
        inputs,
        &ladder.required,
        (ladder.screened.legal.as_slice(), 1),
        ladder.currency.as_deref(),
    )? {
        model::Building::Ready(built) => built,
        model::Building::Abstain(_) => {
            return Ok(unresolved(
                "abstained",
                "the decision abstained before any option was solved",
            ))
        }
    };
    let context = SolveContext::new(inputs, built, None)?;
    let validator = |chosen: &[(usize, &CandidateFacts)]| {
        agent::drafts(inputs, &context.inputs_digest, None, chosen)
    };
    let answer = match search(&context, &validator)? {
        Ok(answer) => answer,
        Err(_) => {
            return Ok(unresolved(
                "abstained",
                "the decision abstained before any option was solved",
            ))
        }
    };
    let Some(var) = (0..context.built.vars.len())
        .map(|at| VarId(at as u32))
        .find(|var| context.built.vars[var.index()].candidate == position)
    else {
        return Ok(unresolved(
            "not_legal",
            "the option is not in the legal set of the assembly programme",
        ));
    };
    if answer.selected.get(var.index()).copied().unwrap_or(false) {
        return Ok(used(Vec::new(), WhyNotVerdict::Selected, false));
    }
    let kind = context.kind_of(var);
    let (why, exhausted) = forced(&context, var, kind);
    let (verdict, reason) = match (&why.violation, &why.forced_objective) {
        (Some(violation), _) => (WhyNotVerdict::Infeasible, eliminated_reason(violation)),
        (None, Some(objective)) => (
            WhyNotVerdict::Worse,
            WhyNotReason {
                kind: "forced_objective".to_string(),
                detail: format!("forcing the option in costs objective scalar {}", String::from(objective.scalar)),
            },
        ),
        (None, None) => (
            WhyNotVerdict::Unresolved,
            WhyNotReason {
                kind: "forced_unsolved".to_string(),
                detail: "the forced re-solve proved nothing".to_string(),
            },
        ),
    };
    Ok(used(vec![reason], verdict, exhausted))
}

fn eliminated_reason(violation: &Violation) -> WhyNotReason {
    let value = serde_json::to_value(violation).unwrap_or_default();
    WhyNotReason {
        kind: value
            .get("violation")
            .and_then(|tag| tag.as_str())
            .unwrap_or("violation")
            .to_string(),
        detail: value.to_string(),
    }
}
