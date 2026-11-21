//! MEDULLA-SDLC-R017.1 — hot-path budget of the served goal record.
//!
//! `Method::GoalPut` → `dispatch_sdlc_methods` → `handle_goal_put`: each record
//! decides over a goal with 20 targets. The p99 per record must stay at most 20 ms.
//! The benchmark gates only the nightly run, never a pull request.
//!
//! Run: cargo bench -p medulla-h-ingest --bench lifecycle_project_mode

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_source_types::lifecycle::{handle_goal_put, Goal, GoalTarget};

const RECORDS: u64 = 1_000;
const P99: Duration = Duration::from_millis(20);

fn goal(n: u64) -> Goal {
    Goal {
        project: "medulla".into(),
        statement: format!("goal {n}"),
        targets: (0..20)
            .map(|t| GoalTarget::Row {
                repo: "medulla".into(),
                id: format!("MEDULLA-SDLC-R{t:03}"),
            })
            .collect(),
    }
}

fn goal_put(c: &mut Criterion) {
    let mut samples: Vec<Duration> = (0..RECORDS)
        .map(|n| {
            let input = goal(n);
            let started = Instant::now();
            black_box(handle_goal_put(black_box(input))).expect("a goal with targets writes");
            started.elapsed()
        })
        .collect();
    samples.sort();
    let p99 = samples[((samples.len() - 1) as f64 * 0.99).ceil() as usize];
    assert!(p99 <= P99, "p99 {p99:?} over the 20 ms budget");
    let mut group = c.benchmark_group("lifecycle/project_mode");
    group.bench_function("goal_put", |b| {
        let mut n = 0;
        b.iter(|| {
            n += 1;
            handle_goal_put(black_box(goal(n)))
        })
    });
    group.finish();
}

criterion_group!(benches, goal_put);
criterion_main!(benches);
