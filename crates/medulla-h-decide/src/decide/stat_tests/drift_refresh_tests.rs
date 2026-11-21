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
use medulla_decision_types::decision::statistical::retrieval_drift::{
    drift_refresh_receipt_node_id, shard_generation_node_id, DriftEvaluated, DriftPolicyNode, DriftRefreshed, ScheduledRefresh,
    ShardSwap,
};
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
    with_ledger_for(h, dir, GRAPH).await
}

/// The same ledger, its cell drawn for `graph`'s partition.
async fn with_ledger_for(h: &Harness, dir: &std::path::Path, graph: &str) {
    let backend = std::sync::Arc::new(
        RedbBackend::open_with_shards(dir.to_str().unwrap().to_string(), 16, 1).unwrap(),
    );
    backend
        .commit_capacity_lease(
            &medulla_kernel::persist::sanitize(graph),
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
    evaluate_in(h, "decider", GRAPH, RefreshPriority::Downtime)
        .await
        .unwrap()
}

async fn evaluate_in(
    h: &Harness,
    who: &str,
    graph: &str,
    priority: RefreshPriority,
) -> Result<DriftEvaluated, String> {
    match learn(
        h,
        who,
        LearningWrite::EvaluateDrift {
            logical_graph: graph.into(),
            capacity_cell: CELL.into(),
            priority,
        },
    )
    .await?
    {
        LearningRecorded::Drift(evaluated) => Ok(*evaluated),
        other => panic!("evaluate drift: {other:?}"),
    }
}

async fn register_policy(
    h: &Harness,
    graph: &str,
    region_budget: u32,
) -> Result<DriftPolicyNode, String> {
    match learn(
        h,
        "decider",
        LearningWrite::RegisterDriftPolicy {
            logical_graph: graph.into(),
            region_budget,
        },
    )
    .await?
    {
        LearningRecorded::DriftPolicy(node) => Ok(*node),
        other => panic!("register drift policy: {other:?}"),
    }
}

async fn refresh(h: &Harness, graph: &str, plan_digest: &str) -> Result<DriftRefreshed, String> {
    match learn(
        h,
        "decider",
        LearningWrite::RefreshDrift {
            logical_graph: graph.into(),
            plan_digest: plan_digest.into(),
            capacity_cell: CELL.into(),
        },
    )
    .await?
    {
        LearningRecorded::DriftRefreshed(receipt) => Ok(*receipt),
        other => panic!("refresh drift: {other:?}"),
    }
}

/// Report one written vector of `graph` through the port the commit path uses.
fn report_in(graph: &str, tenant: &str, shard: u32, seed: u32, token: &str, version: u64) {
    let vector: Vec<f32> = (0..DIM as u32).map(|d| ((seed + d) % 7) as f32).collect();
    let tokens = vec![token.to_string()];
    discover_signals()
        .expect("the drift registry is installed")
        .observe_write(&WrittenVector {
            tenant,
            graph,
            shard,
            node_id: &format!("n{shard}-{seed}"),
            node_type: "Document",
            vector: &vector,
            tokens: &tokens,
            version,
        });
}

/// Seal `shards` of `graph` on in-vocabulary writes, then grow the vocabulary of every shard
/// in `drifting`.
fn grow_vocabulary(graph: &str, tenant: &str, shards: &[u32], drifting: &[u32]) {
    let policy = DecisionPolicy::engine_default();
    let window = super::super::drift_registry::reference_vectors(&policy) as u32;
    for &shard in shards {
        for seed in 0..window {
            report_in(graph, tenant, shard, seed, "known", u64::from(seed));
        }
    }
    for &shard in shards {
        let token = if drifting.contains(&shard) { "never-seen-before" } else { "known" };
        for seed in 0..window {
            report_in(graph, tenant, shard, seed, token, u64::from(window + seed));
        }
    }
}

fn served_tenant() -> String {
    medulla_server_api::carrier::CarrierAuthority::from_verified(&verified())
        .unwrap()
        .tenant_scope()
        .to_string()
}

/// The typed documents the refresh re-embeds, in a materialized graph named `graph`.
async fn documents(h: &Harness, graph: &str, count: usize) -> std::sync::Arc<medulla_graph_core::graph::GraphCore> {
    let core = super::learning_tests::graph_with(h, graph, "1", 384, Vec::new()).await;
    for index in 0..count {
        let props = serde_json::json!({"type": "Document", "text": format!("document {index}")});
        core.add_node(format!("doc-{index}"), rmp_serde::to_vec_named(&props).unwrap());
    }
    core
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

    register_policy(&h, GRAPH, 4096).await.unwrap();
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

const REFUSAL_GRAPH: &str = "drift-l3-refusals";
const RECEIPT_GRAPH: &str = "drift-l3-receipt";

fn generations_of(tenant: &str, graph: &str) -> Vec<(String, u64)> {
    let mut read = discover_signals().unwrap().read(tenant, graph).shard_generations;
    read.sort();
    read
}

/// Verifies MEDULLA-DECISION-ENGINE-R174.8.4: Refusal and the write that executes a refresh plan.
// spec: MEDULLA-DECISION-ENGINE-R174.8.4
#[tokio::test]
async fn refresh_refusals() {
    assert!(install() || discover_signals().is_some());
    let h = Harness::new().await;
    let ledger = tempfile::tempdir().unwrap();
    with_ledger_for(&h, ledger.path(), REFUSAL_GRAPH).await;
    let tenant = served_tenant();
    grow_vocabulary(REFUSAL_GRAPH, &tenant, &[1, 2], &[1, 2]);
    let before = generations_of(&tenant, REFUSAL_GRAPH);
    assert_eq!(before.len(), 2);

    // A graph with no policy refuses before any plan is stored.
    let missing = evaluate_in(&h, "decider", REFUSAL_GRAPH, RefreshPriority::Downtime).await;
    assert!(missing.unwrap_err().starts_with("DRIFT_POLICY_MISSING"));
    assert!(h.store.decision_artifacts_with_prefix(TENANT, "drift-plan:", 16).unwrap().is_empty());

    // A budget of one region refuses a firing plan of two (two evaluations fire a signal).
    assert_eq!(register_policy(&h, REFUSAL_GRAPH, 1).await.unwrap().region_budget, 1);
    evaluate_in(&h, "decider", REFUSAL_GRAPH, RefreshPriority::Downtime).await.unwrap();
    let over = evaluate_in(&h, "decider", REFUSAL_GRAPH, RefreshPriority::Downtime).await;
    assert!(over.unwrap_err().starts_with("REFRESH_REGION_BUDGET"));
    assert!(h.store.decision_artifacts_with_prefix(TENANT, "drift-plan:", 16).unwrap().is_empty());

    // A plan whose shard generations moved since it was scheduled refuses and changes nothing.
    register_policy(&h, REFUSAL_GRAPH, 4096).await.unwrap();
    documents(&h, REFUSAL_GRAPH, 3).await;
    let plan = evaluate_in(&h, "decider", REFUSAL_GRAPH, RefreshPriority::Downtime)
        .await
        .unwrap()
        .scheduled
        .expect("two firing shards schedule a plan");
    refresh(&h, REFUSAL_GRAPH, &plan.plan_digest).await.unwrap();
    let moved = generations_of(&tenant, REFUSAL_GRAPH);
    assert_ne!(moved, before);
    let mut stale: ScheduledRefresh = plan.clone();
    stale.plan.target_generation = "refresh-stale".into();
    stale.plan_digest = medulla_query_types::solve_scalar::digest_text("eg/drift-refresh-plan/v1", &stale.plan);
    h.store
        .put_decision_artifacts(
            TENANT,
            &[(
                format!("drift-plan:{}", stale.plan_digest),
                medulla_persistence_records::decision_jobs::encode_artifact(&stale).unwrap(),
            )],
        )
        .unwrap();
    let conflict = refresh(&h, REFUSAL_GRAPH, &stale.plan_digest).await;
    assert!(conflict.unwrap_err().starts_with("SHARD_SWAP_CONFLICT"));
    assert_eq!(generations_of(&tenant, REFUSAL_GRAPH), moved);

    // A caller without admin:decision-head cannot schedule an on-demand plan.
    let denied = evaluate_in(&h, "stranger", REFUSAL_GRAPH, RefreshPriority::OnDemand).await;
    assert!(denied.unwrap_err().starts_with("ACCESS_DENIED"));
    assert_eq!(generations_of(&tenant, REFUSAL_GRAPH), moved);
}

/// Verifies MEDULLA-DECISION-ENGINE-R174.8.5: signal premises and the refresh receipt.
// spec: MEDULLA-DECISION-ENGINE-R174.8.5
#[tokio::test]
async fn signal_premises_and_refresh_receipt() {
    assert!(install() || discover_signals().is_some());
    let h = Harness::new().await;
    let ledger = tempfile::tempdir().unwrap();
    with_ledger_for(&h, ledger.path(), RECEIPT_GRAPH).await;
    let tenant = served_tenant();
    let window = u64::from(super::super::drift_registry::reference_vectors(&DecisionPolicy::engine_default()) as u32);
    grow_vocabulary(RECEIPT_GRAPH, &tenant, &[1, 2], &[1]);
    let core = documents(&h, RECEIPT_GRAPH, 5).await;
    register_policy(&h, RECEIPT_GRAPH, 4096).await.unwrap();

    evaluate_in(&h, "decider", RECEIPT_GRAPH, RefreshPriority::Downtime).await.unwrap();
    let plan = evaluate_in(&h, "decider", RECEIPT_GRAPH, RefreshPriority::Downtime)
        .await
        .unwrap()
        .scheduled
        .expect("one firing shard schedules a plan");
    // The stored plan carries the signal and the generation its swap checks.
    let stored: ScheduledRefresh = medulla_persistence_records::decision_jobs::decode_artifact(
        &h.store.decision_artifact(TENANT, &format!("drift-plan:{}", plan.plan_digest)).unwrap().unwrap(),
        "plan",
    )
    .unwrap();
    assert_eq!(stored.shard_generations, vec![(1, 1)]);
    assert_eq!(stored.signals.len(), 1);
    let signal = &stored.signals[0];
    assert_eq!(signal.kind, DriftKind::VocabularyGrowth);
    assert_eq!(signal.scope, format!("{tenant}/{RECEIPT_GRAPH}/1"));
    assert_eq!(signal.reference_generation, format!("{RECEIPT_GRAPH}/1@1"));
    assert_eq!(signal.window, window);
    assert!(signal.value_q16 > signal.threshold_q16);

    let receipt = refresh(&h, RECEIPT_GRAPH, &plan.plan_digest).await.unwrap();
    assert_eq!(receipt.plan_digest, plan.plan_digest);
    assert_eq!(receipt.regions, plan.plan.regions.as_slice().to_vec());
    assert_eq!(receipt.reembedded_nodes, 5);
    assert_eq!(
        receipt.shards,
        vec![ShardSwap { shard_id: 1, generation_before: 1, generation_after: 2 }]
    );
    assert_eq!(receipt.signals, stored.signals);
    // The shard's durable pointer node holds the swapped generation, and the registry agrees.
    let pointer: serde_json::Value = rmp_serde::from_slice(
        &core.get_node_properties(&shard_generation_node_id(RECEIPT_GRAPH, 1)).expect("the pointer node"),
    )
    .unwrap();
    assert_eq!(pointer["generation"], 2);
    // The receipt is a graph node written in the same batch as the pointer (R174.8.4).
    let recorded: serde_json::Value = rmp_serde::from_slice(
        &core
            .get_node_properties(&drift_refresh_receipt_node_id(&plan.plan_digest))
            .expect("the receipt node"),
    )
    .unwrap();
    assert_eq!(recorded["plan_digest"], plan.plan_digest);
    assert_eq!(recorded["type"], "DriftRefreshReceipt");
    assert!(generations_of(&tenant, RECEIPT_GRAPH).contains(&("1".to_string(), 2)));

    // A second refresh answers the stored receipt and embeds nothing.
    let embedded = core.semantic_store.read().len();
    let again = refresh(&h, RECEIPT_GRAPH, &plan.plan_digest).await.unwrap();
    assert_eq!(again, receipt);
    assert_eq!(core.semantic_store.read().len(), embedded);
}
