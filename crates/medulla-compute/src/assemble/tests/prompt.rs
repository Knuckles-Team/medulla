//! System-prompt selection per slot (MEDULLA-DECISION-ENGINE-R814.2).

use std::collections::BTreeSet;

use medulla_decision_outcome_types::decision::prompt_candidate::{PromptCandidate, PromptRole};
use medulla_decision_types::decision::record::outcome::AbstainReason;

use crate::assemble::prompt::select_system_prompt;

const ONE: i64 = 1 << 32;

fn required(iris: &[&str]) -> BTreeSet<String> {
    iris.iter().map(|iri| (*iri).to_string()).collect()
}

fn candidate(id: &str, serves: &[&str], confidence_q32: i64) -> PromptCandidate {
    PromptCandidate {
        component_id: id.into(),
        connector: "urn:connector:test".into(),
        role: PromptRole::System,
        serves: required(serves),
        digest: "b".repeat(64),
        confidence_q32,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R814.2
#[test]
fn selection_scores_by_match_and_confidence_and_abstains() {
    let need = required(&["urn:t:a", "urn:t:b"]);
    // Full match at 0.5 confidence beats a half match at full confidence once
    // the half match is legal (min ratio 0.5): 0.5 vs 0.5 ties to the lowest id.
    let full_half = candidate("p:z", &["urn:t:a", "urn:t:b"], ONE / 2);
    let half_full = candidate("p:a", &["urn:t:a"], ONE);
    let picked = select_system_prompt(&need, &[full_half.clone(), half_full.clone()], 500_000)
        .expect("legal");
    assert_eq!(picked.component_id, "p:a", "tie goes to the lowest id");
    assert_eq!(picked.score_q32, ONE / 2);

    // At the default ratio 1.0 the half match is not legal.
    let picked = select_system_prompt(&need, &[full_half.clone(), half_full.clone()], 1_000_000)
        .expect("legal");
    assert_eq!(picked.component_id, "p:z");

    // Higher confidence wins at equal match.
    let strong = candidate("p:y", &["urn:t:a", "urn:t:b"], ONE - 1);
    let picked = select_system_prompt(&need, &[full_half, strong], 1_000_000).expect("legal");
    assert_eq!(picked.component_id, "p:y");
    assert_eq!(picked.score_q32, ONE - 1);

    // Task and template prompts are guidance, never system candidates.
    let mut task = candidate("p:task", &["urn:t:a", "urn:t:b"], ONE);
    task.role = PromptRole::Task;
    assert_eq!(
        select_system_prompt(&need, &[task], 1_000_000),
        Err(AbstainReason::UncoveredCapability {
            iri: "urn:t:a".into()
        })
    );

    // Nothing legal: the first uncovered capability in sorted order.
    let only_b = candidate("p:b", &["urn:t:b"], ONE);
    assert_eq!(
        select_system_prompt(&need, &[only_b], 1_000_000),
        Err(AbstainReason::UncoveredCapability {
            iri: "urn:t:a".into()
        })
    );
    assert_eq!(
        select_system_prompt(&need, &[], 1_000_000),
        Err(AbstainReason::UncoveredCapability {
            iri: "urn:t:a".into()
        })
    );
}
