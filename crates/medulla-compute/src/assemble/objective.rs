//! The policy's objective as integer levels over the legal remainder.
//!
//! `Uncovered` is a HARD row in this engine (every required capability is a
//! covering constraint), so it contributes no level: an assembly that leaves a
//! requirement uncovered is not a worse answer, it is no answer. Every other
//! level is an exact integer term per variable, and an undeclared fact is
//! `Coefficient::Unknown` -- ranked in its own tier below every known value,
//! never read as zero (§7.3).

use medulla_decision_types::{
    decision::DecisionErrorCode, decision::DecisionInputs, decision::ObjectiveLevelKind,
    decision::ObjectiveOrder,
};
use medulla_decision_types::{
    solve::Coefficient, solve::ObjectiveLevelSpec, solve::ObjectiveTerm, solve::VarId,
};

use medulla_decision_types::decision::{
    CandidateFacts, PremiseClass, PremiseProvenance, PremiseRef,
};

use super::facts::{self, Declared};
use super::AssembleError;
use medulla_decision_policy::solve::model::MAX_ABS_COEFFICIENT;

/// One level's value for one candidate.
fn level_value(
    kind: ObjectiveLevelKind,
    candidate: &medulla_decision_types::decision::CandidateFacts,
    currency: Option<&str>,
) -> Declared {
    match kind {
        ObjectiveLevelKind::Uncovered => Declared::None,
        ObjectiveLevelKind::Components => Declared::Known(1),
        ObjectiveLevelKind::DeclaredCost => facts::per_call_cost(candidate, currency),
        ObjectiveLevelKind::DeclaredP95Latency => facts::p95_latency(candidate),
        ObjectiveLevelKind::ArtifactBytes => facts::artifact_bytes(candidate),
        ObjectiveLevelKind::LineCount => facts::line_count(candidate),
        ObjectiveLevelKind::ObservedRunCost => facts::observed_run_cost(candidate),
    }
}

/// The levels the policy's objective ranks on.
fn ranked_levels(order: &ObjectiveOrder) -> Vec<ObjectiveLevelKind> {
    match order {
        ObjectiveOrder::Lexicographic { levels } => levels.iter().copied().collect(),
        ObjectiveOrder::Weighted { weights } => weights
            .iter()
            .filter(|level| level.weight > 0)
            .map(|level| level.level)
            .collect(),
    }
}

/// The first legal candidate whose size or ledger-cost fact is unknown under
/// a level the objective ranks on, in candidate order. Only the size and
/// ledger levels are read here: the declared cost and latency levels keep
/// their own budget screening. The caller applies the policy's
/// `UnknownCostRule`: under `ExcludeWhenStrict` this candidate and level
/// abstain the decision (MEDULLA-DECISION-ENGINE-R014.100.2).
pub(super) fn first_unknown_size_or_cost(
    inputs: &DecisionInputs,
    legal: &[usize],
) -> Option<(String, &'static str)> {
    let ranked = ranked_levels(&inputs.policy.objective);
    legal.iter().find_map(|&index| {
        let candidate = &inputs.candidates.as_slice()[index];
        ranked
            .iter()
            .filter(|kind| is_size_or_cost(**kind))
            .find(|&&kind| level_value(kind, candidate, None) == Declared::Unknown)
            .map(|&kind| (candidate.component_id.clone(), level_label(kind)))
    })
}

fn coefficient(value: Declared) -> Option<Coefficient> {
    match value {
        Declared::Known(0) | Declared::None => None,
        Declared::Known(value) => Some(Coefficient::Known(value)),
        Declared::Unknown => Some(Coefficient::Unknown),
    }
}

pub(super) fn levels(
    inputs: &DecisionInputs,
    vars: &[usize],
    currency: Option<&str>,
) -> Result<Vec<ObjectiveLevelSpec>, AssembleError> {
    let mut specs = ranked_specs(inputs, vars, currency)?;
    if ranks_size_or_cost(&inputs.policy.objective) {
        specs.extend(id_tie_break(vars));
    }
    Ok(specs)
}

fn ranks_size_or_cost(order: &ObjectiveOrder) -> bool {
    ranked_levels(order)
        .iter()
        .any(|kind| is_size_or_cost(*kind))
}

fn is_size_or_cost(kind: ObjectiveLevelKind) -> bool {
    matches!(
        kind,
        ObjectiveLevelKind::ArtifactBytes
            | ObjectiveLevelKind::LineCount
            | ObjectiveLevelKind::ObservedRunCost
    )
}

/// The last level when a size or ledger-cost level is ranked: candidates
/// that tie on every level resolve to the lexicographically smallest
/// `component_id` set. Candidates are sorted by id, so the coefficient of the
/// candidate at distinct rank `r` is `2^r`: a larger id always costs more
/// than every smaller one together, so the cheapest tie avoids the largest ids. Beyond the solver's coefficient range
/// (more than 40 distinct placed candidates) no level is added and the
/// solver's own deterministic order decides
/// (MEDULLA-DECISION-ENGINE-R014.100.2).
fn id_tie_break(vars: &[usize]) -> Option<ObjectiveLevelSpec> {
    let mut distinct: Vec<usize> = vars.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    let n = distinct.len();
    if n == 0 || n > 40 {
        return None;
    }
    Some(ObjectiveLevelSpec {
        label: "component_id".to_string(),
        terms: terms(vars, |index| {
            let rank = distinct.binary_search(&index).ok()?;
            Some(Coefficient::Known(1i64 << rank))
        }),
    })
}

