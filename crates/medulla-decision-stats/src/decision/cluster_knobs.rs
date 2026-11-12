//! Knobs of the clustering kernel (MEDULLA-CLUSTER-R002.5).
//!
//! The kernel holds the same defaults as constants in
//! `medulla_numeric::cluster`; a test below asserts the table and the constants agree.

use medulla_decision_types::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use medulla_decision_types::decision::DecisionPolicy;
use medulla_numeric::cluster::ClusterConfig;

const fn spec(
    name: &'static str,
    unit: &'static str,
    default: KnobValue,
    min: KnobValue,
    max: KnobValue,
    safety: KnobSafety,
) -> KnobSpec {
    KnobSpec {
        name,
        unit,
        default,
        min,
        max,
        safety,
    }
}

use KnobSafety::{Fixed, Tunable};
use KnobValue::{DecimalMicros as Dec, Integer as Int};

/// The knobs of the `cluster` family, one entry per line.
pub const KNOBS: &[KnobSpec] = &[
    spec(
        "cluster.batch_rows",
        "rows",
        Int(0),
        Int(0),
        Int(1_048_576),
        Tunable,
    ),
    spec(
        "cluster.batch_max_rows",
        "rows",
        Int(65_536),
        Int(1_024),
        Int(1_048_576),
        Tunable,
    ),
    spec(
        "cluster.epochs",
        "passes",
        Dec(3_000_000),
        Dec(500_000),
        Dec(50_000_000),
        Tunable,
    ),
    spec(
        "cluster.step_budget_madds",
        "multiply-adds",
        Int(2_147_483_648),
        Int(1_000_000),
        Int(1_000_000_000_000),
        Tunable,
    ),
    spec(
        "cluster.min_steps",
        "steps",
        Int(8),
        Int(1),
        Int(1_000),
        Tunable,
    ),
    spec(
        "cluster.max_steps",
        "steps",
        Int(300),
        Int(1),
        Int(10_000),
        Fixed,
    ),
    spec(
        "cluster.tol",
        "relative inertia change",
        Dec(1_000),
        Dec(1),
        Dec(100_000),
        Tunable,
    ),
    spec(
        "cluster.patience",
        "steps",
        Int(3),
        Int(1),
        Int(50),
        Tunable,
    ),
    spec(
        "cluster.empty_patience",
        "steps",
        Int(2),
        Int(1),
        Int(20),
        Tunable,
    ),
    spec(
        "cluster.ema_window",
        "points",
        Int(8_192),
        Int(64),
        Int(100_000_000),
        Tunable,
    ),
    spec(
        "cluster.plusplus_max_k",
        "cells",
        Int(4_096),
        Int(2),
        Int(65_536),
        Tunable,
    ),
    spec(
        "cluster.memory_budget_bytes",
        "bytes",
        Int(1_073_741_824),
        Int(1_048_576),
        Int(68_719_476_736),
        Tunable,
    ),
    spec(
        "cluster.threads",
        "threads",
        Int(0),
        Int(0),
        Int(256),
        Tunable,
    ),
    spec(
        "cluster.refresh_drift_ratio",
        "ratio",
        Dec(1_500_000),
        Dec(1_050_000),
        Dec(10_000_000),
        Tunable,
    ),
    spec(
        "cluster.online_window",
        "points",
        Int(4_096),
        Int(64),
        Int(100_000_000),
        Tunable,
    ),
    spec(
        "cluster.snapshot_chunk_rows",
        "rows",
        Int(4_096),
        Int(256),
        Int(1_048_576),
        Tunable,
    ),
];

fn int(policy: &DecisionPolicy, name: &str) -> u64 {
    let spec = KNOBS.iter().find(|s| s.name == name);
    match spec.map(|s| policy.knob(s)) {
        Some(KnobValue::Integer(v)) => v.max(0) as u64,
        _ => 0,
    }
}

fn dec(policy: &DecisionPolicy, name: &str) -> f64 {
    let spec = KNOBS.iter().find(|s| s.name == name);
    match spec.map(|s| policy.knob(s)) {
        Some(KnobValue::DecimalMicros(v)) => v as f64 / 1_000_000.0,
        _ => 0.0,
    }
}

/// The kernel configuration a pinned policy carries, or the knob defaults.
pub fn config_from_policy(policy: &DecisionPolicy) -> ClusterConfig {
    ClusterConfig {
        batch_rows: int(policy, "cluster.batch_rows") as usize,
        batch_max_rows: int(policy, "cluster.batch_max_rows") as usize,
        epochs: dec(policy, "cluster.epochs"),
        step_budget_madds: int(policy, "cluster.step_budget_madds"),
        min_steps: int(policy, "cluster.min_steps") as u32,
        max_steps: int(policy, "cluster.max_steps") as u32,
        tol: dec(policy, "cluster.tol"),
        patience: int(policy, "cluster.patience") as u32,
        empty_patience: int(policy, "cluster.empty_patience") as u32,
        ema_window: int(policy, "cluster.ema_window"),
        plusplus_max_k: int(policy, "cluster.plusplus_max_k") as usize,
        memory_budget_bytes: int(policy, "cluster.memory_budget_bytes"),
        threads: int(policy, "cluster.threads") as usize,
        refresh_drift_ratio: dec(policy, "cluster.refresh_drift_ratio"),
        online_window: int(policy, "cluster.online_window"),
        snapshot_chunk_rows: int(policy, "cluster.snapshot_chunk_rows") as usize,
        ..ClusterConfig::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_numeric::cluster as k;
    use medulla_decision_types::decision::knobs::{check_revision, KnobName, KnobRefusalCode};
    use std::collections::BTreeMap;

    // spec: MEDULLA-CLUSTER-R002.5
    #[test]
    fn defaults_lie_inside_bounds_and_equal_kernel_defaults() {
        for spec in KNOBS {
            assert!(spec.admits(&spec.default), "{}", spec.name);
            assert!(spec.name.starts_with("cluster."));
        }
        assert_eq!(KNOBS.len(), 16);
        let from_table = config_from_policy(&DecisionPolicy::engine_default());
        assert_eq!(from_table, ClusterConfig::default());
        assert_eq!(k::DEFAULT_MAX_STEPS, 300);
    }

    // spec: MEDULLA-CLUSTER-R002.5
    #[test]
    fn max_steps_revision_is_refused_not_tunable() {
        let mut revision = BTreeMap::new();
        revision.insert(KnobName::new("cluster.max_steps"), KnobValue::Integer(500));
        let refused = check_revision(&[KNOBS], &revision).unwrap_err();
        assert_eq!(refused.1, KnobRefusalCode::KnobNotTunable);
        revision.clear();
        revision.insert(
            KnobName::new("cluster.batch_rows"),
            KnobValue::Integer(2_000),
        );
        assert!(check_revision(&[KNOBS], &revision).is_ok());
        revision.insert(KnobName::new("cluster.patience"), KnobValue::Integer(0));
        let refused = check_revision(&[KNOBS], &revision).unwrap_err();
        assert_eq!(refused.1, KnobRefusalCode::KnobOutOfBounds);
    }
}
