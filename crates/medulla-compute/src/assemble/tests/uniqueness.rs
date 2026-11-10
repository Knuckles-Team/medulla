//! The uniqueness re-solve (MEDULLA-DECISION-ENGINE-R001.2.1,
//! MEDULLA-DECISION-ENGINE-R001.2.2): the no-good verdict against
//! exhaustive enumeration, and which scope fixes the answer.

use std::collections::BTreeSet;

use medulla_decision_policy::solve::{solve, verify, Model, Verdict};
use medulla_decision_types::decision::record::UniquenessScope;
use medulla_decision_types::decision::request::effective_solver_budget;
use medulla_decision_types::decision::{
    CandidateFacts, DecisionOutcome, PremiseClass, PremiseProvenance, ResolutionKind,
};
use medulla_decision_types::solve::{
    Relation, SolverConfig, SolverConfigSpec, VarId, DEFAULT_CERTIFICATE_LEAVES,
};

use super::super::uniqueness::no_good_row;
use super::super::{assemble, replay_check, Assembly};
use super::fixture::*;

fn unique_library() -> Vec<CandidateFacts> {
    let mut library = agent_basics();
    library.extend([
        cheap_model("model-a"),
        prompt("prompt-a", 1_000),
        tool(
            "tool-a",
            &[
                "eg:capability/retrieval",
                "eg:capability/analysis/summarize",
            ],
            Some(5),
        ),
    ]);
    library
}

/// The solved selection as a mask over the solved model's variables: the
/// plain path's variables are its legal candidates, in candidate order.
fn selected_mask(assembly: &Assembly) -> Vec<bool> {
    let DecisionOutcome::Solved { slots, .. } = &assembly.record.outcome else {
        panic!("the fixture assembles to a solved record");
    };
    let chosen: BTreeSet<String> = slots
        .iter()
        .map(|slot| slot.component.component_id.clone())
        .collect();
    let eliminated: BTreeSet<&str> = assembly
        .record
        .eliminated
        .iter()
        .map(|elimination| elimination.component_id.as_str())
        .collect();
    assembly
        .record
        .inputs
        .candidates
        .as_slice()
        .iter()
        .filter(|candidate| !eliminated.contains(candidate.component_id.as_str()))
        .map(|candidate| chosen.contains(&candidate.component_id))
        .collect()
}

/// The no-good re-solve of the solved model: zero objective, the row over
/// the solved selection, under the record's effective node budget.
fn re_solve(
    assembly: &Assembly,
) -> (
    Model,
    medulla_decision_types::solve::Certificate,
    std::result::Result<Verdict, medulla_decision_policy::solve::VerifyError>,
) {
    let inputs = &assembly.record.inputs;
    let mut cut = assembly
        .model
        .clone()
        .expect("a solved record carries its model");
    cut.objective.clear();
    cut.constraints.push(no_good_row(&selected_mask(assembly)));
    let model = Model::try_from(cut).expect("the cut model is valid");
    let budget = effective_solver_budget(&inputs.policy, &inputs.request);
    let config = SolverConfig::try_from(SolverConfigSpec {
        node_budget: budget.node_budget,
        max_certificate_leaves: DEFAULT_CERTIFICATE_LEAVES,
        bound_denominator: 1,
        accepted_gap: inputs.policy.accepted_gap,
    })
    .expect("the record's budget configures the solver");
    let certificate = solve(&model, &config);
    let verdict = verify(&model, &certificate);
    (model, certificate, verdict)
}

/// How many assignments of the model's rows are feasible, by enumeration.
fn feasible_count(model: &Model) -> u32 {
    let n = model.variable_count();
    assert!(n <= 20, "the fixture stays inside enumeration range");
    (0u32..(1u32 << n))
        .filter(|mask| {
            let selected: Vec<bool> = (0..n).map(|bit| *mask & (1 << bit) != 0).collect();
            model.rows().iter().all(|row| {
                let lhs: i64 = row
                    .terms()
                    .iter()
                    .filter(|term| selected[VarId::index(term.var)])
                    .map(|term| term.coefficient)
                    .sum();
                match row.relation() {
                    Relation::LessEqual => lhs <= row.rhs(),
                    Relation::GreaterEqual => lhs >= row.rhs(),
                    Relation::Equal => lhs == row.rhs(),
                }
            })
        })
        .count() as u32
}

