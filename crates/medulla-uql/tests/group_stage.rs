//! The UQL `GROUP` stage (MEDULLA-FEDERATED-QUERY-R733): grammar, printer round trip,
//! execution over the one typed tabular channel, and its two refusals.

use medulla_exec::budget::Budget;
use medulla_exec::exec::{execute, PlanCtx};
use medulla_exec::rowset::{Cell, RowSet};
use medulla_graph_core::graph::GraphCore;
use medulla_semantic_index::semantic::SemanticStore;
use medulla_query_types::wire::{GroupAgg, GroupAggFunc, Op};
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
        ("p1", "Person", 70),
    ] {
        core.add_node(id.into(), blob(json!({ "type": kind, "size": size })));
    }
    core
}

fn run(core: &GraphCore, text: &str, budget: Budget) -> Result<RowSet, String> {
    let plan = medulla_uql::parse(text).map_err(|error| error.to_string())?;
    let view = core.analysis_snapshot();
    let semantic = SemanticStore::new();
    let ctx = PlanCtx::new(&view, &semantic).with_budget(budget);
    execute(&plan, &ctx)
}

// spec: MEDULLA-FEDERATED-QUERY-R733
#[test]
fn group_by_label_counts_rows() {
    let text = "MATCH () |> GROUP BY label AGGREGATE COUNT AS n, MAX(size), AVG(size) AS mean \
                |> LIMIT 100";
    let plan = medulla_uql::parse(text).unwrap();
    let group = plan
        .ops
        .iter()
        .find(|op| matches!(op, Op::Group { .. }))
        .expect("a Group plan node");
    assert_eq!(
        group,
        &Op::Group {
            keys: vec!["label".into()],
            aggs: vec![
                GroupAgg {
                    func: GroupAggFunc::Count,
                    field: None,
                    alias: Some("n".into()),
                },
                GroupAgg {
                    func: GroupAggFunc::Max,
                    field: Some("size".into()),
                    alias: None,
                },
                GroupAgg {
                    func: GroupAggFunc::Avg,
                    field: Some("size".into()),
                    alias: Some("mean".into()),
                },
            ],
        }
    );
    // The printer is the parser's exact inverse.
    let printed = plan.to_uql().unwrap();
    assert_eq!(medulla_uql::parse(&printed).unwrap(), plan);

    let core = core();
    let rows = run(&core, text, Budget::default()).unwrap();
    assert_eq!(rows.ids(), vec!["Doc", "Person"]);
    assert_eq!(rows.cell("Doc", "label"), Some(&Cell::Text("Doc".into())));
    assert_eq!(rows.cell("Doc", "n"), Some(&Cell::Int(3)));
    assert_eq!(rows.cell("Doc", "max_size"), Some(&Cell::Float(30.0)));
    assert_eq!(rows.cell("Doc", "mean"), Some(&Cell::Float(20.0)));
    assert_eq!(rows.cell("Person", "n"), Some(&Cell::Int(1)));

    // A RETURN may name an aggregate column.
    let returned =
        medulla_uql::parse("MATCH () |> GROUP BY label AGGREGATE COUNT AS n |> RETURN n");
    assert!(returned.is_ok(), "{returned:?}");

    // More groups than the bound refuse UQL_GROUP_LIMIT (knob uql.group.max_groups).
    let tight = Budget {
        max_groups: 1,
        ..Budget::default()
    };
    let refused = run(&core, text, tight).unwrap_err();
    assert!(refused.starts_with("UQL_GROUP_LIMIT"), "{refused}");
}

// spec: MEDULLA-FEDERATED-QUERY-R733
#[test]
fn unknown_field_is_refused() {
    let core = core();
    for text in [
        "MATCH () |> GROUP BY colour AGGREGATE COUNT",
        "MATCH () |> GROUP BY label AGGREGATE SUM(weight)",
    ] {
        let refused = run(&core, text, Budget::default()).unwrap_err();
        assert!(
            refused.starts_with("UQL_UNKNOWN_FIELD"),
            "{text}: {refused}"
        );
    }
    // A non-count aggregate needs a field, and only the five functions parse.
    assert!(medulla_uql::parse("MATCH () |> GROUP BY label AGGREGATE SUM").is_err());
    assert!(medulla_uql::parse("MATCH () |> GROUP BY label AGGREGATE MEDIAN(size)").is_err());
}
