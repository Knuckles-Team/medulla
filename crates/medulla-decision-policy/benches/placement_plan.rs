//! Placement-plan solve bench (MEDULLA-PLACEMENT-R700.6): group `decide/placement_plan`.
//!
//! One plan of 1,000 services over 100 hosts, every service on a declared host that has room,
//! so the exact decomposition keeps every service in place; and one plan where a tenth of the
//! services sit on a host that cannot hold them. The nightly budget for the 1,000-service case
//! is p50 below 12 ms, p99 below 80 ms and peak memory below 64 MiB; the bench never gates a
//! pull request.
//!
//! Run: cargo bench -p medulla-decision-policy --features solve --bench placement_plan

use criterion::{criterion_group, criterion_main, Criterion};
use medulla_decision_policy::solve::placement::{plan_services, PlanOutcome};
use medulla_decision_types::decision::knob_families::placement::PlanKnobs;
use medulla_decision_types::decision::statistical::placement_plan::{HostBudget, ServiceSpec};
use std::hint::black_box;

const HOSTS: usize = 100;
const SERVICES: usize = 1_000;
const GIB: u64 = 1 << 30;

fn hosts() -> Vec<HostBudget> {
    (0..HOSTS)
        .map(|i| HostBudget {
            host_iri: format!("urn:host:{i:04}"),
            cpu_millis: 16_000,
            memory_bytes: 64 * GIB,
            storage_bytes: 1_000 * GIB,
            capabilities: Vec::new(),
        })
        .collect()
}

/// Every service sits on `i % HOSTS`; the first `crowded` services all sit on host 0, which
/// cannot hold them together, so the program holds the services that must move.
fn services(crowded: usize) -> Vec<ServiceSpec> {
    (0..SERVICES)
        .map(|i| ServiceSpec {
            service_iri: format!("urn:service:{i:05}"),
            cpu_millis: 500,
            memory_bytes: 2 * GIB,
            storage_bytes: 10 * GIB,
            required_capability: None,
            current_host: Some(if i < crowded {
                "urn:host:0000".to_string()
            } else {
                format!("urn:host:{:04}", i % HOSTS)
            }),
        })
        .collect()
}

fn bench(c: &mut Criterion) {
    let knobs = PlanKnobs {
        node_budget: 200_000,
    };
    let hosts = hosts();
    let settled = services(0);
    let crowded = services(SERVICES / 10);

    // The settled fleet plans with no move; the crowded fleet solves.
    let PlanOutcome::Planned(plan) = plan_services(&hosts, &settled, &knobs).unwrap() else {
        panic!("a settled fleet plans")
    };
    assert!(plan.moves.is_empty());
    assert!(matches!(
        plan_services(&hosts, &crowded, &knobs).unwrap(),
        PlanOutcome::Planned(_) | PlanOutcome::Abstained(_)
    ));

    let mut group = c.benchmark_group("decide/placement_plan");
    group.sample_size(20);
    group.bench_function("settled_1000_services", |b| {
        b.iter(|| black_box(plan_services(&hosts, &settled, &knobs).unwrap()))
    });
    group.bench_function("crowded_1000_services", |b| {
        b.iter(|| black_box(plan_services(&hosts, &crowded, &knobs).unwrap()))
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
