//! The relation and column names one SQL SELECT text names
//! (MEDULLA-FEDERATED-QUERY-R058.4.10).
//!
//! A natural-language planner answers with SQL text. Before that text runs, the engine
//! parses it once with the parser `classify` uses and accepts exactly one read
//! statement: a single `SELECT` (set operations, `WITH` and subqueries included).
//! Any other statement, and any `SELECT` that writes (`INTO`, a data-modifying CTE),
//! is [`SelectNamesError::NotSelect`]. The names it returns are what grounding
//! checks against the catalog the caller may read.

use std::collections::BTreeSet;
use std::ops::ControlFlow;

use datafusion::sql::sqlparser::ast::{
    visit_expressions, visit_relations, Expr, ObjectNamePart, Query, SelectItem, SetExpr,
    Statement, TableFactor, TableWithJoins,
};
use datafusion::sql::sqlparser::dialect::PostgreSqlDialect;
use datafusion::sql::sqlparser::parser::Parser;

/// The names of one accepted SELECT, all lowercase.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectNames {
    /// Catalog relations it reads (last name part), without CTE and derived-table aliases.
    pub tables: BTreeSet<String>,
    /// Column names it references (last name part), without output aliases.
    pub columns: BTreeSet<String>,
    /// A CTE or derived table renames its columns with an alias list, so a referenced
    /// column cannot be matched to a catalog column by name.
    pub columns_renamed: bool,
}

/// Why a text is not one accepted SELECT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectNamesError {
    /// The text does not parse; carries the parser message.
    Parse(String),
    /// The text parses but is not a single read statement; carries what it is.
    NotSelect(String),
}

#[derive(Default)]
struct Scan {
    hidden: BTreeSet<String>,
    aliases: BTreeSet<String>,
    renamed: bool,
}

fn lower(value: &str) -> String {
    value.to_lowercase()
}

/// Parse `sql` and return the names of its one SELECT.
pub fn select_names(sql: &str) -> Result<SelectNames, SelectNamesError> {
    let statements = Parser::parse_sql(&PostgreSqlDialect {}, sql)
        .map_err(|e| SelectNamesError::Parse(format!("parse error: {e}")))?;
    let [statement] = statements.as_slice() else {
        return Err(SelectNamesError::NotSelect(format!(
            "expected exactly one statement, found {}",
            statements.len()
        )));
    };
    let Statement::Query(query) = statement else {
        let form = statement
            .to_string()
            .split_whitespace()
            .next()
            .unwrap_or("statement")
            .to_uppercase();
        return Err(SelectNamesError::NotSelect(format!(
            "{form} is not a SELECT"
        )));
    };
    let mut scan = Scan::default();
    scan_query(query, &mut scan)?;
    let mut subqueries: Vec<Query> = Vec::new();
    let mut columns = BTreeSet::new();
    let _ = visit_expressions(statement, |expr| {
        match expr {
            Expr::Identifier(ident) => {
                columns.insert(lower(&ident.value));
            }
            Expr::CompoundIdentifier(parts) => {
                if let Some(last) = parts.last() {
                    columns.insert(lower(&last.value));
                }
            }
            Expr::Subquery(q) => subqueries.push((**q).clone()),
            Expr::Exists { subquery, .. } | Expr::InSubquery { subquery, .. } => {
                subqueries.push((**subquery).clone());
            }
            _ => {}
        }
        ControlFlow::<()>::Continue(())
    });
    for subquery in subqueries {
        scan_query(&subquery, &mut scan)?;
    }
    let mut tables = BTreeSet::new();
    let _ = visit_relations(statement, |name| {
        if let Some(ObjectNamePart::Identifier(ident)) = name.0.last() {
            tables.insert(lower(&ident.value));
        }
        ControlFlow::<()>::Continue(())
    });
    let tables = tables.difference(&scan.hidden).cloned().collect();
    let columns = columns.difference(&scan.aliases).cloned().collect();
    Ok(SelectNames {
        tables,
        columns,
        columns_renamed: scan.renamed,
    })
}

fn scan_query(query: &Query, scan: &mut Scan) -> Result<(), SelectNamesError> {
    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            scan.hidden.insert(lower(&cte.alias.name.value));
            scan.renamed |= !cte.alias.columns.is_empty();
            scan_query(&cte.query, scan)?;
        }
    }
    scan_set(&query.body, scan)
}

fn scan_set(body: &SetExpr, scan: &mut Scan) -> Result<(), SelectNamesError> {
    match body {
        SetExpr::Select(select) => {
            if select.into.is_some() {
                return Err(SelectNamesError::NotSelect("SELECT INTO writes".into()));
            }
            for item in &select.projection {
                if let SelectItem::ExprWithAlias { alias, .. } = item {
                    scan.aliases.insert(lower(&alias.value));
                }
            }
            select
                .from
                .iter()
                .try_for_each(|from| scan_joined(from, scan))
        }
        SetExpr::Query(query) => scan_query(query, scan),
        SetExpr::SetOperation { left, right, .. } => {
            scan_set(left, scan)?;
            scan_set(right, scan)
        }
        SetExpr::Values(_) => Ok(()),
        other => Err(SelectNamesError::NotSelect(format!(
            "`{other}` is not a SELECT"
        ))),
    }
}

fn scan_joined(from: &TableWithJoins, scan: &mut Scan) -> Result<(), SelectNamesError> {
    scan_factor(&from.relation, scan)?;
    from.joins
        .iter()
        .try_for_each(|join| scan_factor(&join.relation, scan))
}

fn scan_factor(factor: &TableFactor, scan: &mut Scan) -> Result<(), SelectNamesError> {
    match factor {
        TableFactor::Derived {
            subquery, alias, ..
        } => {
            if let Some(alias) = alias {
                scan.hidden.insert(lower(&alias.name.value));
                scan.renamed |= !alias.columns.is_empty();
            }
            scan_query(subquery, scan)
        }
        TableFactor::NestedJoin {
            table_with_joins, ..
        } => scan_joined(table_with_joins, scan),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R058.4.10
    #[test]
    fn names_of_a_select_with_alias_cte_and_subquery() {
        let names = select_names(
            "WITH recent AS (SELECT id, total FROM orders) \
             SELECT r.id, SUM(r.total) AS spend FROM recent r \
             JOIN customers c ON c.id = r.id \
             WHERE r.id IN (SELECT id FROM vip) ORDER BY spend",
        )
        .expect("select");
        let tables: Vec<_> = names.tables.iter().map(String::as_str).collect();
        assert_eq!(tables, ["customers", "orders", "vip"]);
        assert!(names.columns.contains("total") && !names.columns.contains("spend"));
        assert!(!names.columns_renamed);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R058.4.10
    #[test]
    fn anything_but_one_select_is_not_a_select() {
        for text in [
            "DELETE FROM orders",
            "CREATE TABLE t (id INT)",
            "SELECT 1; DROP TABLE orders",
            "SELECT id INTO archive FROM orders",
            "WITH d AS (DELETE FROM orders RETURNING id) SELECT id FROM d",
        ] {
            assert!(
                matches!(select_names(text), Err(SelectNamesError::NotSelect(_))),
                "{text}"
            );
        }
        assert!(matches!(
            select_names("SELECT FROM WHERE"),
            Err(SelectNamesError::Parse(_))
        ));
    }
}
