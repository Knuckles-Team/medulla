//! MEDULLA-DURABLE-KERNEL-R024 — rollup proof: every named external backend is
//! present in this repository as a `ForeignSourceSpec` federation source or a
//! `MirrorTargetSpec` mirror target, and each entry point is exercised with a
//! representative OFFLINE operation: the typed explicit refusal the unbound
//! drivers (Spark batch, brain-guarded) must return,
//! the bound AGE binding's endpoint-match refusal (under `federation-sql`),
//! and fail-closed mirror sends through the `MirrorTargetRegistry`.
//!
//! The representative QUERY operations of the live-probing bindings (the AGE
//! graph probe, the Trino statement pages) are proved by the per-backend
//! children (R024.1 / R024.7); this rollup test covers exactly what can be
//! exercised without a live backend, offline.

use medulla_federation::federation::ForeignSourceRegistry;
use medulla_federation::federation_brain_guarded::register_brain_guarded;
use medulla_federation::federation_spark::register_spark;
use medulla_federation::{mirror_target::target_for, mirror_target::MirrorTargetRegistry};
use medulla_query_types::wire::{ForeignCypherBackend, ForeignSourceSpec, MirrorTargetSpec};

// Verifies MEDULLA-DURABLE-KERNEL-R024.3: the Cypher source binds the AGE backend only.
// spec: MEDULLA-DURABLE-KERNEL-R024.3
#[cfg(feature = "federation-sql")]
#[test]
fn age_cypher_registers_and_removed_backend_is_refused() {
    use std::collections::HashMap;
    use std::sync::Arc;

    let spec = ForeignSourceSpec::Cypher {
        backend: ForeignCypherBackend::Age,
        endpoint: "postgres://rollup.example.invalid:5432/age_graph".into(),
        graph: "rollup_graph".into(),
        query: "MATCH (r) RETURN r.key AS id".into(),
        id_field: "id".into(),
        score_field: None,
    };
    let reader: Arc<
        dyn Fn(&str, &str) -> Result<Vec<HashMap<String, String>>, String> + Send + Sync,
    > = Arc::new(|_, sql| {
        if sql.contains("ag_catalog.ag_graph") {
            return Ok(vec![HashMap::from([(
                "medulla_name".into(),
                "rollup_graph".into(),
            )])]);
        }
        Ok(vec![HashMap::from([(
            "medulla_id".into(),
            "\"k1\"".into(),
        )])])
    });
    let mut registry = ForeignSourceRegistry::new();
    medulla_federation::federation_age::register_age_with_test_reader(
        &mut registry,
        "rollup-age",
        &spec,
        "postgres://svc:private@rollup.example.invalid:5432/age_graph".into(),
        reader,
    )
    .unwrap();
    assert_eq!(
        registry.resolve("rollup-age").unwrap().ids(),
        vec!["k1".to_string()]
    );
    // A removed backend no longer deserializes: the closed enum names Age only.
    for removed in ["Neo4j", "FalkorDb"] {
        let wire = serde_json::json!({
            "Cypher": {
                "backend": removed,
                "endpoint": "bolt://rollup.example.invalid:7687",
                "graph": "rollup_graph",
                "query": "MATCH (r) RETURN r.key AS id",
                "id_field": "id"
            }
        });
        let err = serde_json::from_value::<ForeignSourceSpec>(wire).unwrap_err();
        assert!(
            err.to_string().contains("FOREIGN_CYPHER_BACKEND_UNKNOWN"),
            "{removed}: {err}"
        );
    }
}

// Partial refusal coverage for MEDULLA-DURABLE-KERNEL-R024; successful-operation acceptance remains open.
#[test]
fn unbound_cypher_backends_refuse_typed_and_keep_the_registry_empty() {
    let mut registry = ForeignSourceRegistry::new();
    let neo4j = register_neo4j(
        &mut registry,
        "rollup-neo4j",
        &rollup_cypher_spec(ForeignCypherBackend::Neo4j),
    )
    .unwrap_err();
    assert!(neo4j.contains("not yet bound"), "Neo4j: {neo4j}");
    let falkor = register_falkordb(
        &mut registry,
        "rollup-falkor",
        &rollup_cypher_spec(ForeignCypherBackend::FalkorDb),
    )
    .unwrap_err();
    assert!(falkor.contains("not yet bound"), "FalkorDb: {falkor}");
    assert!(
        registry.is_empty(),
        "a refused registration must not be stored"
    );
}

