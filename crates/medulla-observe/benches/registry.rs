//! Budgets of the registries: register or progress at most 2 microseconds p99, `spawn` under
//! 3 microseconds over a bare spawn, and a progress guard check under 100 nanoseconds.

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_observe::progress::ProgressGuard;
use medulla_observe::registry::tasks::{
    Outcome, ProgressCursor, TaskKind, TaskRegistry, TaskSpec,
};

fn spec() -> TaskSpec {
    TaskSpec {
        name: "bench".to_string(),
        owner: "bench".to_string(),
        kind: TaskKind::External,
    }
}

fn register_progress_finish(c: &mut Criterion) {
    let registry = TaskRegistry::new(4096);
    c.bench_function("register_and_finish", |b| {
        b.iter(|| {
            let handle = registry.register(spec());
            handle.finish(Outcome::Completed);
        })
    });
    let handle = registry.register(spec());
    let mut units = 0_u64;
    c.bench_function("progress", |b| {
        b.iter(|| {
            units += 1;
            handle.progress(ProgressCursor::Counter(units), units);
        })
    });
    handle.finish(Outcome::Completed);
}

fn spawn_over_bare_spawn(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .expect("bench runtime");
    let _guard = runtime.enter();
    c.bench_function("spawn_registered_job", |b| {
        b.iter(|| {
            let job = medulla_observe::spawn::spawn(spec(), async {});
            runtime.block_on(job).expect("job");
        })
    });
    c.bench_function("bare_spawn", |b| {
        b.iter(|| {
            let job = runtime.spawn(async {});
            runtime.block_on(job).expect("job");
        })
    });
}

fn guard_check(c: &mut Criterion) {
    let mut guard = ProgressGuard::with_limit("bench", u32::MAX);
    let mut counter = 0_u64;
    c.bench_function("progress_guard_check", |b| {
        b.iter(|| {
            counter += 1;
            guard
                .check(&ProgressCursor::Counter(counter))
                .expect("moving cursor");
        })
    });
}

criterion_group!(benches, register_progress_finish, spawn_over_bare_spawn, guard_check);
criterion_main!(benches);
