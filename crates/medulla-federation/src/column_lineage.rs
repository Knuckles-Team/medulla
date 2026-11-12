//! Column lineage from SQL on the shared parser (MEDULLA-UNIFIED-DATA-PLANE-R763.2).
//!
//! `derive_column_lineage` is a pure function: it resolves each output column of one query
//! through FROM aliases, joins, CTEs and subqueries to catalog columns and returns the edges.
//! Anything it cannot resolve becomes a [`LineageGap`], never an edge.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow;

use medulla_catalog_types::attached_catalog::AttachedCatalogGraph;
use medulla_catalog_types::lineage::{
    ColumnLineageEdge, ColumnTransform, LineageError, LineageGap, LineageResult,
};
use medulla_exec::sql_text::SqlDialect;
use sha2::{Digest, Sha256};
use sqlparser::ast::{
    visit_expressions, Expr, GroupByExpr, Ident, ObjectNamePart, Query, SelectItem,
    SelectItemQualifiedWildcardKind, SetExpr, Statement, TableAlias, TableFactor,
};

use crate::sql_analysis::parse_one_statement;

const AGGREGATES: [&str; 7] = [
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "array_agg",
    "string_agg",
];

#[derive(Clone, Debug)]
struct Col {
    name: String,
    sources: Vec<(String, ColumnTransform)>,
    gaps: Vec<String>,
}

#[derive(Clone, Debug)]
struct Rel {
    alias: String,
    cols: Vec<Col>,
}

#[derive(Clone, Debug, Default)]
struct QueryOut {
    cols: Vec<Col>,
    filters: BTreeSet<String>,
    filter_gaps: BTreeSet<String>,
}

type Ctes = BTreeMap<String, QueryOut>;

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn compose(inner: ColumnTransform, outer: ColumnTransform) -> ColumnTransform {
    match outer {
        ColumnTransform::Identity => inner,
        ColumnTransform::Aggregate => ColumnTransform::Aggregate,
        _ => ColumnTransform::Expression,
    }
}

fn strip_nested(expr: &Expr) -> &Expr {
    match expr {
        Expr::Nested(inner) => strip_nested(inner),
        other => other,
    }
}

fn classify(expr: &Expr) -> ColumnTransform {
    match strip_nested(expr) {
        Expr::Identifier(_) | Expr::CompoundIdentifier(_) => ColumnTransform::Identity,
        Expr::Function(function) => {
            let last = function.name.0.last().and_then(|part| match part {
                ObjectNamePart::Identifier(ident) => Some(ident.value.to_lowercase()),
                _ => None,
            });
            match last {
                Some(name) if AGGREGATES.contains(&name.as_str()) => ColumnTransform::Aggregate,
                _ => ColumnTransform::Expression,
            }
        }
        _ => ColumnTransform::Expression,
    }
}

fn output_name(expr: &Expr) -> String {
    match strip_nested(expr) {
        Expr::Identifier(ident) => ident.value.clone(),
        Expr::CompoundIdentifier(parts) => {
            parts.last().map(|p| p.value.clone()).unwrap_or_default()
        }
        other => other.to_string(),
    }
}

/// The column references of an expression, or `None` when it holds a subquery.
fn column_refs(node: &impl sqlparser::ast::Visit) -> Option<Vec<Vec<Ident>>> {
    let mut refs: Vec<Vec<Ident>> = Vec::new();
    let mut has_subquery = false;
    let _ = visit_expressions(node, |expr| {
        match expr {
            Expr::Identifier(ident) => refs.push(vec![ident.clone()]),
            Expr::CompoundIdentifier(parts) => refs.push(parts.clone()),
            Expr::Subquery(_) | Expr::Exists { .. } | Expr::InSubquery { .. } => {
                has_subquery = true;
            }
            _ => {}
        }
        ControlFlow::<()>::Continue(())
    });
    if has_subquery {
        None
    } else {
        Some(refs)
    }
}

struct Resolved {
    sources: Vec<(String, ColumnTransform)>,
    gaps: Vec<String>,
}

