//! The UQL `ORDER` and `SKIP` stages (MEDULLA-FEDERATED-QUERY-R734): grammar, printer round
//! trip, a stable sort with the row id as the last key, paging, and the skip cap.

use medulla_exec::budget::Budget;
use medulla_exec::exec::{execute, PlanCtx};
use medulla_exec::rowset::RowSet;
use medulla_graph_core::graph::GraphCore;
use medulla_query_types::wire::{Op, OrderKey};
use medulla_semantic_index::semantic::SemanticStore;
use serde_json::json;

fn blob(value: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&value).unwrap()
}

fn core() -> GraphCore {
    let core = GraphCore::new();
    for (id, kind, size) in [
        ("d1", "Doc", 10),
        ("d2", "Doc", 30),
        ("d3", "Doc", 20),
        ("d4", "Doc", 20),
        ("p1", "Person", 70),
    ] {
        core.add_node(id.into(), blob(json!({ "type": kind, "size": size })));
    }
    // A node that carries no `size`.
    core.add_node("n0".into(), blob(json!({ "type": "Note" })));
    core
}

fn run(core: &GraphCore, text: &str) -> Result<RowSet, String> {
    let plan = medulla_uql::parse(text).map_err(|error| error.to_string())?;
    let view = core.analysis_snapshot();
    let semantic = SemanticStore::new();
    let ctx = PlanCtx::new(&view, &semantic).with_budget(Budget::default());
    execute(&plan, &ctx)
}

// proves R734: UQL orders and pages rows: the ORDER and SKIP stages
// spec: MEDULLA-FEDERATED-QUERY-R734
#[test]
fn order_is_stable_with_row_id_tiebreak() {
    let text = "MATCH () |> ORDER BY size DESC, label |> SKIP 1 |> LIMIT 3";
    let plan = medulla_uql::parse(text).unwrap();
    assert_eq!(
        plan.ops.iter().find(|op| matches!(op, Op::Order { .. })),
        Some(&Op::Order {
            keys: vec![
                OrderKey {
                    field: "size".into(),
                    descending: true,
                },
                OrderKey {
                    field: "label".into(),
                    descending: false,
                },
            ],
        })
    );
    assert!(plan.ops.contains(&Op::Skip { n: 1 }));
    // The printer is the parser's exact inverse.
    let printed = plan.to_uql().unwrap();
    assert_eq!(medulla_uql::parse(&printed).unwrap(), plan);

    let core = core();
    // Descending size: p1 70, d2 30, then the 20-ties d3 and d4 in id order, d1 10; the
    // node without a size sorts last.
    let all = run(&core, "MATCH () |> ORDER BY size DESC").unwrap();
    assert_eq!(all.ids(), vec!["p1", "d2", "d3", "d4", "d1", "n0"]);
    // The same query twice gives the same order, and the page is a slice of it.
    assert_eq!(
        run(&core, "MATCH () |> ORDER BY size DESC").unwrap().ids(),
        all.ids()
    );
    assert_eq!(run(&core, text).unwrap().ids(), vec!["d2", "d3", "d4"]);
    // Ascending puts the missing value last as well.
    let ascending = run(&core, "MATCH () |> ORDER BY size").unwrap();
    assert_eq!(ascending.ids(), vec!["d1", "d3", "d4", "d2", "p1", "n0"]);
    // A key no row carries is refused.
    let refused = run(&core, "MATCH () |> ORDER BY colour").unwrap_err();
    assert!(refused.starts_with("UQL_UNKNOWN_FIELD"), "{refused}");
}

// spec: MEDULLA-FEDERATED-QUERY-R734
#[test]
fn skip_past_the_cap_is_refused() {
    let core = core();
    let page = run(
        &core,
        "MATCH () |> ORDER BY size DESC |> SKIP 100000 |> LIMIT 10",
    )
    .unwrap();
    assert!(page.is_empty(), "skipping past the end is an empty page");
    let refused = run(&core, "MATCH () |> SKIP 100001").unwrap_err();
    assert!(refused.starts_with("UQL_SKIP_LIMIT"), "{refused}");
    assert!(medulla_uql::parse("MATCH () |> SKIP").is_err());
    assert!(medulla_uql::parse("MATCH () |> ORDER size").is_err());
}
