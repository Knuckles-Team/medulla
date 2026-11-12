//! Nightly bench for incremental view maintenance (MEDULLA-INCREMENTAL-VIEWS-R001.10).
//!
//! A `Scan Doc |> Filter year > 2000` circuit is seeded with 1,000,000 member rows, then
//! 1,000-row deltas (500 retractions and 500 insertions, so the view stays the same size)
//! are applied and timed one by one.
//!
//! Budgets, enforced by this bench (it exits non-zero on a miss):
//! - p99 at most 20 ms per 1,000-row delta;
//! - at most 64 bytes of serialized circuit state per member row (the size of the
//!   `matview_operator_state` row, which is what the circuit persists).
//!
//! Run: `cargo bench -p medulla-plan --features query --bench incremental_delta`

use medulla_exec::algebra::{Op, Plan, Pred};
use medulla_exec::incremental::Circuit;
use medulla_types::delta::{Delta, ZRow};
use serde_json::{json, Map, Value};
use std::hint::black_box;
use std::time::{Duration, Instant};

const SEED_ROWS: usize = 1_000_000;
const SEED_CHUNK: usize = 10_000;
const DELTA_ROWS: usize = 1_000;
const SAMPLES: usize = 300;
const P99_BUDGET: Duration = Duration::from_millis(20);
const STATE_BYTES_PER_ROW: usize = 64;

fn plan() -> Plan {
    Plan::new(vec![
        Op::Scan {
            label: "Doc".into(),
        },
        Op::Filter {
            preds: vec![Pred::GtNum {
                prop: "year".into(),
                n: 2000.0,
            }],
        },
    ])
}

fn doc_id(i: usize) -> String {
    format!("d{i}")
}

/// Properties of doc `i`: always a member of the view (`year > 2000`).
fn props(i: usize) -> Map<String, Value> {
    match json!({ "type": "Doc", "year": 2001 + (i % 20) }) {
        Value::Object(map) => map,
        _ => unreachable!("json! object literal"),
    }
}

fn seed(circuit: &mut Circuit) {
    for start in (0..SEED_ROWS).step_by(SEED_CHUNK) {
        let rows = (start..start + SEED_CHUNK)
            .map(|i| ZRow::insert(doc_id(i), props(i)))
            .collect::<Vec<_>>();
        circuit.apply(&Delta::from(rows)).expect("seed delta");
    }
}

/// Delta `n`: retract the oldest `DELTA_ROWS / 2` live docs, insert as many new ones.
fn delta(n: usize) -> Delta {
    let half = DELTA_ROWS / 2;
    let old = n * half;
    let fresh = SEED_ROWS + n * half;
    let retract = (old..old + half).map(|i| ZRow::retract(doc_id(i), props(i)));
    let insert = (fresh..fresh + half).map(|i| ZRow::insert(doc_id(i), props(i)));
    Delta::from(retract.chain(insert).collect::<Vec<_>>())
}

fn p99(samples: &mut [Duration]) -> Duration {
    samples.sort();
    samples[(samples.len() * 99).div_ceil(100) - 1]
}

fn main() {
    let mut circuit = Circuit::compile(&plan()).expect("plan is incrementally maintainable");
    seed(&mut circuit);
    assert_eq!(circuit.current().rows().len(), SEED_ROWS, "seeded members");

    let mut samples = Vec::with_capacity(SAMPLES);
    for n in 0..SAMPLES {
        let d = delta(n);
        let started = Instant::now();
        black_box(circuit.apply(&d).expect("delta applies"));
        samples.push(started.elapsed());
    }
    let p99 = p99(&mut samples);

    let members = circuit.current().rows().len();
    let state = rmp_serde::to_vec_named(&circuit)
        .expect("circuit state")
        .len();
    let per_row = state / members.max(1);
    println!("incremental_delta: p99 {p99:?} per {DELTA_ROWS}-row delta, {per_row} B state per member row");
    assert!(p99 <= P99_BUDGET, "p99 {p99:?} exceeds {P99_BUDGET:?}");
    assert!(
        per_row <= STATE_BYTES_PER_ROW,
        "{per_row} B per member row exceeds {STATE_BYTES_PER_ROW}"
    );
}
