//! The UQL `EDGES` source (MEDULLA-FEDERATED-QUERY-R735): grammar, printer round trip,
//! execution over the one typed tabular channel and its refusal.

use medulla_exec::exec::{execute, PlanCtx};
use medulla_exec::rowset::{Cell, RowSet};
use medulla_graph_core::graph::GraphCore;
use medulla_query_types::wire::Op;
use medulla_semantic_index::semantic::SemanticStore;
use serde_json::json;

fn blob(value: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&value).unwrap()
}

/// `a -dependsOn{weight:0.7}-> b`, `a -dependsOn{weight:0.2}-> c`, `b -owns-> c`.
fn core() -> GraphCore {
    let core = GraphCore::new();
    for id in ["a", "b", "c"] {
        core.add_node(id.into(), blob(json!({ "type": "Doc" })));
    }
    for (from, to, props) in [
        ("a", "b", json!({ "relationship": "dependsOn", "weight": 0.7 })),
        ("a", "c", json!({ "relationship": "dependsOn", "weight": 0.2 })),
        ("b", "c", json!({ "relationship": "owns" })),
    ] {
        core.add_edge(from.into(), to.into(), blob(props)).unwrap();
    }
    core
}

fn run(core: &GraphCore, text: &str) -> Result<RowSet, String> {
    let plan = medulla_uql::parse(text).map_err(|error| error.to_string())?;
    let view = core.analysis_snapshot();
    let semantic = SemanticStore::new();
    let ctx = PlanCtx::new(&view, &semantic);
    execute(&plan, &ctx)
}

// spec: MEDULLA-FEDERATED-QUERY-R735
#[test]
fn edges_rows_carry_source_target_rel_and_properties() {
    let text = "EDGES :dependsOn WHERE weight >= 0.5";
    let plan = medulla_uql::parse(text).unwrap();
    assert!(matches!(
        plan.ops.first(),
        Some(Op::ScanEdges { rel: Some(rel), preds }) if rel == "dependsOn" && preds.len() == 1
    ));
    // The printer is the parser's exact inverse.
    assert_eq!(medulla_uql::parse(&plan.to_uql().unwrap()).unwrap(), plan);

    let core = core();
    let rows = run(&core, text).unwrap();
    assert_eq!(rows.ids(), vec!["a|b|0"]);
    assert_eq!(rows.cell("a|b|0", "source"), Some(&Cell::Text("a".into())));
    assert_eq!(rows.cell("a|b|0", "target"), Some(&Cell::Text("b".into())));
    assert_eq!(
        rows.cell("a|b|0", "rel"),
        Some(&Cell::Text("dependsOn".into()))
    );
    assert_eq!(rows.cell("a|b|0", "weight"), Some(&Cell::Float(0.7)));

    // Without a relationship or predicate every edge comes out, in key order.
    let all = run(&core, "EDGES").unwrap();
    assert_eq!(all.ids(), vec!["a|b|0", "a|c|0", "b|c|0"]);
    assert_eq!(
        all.cell("b|c|0", "rel"),
        Some(&Cell::Text("owns".into())),
        "a relationship filter is optional"
    );

    // The source composes with the stages that follow it.
    let page = run(&core, "EDGES |> ORDER BY id |> LIMIT 2").unwrap();
    assert_eq!(page.ids(), vec!["a|b|0", "a|c|0"]);

    // A predicate that is not relational refuses.
    let refused = run(&core, "EDGES WHERE JSONPATH '$.weight' EXISTS");
    assert!(refused.is_err(), "{refused:?}");
}