fn resolve_ref(parts: &[Ident], rels: &[Rel]) -> Result<Resolved, String> {
    let column = parts.last().map(|p| p.value.as_str()).unwrap_or_default();
    let found: Vec<&Col> = if parts.len() >= 2 {
        let qualifier = parts[parts.len() - 2].value.as_str();
        let rel = rels
            .iter()
            .find(|rel| same(&rel.alias, qualifier))
            .ok_or_else(|| format!("unknown table alias {qualifier}"))?;
        rel.cols.iter().filter(|c| same(&c.name, column)).collect()
    } else {
        rels.iter()
            .flat_map(|rel| rel.cols.iter())
            .filter(|c| same(&c.name, column))
            .collect()
    };
    match found.as_slice() {
        [] => Err(format!("unknown column {column}")),
        [one] => Ok(Resolved {
            sources: one.sources.clone(),
            gaps: one.gaps.clone(),
        }),
        _ => Err(format!("ambiguous column {column}")),
    }
}

fn rename(mut cols: Vec<Col>, alias: &Option<TableAlias>) -> Vec<Col> {
    if let Some(alias) = alias {
        for (col, declared) in cols.iter_mut().zip(alias.columns.iter()) {
            col.name = declared.name.value.clone();
        }
    }
    cols
}

fn table_cols(
    catalog: &AttachedCatalogGraph,
    parts: &[Ident],
) -> Result<(String, Vec<Col>), String> {
    let table = parts.last().map(|p| p.value.as_str()).unwrap_or_default();
    let schema = (parts.len() >= 2).then(|| parts[parts.len() - 2].value.as_str());
    let matches: Vec<_> = catalog
        .tables
        .iter()
        .filter(|t| same(&t.name, table) && schema.map(|s| same(&t.schema, s)).unwrap_or(true))
        .collect();
    match matches.as_slice() {
        [] => Err(format!("table {table} is not in the catalog")),
        [t] => {
            let prefix = if t.schema.is_empty() {
                t.name.clone()
            } else {
                format!("{}.{}", t.schema, t.name)
            };
            let cols = t
                .columns
                .iter()
                .map(|c| Col {
                    name: c.name.clone(),
                    sources: vec![(format!("{prefix}.{}", c.name), ColumnTransform::Identity)],
                    gaps: Vec::new(),
                })
                .collect();
            Ok((t.name.clone(), cols))
        }
        _ => Err(format!("table {table} is ambiguous in the catalog")),
    }
}

struct Scope<'a> {
    catalog: &'a AttachedCatalogGraph,
    ctes: &'a Ctes,
}

fn add_factor(
    scope: &Scope<'_>,
    factor: &TableFactor,
    rels: &mut Vec<Rel>,
    filters: &mut QueryOut,
) -> Result<(), String> {
    match factor {
        TableFactor::Table { name, alias, .. } => {
            let parts: Vec<Ident> = name
                .0
                .iter()
                .filter_map(|p| match p {
                    ObjectNamePart::Identifier(i) => Some(i.clone()),
                    _ => None,
                })
                .collect();
            let table = parts
                .last()
                .map(|p| p.value.to_lowercase())
                .unwrap_or_default();
            let (base, cols) = match scope.ctes.get(&table).filter(|_| parts.len() == 1) {
                Some(out) => {
                    filters.filters.extend(out.filters.iter().cloned());
                    filters.filter_gaps.extend(out.filter_gaps.iter().cloned());
                    (parts[0].value.clone(), out.cols.clone())
                }
                None => table_cols(scope.catalog, &parts)?,
            };
            let alias_name = alias.as_ref().map(|a| a.name.value.clone()).unwrap_or(base);
            rels.push(Rel {
                alias: alias_name,
                cols: rename(cols, alias),
            });
            Ok(())
        }
        TableFactor::Derived {
            subquery, alias, ..
        } => {
            let out = resolve_query(scope, subquery)?;
            filters.filters.extend(out.filters.iter().cloned());
            filters.filter_gaps.extend(out.filter_gaps.iter().cloned());
            rels.push(Rel {
                alias: alias
                    .as_ref()
                    .map(|a| a.name.value.clone())
                    .unwrap_or_default(),
                cols: rename(out.cols, alias),
            });
            Ok(())
        }
        TableFactor::NestedJoin {
            table_with_joins, ..
        } => {
            add_factor(scope, &table_with_joins.relation, rels, filters)?;
            for join in &table_with_joins.joins {
                add_factor(scope, &join.relation, rels, filters)?;
            }
            Ok(())
        }
        _ => Err("unsupported table expression".to_string()),
    }
}

