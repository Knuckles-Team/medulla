//! `CREATE [UNIQUE] INDEX` over ordinary scalar columns
//! (MEDULLA-ENGINE-PARITY-R010.2). The pgvector ANN spelling is recognized
//! textually before the parser runs; everything that reaches this module is the
//! scalar B-tree form, with optional closed-set key expressions and a `WHERE`
//! predicate.

use datafusion::sql::sqlparser::ast::{
    CreateIndex, Expr, FunctionArg, FunctionArgExpr, FunctionArguments, IndexColumn, IndexType,
};
use medulla_table_store::tables::{IndexColumnPlan, IndexPlan};
use medulla_tables::index::{IndexExpr, INDEX_EXPR_NONDETERMINISTIC};

use super::{decode_check_expr, ident_from_expr, last_ident, StatementKind};

/// Decode a `CREATE INDEX` statement into [`StatementKind::CreateIndex`].
pub(super) fn classify_create_index(create: &CreateIndex) -> Result<StatementKind, String> {
    match &create.using {
        None | Some(IndexType::BTree) => {}
        Some(other) => return Err(format!("CREATE INDEX USING {other} is not supported")),
    }
    if !create.include.is_empty() || create.nulls_distinct.is_some() || !create.with.is_empty() {
        return Err("CREATE INDEX INCLUDE, NULLS [NOT] DISTINCT and WITH are not supported".into());
    }
    let columns = create
        .columns
        .iter()
        .map(index_column)
        .collect::<Result<Vec<_>, _>>()?;
    if columns.is_empty() {
        return Err("CREATE INDEX needs at least one column".into());
    }
    let predicate = create
        .predicate
        .as_ref()
        .map(|expr| decode_check_expr(expr).map_err(|e| format!("index predicate: {e}")))
        .transpose()?;
    Ok(StatementKind::CreateIndex(IndexPlan {
        name: create.name.as_ref().map(last_ident),
        table: last_ident(&create.table_name),
        columns,
        predicate,
        unique: create.unique,
        if_not_exists: create.if_not_exists,
    }))
}

/// One key column: a plain column, or `fn(column)` with `fn` in the closed
/// [`IndexExpr`] set. Any other expression is refused with
/// `INDEX_EXPR_NONDETERMINISTIC`.
fn index_column(column: &IndexColumn) -> Result<IndexColumnPlan, String> {
    if column.operator_class.is_some() || column.column.options.nulls_first.is_some() {
        return Err("CREATE INDEX operator classes and NULLS FIRST/LAST are not supported".into());
    }
    let (name, expr) = key_expression(&column.column.expr)?;
    Ok(IndexColumnPlan {
        column: name,
        expr,
        descending: column.column.options.asc == Some(false),
    })
}

fn key_expression(expr: &Expr) -> Result<(String, Option<IndexExpr>), String> {
    match expr {
        Expr::Nested(inner) => key_expression(inner),
        Expr::Identifier(_) | Expr::CompoundIdentifier(_) => Ok((ident_from_expr(expr)?, None)),
        Expr::Function(function) => {
            let parsed = IndexExpr::parse(&last_ident(&function.name))?;
            let FunctionArguments::List(list) = &function.args else {
                return Err(not_allowed(expr));
            };
            match list.args.as_slice() {
                [FunctionArg::Unnamed(FunctionArgExpr::Expr(arg))] => {
                    Ok((ident_from_expr(arg)?, Some(parsed)))
                }
                _ => Err(not_allowed(expr)),
            }
        }
        other => Err(not_allowed(other)),
    }
}

fn not_allowed(expr: &Expr) -> String {
    format!(
        "{INDEX_EXPR_NONDETERMINISTIC}: `{expr}` is not an allowed index expression (a column, or lower, upper, abs, length of one column)"
    )
}
