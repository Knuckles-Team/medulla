//! Source group expansion on the read gate (MEDULLA-IDENTITY-R706).
//!
//! Nightly bench group `ingest/group_expansion`: a record restricted to a group is
//! decided for a caller against a group of 100,000 members. The per-record decision
//! (decode of the access properties, then group expansion) must stay under 50
//! microseconds at p99. The run exits non-zero when the budget is exceeded. A service
//! level, not a pull request gate.
//!
// spec: MEDULLA-IDENTITY-R706
//!
//! Run: cargo bench --bench ingest_group_expansion

use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla_graph_core::rls::{source_access, source_access_verdict, SourceGroups};

const MEMBERS: usize = 100_000;
const READS: usize = 10_000;
const BUDGET_P99: Duration = Duration::from_micros(50);

fn main() {
    let members: Vec<String> = (0..MEMBERS).map(|i| format!("usr:{i}")).collect();
    let group_blob = rmp_serde::to_vec_named(&serde_json::json!({
        "type": "SourceGroup", "group_id": "eng", "members": members,
    }))
    .expect("group encodes");
    let groups = Arc::new(SourceGroups::from_nodes([group_blob.as_slice()]));
    let record_blob = rmp_serde::to_vec_named(&serde_json::json!({
        "source_access_public": false,
        "source_access_read_roles": ["audit", "finance", "group:ops", "group:eng"],
        "source_classification": "restricted",
        "acl_synced_at_ms": 1_000u64,
    }))
    .expect("record encodes");
    let mut samples = Vec::with_capacity(READS);
    for i in 0..READS {
        let principal = format!("usr:{}", (i * 7) % (MEMBERS * 2));
        let started = Instant::now();
        let access = source_access(&record_blob).expect("governed record");
        let shared = groups.clone();
        let verdict = source_access_verdict(
            &access,
            &principal,
            &[],
            &mut || shared.clone(),
            2_000,
            3_600,
        );
        samples.push(started.elapsed());
        std::hint::black_box(verdict);
    }
    samples.sort();
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    println!("ingest/group_expansion p99 = {p99:?} (budget {BUDGET_P99:?})");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
}
