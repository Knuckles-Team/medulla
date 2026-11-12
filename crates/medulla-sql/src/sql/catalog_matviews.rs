//! The `medulla_catalog.matviews` system table (MEDULLA-INCREMENTAL-VIEWS-R005.3).
//!
//! One row per published view on the process-global status board
//! (`medulla_core::view_status`). `register_system_catalogs` filters the snapshot to
//! the graphs the caller may read before rendering this batch.

use std::sync::Arc;

use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;

use medulla_core::view_status::ViewStatus;

/// The fixed column layout of `medulla_catalog.matviews`, in order
/// (MEDULLA-INCREMENTAL-VIEWS-R005.3).
pub fn matviews_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("name", DataType::Utf8, false),
        Field::new("family", DataType::Utf8, false),
        Field::new("graph", DataType::Utf8, false),
        Field::new("mode", DataType::Utf8, false),
        Field::new("reason_code", DataType::Utf8, true),
        Field::new("reason", DataType::Utf8, true),
        Field::new("state_bytes", DataType::Int64, false),
        Field::new("applied_changes", DataType::Int64, false),
        Field::new("shadow_checks", DataType::Int64, false),
        Field::new("shadow_mismatches", DataType::Int64, false),
    ]))
}

/// Render `rows` — already filtered to graphs the caller may read — as the `matviews`
/// system-table batch.
pub fn matviews_batch(rows: &[ViewStatus]) -> Result<(SchemaRef, RecordBatch), String> {
    let schema = matviews_schema();
    let names: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();
    let families: Vec<String> = rows.iter().map(|r| r.family.as_str().to_string()).collect();
    let graphs: Vec<String> = rows.iter().map(|r| r.graph.clone()).collect();
    let modes: Vec<String> = rows.iter().map(|r| r.mode.as_str().to_string()).collect();
    let reason_codes: Vec<Option<String>> = rows.iter().map(|r| r.reason_code.clone()).collect();
    let reasons: Vec<Option<String>> = rows.iter().map(|r| r.reason.clone()).collect();
    let state_bytes: Vec<i64> = rows.iter().map(|r| r.state_bytes as i64).collect();
    let applied: Vec<i64> = rows.iter().map(|r| r.applied_changes as i64).collect();
    let checks: Vec<i64> = rows.iter().map(|r| r.shadow_checks as i64).collect();
    let mismatches: Vec<i64> = rows.iter().map(|r| r.shadow_mismatches as i64).collect();
    RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(names)),
            Arc::new(StringArray::from(families)),
            Arc::new(StringArray::from(graphs)),
            Arc::new(StringArray::from(modes)),
            Arc::new(StringArray::from_iter(reason_codes.into_iter())),
            Arc::new(StringArray::from_iter(reasons.into_iter())),
            Arc::new(Int64Array::from(state_bytes)),
            Arc::new(Int64Array::from(applied)),
            Arc::new(Int64Array::from(checks)),
            Arc::new(Int64Array::from(mismatches)),
        ],
    )
    .map(|b| (schema, b))
    .map_err(|e| format!("medulla_catalog.matviews batch: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_core::view_status::{ViewFamily, ViewMode};

    fn row(name: &str, mode: ViewMode, reason_code: Option<&str>) -> ViewStatus {
        ViewStatus {
            name: name.to_string(),
            family: ViewFamily::Plan,
            graph: "g".to_string(),
            mode,
            reason_code: reason_code.map(str::to_string),
            reason: None,
            state_bytes: 16,
            applied_changes: 3,
            shadow_checks: 1,
            shadow_mismatches: 0,
        }
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R005.3
    /// Verifies MEDULLA-INCREMENTAL-VIEWS-R005.3: System table `medulla_catalog.matviews`
    #[test]
    fn matviews_batch_has_fixed_columns() {
        let (schema, batch) = matviews_batch(&[
            row("a", ViewMode::Incremental, None),
            row("b", ViewMode::Recompute, Some("MATVIEW_NOT_INCREMENTAL")),
        ])
        .unwrap();
        let fields: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            fields,
            vec![
                "name",
                "family",
                "graph",
                "mode",
                "reason_code",
                "reason",
                "state_bytes",
                "applied_changes",
                "shadow_checks",
                "shadow_mismatches"
            ]
        );
        // identity columns are non-null; the two reason columns are nullable.
        for i in 0..4 {
            assert!(
                !schema.field(i).is_nullable(),
                "column {i} must be non-null"
            );
        }
        assert!(schema.field(4).is_nullable());
        assert!(schema.field(5).is_nullable());
        for i in 6..10 {
            assert!(
                !schema.field(i).is_nullable(),
                "column {i} must be non-null"
            );
        }
        assert_eq!(batch.num_rows(), 2);
    }
}
