//! EXPLAIN reports the plan DAG's node and root counts
//! (MEDULLA-FEDERATED-QUERY-R720.8).
#![cfg(feature = "query")]

use medulla_exec::exec::PlanCtx;
use medulla_graph_core::graph::GraphCore;
use medulla_plan::uql::serve::run_statement;
use medulla_semantic_index::semantic::SemanticStore;
use medulla_query_types::wire::UqlResult;
use medulla_uql::{parse_statement, Params};

fn explain(src: &str) -> (u32, u32, usize) {
    let core = GraphCore::new();
    core.add_node(
        "d1".into(),
        rmp_serde::to_vec_named(&serde_json::json!({ "type": "Doc", "year": 2020 })).unwrap(),
    );
    let view = core.analysis_snapshot();
    let semantic = SemanticStore::new();
    let ctx = PlanCtx::new(&view, &semantic);
    let stmt = parse_statement(src, &Params::new()).expect("program parses");
    match run_statement(&stmt, &ctx).expect("explain runs") {
        UqlResult::Explain {
            plan_node_count,
            plan_root_count,
            stages,
            ..
        } => (plan_node_count, plan_root_count, stages.len()),
        other => panic!("explain expected, got {other:?}"),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R720.8
/// Verifies MEDULLA-FEDERATED-QUERY-R720.8: a multi-stage program is one plan, one root.
#[test]
fn explain_reports_nodes_and_root_count() {
    let (nodes, roots, stages) = explain("EXPLAIN MATCH (:Doc) WHERE year > 2019 |> LIMIT 5");
    assert_eq!(roots, 1, "one program is one plan with one root");
    assert_eq!(nodes as usize, stages, "every node is listed as a stage");
    assert!(nodes >= 2);

    let (dag_nodes, dag_roots, dag_stages) = explain(
        "EXPLAIN LET a = MATCH (:Doc); LET b = MATCH (:Doc) |> LIMIT 1; JOIN a, b |> LIMIT 10",
    );
    assert_eq!(dag_roots, 1, "the joined stage is the only root of the DAG");
    assert_eq!(dag_nodes as usize, dag_stages);
    assert!(dag_nodes >= 3, "two bindings and the joined stage");
}
