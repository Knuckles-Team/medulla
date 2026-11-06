//! The UQL epistemic stages on a fixed fixture (MEDULLA-FEDERATED-QUERY-R720.2, R720.3,
//! R720.5): `CONFIDENCE`, `BELIEF AS OF`, `DECAY HALF_LIFE`, `CLUSTER BY EMBEDDING` and
//! their refusals, parsed from UQL text and run through the executor.
#![cfg(all(feature = "query", feature = "epistemic"))]

use medulla_exec::algebra::{Op, Plan};
use medulla_exec::exec::{PlanCtx, PlanExt};
use medulla_graph_core::graph::{GraphCore, GraphView};
use medulla_semantic_index::semantic::SemanticStore;
use serde_json::json;

/// The fixed "now" of every decay check (epoch ms).
const NOW_MS: i64 = 1_700_000_000_000;

fn blob(v: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&v).unwrap()
}

/// Claims with valid times (fresh: now, old: one hour ago, late: recorded at tx 500) and
/// docs with two well-separated embedding directions.
fn fixture() -> (GraphView, SemanticStore) {
    let core = GraphCore::new();
    core.add_node(
        "fresh".into(),
        blob(json!({"type": "Claim", "confidence": 0.8, "valid_time_ms": NOW_MS})),
    );
    core.add_node(
        "old".into(),
        blob(json!({"type": "Claim", "confidence": 0.8, "valid_time_ms": NOW_MS - 3_600_000})),
    );
    core.add_node(
        "late".into(),
        blob(json!({"type": "Claim", "confidence": 0.7, "valid_time_ms": NOW_MS, "tx_from": 500})),
    );
    let mut semantic = SemanticStore::new();
    for (id, v) in [
        ("a1", [1.0, 0.0, 0.0]),
        ("a2", [0.9, 0.1, 0.0]),
        ("a3", [2.0, 0.1, 0.0]),
        ("b1", [0.0, 1.0, 0.0]),
        ("b2", [0.1, 0.9, 0.0]),
        ("b3", [0.0, 3.0, 0.1]),
    ] {
        core.add_node(id.into(), blob(json!({"type": "Doc"})));
        semantic.add_embedding(id.into(), v.to_vec()).unwrap();
    }
    core.add_node("bare".into(), blob(json!({"type": "Note"})));
    (core.analysis_snapshot(), semantic)
}

fn plan(uql: &str) -> Plan {
    medulla_uql::parse(uql).unwrap_or_else(|e| panic!("UQL `{uql}`: {}", e.msg))
}

fn score(rows: &medulla_exec::rowset::RowSet, id: &str) -> f32 {
    rows.rows()
        .iter()
        .find(|r| r.id == id)
        .and_then(|r| r.score)
        .unwrap_or_else(|| panic!("row {id} has a score"))
}

