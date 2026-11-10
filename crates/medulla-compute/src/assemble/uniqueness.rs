//! Uniqueness of the solved selection (MEDULLA-DECISION-ENGINE-R001.2.1,
//! MEDULLA-DECISION-ENGINE-R001.2.2).
//!
//! The solved model may admit several feasible selections; the objective
//! only ranks them. A uniqueness re-solve asks whether the hard rows alone
//! fix the solved selection S: clone the solved model, zero the objective,
//! add the no-good row `Σ_{i∈S} x_i − Σ_{j∉S} x_j ≤ |S| − 1` (which cuts
//! exactly S) and re-solve under the same node budget. Only a verified
//! `ProvenInfeasible` proves the model admits no other selection; any other
//! verdict proves nothing, and an unresolved re-solve never guesses.
//!
//! Two scopes. `AfterElimination` re-solves the solved model, the one step
//! 1b cut to its legal candidates; `AllCandidates` re-solves the model
//! rebuilt over every candidate, before any removal. When nothing was
//! eliminated the solved model already is the all-candidates model, so one
//! certificate serves both scopes and no third solve runs. Proven in both
//! scopes, the answer rests on the is_a coverage alone (`Entailment`);
//! proven in the first only, the removed candidates fixed it (`Constraint`).

use medulla_decision_policy::solve::{solve, verify, Model, Verdict};
use medulla_decision_types::decision::record::{UniquenessProof, UniquenessScope};
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance, PremiseRef};
use medulla_decision_types::solve::{
    Certificate, ConstraintBody, ConstraintSpec, ModelSpec, Relation, SolverConfig, Term, VarId,
};

use super::conclude::Ladder;
use super::model::{self, Building, VarRef};
use super::search::{Answer, SolveContext};

/// What the uniqueness re-solves proved, and the record fields they add.
#[derive(Default)]
pub(super) struct Proofs {
    /// One proof per proven scope: AfterElimination first, AllCandidates
    /// second.
    pub(super) proofs: Vec<UniquenessProof>,
    /// The evidence the proofs add to the record's premises.
    pub(super) premises: Vec<PremiseRef>,
    /// Both scopes proven: the is_a coverage alone fixed the answer.
    pub(super) entailment: bool,
}

/// The no-good re-solves over the solved selection, plain path only:
/// templates and topology questions keep `Optimization`, because their extra
/// variables are choices the selection row cannot cut.
pub(super) fn unique_selection(
    ladder: &Ladder<'_>,
    context: &SolveContext<'_>,
    answer: &Answer,
) -> Proofs {
    let inputs = ladder.inputs;
    let subject = medulla_agent_types::agent_graph::draft_definition_digest(&answer.drafts.1);
    let mut out = Proofs::default();
    let after = no_good_re_solve(
        &answer.spec,
        selection(&context.built.vars, answer).as_slice(),
        &context.config,
    );
    let Some(certificate) = after else {
        return out;
    };
    out.proofs.push(UniquenessProof {
        scope: UniquenessScope::AfterElimination,
        certificate: certificate.clone(),
    });
    out.premises.push(premise(
        &subject,
        "unique_feasible_selection",
        PremiseProvenance::Policy {
            policy_digest: inputs.policy_digest.clone(),
        },
    ));
    if ladder.screened.eliminated.is_empty() {
        // The solved model is already over every candidate: one certificate
        // serves both scopes, and no third solve runs.
        out.proofs.push(UniquenessProof {
            scope: UniquenessScope::AllCandidates,
            certificate,
        });
        out.premises
            .push(coverage_premise(&subject, inputs.ontology_digest.clone()));
        out.entailment = true;
    } else if let Some(certificate) = all_candidates_re_solve(ladder, context, answer) {
        out.proofs.push(UniquenessProof {
            scope: UniquenessScope::AllCandidates,
            certificate,
        });
        out.premises
            .push(coverage_premise(&subject, inputs.ontology_digest.clone()));
        out.entailment = true;
    }
    out
}

