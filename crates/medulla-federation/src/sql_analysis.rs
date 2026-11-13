//! SQL statement analysis on the one shared parser (MEDULLA-UNIFIED-DATA-PLANE-R763.2 and
//! R764.3.1). `parse_one_statement` is the single parse entry; `referenced_tables` lists the
//! tables a statement reads, for usage accounting.

use std::collections::BTreeSet;
use std::ops::ControlFlow;

use medulla_catalog_types::lineage::{
    LineageError, LINEAGE_MAX_SQL_BYTES_DEFAULT, LINEAGE_MAX_SQL_BYTES_RANGE,
};
use medulla_exec::sql_text::SqlDialect;
use sqlparser::ast::{ObjectName, ObjectNamePart, Query, Statement, Visit, Visitor};
use sqlparser::dialect::{MySqlDialect, PostgreSqlDialect};
use sqlparser::parser::Parser;

/// Environment override of `knob lineage.max_sql_bytes`.
pub const ENV_LINEAGE_MAX_SQL_BYTES: &str = "MEDULLA_LINEAGE_MAX_SQL_BYTES";

/// The effective `knob lineage.max_sql_bytes`: the environment value when it lies inside the
/// knob's bounds, the default otherwise.
pub fn lineage_max_sql_bytes() -> usize {
    std::env::var(ENV_LINEAGE_MAX_SQL_BYTES)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|n| (LINEAGE_MAX_SQL_BYTES_RANGE.0..=LINEAGE_MAX_SQL_BYTES_RANGE.1).contains(n))
        .unwrap_or(LINEAGE_MAX_SQL_BYTES_DEFAULT)
}

/// Parse exactly one statement. Refuses text over `knob lineage.max_sql_bytes`, text that does
/// not parse, and text holding zero or several statements.
pub fn parse_one_statement(sql: &str, dialect: SqlDialect) -> Result<Statement, LineageError> {
    if sql.len() > lineage_max_sql_bytes() {
        return Err(LineageError::TooLarge(sql.len()));
    }
    let parsed = match dialect {
        SqlDialect::Postgres => Parser::parse_sql(&PostgreSqlDialect {}, sql),
        SqlDialect::MySql => Parser::parse_sql(&MySqlDialect {}, sql),
    };
    let mut statements = parsed.map_err(|e| LineageError::Parse(e.to_string()))?;
    if statements.len() != 1 {
        return Err(LineageError::Parse(format!(
            "expected one statement, found {}",
            statements.len()
        )));
    }
    Ok(statements.remove(0))
}

struct TableCollector {
    ctes: BTreeSet<String>,
    tables: Vec<String>,
}

fn dotted(name: &ObjectName) -> String {
    name.0
        .iter()
        .filter_map(|part| match part {
            ObjectNamePart::Identifier(ident) => Some(ident.value.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(".")
}

impl Visitor for TableCollector {
    type Break = ();

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        if let Some(with) = &query.with {
            for cte in &with.cte_tables {
                self.ctes.insert(cte.alias.name.value.to_lowercase());
            }
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_relation(&mut self, relation: &ObjectName) -> ControlFlow<()> {
        self.tables.push(dotted(relation));
        ControlFlow::Continue(())
    }
}

/// The distinct base tables a statement reads, schema-qualified as written, sorted. Names bound
/// by a `WITH` clause are not tables.
pub fn referenced_tables(sql: &str, dialect: SqlDialect) -> Result<Vec<String>, LineageError> {
    let statement = parse_one_statement(sql, dialect)?;
    let mut collector = TableCollector {
        ctes: BTreeSet::new(),
        tables: Vec::new(),
    };
    let _ = statement.visit(&mut collector);
    let ctes = collector.ctes;
    let tables: BTreeSet<String> = collector
        .tables
        .into_iter()
        .filter(|name| !ctes.contains(&name.to_lowercase()))
        .collect();
    Ok(tables.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R763.2
    #[test]
    fn referenced_tables_skip_ctes_and_dedupe() {
        let sql = "WITH recent AS (SELECT id FROM public.orders) \
                   SELECT r.id FROM recent r JOIN public.customers c ON c.id = r.id \
                   JOIN public.orders o ON o.id = r.id";
        let tables = referenced_tables(sql, SqlDialect::Postgres).unwrap();
        assert_eq!(tables, vec!["public.customers", "public.orders"]);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R763.2
    #[test]
    fn exactly_one_statement_is_required() {
        let two = parse_one_statement("SELECT 1; SELECT 2", SqlDialect::Postgres);
        assert!(matches!(two, Err(LineageError::Parse(_))));
        let none = parse_one_statement("", SqlDialect::MySql);
        assert!(matches!(none, Err(LineageError::Parse(_))));
        let junk = parse_one_statement("FROBNICATE nope", SqlDialect::MySql);
        assert!(matches!(junk, Err(LineageError::Parse(_))));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R763.2
    #[test]
    fn oversized_text_is_refused_before_parsing() {
        let big = format!("SELECT '{}'", "x".repeat(LINEAGE_MAX_SQL_BYTES_DEFAULT));
        assert!(matches!(
            parse_one_statement(&big, SqlDialect::Postgres),
            Err(LineageError::TooLarge(_))
        ));
    }
}
