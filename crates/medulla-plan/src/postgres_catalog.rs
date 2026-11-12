//! Live PostgreSQL catalog read (MEDULLA-UNIFIED-DATA-PLANE-R013.1).
//!
//! [`read_catalog`] reads the catalog of a registered source through
//! `medulla_federation::pg_introspect` and assembles the typed, hashed
//! [`AttachedCatalogGraph`] of MEDULLA-UNIFIED-DATA-PLANE-R003.1, views included
//! (MEDULLA-UNIFIED-DATA-PLANE-R763.7). It runs in the probe job of R014.2.3, never in the
//! registration write.

use std::collections::BTreeMap;

use medulla_catalog_types::attached_catalog::{
    AttachedCatalogGraph, CatalogColumn, CatalogConstraint, CatalogConstraintKind, CatalogTable,
    CatalogView,
};
use medulla_catalog_types::lineage::LineageDialect;
use medulla_federation::pg_introspect::{read_pg_catalog, PgCatalogRows};

use crate::mysql_catalog::{refuse, CatalogRefusal, CATALOG_VERSION, SOURCE_UNAVAILABLE};

/// Assemble the typed catalog from one catalog read. Pure. A view's definition over
/// `max_sql_bytes` is kept as a gap ([`CatalogView::bounded`]).
pub fn build_catalog(
    rows: &PgCatalogRows,
    max_sql_bytes: usize,
) -> Result<AttachedCatalogGraph, CatalogRefusal> {
    let mut tables: BTreeMap<(String, String), CatalogTable> = rows
        .tables
        .iter()
        .map(|(schema, name)| {
            (
                (schema.clone(), name.clone()),
                CatalogTable {
                    schema: schema.clone(),
                    name: name.clone(),
                    columns: Vec::new(),
                    constraints: Vec::new(),
                },
            )
        })
        .collect();
    for column in &rows.columns {
        // A view's column belongs to its `CatalogView`, not to a table.
        if let Some(table) = tables.get_mut(&(column.schema.clone(), column.table.clone())) {
            table.columns.push(CatalogColumn {
                name: column.name.clone(),
                type_name: column.udt.clone(),
                nullable: column.nullable,
            });
        }
    }
    let mut keyed: BTreeMap<(String, String, String), CatalogConstraint> = BTreeMap::new();
    for key in &rows.keys {
        keyed
            .entry((
                key.schema.clone(),
                key.table.clone(),
                key.constraint.clone(),
            ))
            .or_insert_with(|| CatalogConstraint {
                name: key.constraint.clone(),
                kind: if key.primary {
                    CatalogConstraintKind::PrimaryKey
                } else {
                    CatalogConstraintKind::Unique
                },
                columns: Vec::new(),
                references: None,
            })
            .columns
            .push(key.column.clone());
    }
    for ((schema, table, _), constraint) in keyed {
        if let Some(table) = tables.get_mut(&(schema, table)) {
            table.constraints.push(constraint);
        }
    }
    let views = rows
        .views
        .iter()
        .map(|view| {
            CatalogView::bounded(
                &view.schema,
                &view.name,
                LineageDialect::Postgres,
                &view.definition,
                max_sql_bytes,
            )
        })
        .collect();
    let graph = AttachedCatalogGraph {
        tables: tables.into_values().collect(),
        views,
        version: CATALOG_VERSION,
    };
    graph
        .validate()
        .map_err(|duplicate| refuse(SOURCE_UNAVAILABLE, duplicate.to_string()))?;
    Ok(graph)
}

/// Read the live catalog of the PostgreSQL source at `dsn`, after the shared outbound gate.
/// `schemas` is the allowed scope (empty reads every non-system schema); `pool_size` is
/// `foreign.mysql.pool_size`'s sibling bound, clamped to 1..=64.
pub async fn read_catalog(
    dsn: &str,
    schemas: &[String],
    pool_size: u32,
) -> Result<AttachedCatalogGraph, CatalogRefusal> {
    medulla_federation::federation_ssrf::check_sql_dsn(dsn)
        .map_err(|e| refuse(SOURCE_UNAVAILABLE, e))?;
    let rows = read_pg_catalog(dsn, schemas, pool_size.clamp(1, 64))
        .await
        .map_err(|e| refuse(SOURCE_UNAVAILABLE, e))?;
    build_catalog(
        &rows,
        medulla_federation::sql_analysis::lineage_max_sql_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_federation::pg_introspect::{PgColumn, PgKeyColumn, PgView};

    fn rows() -> PgCatalogRows {
        let column = |table: &str, name: &str| PgColumn {
            schema: "public".into(),
            table: table.into(),
            name: name.into(),
            udt: "int4".into(),
            nullable: false,
        };
        PgCatalogRows {
            version: "16.2".into(),
            tables: vec![("public".into(), "orders".into())],
            columns: vec![
                column("orders", "id"),
                column("orders", "amount"),
                column("big_orders", "id"),
            ],
            keys: vec![PgKeyColumn {
                schema: "public".into(),
                table: "orders".into(),
                constraint: "orders_pkey".into(),
                primary: true,
                column: "id".into(),
            }],
            views: vec![PgView {
                schema: "public".into(),
                name: "big_orders".into(),
                definition: " SELECT orders.id FROM orders WHERE orders.amount > 100;".into(),
            }],
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R013.1, MEDULLA-UNIFIED-DATA-PLANE-R763.7
    #[test]
    fn the_catalog_holds_tables_keys_and_view_definitions() {
        let graph = build_catalog(&rows(), 1_048_576).unwrap();
        assert_eq!(graph.tables.len(), 1);
        assert_eq!(
            graph.tables[0].columns.len(),
            2,
            "a view column is not a table column"
        );
        assert_eq!(
            graph.tables[0].constraints[0].kind,
            CatalogConstraintKind::PrimaryKey
        );
        assert_eq!(graph.views.len(), 1);
        assert_eq!(graph.views[0].dialect, LineageDialect::Postgres);
        assert!(graph.views[0].definition_sql.contains("orders.amount"));
        let gapped = build_catalog(&rows(), 10).unwrap();
        assert!(gapped.views[0].definition_gap);
        assert!(gapped.views[0].definition_sql.is_empty());
    }
}
