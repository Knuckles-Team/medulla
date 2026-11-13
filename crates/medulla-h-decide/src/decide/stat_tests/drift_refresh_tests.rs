//! Drift monitors select the shards a refresh covers
//! (MEDULLA-DECISION-ENGINE-R174.8): only the shard whose vocabulary grew
//! fires, and the refresh plan names exactly that shard's region.

use medulla_decision_stats::decision::drift::{DriftMonitor, DriftThresholds, Moments, VocabBloom};
use medulla_decision_types::decision::statistical::retrieval_drift::{
    DriftKind, RefreshPlan, RefreshPriority, RegionRef,
};
use medulla_decision_types::decision::DecisionPolicy;

use super::super::drift_registry::install;
use super::retrieval_tests::learn;
use super::*;
use medulla_decision_types::decision::statistical::retrieval::{LearningRecorded, LearningWrite};
use medulla_decision_types::decision::statistical::retrieval_drift::DriftEvaluated;
use medulla_mutation_types::{capacity_lease::CapacityCell, capacity_lease::CapacityResourceClass};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::protocol::Method;
use medulla_retrieval::discover_signals::{discover_signals, WrittenVector};
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_work_types::native_control::{CapacityCellUpdateRequest, NativeControlSchemaVersion};

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
// spec: MEDULLA-DECISION-ENGINE-R174.8.1
// spec: MEDULLA-DECISION-ENGINE-R174.8.2, MEDULLA-DECISION-ENGINE-R174.8.6
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
            .filter_map(|(id, m)| (!m.evaluate(&thresholds).is_empty()).then_some(id as u32))
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

const GRAPH: &str = "drift-l3-graph";
const CELL: &str = "cell-drift";

/// A durable capacity ledger with one CPU cell, installed in the served state.
async fn with_ledger(h: &Harness, dir: &std::path::Path) {
    let backend = std::sync::Arc::new(
        RedbBackend::open_with_shards(dir.to_str().unwrap().to_string(), 16, 1).unwrap(),
    );
    backend
        .commit_capacity_lease(
            &medulla_kernel::persist::sanitize(GRAPH),
            Method::UpdateCapacityCell {
                request: CapacityCellUpdateRequest {
                    schema_version: NativeControlSchemaVersion::V1,
                    cell: CapacityCell {
                        cell_id: CELL.into(),
                        parent_id: None,
                        resource_class: CapacityResourceClass::Cpu,
                        capacity: 8,
                        reserved_floor: 0,
                        epoch: 1,
                        policy_digest: "d".repeat(64),
                        updated_at_ms: 0,
                        throttle: None,
                    },
                    expected_epoch: None,
                    now_ms: 1,
                },
            },
        )
        .await
        .unwrap();
    h.state.write().await.persistence = Some(backend);
}

/// Report one written vector through the port the commit path uses.
fn report(tenant: &str, shard: u32, seed: u32, token: &str, version: u64) {
    let vector: Vec<f32> = (0..DIM as u32).map(|d| ((seed + d) % 7) as f32).collect();
    let tokens = vec![token.to_string()];
    discover_signals()
        .expect("the drift registry is installed")
        .observe_write(&WrittenVector {
            tenant,
            graph: GRAPH,
            shard,
            node_id: &format!("n{shard}-{seed}"),
            node_type: "Document",
            vector: &vector,
            tokens: &tokens,
            version,
        });
}

async fn evaluate(h: &Harness) -> DriftEvaluated {
    match learn(
        h,
        "decider",
        LearningWrite::EvaluateDrift {
            logical_graph: GRAPH.into(),
            capacity_cell: CELL.into(),
        },
    )
    .await
    .unwrap()
    {
        LearningRecorded::Drift(evaluated) => *evaluated,
        other => panic!("evaluate drift: {other:?}"),
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R174.8.3: Wiring: monitor update and scheduling.
// spec: MEDULLA-DECISION-ENGINE-R174.8.3
#[tokio::test]
async fn monitor_update_schedules_one_plan() {
    assert!(install() || discover_signals().is_some());
    let h = Harness::new().await;
    let ledger = tempfile::tempdir().unwrap();
    with_ledger(&h, ledger.path()).await;
    let tenant = medulla_server_api::carrier::CarrierAuthority::from_verified(&verified())
        .unwrap()
        .tenant_scope()
        .to_string();
    let policy = DecisionPolicy::engine_default();
    let window = super::super::drift_registry::reference_vectors(&policy) as u32;

    // Two shards seal their reference on in-vocabulary writes.
    for shard in [1, 2] {
        for seed in 0..window {
            report(&tenant, shard, seed, "known", u64::from(seed));
        }
    }
    // Shard 1 then receives vectors carrying tokens the reference never saw; shard 2 does not.
    for seed in 0..window {
        report(
            &tenant,
            1,
            seed,
            "never-seen-before",
            u64::from(window + seed),
        );
        report(&tenant, 2, seed, "known", u64::from(window + seed));
    }
    // Another tenant drifts the same graph name; it must not reach this tenant's evaluation.
    for seed in 0..window {
        report("other-tenant", 1, seed, "known", u64::from(seed));
    }
    for seed in 0..window {
        report(
            "other-tenant",
            1,
            seed,
            "never-seen-before",
            u64::from(window + seed),
        );
    }

    // One evaluation is not a firing signal: it takes two consecutive exceedances.
    let first = evaluate(&h).await;
    assert_eq!(first.monitored_shards, 2);
    assert!(first.firing.is_empty() && first.scheduled.is_none());
    let second = evaluate(&h).await;
    assert_eq!(second.firing.len(), 1);
    assert_eq!(second.firing[0].shard_id, 1);
    assert_eq!(second.firing[0].kinds, vec![DriftKind::VocabularyGrowth]);
    let scheduled = second.scheduled.expect("a firing drift schedules a plan");
    assert_eq!(scheduled.plan.regions.as_slice().len(), 1);
    assert_eq!(scheduled.plan.regions.as_slice()[0].shard_id, 1);

    // The same firing evaluated again finds the one plan; it schedules nothing new.
    let third = evaluate(&h).await;
    assert_eq!(
        third.scheduled.map(|s| s.plan_digest),
        Some(scheduled.plan_digest)
    );
    let stored = h
        .store
        .decision_artifacts_with_prefix(TENANT, "drift-plan:", 16)
        .unwrap();
    assert_eq!(stored.len(), 1);

    // Discover reads the firing shard through the port, under this tenant only.
    let signals = discover_signals().unwrap();
    assert_eq!(
        signals.read(&tenant, GRAPH).firing_shards,
        vec!["1".to_string()]
    );
    assert_eq!(signals.read(&tenant, GRAPH).shard_generations.len(), 2);
    let other = signals.read("tenant-without-writes", GRAPH);
    assert!(other.firing_shards.is_empty() && other.shard_generations.is_empty());
}
