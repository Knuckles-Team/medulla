//! The partial-index scan of a user table (MEDULLA-ENGINE-PARITY-R010.2): the rows
//! a partial index covers stand in for the full scan when the query's `WHERE`
//! implies the index predicate. DataFusion still applies the whole filter above
//! the scan, so the index only ever narrows the rows.

use std::fmt;
use std::sync::Arc;

use datafusion::common::Statistics;
use datafusion::error::Result as DfResult;
use datafusion::execution::TaskContext;
use datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, ExecutionPlan, PlanProperties, SendableRecordBatchStream,
};
use medulla_tables::schema::{Cell, CheckExpr, ColumnType, TableSchema};

/// A scan over the rows of one named index. It forwards execution to the plan that
/// materializes those rows and only changes how the plan is shown, so `EXPLAIN`
/// names the index that served the read.
#[derive(Debug)]
pub(crate) struct IndexScanExec {
    index: String,
    inner: Arc<dyn ExecutionPlan>,
}

impl IndexScanExec {
    pub(crate) fn new(index: String, inner: Arc<dyn ExecutionPlan>) -> Self {
        Self { index, inner }
    }
}

impl DisplayAs for IndexScanExec {
    fn fmt_as(&self, _t: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "IndexScanExec: index={}", self.index)
    }
}

impl ExecutionPlan for IndexScanExec {
    fn name(&self) -> &str {
        "IndexScanExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.inner.properties()
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![&self.inner]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> DfResult<Arc<dyn ExecutionPlan>> {
        let inner = children.into_iter().next().ok_or_else(|| {
            datafusion::error::DataFusionError::Internal(
                "IndexScanExec takes exactly one child".into(),
            )
        })?;
        Ok(Arc::new(Self::new(self.index.clone(), inner)))
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> DfResult<SendableRecordBatchStream> {
        self.inner.execute(partition, context)
    }

    fn partition_statistics(&self, partition: Option<usize>) -> DfResult<Arc<Statistics>> {
        self.inner.partition_statistics(partition)
    }
}

/// Every leaf of an `AND` tree.
pub(crate) fn conjuncts(expr: &CheckExpr) -> Vec<&CheckExpr> {
    match expr {
        CheckExpr::And(left, right) => {
            let mut leaves = conjuncts(left);
            leaves.extend(conjuncts(right));
            leaves
        }
        other => vec![other],
    }
}

/// Does the query leaf say the same thing as the index-predicate leaf? Literals
/// are compared as the cells the column stores, so `5` and `5.0` agree on a
/// numeric column.
fn same_leaf(index: &CheckExpr, query: &CheckExpr, schema: &TableSchema) -> bool {
    match (index, query) {
        (
            CheckExpr::Cmp {
                column: ic,
                op: io,
                value: iv,
            },
            CheckExpr::Cmp {
                column: qc,
                op: qo,
                value: qv,
            },
        ) => {
            ic == qc
                && io == qo
                && schema
                    .column(ic)
                    .is_some_and(|col| same_literal(iv, qv, col.ty))
        }
        (other_index, other_query) => other_index == other_query,
    }
}

fn same_literal(left: &serde_json::Value, right: &serde_json::Value, ty: ColumnType) -> bool {
    match (Cell::coerce(left, ty, true), Cell::coerce(right, ty, true)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// An index predicate is implied by the query when each of its conjuncts equals a
/// conjunct of the query's `WHERE` (structural equality, no theorem proving).
pub(crate) fn predicate_implied(
    predicate: &CheckExpr,
    query: &[CheckExpr],
    schema: &TableSchema,
) -> bool {
    conjuncts(predicate)
        .into_iter()
        .all(|leaf| query.iter().any(|q| same_leaf(leaf, q, schema)))
}
