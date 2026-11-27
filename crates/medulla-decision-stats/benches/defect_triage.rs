//! MEDULLA-SDLC-R004.6: the `decide/defect_triage` group times the triage of a defect of 20
//! stack frames over a graph of 1,000,000 indexed symbols. The symbol index is built once from
//! generated symbols, each frame is resolved through `SymbolIndex::resolve_frame` (the one
//! stack-frame resolver), and the resolved suspects are ranked, matched against 1,000 open
//! defects and decided by the pure triage kernel. The p99 per triage must stay at most 10 ms.
//! A benchmark never gates a PR.
//!
//! Run with `cargo bench -p medulla-decision-stats --features decision --bench defect_triage`.

use std::hint::black_box;
use std::time::Instant;

use medulla_decision_outcome_types::decision::defect_triage::{
    cap_frames, decide_triage, find_duplicate, pick_owner, rank_suspects, OpenDefect, SuspectInput,
    TriageEvidence, TriageKnobs,
};
use medulla_decision_types::decision::DecisionPolicy;
use medulla_source_types::lifecycle::Severity;
use medulla_source_types::stack_frame::{SymbolIndex, SymbolRef};

const SYMBOLS: usize = 1_000_000;
const FRAMES: usize = 20;
const OPEN_DEFECTS: usize = 1_000;
const SAMPLES: usize = 200;
const BUDGET_P99_MS: u128 = 10;

fn symbols() -> Vec<SymbolRef> {
    (0..SYMBOLS)
        .map(|n| SymbolRef {
            id: format!("symbol:{n}"),
            file_path: format!("src/module_{}/file_{}.rs", n % 5_000, n / 5_000),
            line: 10 + (n % 90) as u64 * 10,
            end_line: 19 + (n % 90) as u64 * 10,
            qualified_symbol: format!("module_{}::function_{n}", n % 5_000),
        })
        .collect()
}

fn frames(round: usize) -> Vec<String> {
    (0..FRAMES)
        .map(|k| {
            let n = (round * 7_919 + k * 50_021) % SYMBOLS;
            let line = 12 + (n % 90) * 10;
            format!(
                "at run (src/module_{}/file_{}.rs:{line}:3)",
                n % 5_000,
                n / 5_000
            )
        })
        .collect()
}

fn open_defects() -> Vec<OpenDefect> {
    (0..OPEN_DEFECTS)
        .map(|n| OpenDefect {
            defect_id: format!("defect-{n}"),
            repository: "bench".into(),
            suspects: (0..5).map(|k| format!("symbol:{}", n * 5 + k)).collect(),
        })
        .collect()
}

fn main() {
    let policy = DecisionPolicy::engine_default();
    let knobs = TriageKnobs::from_policy(&policy);
    let index = SymbolIndex::new(symbols());
    let open = open_defects();
    let owners = vec!["owner-1".to_string()];
    let mut samples: Vec<u128> = Vec::with_capacity(SAMPLES);
    for round in 0..SAMPLES {
        let lines = frames(round);
        let started = Instant::now();
        let kept = cap_frames(&lines, &knobs);
        let inputs: Vec<SuspectInput> = kept
            .iter()
            .filter_map(|line| index.resolve_frame(line))
            .map(|symbol| SuspectInput {
                symbol_id: symbol.id.clone(),
                blame_changeset: format!("changeset-{}", symbol.line % 50),
                blame_at: 1_000 + symbol.line,
            })
            .collect();
        let suspects = rank_suspects(&inputs, None, 2_000, &knobs);
        let duplicate = find_duplicate("bench", &suspects, &open, &knobs);
        let owner = pick_owner(&owners, &[]);
        let decision = decide_triage(
            "defect-bench",
            Severity::Medium,
            lines.len() as u32,
            TriageEvidence {
                suspects,
                owner,
                duplicate_of: duplicate,
                evidence_refs: kept
                    .iter()
                    .map(|line| format!("symptom:{}", line.len()))
                    .collect(),
            },
            &knobs,
        );
        black_box(decision);
        samples.push(started.elapsed().as_micros());
    }
    samples.sort_unstable();
    let p99_us = samples[((samples.len() - 1) as f64 * 0.99).ceil() as usize];
    println!(
        "{{\"group\":\"decide/defect_triage\",\"symbols\":{SYMBOLS},\"frames\":{FRAMES},\"p99_us\":{p99_us}}}"
    );
    assert!(
        p99_us <= BUDGET_P99_MS * 1_000,
        "p99 {p99_us} us over the {BUDGET_P99_MS} ms budget"
    );
}
