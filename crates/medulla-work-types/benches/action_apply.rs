//! MEDULLA-UNIFIED-DATA-PLANE-R700.6 hot-path receipt: group `action_apply`.
//!
//! Budget (spec): p50 2 ms, p99 12 ms, peak memory 32 MiB per request. One
//! request registers-validates a two-criteria, two-effect action, submits a
//! failing value (no effect resolves) and then a passing value (both effects
//! resolve) through `ActionSpec::submit`, the call `ActionSubmit` runs.
//! Benchmarks never gate a PR.
//!
//! Deterministic: fixed parameter values, no RNG.
//!
//! Run: cargo bench -p medulla-work-types --bench action_apply

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use medulla_work_types::action_spec::{
    ActionEffectSpec, ActionParameter, ActionSpec, ActionStatus, CriterionOp, EffectKind,
    ParamType, ParamValue, SubmissionCriterion,
};

const WARMUP: usize = 200;
const ITERATIONS: usize = 5_000;

fn spec() -> ActionSpec {
    let criterion = |name: &str, op, value| SubmissionCriterion {
        name: name.to_string(),
        param: "amount".to_string(),
        op,
        value: ParamValue::Integer(value),
    };
    ActionSpec {
        name: "approve_refund".to_string(),
        params: vec![ActionParameter {
            name: "amount".to_string(),
            ty: ParamType::Integer,
            required: true,
        }],
        criteria: vec![
            criterion("positive", CriterionOp::Gt, 0),
            criterion("under_cap", CriterionOp::Le, 100),
        ],
        effects: vec![
            ActionEffectSpec {
                kind: EffectKind::CreateNode,
                template:
                    r#"{"id": "refund-1", "label": "Refund", "properties": {"amount": {{amount}}}}"#
                        .to_string(),
            },
            ActionEffectSpec {
                kind: EffectKind::SetProperty,
                template: r#"{"node": "acct-1", "property": "refunded", "value": {{amount}}}"#
                    .to_string(),
            },
        ],
    }
}

fn request(spec: &ActionSpec, attached: &BTreeSet<String>) -> usize {
    spec.validate().expect("valid spec");
    let submit = |amount| {
        spec.submit(
            BTreeMap::from([("amount".into(), ParamValue::Integer(amount))]),
            attached,
        )
    };
    let failed = submit(500);
    assert_eq!(failed.status, ActionStatus::Refused);
    assert!(failed.effects.is_empty());
    let passed = submit(60);
    assert_eq!(passed.status, ActionStatus::Applied);
    passed.effects.len()
}

/// The process's resident set in MiB (VmRSS), or NaN off Linux.
fn rss_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmRSS:"))
                .and_then(|rest| rest.trim().strip_suffix(" kB"))
                .and_then(|kb| kb.parse::<f64>().ok())
        })
        .map_or(f64::NAN, |kb| kb / 1024.0)
}

fn main() {
    let (spec, attached) = (spec(), BTreeSet::new());
    for _ in 0..WARMUP {
        assert_eq!(request(&spec, &attached), 2);
    }
    let rss_before = rss_mib();
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let effects = request(&spec, &attached);
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(effects, 2);
    }
    let rss_growth = rss_mib() - rss_before;
    samples.sort_by(f64::total_cmp);
    let at = |fraction: f64| samples[((samples.len() - 1) as f64 * fraction).round() as usize];
    println!(
        "action_apply: p50 {:.4} ms (budget 2), p99 {:.4} ms (budget 12), rss growth {rss_growth:.2} MiB (budget 32)",
        at(0.50),
        at(0.99)
    );
}
