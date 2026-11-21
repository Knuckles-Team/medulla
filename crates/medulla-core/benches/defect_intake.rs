//! Defect intake (MEDULLA-SDLC-R003.6).
//!
//! Nightly bench group `lifecycle/defect_intake`: `DefectBook::file` over 100,000 symptoms of
//! 1,000 signatures. Budget per symptom: p99 below 1 ms. The run exits non-zero when the budget
//! is exceeded. A service level, never a pull request gate; the gate is the served journey.
//!
// spec: MEDULLA-SDLC-R003.6
//!
//! Run: cargo bench -p medulla-core --bench defect_intake

use std::time::{Duration, Instant};

use medulla_core::lifecycle::defect::DefectBook;
use medulla_source_types::lifecycle::{DefectIntake, Symptom};

const SYMPTOMS: usize = 100_000;
const SIGNATURES: usize = 1_000;
const BUDGET_P99: Duration = Duration::from_millis(1);

fn main() {
    let mut book = DefectBook::from_defects(Vec::new());
    let mut samples = Vec::with_capacity(SYMPTOMS);
    for index in 0..SYMPTOMS {
        let signature = index % SIGNATURES;
        let symptom = Symptom {
            intake: DefectIntake::Direct,
            source_record_id: format!("record-{index}"),
            observed_at: index as u64,
            // Line numbers and addresses normalize away; the signature name stays.
            text: format!("panic in module{signature} at src/a.rs:{index} addr 0x{index:x}"),
        };
        let started = Instant::now();
        book.file(
            "bench",
            DefectIntake::Direct,
            &format!("signature {signature}"),
            None,
            symptom,
        );
        samples.push(started.elapsed());
    }
    assert_eq!(book.defects().len(), SIGNATURES);
    samples.sort();
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    println!("lifecycle/defect_intake p99 = {p99:?} (budget {BUDGET_P99:?})");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
}
