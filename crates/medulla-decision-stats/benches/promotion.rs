//! MEDULLA-DECISION-ENGINE-R805.6: the `decide/promotion` group times the two
//! pure gate rules, `challenger_wins` and `regression_rollback`, over 10000
//! cases each; JSON on stdout. The pointer compare-and-set budget (5 ms p99 on
//! the reference host) belongs to the served journey, not to these kernels, so
//! this group holds the gate rules at p99 20 microseconds per case in the
//! optimized profile. A benchmark never gates a PR.
//! Run with `cargo bench -p medulla-decision-stats --features decision --bench promotion`.

use std::hint::black_box;
use std::time::Instant;

use medulla_decision_outcome_types::decision::promotion::{
    ChampionPointer, GraderKind, HeldOutEvidence, ReceiptKind, RevisionSubject,
    HELD_OUT_EVIDENCE_SCHEMA_VERSION,
};
use medulla_decision_stats::decision::promotion::{challenger_wins, regression_rollback};
use medulla_decision_types::decision::DecisionPolicy;
use medulla_types::contract::OpaqueId;

const CASES: usize = 10_000;
const BUDGET_P99_US: u128 = 20;

fn evidence(index: usize) -> HeldOutEvidence {
    let challenger = 600_000 + (index as i64 % 400) * 500;
    let champion = 600_000 + (index as i64 % 250) * 400;
    HeldOutEvidence {
        schema_version: HELD_OUT_EVIDENCE_SCHEMA_VERSION,
        subject: RevisionSubject::Program {
            program_ref: "program-bench".to_owned(),
        },
        challenger_ref: format!("revision:{index}"),
        champion_ref: Some("revision:champion".to_owned()),
        receipt_kind: ReceiptKind::RolloutSet,
        receipt_digest: format!("sha256:{index:064x}"),
        evaluation_set_digest: "sha256:set".to_owned(),
        challenger_score_ppm: challenger,
        champion_score_ppm: Some(champion),
        delta_ppm: challenger - champion,
        case_count: 50 + (index as u64 % 200),
        independent: true,
        grader_kind: GraderKind::TraceMetric,
        measurement: None,
        review: None,
    }
}

fn percentile(mut micros: Vec<u128>, quantile: usize) -> u128 {
    micros.sort_unstable();
    micros[micros.len() * quantile / 100]
}

fn main() {
    let policy = DecisionPolicy::engine_default();
    let pointer = ChampionPointer {
        subject: RevisionSubject::Program {
            program_ref: "program-bench".to_owned(),
        },
        revision_ref: OpaqueId::new("revision:champion".to_owned()).unwrap(),
        parent_ref: Some(OpaqueId::new("revision:previous".to_owned()).unwrap()),
        promoted_by_decision: OpaqueId::new("decision:promoted".to_owned()).unwrap(),
    };
    let cases: Vec<HeldOutEvidence> = (0..CASES).map(evidence).collect();
    let promoted = evidence(0);

    let mut wins = Vec::with_capacity(CASES);
    for case in &cases {
        let start = Instant::now();
        black_box(challenger_wins(black_box(case), &policy));
        wins.push(start.elapsed().as_micros());
    }
    let mut rollbacks = Vec::with_capacity(CASES);
    for case in &cases {
        let start = Instant::now();
        black_box(regression_rollback(&pointer, black_box(&promoted), black_box(case), &policy));
        rollbacks.push(start.elapsed().as_micros());
    }
    let (wins_p99, rollback_p99) = (percentile(wins.clone(), 99), percentile(rollbacks.clone(), 99));
    println!(
        "{{\"bench\":\"decide/promotion\",\"cases\":{CASES},\
         \"challenger_wins_p50_us\":{},\"challenger_wins_p99_us\":{wins_p99},\
         \"regression_rollback_p50_us\":{},\"regression_rollback_p99_us\":{rollback_p99},\
         \"budget_p99_us\":{BUDGET_P99_US},\"optimized\":{}}}",
        percentile(wins, 50),
        percentile(rollbacks, 50),
        !cfg!(debug_assertions)
    );
    if !cfg!(debug_assertions) {
        assert!(wins_p99 <= BUDGET_P99_US, "challenger_wins exceeded its budget");
        assert!(rollback_p99 <= BUDGET_P99_US, "regression_rollback exceeded its budget");
    }
}