// spec: MEDULLA-FEDERATED-QUERY-R720.2
#[test]
fn confidence_asof_decay() {
    let (view, semantic) = fixture();
    let ctx = PlanCtx::new(&view, &semantic).with_now_ms(NOW_MS);

    // CONFIDENCE scores every claim by its propagated belief.
    let confident = plan("MATCH (:Claim) |> CONFIDENCE").execute(&ctx).unwrap();
    assert_eq!(confident.len(), 3);
    assert!(confident.rows().iter().all(|r| r.score.is_some()));

    // CONFIDENCE <threshold> drops the rows scoring below it.
    let top = confident
        .rows()
        .iter()
        .filter_map(|r| r.score)
        .fold(0.0_f32, f32::max);
    let bottom = confident
        .rows()
        .iter()
        .filter_map(|r| r.score)
        .fold(1.0_f32, f32::min);
    let all = plan(&format!(
        "MATCH (:Claim) |> CONFIDENCE {}",
        f64::from(bottom)
    ))
    .execute(&ctx)
    .unwrap();
    assert_eq!(
        all.len(),
        3,
        "a threshold at the lowest score keeps every row"
    );
    let none = plan("MATCH (:Claim) |> CONFIDENCE 1")
        .execute(&ctx)
        .unwrap();
    assert!(none.rows().iter().all(|r| r.score.unwrap_or(0.0) >= 1.0));
    assert!(top <= 1.0);
    assert_eq!(
        plan("MATCH (:Claim) |> CONFIDENCE 0.6").ops[1],
        Op::ConfidenceOp {
            threshold: Some(0.6)
        }
    );
    assert!(medulla_uql::parse("MATCH (:Claim) |> CONFIDENCE 2").is_err());

    // BELIEF AS OF reads the transaction-time state: `late` is not yet recorded at 100.
    let at_100 = plan("MATCH (:Claim) |> BELIEF AS OF @100")
        .execute(&ctx)
        .unwrap();
    assert!(!at_100.ids().contains(&"late".to_string()));
    let at_600 = plan("MATCH (:Claim) |> BELIEF AS OF @600")
        .execute(&ctx)
        .unwrap();
    assert!(at_600.ids().contains(&"late".to_string()));

    // DECAY: an unscored row takes 1.0, then × 0.5^(age / half_life).
    let decayed = plan("MATCH (:Claim) |> DECAY HALF_LIFE 3600")
        .execute(&ctx)
        .unwrap();
    assert!((score(&decayed, "fresh") - 1.0).abs() < 1e-6);
    assert!((score(&decayed, "old") - 0.5).abs() < 1e-6);

    // DECAY multiplies an existing score.
    let rescored = plan("MATCH (:Claim) |> CONFIDENCE |> DECAY HALF_LIFE 1800")
        .execute(&ctx)
        .unwrap();
    let before = score(&confident, "old");
    assert!((score(&rescored, "old") - before * 0.25).abs() < 1e-5);
    assert_eq!(
        plan("MATCH (:Claim) |> DECAY HALF_LIFE 3600").ops[1],
        Op::Decay {
            half_life_secs: 3600.0
        }
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R720.3
#[test]
fn cluster_by_embedding_is_deterministic() {
    let (view, semantic) = fixture();
    let ctx = PlanCtx::new(&view, &semantic);
    let program = plan("MATCH (:Doc) |> CLUSTER BY EMBEDDING K 2");
    assert_eq!(program.ops[1], Op::ClusterByEmbedding { k: 2 });

    let first = program.execute(&ctx).unwrap();
    let second = program.execute(&ctx).unwrap();
    assert_eq!(first, second, "seed 0 repeats the clustering");
    assert_eq!(first.len(), 6);

    let label = |id: &str| first.value(id, "cluster_id").expect("cluster_id channel");
    // Cosine distance: magnitude does not matter, direction does.
    assert_eq!(label("a1"), label("a2"));
    assert_eq!(label("a1"), label("a3"));
    assert_eq!(label("b1"), label("b2"));
    assert_eq!(label("b1"), label("b3"));
    assert_ne!(label("a1"), label("b1"));
    for id in ["a1", "a2", "a3", "b1", "b2", "b3"] {
        let d = first
            .value(id, "centroid_distance")
            .expect("centroid_distance channel");
        assert!((0.0..=0.1).contains(&d), "{id} sits near its centroid: {d}");
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R720.5
#[test]
fn decay_and_cluster_refusals() {
    let (view, semantic) = fixture();
    let ctx = PlanCtx::new(&view, &semantic).with_now_ms(NOW_MS);

    let no_time = plan("MATCH (:Doc) |> DECAY HALF_LIFE 60")
        .execute(&ctx)
        .unwrap_err();
    assert!(no_time.starts_with("UQL_E_DECAY_NO_TIME"), "{no_time}");

    let no_vector = plan("MATCH (:Note) |> CLUSTER BY EMBEDDING K 1")
        .execute(&ctx)
        .unwrap_err();
    assert!(
        no_vector.starts_with("UQL_E_CLUSTER_NO_VECTOR"),
        "{no_vector}"
    );
    // One row without a vector refuses the whole stage: no partial rows.
    let mixed = Plan::new(vec![Op::ScanAll {}, Op::ClusterByEmbedding { k: 2 }]).execute(&ctx);
    assert!(mixed.unwrap_err().starts_with("UQL_E_CLUSTER_NO_VECTOR"));
}