// spec: MEDULLA-DECISION-ENGINE-R001.2.1
#[test]
fn no_good_resolve_agrees_with_exhaustive_enumeration() {
    // One feasible selection: the no-good re-solve proves it, and the
    // enumeration agrees that nothing else is feasible. With nothing
    // eliminated the one certificate serves both scopes.
    let assembly = assemble(
        inputs(request(&["eg:task/research"], &[]), unique_library()),
        identity(),
    )
    .expect("assembles");
    let (model, _certificate, verdict) = re_solve(&assembly);
    assert!(
        matches!(verdict, Ok(Verdict::ProvenInfeasible { .. })),
        "the unique selection is proven by the no-good re-solve: {verdict:?}"
    );
    assert_eq!(feasible_count(&model), 0, "no other selection is feasible");
    let uniqueness = assembly.record.uniqueness.as_slice();
    assert_eq!(uniqueness.len(), 2, "both scopes proven by one certificate");
    assert_eq!(uniqueness[0].scope, UniquenessScope::AfterElimination);
    assert_eq!(uniqueness[1].scope, UniquenessScope::AllCandidates);
    assert_eq!(
        uniqueness[0].certificate, uniqueness[1].certificate,
        "nothing was eliminated: no third solve runs"
    );
    assert_eq!(
        assembly.record.resolution_kind,
        ResolutionKind::Entailment,
        "the is_a coverage alone fixed the answer"
    );
    replay_check(&assembly.record).expect("replay re-derives the proofs");

    // Two feasible selections: the re-solve does not prove infeasible, the
    // enumeration finds the alternative, and the record keeps Optimization
    // with no proof.
    let mut library = unique_library();
    library.push(tool(
        "tool-b",
        &[
            "eg:capability/retrieval",
            "eg:capability/analysis/summarize",
        ],
        Some(3),
    ));
    let assembly = assemble(
        inputs(request(&["eg:task/research"], &[]), library),
        identity(),
    )
    .expect("assembles");
    let (model, _certificate, verdict) = re_solve(&assembly);
    assert!(
        !matches!(verdict, Ok(Verdict::ProvenInfeasible { .. })),
        "a feasible alternative exists: {verdict:?}"
    );
    assert!(
        feasible_count(&model) >= 1,
        "enumeration finds the alternative"
    );
    assert_eq!(
        assembly.record.resolution_kind,
        ResolutionKind::Optimization,
        "an unresolved re-solve never guesses"
    );
    assert!(assembly.record.uniqueness.is_empty());
    replay_check(&assembly.record).expect("replays");
}

// spec: MEDULLA-DECISION-ENGINE-R001.2.2
#[test]
fn coverage_alone_fixes_the_answer_as_entailment() {
    // An eliminated candidate that still covers the requirement: coverage
    // alone does NOT fix the answer -- the constraint kind, one proof, and
    // the record's certificate verifies the after-elimination cut.
    let mut library = unique_library();
    library.push(tool(
        "tool-b",
        &[
            "eg:capability/retrieval",
            "eg:capability/analysis/summarize",
        ],
        Some(3),
    ));
    let mut denied = request(&["eg:task/research"], &[]);
    denied.requirements.denies = bounded(vec!["tool-b".to_string()]);
    let assembly = assemble(inputs(denied, library), identity()).expect("assembles");
    let (cut, certificate, verdict) = re_solve(&assembly);
    assert!(
        matches!(verdict, Ok(Verdict::ProvenInfeasible { .. })),
        "the after-elimination cut is proven: {verdict:?}"
    );
    assert_eq!(feasible_count(&cut), 0);
    let uniqueness = assembly.record.uniqueness.as_slice();
    assert_eq!(
        uniqueness.len(),
        1,
        "the all-candidates scope is not proven"
    );
    assert_eq!(uniqueness[0].scope, UniquenessScope::AfterElimination);
    assert_eq!(
        uniqueness[0].certificate, certificate,
        "the record carries the re-solve's certificate"
    );
    assert_eq!(
        assembly.record.resolution_kind,
        ResolutionKind::Constraint,
        "a feasible alternative in the all-candidates scope keeps Constraint"
    );
    let premises = assembly.record.premises.as_slice();
    assert!(
        premises
            .iter()
            .any(|premise| premise.fact == "unique_feasible_selection"),
        "the selection proof is recorded"
    );
    assert!(
        !premises
            .iter()
            .any(|premise| premise.fact == "unique_under_coverage_alone"),
        "the coverage premise is not claimed without its proof"
    );
    replay_check(&assembly.record).expect("replay re-derives the proof");

    // An eliminated candidate that covers nothing: over every candidate the
    // is_a coverage alone still fixes the answer -- the entailment kind,
    // both proofs, both premises.
    let mut library = unique_library();
    library.push(model(
        "model-b",
        ModelSpec {
            window: 500,
            tools: true,
            per_call: Some(10),
            p95: Some(100),
        },
    ));
    let mut denied = request(&["eg:task/research"], &[]);
    denied.requirements.denies = bounded(vec!["model-b".to_string()]);
    let assembly = assemble(inputs(denied, library), identity()).expect("assembles");
    assert_eq!(assembly.record.eliminated.len(), 1);
    assert_eq!(
        assembly.record.resolution_kind,
        ResolutionKind::Entailment,
        "the is_a coverage alone fixed the answer"
    );
    let uniqueness = assembly.record.uniqueness.as_slice();
    assert_eq!(uniqueness.len(), 2, "both scopes proven");
    assert_eq!(uniqueness[0].scope, UniquenessScope::AfterElimination);
    assert_eq!(uniqueness[1].scope, UniquenessScope::AllCandidates);
    let premises = assembly.record.premises.as_slice();
    let selection = premises
        .iter()
        .find(|premise| premise.fact == "unique_feasible_selection")
        .expect("the selection proof is recorded");
    assert_eq!(selection.class, PremiseClass::Proof);
    assert!(matches!(
        selection.provenance,
        PremiseProvenance::Policy { .. }
    ));
    let coverage = premises
        .iter()
        .find(|premise| premise.fact == "unique_under_coverage_alone")
        .expect("the coverage proof is recorded");
    assert_eq!(coverage.class, PremiseClass::Proof);
    assert!(matches!(
        coverage.provenance,
        PremiseProvenance::NativeOntology { .. }
    ));
    replay_check(&assembly.record).expect("replay re-derives the proofs");
}
