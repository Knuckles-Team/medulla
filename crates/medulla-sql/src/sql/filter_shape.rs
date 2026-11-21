//! Shared recognition of pushable filter shapes for table providers.
//!
//! Every provider in this crate that narrows a scan from a `WHERE` conjunct asks
//! the same two questions: "is this a `column = literal` equality (either side)?"
//! and "which of these conjuncts do I report as `Inexact`?". Both live here so the
//! shape rule is decided in one place.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use datafusion::common::tree_node::{TreeNode, TreeNodeRecursion};
use datafusion::common::{Column, ScalarValue};
use datafusion::error::Result as DfResult;
use datafusion::logical_expr::{Expr, LogicalPlan, Operator, TableProviderFilterPushDown};
use datafusion::physical_plan::ExecutionPlan;

/// What a table provider's `scan` returns: the physical plan, or DataFusion's error.
pub(crate) type ScanPlan = DfResult<Arc<dyn ExecutionPlan>>;

/// The `(column, literal)` of a `column = literal` / `literal = column` equality;
/// `None` for any other expression shape.
pub(crate) fn column_eq_literal(expr: &Expr) -> Option<(&Column, &ScalarValue)> {
    let Expr::BinaryExpr(be) = expr else {
        return None;
    };
    if be.op != Operator::Eq {
        return None;
    }
    match (be.left.as_ref(), be.right.as_ref()) {
        (Expr::Column(c), Expr::Literal(v, _)) | (Expr::Literal(v, _), Expr::Column(c)) => {
            Some((c, v))
        }
        _ => None,
    }
}

/// The column an operand of a text-search predicate names, through casts and aliases.
fn operand_column(expr: &Expr) -> Option<&Column> {
    match expr {
        Expr::Column(column) => Some(column),
        Expr::Cast(cast) => operand_column(&cast.expr),
        Expr::TryCast(cast) => operand_column(&cast.expr),
        Expr::Alias(alias) => operand_column(&alias.expr),
        _ => None,
    }
}

/// The distinct `(table, column)` pairs a plan uses as an operand of `LIKE`,
/// `ILIKE`, `NOT LIKE`, `NOT ILIKE` or `SIMILAR TO` (MEDULLA-FEDERATED-QUERY-R077.6.3).
/// The plan is walked once, subqueries included. A qualifier is resolved to the
/// scanned table it names, directly or through one alias; exact filters,
/// projections and joins are not text-search uses.
pub(crate) fn text_search_columns(plan: &LogicalPlan) -> Vec<(String, String)> {
    let mut tables: HashMap<String, String> = HashMap::new();
    let mut operands: Vec<Column> = Vec::new();
    let _ = plan.apply_with_subqueries(|node| {
        match node {
            LogicalPlan::TableScan(scan) => {
                tables.insert(scan.table_name.to_string(), scan.table_name.table().to_string());
            }
            LogicalPlan::SubqueryAlias(alias) => {
                if let LogicalPlan::TableScan(scan) = alias.input.as_ref() {
                    tables.insert(alias.alias.to_string(), scan.table_name.table().to_string());
                }
            }
            _ => {}
        }
        for expr in node.expressions() {
            let _ = expr.apply(|inner| {
                if let Expr::Like(like) | Expr::SimilarTo(like) = inner {
                    for operand in [like.expr.as_ref(), like.pattern.as_ref()] {
                        if let Some(column) = operand_column(operand) {
                            operands.push(column.clone());
                        }
                    }
                }
                Ok(TreeNodeRecursion::Continue)
            });
        }
        Ok(TreeNodeRecursion::Continue)
    });
    let found: BTreeSet<(String, String)> = operands
        .into_iter()
        .filter_map(|column| {
            let qualifier = column.relation.as_ref()?.to_string();
            Some((tables.get(&qualifier)?.clone(), column.name))
        })
        .collect();
    found.into_iter().collect()
}

/// Classify each conjunct: `Inexact` when `pushed` recognizes it (the provider
/// narrows the scan and DataFusion re-applies the filter above it), otherwise
/// `Unsupported` (an ordinary post-scan filter).
pub(crate) fn classify_pushdown(
    filters: &[&Expr],
    pushed: impl Fn(&Expr) -> bool,
) -> Vec<TableProviderFilterPushDown> {
    filters
        .iter()
        .map(|f| {
            if pushed(f) {
                TableProviderFilterPushDown::Inexact
            } else {
                TableProviderFilterPushDown::Unsupported
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::logical_expr::{col, lit};

    /// MEDULLA-FEDERATED-QUERY-R077.6.3
    #[test]
    fn like_operands_are_text_search_uses_and_equalities_are_not() {
        use datafusion::arrow::datatypes::{DataType, Field, Schema};
        use datafusion::logical_expr::table_scan;
        let schema = Schema::new(vec![
            Field::new("body", DataType::Utf8, true),
            Field::new("id", DataType::Int64, true),
        ]);
        let plan = table_scan(Some("docs"), &schema, None)
            .unwrap()
            .filter(
                col("docs.body")
                    .like(lit("%x%"))
                    .and(col("docs.id").eq(lit(1_i64))),
            )
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            text_search_columns(&plan),
            vec![("docs".to_string(), "body".to_string())]
        );
    }

    #[test]
    fn equality_is_recognized_on_either_side() {
        let left = col("src").eq(lit("a"));
        let right = lit("a").eq(col("src"));
        for expr in [left, right] {
            let (column, value) = column_eq_literal(&expr).expect("equality shape");
            assert_eq!(column.name, "src");
            assert_eq!(value, &ScalarValue::Utf8(Some("a".to_string())));
        }
    }

    #[test]
    fn other_shapes_are_not_equalities() {
        assert!(column_eq_literal(&col("n").gt(lit(1_i64))).is_none());
        assert!(column_eq_literal(&col("a").eq(col("b"))).is_none());
        assert!(column_eq_literal(&col("flag")).is_none());
    }

    #[test]
    fn classification_follows_the_predicate() {
        let pushed = col("src").eq(lit("a"));
        let kept = col("n").gt(lit(1_i64));
        let got = classify_pushdown(&[&pushed, &kept], |f| column_eq_literal(f).is_some());
        assert_eq!(
            got,
            vec![
                TableProviderFilterPushDown::Inexact,
                TableProviderFilterPushDown::Unsupported
            ]
        );
    }
}
