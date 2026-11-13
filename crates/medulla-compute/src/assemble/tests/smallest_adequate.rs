//! Adequacy elimination of the size-and-adequacy facts
//! (MEDULLA-DECISION-ENGINE-R014.100.1).

use medulla_decision_types::decision::size::{Adequacy, SourceSize};
use medulla_decision_types::decision::{CandidateFacts, DecisionPolicy, Violation};

use super::fixture::{cheap_model, inputs_with, request};
use crate::assemble::eliminate::screen;

fn with_adequacy(id: &str, adequate: Option<bool>) -> CandidateFacts {
    let mut model = cheap_model(id);
    model.size = Some(SourceSize {
        artifact_bytes: 4_096,
        line_count: 120,
        measured_revision: 3,
    });
    model.adequacy = adequate.map(|adequate| Adequacy {
        adequate,
        evaluation_id: format!("eval-{id}"),
    });
    model
}

fn screened(require: bool) -> (Vec<String>, Vec<(String, Violation)>) {
    let mut request = request(&[], &["eg:capability/generation/text"]);
    request.requirements.constraints.require_adequacy = require;
    let candidates = vec![
        with_adequacy("a-adequate", Some(true)),
        with_adequacy("b-inadequate", Some(false)),
        with_adequacy("c-unknown", None),
    ];
    let inputs = inputs_with(request, candidates, DecisionPolicy::engine_default());
    let screened = screen(&inputs, None);
    let legal = screened
        .legal
        .iter()
        .map(|i| inputs.candidates[*i].component_id.clone())
        .collect();
    let eliminated = screened
        .eliminated
        .into_iter()
        .map(|e| (e.component_id, e.violation))
        .collect();
    (legal, eliminated)
}

// spec: MEDULLA-DECISION-ENGINE-R014.100.1
#[test]
fn inadequate_and_unknown_adequacy_are_eliminated() {
    // Without `require_adequacy` only a known-inadequate candidate is removed.
    let (legal, eliminated) = screened(false);
    assert_eq!(legal, ["a-adequate", "c-unknown"]);
    assert_eq!(
        eliminated,
        [("b-inadequate".to_string(), Violation::Inadequate)]
    );

    // With it, a candidate nobody evaluated is removed too, as its own violation.
    let (legal, eliminated) = screened(true);
    assert_eq!(legal, ["a-adequate"]);
    assert_eq!(
        eliminated,
        [
            ("b-inadequate".to_string(), Violation::Inadequate),
            ("c-unknown".to_string(), Violation::UnknownAdequacy),
        ]
    );

    // Absent facts keep their bytes: neither field serialises when absent.
    let bare = cheap_model("bare");
    let json = serde_json::to_value(&bare).expect("candidate serialises");
    assert!(json.get("size").is_none() && json.get("adequacy").is_none());
}