fn note_filter(node: &impl sqlparser::ast::Visit, rels: &[Rel], out: &mut QueryOut) {
    match column_refs(node) {
        None => {
            out.filter_gaps
                .insert("subquery in a filter is not traced".to_string());
        }
        Some(refs) => {
            for parts in refs {
                match resolve_ref(&parts, rels) {
                    Ok(found) => {
                        out.filters
                            .extend(found.sources.into_iter().map(|(k, _)| k));
                        out.filter_gaps.extend(found.gaps);
                    }
                    Err(reason) => {
                        out.filter_gaps.insert(reason);
                    }
                }
            }
        }
    }
}

fn project(rels: &[Rel], item: &SelectItem, out: &mut QueryOut) -> Result<(), String> {
    let (expr, alias) = match item {
        SelectItem::UnnamedExpr(expr) => (expr, None),
        SelectItem::ExprWithAlias { expr, alias } => (expr, Some(alias.value.clone())),
        SelectItem::Wildcard(_) => {
            out.cols
                .extend(rels.iter().flat_map(|r| r.cols.iter().cloned()));
            return Ok(());
        }
        SelectItem::QualifiedWildcard(SelectItemQualifiedWildcardKind::ObjectName(name), _) => {
            let qualifier = name
                .0
                .last()
                .and_then(|p| match p {
                    ObjectNamePart::Identifier(i) => Some(i.value.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let rel = rels
                .iter()
                .find(|r| same(&r.alias, &qualifier))
                .ok_or_else(|| format!("unknown table alias {qualifier}"))?;
            out.cols.extend(rel.cols.iter().cloned());
            return Ok(());
        }
        _ => return Err("unsupported select item".to_string()),
    };
    let name = alias.unwrap_or_else(|| output_name(expr));
    let outer = classify(expr);
    let mut col = Col {
        name,
        sources: Vec::new(),
        gaps: Vec::new(),
    };
    match column_refs(expr) {
        None => col
            .gaps
            .push("subquery expression is not traced".to_string()),
        Some(refs) => {
            for parts in refs {
                match resolve_ref(&parts, rels) {
                    Ok(found) => {
                        col.gaps.extend(found.gaps);
                        col.sources.extend(
                            found
                                .sources
                                .into_iter()
                                .map(|(key, inner)| (key, compose(inner, outer))),
                        );
                    }
                    Err(reason) => col.gaps.push(reason),
                }
            }
        }
    }
    out.cols.push(col);
    Ok(())
}

fn resolve_set(scope: &Scope<'_>, body: &SetExpr) -> Result<QueryOut, String> {
    match body {
        SetExpr::Select(select) => {
            let mut rels: Vec<Rel> = Vec::new();
            let mut out = QueryOut::default();
            for from in &select.from {
                add_factor(scope, &from.relation, &mut rels, &mut out)?;
                for join in &from.joins {
                    add_factor(scope, &join.relation, &mut rels, &mut out)?;
                }
            }
            for from in &select.from {
                for join in &from.joins {
                    note_filter(&join.join_operator, &rels, &mut out);
                }
            }
            if let Some(selection) = &select.selection {
                note_filter(selection, &rels, &mut out);
            }
            if let Some(having) = &select.having {
                note_filter(having, &rels, &mut out);
            }
            if let GroupByExpr::Expressions(exprs, _) = &select.group_by {
                for expr in exprs {
                    note_filter(expr, &rels, &mut out);
                }
            }
            for item in &select.projection {
                project(&rels, item, &mut out)?;
            }
            Ok(out)
        }
        SetExpr::Query(query) => resolve_query(scope, query),
        SetExpr::SetOperation { left, right, .. } => {
            let mut left = resolve_set(scope, left)?;
            let right = resolve_set(scope, right)?;
            if left.cols.len() != right.cols.len() {
                return Err("set operation sides differ in column count".to_string());
            }
            for (l, r) in left.cols.iter_mut().zip(right.cols) {
                l.sources.extend(r.sources);
                l.gaps.extend(r.gaps);
            }
            left.filters.extend(right.filters);
            left.filter_gaps.extend(right.filter_gaps);
            Ok(left)
        }
        _ => Err("unsupported query body".to_string()),
    }
}

fn resolve_query(scope: &Scope<'_>, query: &Query) -> Result<QueryOut, String> {
    let mut local = scope.ctes.clone();
    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            let inner = Scope {
                catalog: scope.catalog,
                ctes: &local,
            };
            let mut out = resolve_query(&inner, &cte.query)?;
            out.cols = rename(out.cols, &Some(cte.alias.clone()));
            local.insert(cte.alias.name.value.to_lowercase(), out);
        }
    }
    let scope = Scope {
        catalog: scope.catalog,
        ctes: &local,
    };
    resolve_set(&scope, &query.body)
}

/// Derive the column edges of one SQL query against `catalog`, naming output columns
/// `<target>.<column>`. A column used only in WHERE, ON, GROUP BY or HAVING gives a
/// `filter_only` edge to `<target>.*`.
pub fn derive_column_lineage(
    sql: &str,
    dialect: SqlDialect,
    catalog: &AttachedCatalogGraph,
    target: &str,
) -> Result<LineageResult, LineageError> {
    let statement = parse_one_statement(sql, dialect)?;
    let Statement::Query(query) = statement else {
        return Err(LineageError::NotAQuery);
    };
    let digest = hex::encode(Sha256::digest(sql.as_bytes()));
    let scope = Scope {
        catalog,
        ctes: &Ctes::new(),
    };
    let mut result = LineageResult::default();
    let out = match resolve_query(&scope, &query) {
        Ok(out) => out,
        Err(reason) => {
            result.gaps.push(LineageGap {
                output: format!("{target}.*"),
                reason,
            });
            return Ok(result);
        }
    };
    let mut edges: BTreeSet<(String, String, u8)> = BTreeSet::new();
    let mut gaps: BTreeSet<LineageGap> = BTreeSet::new();
    let mut output_sources: BTreeSet<&str> = BTreeSet::new();
    let transforms = [
        ColumnTransform::Identity,
        ColumnTransform::Expression,
        ColumnTransform::Aggregate,
        ColumnTransform::FilterOnly,
    ];
    for col in &out.cols {
        let output = format!("{target}.{}", col.name);
        for (source, transform) in &col.sources {
            output_sources.insert(source.as_str());
            let rank = transforms.iter().position(|t| t == transform).unwrap_or(1) as u8;
            edges.insert((source.clone(), output.clone(), rank));
        }
        for reason in &col.gaps {
            gaps.insert(LineageGap {
                output: output.clone(),
                reason: reason.clone(),
            });
        }
    }
    let filter_target = format!("{target}.*");
    for key in &out.filters {
        if !output_sources.contains(key.as_str()) {
            edges.insert((key.clone(), filter_target.clone(), 3));
        }
    }
    for reason in &out.filter_gaps {
        gaps.insert(LineageGap {
            output: filter_target.clone(),
            reason: reason.clone(),
        });
    }
    result.edges = edges
        .into_iter()
        .map(|(source_column, target_column, rank)| ColumnLineageEdge {
            source_column,
            target_column,
            transform: transforms[rank as usize],
            query_digest: digest.clone(),
        })
        .collect();
    result.gaps = gaps.into_iter().collect();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_catalog_types::attached_catalog::{CatalogColumn, CatalogTable};

    fn table(schema: &str, name: &str, cols: &[&str]) -> CatalogTable {
        CatalogTable {
            schema: schema.to_string(),
            name: name.to_string(),
            columns: cols
                .iter()
                .map(|c| CatalogColumn {
                    name: (*c).to_string(),
                    type_name: "text".to_string(),
                    nullable: true,
                })
                .collect(),
        }
    }

    fn catalog() -> AttachedCatalogGraph {
        AttachedCatalogGraph {
            tables: vec![
                table(
                    "public",
                    "orders",
                    &["id", "customer_id", "amount", "status"],
                ),
                table("public", "customers", &["id", "name", "region"]),
            ],
            version: 1,
        }
    }

    fn edge<'a>(r: &'a LineageResult, source: &str, target: &str) -> Option<&'a ColumnLineageEdge> {
        r.edges
            .iter()
            .find(|e| e.source_column == source && e.target_column == target)
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R763.2
    #[test]
    fn view_columns_resolve() {
        let sql = "SELECT c.name AS customer, SUM(o.amount) AS total, o.amount * 2 AS doubled, \
                   o.* FROM public.orders o JOIN public.customers c ON c.id = o.customer_id \
                   WHERE c.region = 'eu' AND o.status = 'open' GROUP BY c.name";
        let r = derive_column_lineage(sql, SqlDialect::Postgres, &catalog(), "v").unwrap();
        let t = |s: &str, d: &str| edge(&r, s, d).map(|e| e.transform);
        assert_eq!(
            t("public.customers.name", "v.customer"),
            Some(ColumnTransform::Identity)
        );
        assert_eq!(
            t("public.orders.amount", "v.total"),
            Some(ColumnTransform::Aggregate)
        );
        assert_eq!(
            t("public.orders.amount", "v.doubled"),
            Some(ColumnTransform::Expression)
        );
        // `o.*` expands from the catalog.
        assert_eq!(
            t("public.orders.id", "v.id"),
            Some(ColumnTransform::Identity)
        );
        // Used only in WHERE or ON: a filter edge, not an output edge.
        assert_eq!(
            t("public.customers.region", "v.*"),
            Some(ColumnTransform::FilterOnly)
        );
        assert_eq!(
            t("public.customers.id", "v.*"),
            Some(ColumnTransform::FilterOnly)
        );
        // `status` is also selected through `o.*`, so it is not filter-only.
        assert!(edge(&r, "public.orders.status", "v.*").is_none());
        assert!(r.gaps.is_empty(), "{:?}", r.gaps);
        let digest = &r.edges[0].query_digest;
        assert!(r.edges.iter().all(|e| &e.query_digest == digest));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R763.2
    #[test]
    fn unknown_and_ambiguous_columns_are_gaps_not_edges() {
        let sql = "SELECT nope AS missing, id FROM public.orders o \
                   JOIN public.customers c ON c.id = o.customer_id";
        let r = derive_column_lineage(sql, SqlDialect::Postgres, &catalog(), "v").unwrap();
        let gap = |out: &str| {
            r.gaps
                .iter()
                .find(|g| g.output == out)
                .map(|g| g.reason.clone())
        };
        assert!(gap("v.missing").unwrap().contains("unknown column"));
        assert!(gap("v.id").unwrap().contains("ambiguous"));
        assert!(r
            .edges
            .iter()
            .all(|e| e.target_column != "v.missing" && e.target_column != "v.id"));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R763.2
    #[test]
    fn ctes_subqueries_and_unions_resolve_through_to_the_catalog() {
        let sql = "WITH big AS (SELECT customer_id, SUM(amount) AS spent FROM public.orders \
                   GROUP BY customer_id) \
                   SELECT b.spent AS value, b.customer_id AS who FROM big b \
                   UNION ALL SELECT d.id, d.id FROM (SELECT id FROM public.customers) d";
        let r = derive_column_lineage(sql, SqlDialect::MySql, &catalog(), "u").unwrap();
        let t = |s: &str, d: &str| edge(&r, s, d).map(|e| e.transform);
        // An identity over an aggregate stays an aggregate.
        assert_eq!(
            t("public.orders.amount", "u.value"),
            Some(ColumnTransform::Aggregate)
        );
        assert_eq!(
            t("public.orders.customer_id", "u.who"),
            Some(ColumnTransform::Identity)
        );
        // UNION maps by position: both right-hand columns land on the left names.
        assert_eq!(
            t("public.customers.id", "u.value"),
            Some(ColumnTransform::Identity)
        );
        assert_eq!(
            t("public.customers.id", "u.who"),
            Some(ColumnTransform::Identity)
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R763.2
    #[test]
    fn non_queries_and_unknown_tables_are_refused_or_gapped() {
        let refused = derive_column_lineage(
            "DELETE FROM public.orders",
            SqlDialect::Postgres,
            &catalog(),
            "v",
        );
        assert_eq!(refused, Err(LineageError::NotAQuery));
        let r = derive_column_lineage(
            "SELECT x FROM public.missing",
            SqlDialect::Postgres,
            &catalog(),
            "v",
        )
        .unwrap();
        assert!(r.edges.is_empty());
        assert_eq!(r.gaps.len(), 1);
    }
}
