//! Nightly hot-path proof for the schedule tick (MEDULLA-PLACEMENT-R751.753).
//!
//! One tick pass over 100,000 interval schedules: computes each schedule's due windows for a
//! one-second pass and records each fresh window in the `TickLedger`. Before timing with
//! criterion it asserts the budgets: p50 below 3 ms and p99 below 9 ms over 50 passes, and
//! schedule memory below 24 MiB. The bench never gates a PR.
//!
//! Run: cargo bench -p medulla-jobs schedule_tick

// spec: MEDULLA-PLACEMENT-R751.753

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_jobs::schedule::{ScheduleSpec, ScheduleTick, TickLedger};

const SCHEDULES: usize = 100_000;
const PASSES: usize = 50;
const P50_BUDGET: Duration = Duration::from_millis(3);
const P99_BUDGET: Duration = Duration::from_millis(9);
const MEMORY_BUDGET_BYTES: usize = 24 * 1024 * 1024;

fn schedules() -> Vec<(String, ScheduleSpec)> {
    (0..SCHEDULES)
        .map(|i| {
            (
                format!("schedule:t:{i}"),
                ScheduleSpec::Interval {
                    every_ms: 1_000 + (i as u64 % 60) * 1_000,
                    start_ms: 0,
                },
            )
        })
        .collect()
}

fn pass(schedules: &[(String, ScheduleSpec)], ledger: &mut TickLedger, from_ms: u64) -> usize {
    let mut fresh = 0;
    for (id, spec) in schedules {
        for window in spec.due_windows(from_ms, from_ms + 1_000).unwrap() {
            let tick = ScheduleTick {
                schedule_id: id.clone(),
                window_start_ms: window,
            };
            if ledger.record(&tick).is_ok() {
                ledger.acknowledge(&tick);
                fresh += 1;
            }
        }
    }
    fresh
}

fn assert_budgets() {
    let list = schedules();
    let memory = list.len() * std::mem::size_of::<(String, ScheduleSpec)>() + list.len() * 24;
    assert!(
        memory < MEMORY_BUDGET_BYTES,
        "schedule memory {memory} B over budget"
    );
    let mut ledger = TickLedger::default();
    let mut samples = Vec::with_capacity(PASSES);
    for i in 0..PASSES {
        let started = Instant::now();
        black_box(pass(&list, &mut ledger, 1_000_000 + i as u64 * 1_000));
        samples.push(started.elapsed());
    }
    samples.sort();
    let p50 = samples[PASSES / 2];
    let p99 = samples[PASSES * 99 / 100];
    assert!(p50 < P50_BUDGET, "p50 {p50:?} over {P50_BUDGET:?}");
    assert!(p99 < P99_BUDGET, "p99 {p99:?} over {P99_BUDGET:?}");
}

fn bench(c: &mut Criterion) {
    assert_budgets();
    let list = schedules();
    c.bench_function("jobs/schedule_tick", |b| {
        let mut ledger = TickLedger::default();
        let mut from = 1_000_000u64;
        b.iter(|| {
            from += 1_000;
            black_box(pass(&list, &mut ledger, from))
        })
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
