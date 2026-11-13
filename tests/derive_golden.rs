//! MEDULLA-DECISION-ENGINE-R117.1.1 — cross-surface golden equivalence: UQL `DERIVE`, the SQL
//! window functions and the PromQL series functions all run the one `Kernel` series program,
//! and agree with the pandas reference (and with each other) within an absolute 1e-9.
//!
//! The reference vectors are the fixtures of `crates/medulla-numeric/tests/series_golden.rs`
//! (no second copy is committed); `tests/data/derive_golden.json` names the fifteen functions,
//! their parameters and the reference case each is checked against. `div` has no pandas case:
//! its oracle is the ratio `x / y` computed here.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, Int64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use medulla_exec::exec::derive_op;
use medulla_exec::rowset::RowSet;
use medulla_sql::sql::exec_sql_over_tables;
use medulla_tsdb::promql::{query_range, MemSeriesSource};
use medulla_types::series_expr::{DeriveColumn, SeriesExpr, SIGNATURES};
use serde::Deserialize;

const REFERENCE: &str = include_str!("../crates/medulla-numeric/tests/fixtures/series/golden.json");
const MANIFEST: &str = include_str!("data/derive_golden.json");
const NS: i64 = 1_000_000_000;

#[derive(Deserialize)]
struct Reference {
    x: Vec<f64>,
    y: Vec<f64>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    func: String,
    params: Vec<f64>,
    values: Vec<Option<f64>>,
}

#[derive(Deserialize)]
struct Manifest {
    tolerance: f64,
    functions: Vec<Function>,
}

#[derive(Deserialize)]
struct Function {
    name: String,
    params: Vec<f64>,
    reference: Option<String>,
}

fn ts(i: usize) -> i64 {
    (i as i64 + 1) * NS
}

fn agree(got: Option<f64>, want: Option<f64>, tol: f64) -> bool {
    match (got, want) {
        (Some(g), Some(w)) => (g - w).abs() <= tol,
        (None, None) => true,
        _ => false,
    }
}

/// The expected series: the pandas case, or the `x / y` oracle for `div`.
fn expected(f: &Function, r: &Reference) -> Vec<Option<f64>> {
    match &f.reference {
        Some(name) => {
            let case = r
                .cases
                .iter()
                .find(|c| &c.func == name)
                .unwrap_or_else(|| panic!("reference case `{name}` missing"));
            assert_eq!(
                case.params, f.params,
                "{name}: manifest and reference params"
            );
            case.values.clone()
        }
        None => r.x.iter().zip(&r.y).map(|(x, y)| Some(x / y)).collect(),
    }
}

fn uql(f: &Function, r: &Reference) -> Vec<Option<f64>> {
    let sig = SIGNATURES
        .iter()
        .find(|s| s.name == f.name)
        .unwrap_or_else(|| panic!("`{}` is not a series function", f.name));
    let args = (0..sig.series)
        .map(|i| SeriesExpr::channel(&format!("v{i}")))
        .collect();
    let column = DeriveColumn {
        name: "out".into(),
        expr: SeriesExpr::call(sig.func, args, f.params.clone()),
    };
    let ids: Vec<String> = (0..r.x.len()).map(|i| format!("s@{}", ts(i))).collect();
    let mut values = BTreeMap::new();
    for (i, id) in ids.iter().enumerate() {
        values.insert(
            id.clone(),
            BTreeMap::from([("v0".to_string(), r.x[i]), ("v1".to_string(), r.y[i])]),
        );
    }
    let input = RowSet::from_rows(ids.iter().map(|id| (id.clone(), None))).with_values(values);
    let out = derive_op(input, &[column]).expect("DERIVE runs");
    ids.iter().map(|id| out.value(id, "out")).collect()
}

fn sql(f: &Function, r: &Reference) -> Vec<Option<f64>> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("ts", DataType::Int64, false),
        Field::new("v0", DataType::Float64, false),
        Field::new("v1", DataType::Float64, false),
    ]));
    let cols: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from((0..r.x.len()).map(ts).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(r.x.clone())),
        Arc::new(Float64Array::from(r.y.clone())),
    ];
    let batch = RecordBatch::try_new(schema.clone(), cols).expect("batch");
    let sig = SIGNATURES
        .iter()
        .find(|s| s.name == f.name)
        .expect("signature");
    let mut args: Vec<String> = (0..sig.series).map(|i| format!("v{i}")).collect();
    args.extend(f.params.iter().map(|p| format!("{p}")));
    let query = format!(
        "SELECT medulla_{}({}) OVER (ORDER BY ts) AS out FROM s ORDER BY ts",
        f.name,
        args.join(", ")
    );
    let result = exec_sql_over_tables(vec![("s".to_string(), schema, vec![batch])], &query)
        .unwrap_or_else(|e| panic!("{query}: {e}"));
    result.rows.iter().map(|row| row[0].as_f64()).collect()
}

fn promql(f: &Function, r: &Reference) -> Vec<Option<f64>> {
    let sig = SIGNATURES
        .iter()
        .find(|s| s.name == f.name)
        .expect("signature");
    let mut source = MemSeriesSource::new();
    let points = |v: &[f64]| v.iter().enumerate().map(|(i, v)| (ts(i), *v)).collect();
    source.push(MemSeriesSource::labels("x", &[]), points(&r.x));
    source.push(MemSeriesSource::labels("y", &[]), points(&r.y));
    let mut args = vec!["x[1000s]".to_string()];
    if sig.series == 2 {
        args.push("y[1000s]".to_string());
    }
    args.extend(f.params.iter().map(|p| format!("{p}")));
    let expr = format!("medulla_{}({})", f.name, args.join(", "));
    let series = query_range(&source, &expr, ts(0), ts(r.x.len() - 1), NS)
        .unwrap_or_else(|e| panic!("{expr}: {e}"));
    assert!(series.len() <= 1, "{expr}: one output series");
    let got: BTreeMap<i64, f64> = series.into_iter().flat_map(|s| s.points).collect();
    (0..r.x.len()).map(|i| got.get(&ts(i)).copied()).collect()
}

// spec: MEDULLA-DECISION-ENGINE-R117.1.1
#[test]
fn uql_sql_promql_outputs_agree_on_golden_reference() {
    let reference: Reference = serde_json::from_str(REFERENCE).expect("reference parses");
    let manifest: Manifest = serde_json::from_str(MANIFEST).expect("manifest parses");
    assert_eq!(manifest.functions.len(), 15, "the fifteen golden functions");
    for f in &manifest.functions {
        let want = expected(f, &reference);
        for (surface, got) in [
            ("UQL DERIVE", uql(f, &reference)),
            ("SQL window", sql(f, &reference)),
            ("PromQL", promql(f, &reference)),
        ] {
            assert_eq!(got.len(), want.len(), "{} on {surface}: length", f.name);
            for (i, (g, w)) in got.iter().zip(&want).enumerate() {
                assert!(
                    agree(*g, *w, manifest.tolerance),
                    "{} on {surface} at row {i}: got {g:?}, want {w:?}",
                    f.name
                );
            }
        }
    }
}
