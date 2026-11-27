//! Stewardship reviewer resolution (MEDULLA-TYPED-PACKS-R802.6).
//!
//! Nightly bench group `decide/stewardship_resolve`: `resolve` over 100,000 assignments.
//! Budget per resolution: p99 below 1 ms. The run exits non-zero when the budget is exceeded.
//! A service level, never a pull request gate; the gate is the served change-request journey.
//!
// spec: MEDULLA-TYPED-PACKS-R802.6
//!
//! Run: cargo bench -p medulla-core --bench stewardship_resolve

use std::time::{Duration, Instant};

use medulla_catalog_types::stewardship::{StewardRole, StewardshipAssignment, StewardshipScope};
use medulla_core::stewardship::{resolve, TargetScopes};

const ASSIGNMENTS: usize = 100_000;
const RESOLUTIONS: usize = 200;
const BUDGET_P99: Duration = Duration::from_millis(1);

fn main() {
    let assignments: Vec<StewardshipAssignment> = (0..ASSIGNMENTS)
        .map(|n| StewardshipAssignment {
            scope: match n % 3 {
                0 => StewardshipScope::Asset(format!("table:bench-{n}")),
                1 => StewardshipScope::ConceptScheme(format!("scheme:bench-{n}")),
                _ => StewardshipScope::Domain(format!("domain:bench-{n}")),
            },
            role: StewardRole::Steward,
            principal: format!("steward-{}", n % 97),
        })
        .collect();
    let target = TargetScopes {
        asset: Some("table:bench-99999".to_string()),
        concept_scheme: Some("scheme:bench-99997".to_string()),
        domain: Some("domain:bench-99998".to_string()),
    };
    let mut samples = Vec::with_capacity(RESOLUTIONS);
    for _ in 0..RESOLUTIONS {
        let started = Instant::now();
        let governing = resolve(&assignments, &target);
        samples.push(started.elapsed());
        assert_eq!(governing.len(), 1);
    }
    samples.sort();
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    println!("decide/stewardship_resolve p99 = {p99:?} (budget {BUDGET_P99:?})");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
}
