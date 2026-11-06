//! Drift monitors select the shards a refresh covers
//! (MEDULLA-DECISION-ENGINE-R174.8): only the shard whose vocabulary grew
//! fires, and the refresh plan names exactly that shard's region.

use medulla_decision_stats::decision::drift::{DriftMonitor, DriftThresholds, Moments, VocabBloom};
use medulla_decision_types::decision::statistical::retrieval_drift::{
    DriftKind, RefreshPlan, RefreshPriority, RegionRef,
};
use medulla_decision_types::decision::DecisionPolicy;

const DIM: usize = 3;

fn monitor() -> DriftMonitor {
    let mut reference = Moments::new(DIM);
    for seed in 0..30_u32 {
        let v: Vec<f64> = (0..DIM as u32).map(|d| f64::from((seed + d) % 7)).collect();
        reference.push(&v).unwrap();
    }
    let mut vocabulary = VocabBloom::default();
    vocabulary.insert("known");
    DriftMonitor::activate(reference, vocabulary)
}

/// Write the same 30 vectors the reference saw, carrying `token`.
fn write(monitor: &mut DriftMonitor, token: &str) {
    for seed in 0..30_u32 {
        let v: Vec<f64> = (0..DIM as u32).map(|d| f64::from((seed + d) % 7)).collect();
        monitor.observe(&v, &[token]).unwrap();
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R174.8.2: Behavior: shift and vocabulary monitors.
// spec: MEDULLA-DECISION-ENGINE-R174.8.2
#[test]
fn vocabulary_drift_refreshes_changed_shards() {
    let thresholds = DriftThresholds::from_policy(&DecisionPolicy::engine_default());
    let mut shards = [monitor(), monitor(), monitor()];
    write(&mut shards[0], "known");
    write(&mut shards[1], "never-seen-before");
    write(&mut shards[2], "known");

    let mut changed = Vec::new();
    for _ in 0..2 {
        changed = shards
            .iter_mut()
            .enumerate()
            .filter(|(_, m)| !m.evaluate(&thresholds).is_empty())
            .map(|(id, _)| id as u32)
            .collect();
    }
    assert_eq!(changed, vec![1]);
    assert_eq!(shards[1].firing(), vec![DriftKind::VocabularyGrowth]);

    let regions = changed
        .iter()
        .map(|&shard_id| RegionRef {
            shard_id,
            node_type: "Document".into(),
            revision_from: 0,
            revision_to: 30,
        })
        .collect();
    let plan = RefreshPlan::new(
        "g".into(),
        regions,
        DriftKind::VocabularyGrowth,
        RefreshPriority::Downtime,
        "gen-2".into(),
    )
    .unwrap();
    assert_eq!(plan.regions.as_slice().len(), 1);
    assert_eq!(plan.regions.as_slice()[0].shard_id, 1);
}