/// The solved selection as a mask over the model's variables.
fn selection(vars: &[VarRef], answer: &Answer) -> Vec<bool> {
    answer.selected.iter().take(vars.len()).copied().collect()
}

/// The R001.2.1 scope: the solved model, zero objective, cut by the no-good
/// row over the solved selection.
fn no_good_re_solve(
    spec: &ModelSpec,
    selected: &[bool],
    config: &SolverConfig,
) -> Option<Certificate> {
    let mut cut = spec.clone();
    cut.objective.clear();
    cut.constraints.push(no_good_row(selected));
    prove(&cut, config)
}

/// The R001.2.2 scope: the model rebuilt over every candidate, before step
/// 1b removed any, cut by the same no-good row. Only reached on the plain
/// path, where one slot makes the variable of candidate `c` index `c`.
fn all_candidates_re_solve(
    ladder: &Ladder<'_>,
    context: &SolveContext<'_>,
    answer: &Answer,
) -> Option<Certificate> {
    let inputs = ladder.inputs;
    let candidates = inputs.candidates.len();
    let all: Vec<usize> = (0..candidates).collect();
    let built = match model::build(
        inputs,
        &ladder.required,
        (&all, 1),
        ladder.currency.as_deref(),
    ) {
        Ok(Building::Ready(built)) => built,
        // A rebuild that cannot stand cannot prove the scope.
        Ok(Building::Abstain(_)) | Err(_) => return None,
    };
    let mut selected = vec![false; candidates];
    for (position, &is_chosen) in answer
        .selected
        .iter()
        .take(context.built.vars.len())
        .enumerate()
    {
        if is_chosen {
            selected[context.built.vars[position].candidate] = true;
        }
    }
    let mut spec = built.spec;
    spec.objective.clear();
    spec.constraints.push(no_good_row(&selected));
    prove(&spec, &context.config)
}

/// Solve the cut model and verify its certificate: the certificate only
/// when the re-solve proves the model infeasible. Any other verdict, and
/// every unresolved re-solve, proves nothing.
fn prove(spec: &ModelSpec, config: &SolverConfig) -> Option<Certificate> {
    let model = Model::try_from(spec.clone()).ok()?;
    let certificate = solve(&model, config);
    matches!(
        verify(&model, &certificate),
        Ok(Verdict::ProvenInfeasible { .. })
    )
    .then_some(certificate)
}

/// `Σ_{i∈S} x_i − Σ_{j∉S} x_j ≤ |S| − 1`: exactly the selection S is cut.
/// Any other selection misses S in at least one of the two sums, so its
/// left-hand side stays at |S| − 1 or below.
pub(super) fn no_good_row(selected: &[bool]) -> ConstraintSpec {
    let chosen = selected.iter().filter(|&&is_chosen| is_chosen).count();
    ConstraintSpec {
        label: "uniqueness".to_string(),
        body: ConstraintBody::Linear {
            terms: selected
                .iter()
                .enumerate()
                .map(|(position, &is_chosen)| Term {
                    var: VarId(position as u32),
                    coefficient: if is_chosen { 1 } else { -1 },
                })
                .collect(),
            relation: Relation::LessEqual,
            rhs: chosen as i64 - 1,
        },
    }
}

/// The evidence a proven scope adds to the record: a proof pinned to the
/// assembly it fixes.
fn premise(subject: &str, fact: &str, provenance: PremiseProvenance) -> PremiseRef {
    PremiseRef {
        subject: subject.to_string(),
        fact: fact.to_string(),
        class: PremiseClass::Proof,
        provenance,
    }
}

/// The R001.2.2 premise: the is_a coverage alone fixed the answer.
fn coverage_premise(subject: &str, ontology_digest: String) -> PremiseRef {
    premise(
        subject,
        "unique_under_coverage_alone",
        PremiseProvenance::NativeOntology { ontology_digest },
    )
}
