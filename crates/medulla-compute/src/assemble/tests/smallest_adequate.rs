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

mod objective_levels {
    //! Size and ledger-cost objective levels (MEDULLA-DECISION-ENGINE-R014.100.2).

    use medulla_decision_types::decision::size::ObservedRunCost;
    use medulla_decision_types::decision::{
        AbstainReason, CandidateFacts, DecisionOutcome, DecisionPolicy, ObjectiveLevelKind,
        ObjectiveOrder, UnknownCostRule,
    };

    use super::super::fixture::{
        agent_basics, bounded, cheap_model, identity, inputs_with, prompt, request, tool,
    };
    use super::SourceSize;
    use crate::assemble::assemble;

    const CAPS: [&str; 3] = [
        "eg:capability/retrieval/web-search",
        "eg:capability/retrieval/vector-search",
        "eg:capability/analysis/summarize",
    ];

    /// (id, covered capability indexes, artifact bytes, ledger run cost micros)
    const TOOLS: [(&str, &[usize], u64, u64); 6] = [
        ("t-0", &[0], 100, 5),
        ("t-1", &[1], 100, 5),
        ("t-2", &[2], 100, 5),
        ("t-3", &[0, 1, 2], 301, 1),
        ("t-4", &[0, 1], 200, 9),
        ("t-5", &[0, 1], 200, 9),
    ];

    fn sized(mut candidate: CandidateFacts, bytes: u64, cost: u64) -> CandidateFacts {
        candidate.size = Some(SourceSize {
            artifact_bytes: bytes,
            line_count: 10,
            measured_revision: 2,
        });
        candidate.observed_run_cost = Some(ObservedRunCost {
            micros: cost,
            measured_revision: 7,
        });
        candidate
    }

    fn library() -> Vec<CandidateFacts> {
        let mut out: Vec<CandidateFacts> = agent_basics()
            .into_iter()
            .chain([cheap_model("m-0"), prompt("p-0", 100)])
            .map(|candidate| sized(candidate, 50, 1))
            .collect();
        for (id, covers, bytes, cost) in TOOLS {
            let classes: Vec<&str> = covers.iter().map(|&cap| CAPS[cap]).collect();
            out.push(sized(tool(id, &classes, Some(10)), bytes, cost));
        }
        out
    }

    fn policy(rule: UnknownCostRule) -> DecisionPolicy {
        let mut policy = DecisionPolicy::engine_default();
        policy.objective = ObjectiveOrder::Lexicographic {
            levels: bounded(vec![
                ObjectiveLevelKind::Uncovered,
                ObjectiveLevelKind::ArtifactBytes,
                ObjectiveLevelKind::ObservedRunCost,
            ]),
        };
        policy.unknown_cost = rule;
        policy
    }

    fn solve(library: Vec<CandidateFacts>, rule: UnknownCostRule) -> DecisionOutcome {
        let asked = request(&[], &CAPS);
        let inputs = inputs_with(asked, library, policy(rule));
        assemble(inputs, identity())
            .expect("the inputs are consistent")
            .record
            .outcome
    }

    fn chosen_tools(outcome: &DecisionOutcome) -> Vec<String> {
        let DecisionOutcome::Solved { slots, .. } = outcome else {
            panic!("the assembly solves: {outcome:?}")
        };
        let mut ids: Vec<String> = slots
            .iter()
            .map(|slot| slot.component.component_id.clone())
            .filter(|id| id.starts_with("t-"))
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// The independent oracle: every tool subset that covers every capability,
    /// ranked by total bytes, then total ledger cost, then the largest ids
    /// avoided first.
    fn brute_force() -> Vec<String> {
        let mut best: Option<((u64, u64, Vec<usize>), Vec<String>)> = None;
        for mask in 0u32..(1 << TOOLS.len()) {
            let picked: Vec<usize> = (0..TOOLS.len()).filter(|i| mask & (1 << i) != 0).collect();
            let covered =
                (0..CAPS.len()).all(|cap| picked.iter().any(|&i| TOOLS[i].1.contains(&cap)));
            if !covered {
                continue;
            }
            let bytes: u64 = picked.iter().map(|&i| TOOLS[i].2).sum();
            let cost: u64 = picked.iter().map(|&i| TOOLS[i].3).sum();
            let mut descending = picked.clone();
            descending.reverse();
            let key = (bytes, cost, descending);
            if best.as_ref().is_none_or(|(held, _)| key < *held) {
                let ids = picked.iter().map(|&i| TOOLS[i].0.to_string()).collect();
                best = Some((key, ids));
            }
        }
        best.expect("a covering subset exists").1
    }

    // spec: MEDULLA-DECISION-ENGINE-R014.100.2
    #[test]
    fn lexicographic_size_then_cost_matches_brute_force() {
        let outcome = solve(library(), UnknownCostRule::RankBelowKnown);
        assert_eq!(chosen_tools(&outcome), brute_force());
        // The same inputs always give the same answer.
        let again = solve(library(), UnknownCostRule::RankBelowKnown);
        assert_eq!(chosen_tools(&again), chosen_tools(&outcome));
    }

    // spec: MEDULLA-DECISION-ENGINE-R014.100.2
    #[test]
    fn unknown_size_follows_the_unknown_cost_rule() {
        let mut unmeasured = library();
        let position = unmeasured
            .iter()
            .position(|candidate| candidate.component_id == "t-3")
            .expect("t-3 is in the library");
        unmeasured[position].size = None;

        // Excluded when strict: the decision abstains and names the fact.
        let DecisionOutcome::Abstained { reasons } =
            solve(unmeasured.clone(), UnknownCostRule::ExcludeWhenStrict)
        else {
            panic!("an unknown size under ExcludeWhenStrict abstains")
        };
        assert!(reasons.iter().any(|reason| matches!(
            reason,
            AbstainReason::UnknownFact { component_id, field }
                if component_id == "t-3" && field == "artifact_bytes"
        )));

        // Ranked below every known value otherwise, never read as zero: the
        // all-in-one tool, which would be cheapest at zero bytes, is not chosen.
        let outcome = solve(unmeasured, UnknownCostRule::RankBelowKnown);
        assert!(!chosen_tools(&outcome).contains(&"t-3".to_string()));
    }
}
