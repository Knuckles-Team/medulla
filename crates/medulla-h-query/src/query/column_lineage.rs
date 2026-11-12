//! `Method::ColumnLineage` (MEDULLA-UNIFIED-DATA-PLANE-R763.2): derive the column edges of one
//! SQL query against a caller-supplied catalog with the shared parser.
//!
//! The statement text and the catalog both come from the request, so no graph row is read and
//! nothing is committed. Unresolved or ambiguous columns return as typed gaps, never edges.

use super::*;
use medulla_catalog_types::lineage::ColumnLineageOp;
#[cfg(feature = "federation-sql")]
use medulla_catalog_types::{
    attached_catalog::{AttachedCatalogGraph, CatalogColumn, CatalogTable},
    lineage::{LineageCatalogTable, LineageDialect},
};
use medulla_protocol_types::result_contract::query::ColumnLineage;

#[cfg(feature = "federation-sql")]
fn catalog_of(tables: Vec<LineageCatalogTable>) -> AttachedCatalogGraph {
    AttachedCatalogGraph {
        tables: tables
            .into_iter()
            .map(|table| CatalogTable {
                schema: table.schema,
                name: table.name,
                columns: table
                    .columns
                    .into_iter()
                    .map(|column| CatalogColumn {
                        name: column.name,
                        type_name: String::new(),
                        nullable: true,
                    })
                    .collect(),
                constraints: Vec::new(),
            })
            .collect(),
        views: Vec::new(),
        version: 1,
    }
}

/// Serve one `ColumnLineage` operation.
#[cfg(feature = "federation-sql")]
pub(crate) fn handle_column_lineage(
    ctx: &QueryHandlerCtx<'_>,
    op: Box<ColumnLineageOp>,
) -> Response {
    use medulla_exec::sql_text::SqlDialect;
    let ColumnLineageOp::DeriveFromSql {
        sql,
        dialect,
        target,
        catalog,
    } = *op;
    let dialect = match dialect {
        LineageDialect::Postgres => SqlDialect::Postgres,
        LineageDialect::MySql => SqlDialect::MySql,
    };
    let catalog = catalog_of(catalog);
    if let Err(duplicate) = catalog.validate() {
        return Response::err(
            ctx.req_id,
            format!("LINEAGE_CATALOG_AMBIGUOUS: {duplicate}"),
        );
    }
    match medulla_federation::column_lineage::derive_column_lineage(
        &sql, dialect, &catalog, &target,
    ) {
        Ok(result) => Response::ok(ctx.req_id, ResultPayload::of::<ColumnLineage>(result)),
        Err(refusal) => Response::err(ctx.req_id, refusal.to_string()),
    }
}

/// Without `federation-sql` the parser is not built; the method says so.
#[cfg(not(feature = "federation-sql"))]
pub(crate) fn handle_column_lineage(
    ctx: &QueryHandlerCtx<'_>,
    _op: Box<ColumnLineageOp>,
) -> Response {
    Response::err(
        ctx.req_id,
        "ENGINE_UNAVAILABLE: column lineage needs the federation-sql build",
    )
}