// Partial refusal coverage for MEDULLA-DURABLE-KERNEL-R024; successful-operation acceptance remains open.
#[test]
fn unbound_spark_and_brain_guarded_backends_refuse_typed() {
    let mut registry = ForeignSourceRegistry::new();
    let spark = ForeignSourceSpec::SparkBatch {
        artifact_ref: "artifact://rollup-batch".into(),
        id_field: "id".into(),
        score_field: None,
    };
    let spark_err = register_spark(&mut registry, "rollup-spark", &spark).unwrap_err();
    assert!(
        spark_err.contains("no bound artifact-store driver"),
        "Spark: {spark_err}"
    );
    let guarded = ForeignSourceSpec::BrainGuarded {
        endpoint: "https://rollup.example.invalid".into(),
        guard_policy: "rollup-policy".into(),
        query: "SELECT key FROM rollup_table".into(),
        id_field: "id".into(),
        score_field: None,
    };
    let guarded_err =
        register_brain_guarded(&mut registry, "rollup-guarded", &guarded).unwrap_err();
    assert!(
        guarded_err.contains("never run unguarded"),
        "brain-guarded: {guarded_err}"
    );
    assert!(
        registry.is_empty(),
        "a refused registration must not be stored"
    );
}

// Partial refusal coverage for MEDULLA-DURABLE-KERNEL-R024; successful-operation acceptance remains open.
#[test]
fn mirror_targets_fail_closed_and_unknown_names_error_cleanly() {
    let mut targets = MirrorTargetRegistry::new();
    targets
        .register(
            "rollup-fan",
            MirrorTargetSpec::FanOut {
                targets: vec!["a".into(), "b".into()],
            },
        )
        .unwrap();
    let fan = targets.send_to("rollup-fan", b"row").unwrap_err();
    assert!(
        fan.contains("requires a verified registration"),
        "fan-out: {fan}"
    );
    let named_spec = MirrorTargetSpec::Named {
        name: "rollup-sink".into(),
    };
    let named = target_for(&named_spec).unwrap();
    let named_err = named.send(b"row").unwrap_err();
    assert!(
        named_err.contains("requires a verified registration"),
        "named: {named_err}"
    );
    let outbox_spec = MirrorTargetSpec::Outbox {
        consumer: "rollup-consumer".into(),
    };
    let outbox = target_for(&outbox_spec).unwrap();
    let outbox_err = outbox.send(b"row").unwrap_err();
    assert!(
        outbox_err.contains("requires a verified registration"),
        "outbox: {outbox_err}"
    );
    let unknown = targets.send_to("rollup-unknown", b"row").unwrap_err();
    assert!(
        unknown.contains("no mirror target registered"),
        "unknown name: {unknown}"
    );
}

#[cfg(feature = "federation-sql")]
fn rollup_age_spec() -> ForeignSourceSpec {
    ForeignSourceSpec::Cypher {
        backend: ForeignCypherBackend::Age,
        endpoint: "postgres://rollup.example.invalid:5432/age_graph".into(),
        graph: "rollup_graph".into(),
        query: "MATCH (r) RETURN r.key AS id".into(),
        id_field: "id".into(),
        score_field: None,
    }
}

// Partial refusal coverage for MEDULLA-DURABLE-KERNEL-R024; successful-operation acceptance remains open.
#[cfg(feature = "federation-sql")]
#[test]
fn age_binding_with_a_mismatched_dsn_is_refused_offline() {
    let mut registry = ForeignSourceRegistry::new();
    let err = medulla_federation::federation_age::register_age(
        &mut registry,
        "rollup-age",
        &rollup_age_spec(),
        "postgres://other.example.invalid:5432/age_graph".into(),
    )
    .unwrap_err();
    assert!(err.contains("does not match"), "AGE: {err}");
    assert!(registry.is_empty(), "a refused binding must not be stored");
}
