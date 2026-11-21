//! System-prompt selection through the assemble path
//! (MEDULLA-DECISION-ENGINE-R814.2): a covering candidate is selected and a
//! slot no candidate covers abstains, naming the first uncovered capability.

use std::collections::BTreeSet;

use medulla_compute::assemble::prompt::select_system_prompt;
use medulla_decision_outcome_types::decision::prompt_candidate::{PromptCandidate, PromptRole};
use medulla_decision_types::decision::knob_families::prompt_candidate::KNOBS;
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::record::outcome::AbstainReason;

const ONE: i64 = 1 << 32;

fn set(iris: &[&str]) -> BTreeSet<String> {
    iris.iter().map(|iri| (*iri).to_string()).collect()
}

fn candidate(id: &str, connector: &str, serves: &[&str]) -> PromptCandidate {
    PromptCandidate {
        component_id: id.into(),
        connector: connector.into(),
        role: PromptRole::System,
        serves: set(serves),
        digest: "c".repeat(64),
        confidence_q32: ONE,
    }
}

fn default_min_match_micros() -> i64 {
    match KNOBS[0].default {
        KnobValue::DecimalMicros(value) => value,
        other => panic!("min_match_ratio is a decimal knob, not {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R814.2
#[test]
fn assemble_selects_covering_candidate_and_abstains_without_one() {
    let min = default_min_match_micros();
    let candidates = [
        candidate("prompt:a", "urn:connector:a", &["urn:task:a"]),
        candidate("prompt:b", "urn:connector:b", &["urn:task:b"]),
    ];
    let picked = select_system_prompt(&set(&["urn:task:b"]), &candidates, min).expect("covered");
    assert_eq!(picked.component_id, "prompt:b");
    assert_eq!(picked.score_q32, ONE);

    let abstained = select_system_prompt(&set(&["urn:task:c"]), &candidates, min);
    assert_eq!(
        abstained,
        Err(AbstainReason::UncoveredCapability {
            capability: "urn:task:c".into()
        })
    );
}
