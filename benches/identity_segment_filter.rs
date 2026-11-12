//! Hot-path journey of the visible-unit filter (MEDULLA-IDENTITY-R023.6).
//!
//! Nightly bench group `identity/segment_filter`: narrows a 10,000-unit view
//! once per read through the served read-scope helper `scope_view` (the body
//! of `filter_visible_units`), and budgets p95 under 2 ms per read. Records
//! p50, p95 and p99 per read into `target/identity-segment-filter-report.json`
//! (or `IDENTITY_SEGMENT_FILTER_REPORT`); `scripts/bench_gate.py` consumes the
//! group. The nightly job runs it; it never gates a pull request.
//!
//! Run: cargo bench --features full --bench identity_segment_filter
//!
//! spec: MEDULLA-IDENTITY-R023.6

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use medulla_graph_core::graph::{GraphCore, GraphView};
use medulla_identity_types::identity::UnitPrincipal;
use medulla_identity_types::tenancy::TenantId;
use medulla_server_api::read_scope::{scope_view, ReadCaller};

const UNITS_PER_READ: usize = 10_000;
const TENANTS: usize = 10;
const READS: usize = 200;
const WARMUP: usize = 20;
const P95_BUDGET_MS: f64 = 2.0;

/// The reader: an owner outside every tenant's owner pool, in tenant 0.
fn caller() -> ReadCaller {
    ReadCaller {
        principal: UnitPrincipal {
            id: "usr-reader".to_owned(),
            placement: "g".to_owned(),
            tenant: "tenant-0".to_owned(),
        },
        tenant: TenantId::new("tenant-0").ok(),
    }
}

/// A view of `UNITS_PER_READ` knowledge units: half tenant-shared, half
/// private, spread over `TENANTS` tenants.
fn build_view() -> (Arc<GraphView>, ReadCaller) {
    let core = GraphCore::new();
    let mut owners: BTreeMap<String, UnitPrincipal> = BTreeMap::new();
    for index in 0..UNITS_PER_READ {
        let tenant_index = index % TENANTS;
        let tenant = format!("tenant-{tenant_index}");
        let scope = if index % 2 == 0 { "tenant" } else { "private" };
        let owner = format!("usr-owner-{tenant_index}-{}", index % 200);
        owners
            .entry(owner.clone())
            .or_insert_with(|| UnitPrincipal {
                id: owner.clone(),
                placement: "g".to_owned(),
                tenant: tenant.clone(),
            });
        let props = serde_json::json!({
            "node_type": "KnowledgeUnit",
            "owner": owner,
            "owner_tenant": tenant,
            "version": 1,
            "share_scope": scope,
        });
        core.add_node(
            format!("ku-{index:06}"),
            rmp_serde::to_vec_named(&props).expect("encode unit"),
        );
    }
    (Arc::new(core.analysis_snapshot()), caller())
}

fn percentile(sorted: &[f64], percent: f64) -> f64 {
    let rank = (percent / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn main() {
    let (view, caller) = build_view();
    let plan_variables = vec!["n".to_owned()];

    for _ in 0..WARMUP {
        scope_view(view.clone(), &caller, &plan_variables).expect("view is scopable");
    }

    let mut samples_ms: Vec<f64> = Vec::with_capacity(READS);
    let mut hidden = 0usize;
    for _ in 0..READS {
        let started = Instant::now();
        let scoped = scope_view(view.clone(), &caller, &plan_variables).expect("view is scopable");
        samples_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        hidden = scoped.hidden_count;
    }

    samples_ms.sort_by(|a, b| a.partial_cmp(b).expect("durations order"));
    let p50 = percentile(&samples_ms, 50.0);
    let p95 = percentile(&samples_ms, 95.0);
    let p99 = percentile(&samples_ms, 99.0);

    assert!(hidden > UNITS_PER_READ / 2, "expected most units hidden");
    let body = serde_json::json!({
        "groups": {
            "identity/segment_filter": {
                "units_per_read": UNITS_PER_READ,
                "reads": READS,
                "hidden_units": hidden,
                "p50_ms": p50,
                "p95_ms": p95,
                "p99_ms": p99,
            }
        }
    });
    let path = std::env::var("IDENTITY_SEGMENT_FILTER_REPORT")
        .unwrap_or_else(|_| "target/identity-segment-filter-report.json".to_owned());
    std::fs::write(&path, serde_json::to_vec_pretty(&body).unwrap())
        .expect("write segment filter report");

    println!(
        "identity/segment_filter: {UNITS_PER_READ} units/read, {READS} reads, \
         hidden {hidden} — p50 {p50:.3} ms, p95 {p95:.3} ms, p99 {p99:.3} ms"
    );
    if p95 > P95_BUDGET_MS {
        println!("FAIL: p95 {p95:.3} ms over the {P95_BUDGET_MS} ms budget");
        std::process::exit(1);
    }
}
