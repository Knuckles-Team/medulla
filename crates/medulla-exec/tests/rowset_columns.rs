//! The one typed tabular channel of `RowSet` (MEDULLA-FEDERATED-QUERY-R737): `GROUP` and
//! `RETURN` (the projection) both write through it, and both read back as named, typed
//! columns with `id` and `score` as ordinary columns.

#![cfg(feature = "query")]

use std::collections::BTreeMap;

use medulla_exec::exec::{apply, PlanCtx};
use medulla_exec::rowset::{Cell, Column, ColumnType, RowSet, ID_COLUMN, SCORE_COLUMN};
use medulla_graph_core::graph::GraphCore;
use medulla_semantic_index::semantic::SemanticStore;
use medulla_query_types::wire::{GroupAgg, GroupAggFunc, Op};
use serde_json::json;

fn blob(value: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&value).unwrap()
}

fn column(name: &str, ty: ColumnType) -> Column {
    Column {
        name: name.to_string(),
        ty,
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R737
#[test]
fn group_and_projection_share_one_typed_row_shape() {
    let core = GraphCore::new();
    for (id, kind, weight) in [("a", "Doc", 1.0), ("b", "Doc", 3.0), ("c", "Topic", 5.0)] {
        core.add_node(id.into(), blob(json!({ "type": kind, "weight": weight })));
    }
    let view = core.analysis_snapshot();
    let semantic = SemanticStore::new();
    let ctx = PlanCtx::new(&view, &semantic);
    let input = RowSet::from_ids(["a".to_string(), "b".to_string(), "c".to_string()]);

    // GROUP writes its keys and aggregates as typed cells of the one channel.
    let grouped = apply(
        &Op::Group {
            keys: vec!["label".into()],
            aggs: vec![
                GroupAgg {
                    func: GroupAggFunc::Count,
                    field: None,
                    alias: Some("n".into()),
                },
                GroupAgg {
                    func: GroupAggFunc::Avg,
                    field: Some("weight".into()),
                    alias: None,
                },
            ],
        },
        input.clone(),
        &ctx,
    )
    .unwrap();
    assert_eq!(
        grouped.columns(),
        vec![
            column(ID_COLUMN, ColumnType::Text),
            column(SCORE_COLUMN, ColumnType::Float),
            column("label", ColumnType::Text),
            column("n", ColumnType::Int),
            column("avg_weight", ColumnType::Float),
        ],
        "the keys then the aggregates, after the ordinary id and score columns"
    );
    assert_eq!(
        grouped.typed_rows(),
        vec![
            vec![
                Some(Cell::Text("Doc".into())),
                None,
                Some(Cell::Text("Doc".into())),
                Some(Cell::Int(2)),
                Some(Cell::Float(2.0)),
            ],
            vec![
                Some(Cell::Text("Topic".into())),
                None,
                Some(Cell::Text("Topic".into())),
                Some(Cell::Int(1)),
                Some(Cell::Float(5.0)),
            ],
        ]
    );

    // A value channel written by any stage is a Float cell of the same table, and the
    // projection narrows that same table: the shape a reader sees is identical.
    let values: BTreeMap<String, BTreeMap<String, f64>> = [("a", 0.5), ("b", 0.25)]
        .into_iter()
        .map(|(id, v)| {
            (
                id.to_string(),
                BTreeMap::from([("v0".to_string(), v), ("v1".to_string(), v * 2.0)]),
            )
        })
        .collect();
    let projected = apply(
        &Op::Project {
            channels: vec!["v1".into()],
        },
        RowSet::from_scored([("a".to_string(), 0.9), ("b".to_string(), 0.1)]).with_values(values),
        &ctx,
    )
    .unwrap();
    assert_eq!(
        projected.columns(),
        vec![
            column(ID_COLUMN, ColumnType::Text),
            column(SCORE_COLUMN, ColumnType::Float),
            column("v1", ColumnType::Float),
        ]
    );
    let first = &projected.typed_rows()[0];
    assert_eq!(first[0], Some(Cell::Text("a".into())));
    assert_eq!(first[1], Some(Cell::Float(f64::from(0.9_f32))));
    assert_eq!(first[2], Some(Cell::Float(1.0)));
    // The float view and the cell are the same value: there is no second result shape.
    assert_eq!(projected.value("a", "v1"), Some(1.0));
    assert_eq!(projected.value("a", "v0"), None, "projected away");
    assert_eq!(grouped.values()["Doc"]["avg_weight"], 2.0);
}
