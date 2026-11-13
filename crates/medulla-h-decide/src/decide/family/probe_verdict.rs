//! The `probe_verdict` question family (MEDULLA-DECISION-ENGINE-R138.3).
//!
//! A probe verdict judges one red-team attack from its scorers' results. The
//! request names the attack (`attack_id`) and the applied transforms
//! (`transform_ids`); the tenant's attack, transform and scorer components
//! come from the Agent Library; the declared candidates are the scorer
//! results (`option_id` is the scorer id, `score_permille` the score as a Q32
//! fraction of 1.0). The sealed answer is advisory with three labelled
//! options, `resisted`, `compromised` and `inconclusive`; the chosen one
//! scores 1.0. Nothing is written.

use medulla_agent_types::agent_component::red_team::{
    RedTeamAttack, RedTeamScorer, RedTeamTransform,
};
use medulla_agent_types::agent_component::{AgentComponentEntry, AgentComponentKind};
use medulla_decision_outcome_types::decision::red_team::{
    decide_probe_verdict, select_scorers, ProbeRefusal, ProbeVerdict, RedTeamPolicy, ScorerResult,
};
use medulla_decision_types::decision::record::outcome::DecisionRefusal;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, ScoredOption, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{
    DecisionPolicy, LibraryCandidateScope, QuantScaleTag, QuantisedValue, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::candidates::library_candidates;
use crate::decide::stat_executor::{Executed, ExecutionContext};

use super::support::{abstained, advisory, invalid, param};
use super::DecisionFamily;

/// The number key carrying a scorer's score, per mille as a Q32 fraction.
const SCORE_KEY: &str = "score_permille";

/// The family of the `probe_verdict` question kind.
pub struct ProbeVerdictFamily;

fn refused(refusal: &ProbeRefusal) -> String {
    invalid(format!("{}: {}", refusal.code().as_str(), refusal.detail()))
}

/// The attack id and the transform ids the request names.
fn names(request: &DecideRequest) -> Result<(String, Vec<String>), String> {
    let attack = match param(request, "attack_id")? {
        Some(TypedValue::Iri(id)) => id.clone(),
        _ => return Err(invalid("parameter attack_id is a required IRI")),
    };
    let transforms = match param(request, "transform_ids")? {
        None => Vec::new(),
        Some(TypedValue::IriList(ids)) => ids.iter().cloned().collect(),
        Some(_) => return Err(invalid("parameter transform_ids is not an IRI list")),
    };
    Ok((attack, transforms))
}

/// The score of a declared option in per mille: the rounded Q32 fraction.
/// `None` when the option carries no `score_permille`.
fn permille(option: &DeclaredOption) -> Option<i64> {
    option
        .numbers
        .iter()
        .find(|n| n.key == SCORE_KEY)
        .map(|n| ((i128::from(n.q32) * 1000 + (1 << 31)) >> 32) as i64)
}

fn entry_of<'a>(
    entries: &'a [AgentComponentEntry],
    kind: AgentComponentKind,
    id: &str,
) -> Option<&'a AgentComponentEntry> {
    entries
        .iter()
        .find(|entry| entry.kind == kind && entry.component_id == id)
}

fn scored(label: &str, chosen: bool) -> ScoredOption {
    ScoredOption {
        option_id: label.to_string(),
        score: QuantisedValue {
            scale: QuantScaleTag::Q32,
            value: if chosen { 1i64 << 32 } else { 0 },
        },
        probability: None,
    }
}

fn label(verdict: ProbeVerdict) -> &'static str {
    match verdict {
        ProbeVerdict::Resisted => "resisted",
        ProbeVerdict::Compromised => "compromised",
        ProbeVerdict::Inconclusive => "inconclusive",
    }
}

fn conclude_probe(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let (attack_id, transform_ids) = names(request)?;
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid(
            "probe_verdict declares one option per scorer result",
        ));
    };
    // The tenant's red-team components, read as the library candidates are.
    let scope = LibraryCandidateScope {
        kinds: BoundedVec::new(vec![
            AgentComponentKind::RedTeamAttack,
            AgentComponentKind::RedTeamTransform,
            AgentComponentKind::RedTeamScorer,
        ])
        .map_err(invalid)?,
        classification_under: None,
    };
    let entries = library_candidates(ctx.store, ctx.tenant_id, &scope)?.entries;
    let attack =
        entry_of(&entries, AgentComponentKind::RedTeamAttack, &attack_id).ok_or_else(|| {
            refused(&ProbeRefusal::AttackUnknown {
                attack_id: attack_id.clone(),
            })
        })?;
    let attack = RedTeamAttack::from_attributes(&attack.component_id, &attack.attributes)
        .map_err(invalid)?;
    for id in &transform_ids {
        let entry = entry_of(&entries, AgentComponentKind::RedTeamTransform, id)
            .ok_or_else(|| invalid(format!("no visible transform {id}")))?;
        RedTeamTransform::from_attributes(&entry.component_id, &entry.attributes)
            .map_err(invalid)?;
    }
    let scorers = entries
        .iter()
        .filter(|entry| entry.kind == AgentComponentKind::RedTeamScorer)
        .map(|entry| RedTeamScorer::from_attributes(&entry.component_id, &entry.attributes))
        .collect::<Result<Vec<_>, _>>()
        .map_err(invalid)?;
    let probe_policy = RedTeamPolicy::from_policy(policy);
    let selected = select_scorers(&attack, &scorers, &probe_policy);
    if selected.is_empty() {
        return Ok(abstained(
            QuestionKind::ProbeVerdict,
            &ProbeRefusal::NoScorerForCategory {
                category: attack.category.clone(),
            },
        ));
    }
    if let Some(unknown) = options
        .iter()
        .find(|option| !selected.iter().any(|s| s.id == option.option_id))
    {
        return Err(refused(&ProbeRefusal::ResultScorerUnknown {
            scorer_id: unknown.option_id.clone(),
        }));
    }
    let mut results = Vec::with_capacity(selected.len());
    for scorer in selected {
        let given = options
            .iter()
            .find(|option| option.option_id == scorer.id)
            .and_then(permille);
        if let Some(value) = given.filter(|v| !(0..=1000).contains(v)) {
            return Err(refused(&ProbeRefusal::ScoreOutOfRange {
                scorer_id: scorer.id.clone(),
                permille: value,
            }));
        }
        results.push(ScorerResult {
            scorer: scorer.clone(),
            score: given.map(|v| v as u16),
        });
    }
    let verdict = decide_probe_verdict(&results, &probe_policy);
    let scores = [
        ProbeVerdict::Resisted,
        ProbeVerdict::Compromised,
        ProbeVerdict::Inconclusive,
    ]
    .map(|candidate| scored(label(candidate), candidate == verdict));
    let mut executed = advisory(Vec::new())?;
    executed.ladder.outcome = StatisticalOutcome::Advisory {
        scores: BoundedVec::new(scores.to_vec()).map_err(invalid)?,
        calibrated: false,
    };
    Ok(executed)
}

impl DecisionFamily for ProbeVerdictFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        names(request).map(|_| ())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_probe(ctx, request, policy))
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(ResolutionKind::Constraint)
    }
}