fn ranked_specs(
    inputs: &DecisionInputs,
    vars: &[usize],
    currency: Option<&str>,
) -> Result<Vec<ObjectiveLevelSpec>, AssembleError> {
    let candidates = inputs.candidates.as_slice();
    match &inputs.policy.objective {
        ObjectiveOrder::Lexicographic { levels } => Ok(levels
            .iter()
            .filter(|kind| **kind != ObjectiveLevelKind::Uncovered)
            .map(|&kind| ObjectiveLevelSpec {
                label: level_label(kind).to_string(),
                terms: terms(vars, |index| {
                    coefficient(level_value(kind, &candidates[index], currency))
                }),
            })
            .collect()),
        ObjectiveOrder::Weighted { weights } => {
            let mut overflow = false;
            let terms = terms(vars, |index| {
                let weighted = weighted_value(weights.as_slice(), &candidates[index], currency);
                overflow |= weighted.is_err();
                weighted.ok().flatten()
            });
            if overflow {
                return Err(AssembleError::new(
                    DecisionErrorCode::AssemblyModelInvalid,
                    "a weighted objective coefficient exceeds the solver's coefficient range",
                ));
            }
            Ok(vec![ObjectiveLevelSpec {
                label: "weighted".to_string(),
                terms,
            }])
        }
    }
}

fn terms(
    vars: &[usize],
    mut value: impl FnMut(usize) -> Option<Coefficient>,
) -> Vec<ObjectiveTerm> {
    vars.iter()
        .enumerate()
        .filter_map(|(position, &index)| {
            value(index).map(|coefficient| ObjectiveTerm {
                var: VarId(position as u32),
                coefficient,
            })
        })
        .collect()
}

/// `Σ weight · value` for one candidate: unknown when any weighted value is,
/// an error when the exact sum leaves the solver's coefficient range.
fn weighted_value(
    weights: &[medulla_decision_types::decision::WeightedLevel],
    candidate: &medulla_decision_types::decision::CandidateFacts,
    currency: Option<&str>,
) -> Result<Option<Coefficient>, ()> {
    let mut total: i64 = 0;
    for level in weights.iter().filter(|level| level.weight > 0) {
        match level_value(level.level, candidate, currency) {
            Declared::None => {}
            Declared::Unknown => return Ok(Some(Coefficient::Unknown)),
            Declared::Known(value) => {
                let weight = i64::try_from(level.weight).map_err(|_| ())?;
                let term = value.checked_mul(weight).ok_or(())?;
                total = total.checked_add(term).ok_or(())?;
            }
        }
    }
    if total > MAX_ABS_COEFFICIENT {
        return Err(());
    }
    Ok(coefficient(Declared::Known(total)))
}

fn level_label(kind: ObjectiveLevelKind) -> &'static str {
    match kind {
        ObjectiveLevelKind::Uncovered => "uncovered",
        ObjectiveLevelKind::Components => "components",
        ObjectiveLevelKind::DeclaredCost => "declared_cost",
        ObjectiveLevelKind::DeclaredP95Latency => "declared_p95_latency",
        ObjectiveLevelKind::ArtifactBytes => "artifact_bytes",
        ObjectiveLevelKind::LineCount => "line_count",
        ObjectiveLevelKind::ObservedRunCost => "observed_run_cost",
    }
}

/// One premise `objective:<level>` per size or ledger-cost level the
/// objective ranks on and per chosen candidate that declares a value, class
/// `Observation`, naming the revision that measured it
/// (MEDULLA-DECISION-ENGINE-R014.100.2). A level whose value is unknown adds
/// nothing: the record never states a value nobody measured.
pub(super) fn premises(inputs: &DecisionInputs, chosen: &[&CandidateFacts]) -> Vec<PremiseRef> {
    let ranked = ranked_levels(&inputs.policy.objective);
    let mut out = Vec::new();
    for candidate in chosen {
        for &kind in &ranked {
            let revision = match kind {
                ObjectiveLevelKind::ArtifactBytes | ObjectiveLevelKind::LineCount => candidate
                    .size
                    .as_ref()
                    .map(|size| format!("revision:{}", size.measured_revision)),
                ObjectiveLevelKind::ObservedRunCost => candidate
                    .observed_run_cost
                    .as_ref()
                    .map(|cost| format!("ledger:{}", cost.measured_revision)),
                _ => None,
            };
            let Some(evaluation_id) = revision else {
                continue;
            };
            if let Declared::Known(value) = level_value(kind, candidate, None) {
                out.push(PremiseRef {
                    subject: candidate.component_id.clone(),
                    fact: format!("objective:{}={value}", level_label(kind)),
                    class: PremiseClass::Observation,
                    provenance: PremiseProvenance::Observation { evaluation_id },
                });
            }
        }
    }
    out
}
