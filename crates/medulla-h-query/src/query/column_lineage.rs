//! `Method::ColumnLineage` (MEDULLA-UNIFIED-DATA-PLANE-R763.2, R763.3): derive the column edges
//! of one SQL query against a caller-supplied catalog (`DeriveFromSql`) or of every view of the
//! caller's own held catalog of an attached source (`DeriveFromCatalog`), and walk the stored
//! column edges (`Impact`).
//!
//! Nothing here commits: the edge write is the lineage drain (`handle_lineage_ingest`).
//! Unresolved or ambiguous columns return as typed gaps, never edges. `Impact` reads the
//! caller's RLS-projected graph, so a tenant sees only its own edges.

use super::*;
use medulla_catalog_types::lineage::{
    ColumnLineageEdge, ColumnLineageOp, ColumnTransform, ImpactDirection, LineageError,
    LineageResult, COLUMN_DERIVES_FROM, LINEAGE_IMPACT_MAX_DEPTH_DEFAULT,
};
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

/// Derive the column edges of one statement on the shared parser. The lineage drain reaches
/// this through the `QueryApi` seam.
#[cfg(feature = "federation-sql")]
pub fn derive_lineage(
    sql: &str,
    dialect: LineageDialect,
    catalog: &AttachedCatalogGraph,
    target: &str,
) -> Result<LineageResult, LineageError> {
    use medulla_exec::sql_text::SqlDialect;
    let dialect = match dialect {
        LineageDialect::Postgres => SqlDialect::Postgres,
        LineageDialect::MySql => SqlDialect::MySql,
    };
    medulla_federation::column_lineage::derive_column_lineage(sql, dialect, catalog, target)
}

/// The edges of every view of `catalog`, each view a target named `<schema>.<view>`. A view the
/// parser cannot read, or whose definition was not kept, is a gap on `<view>.*`.
#[cfg(feature = "federation-sql")]
fn derive_catalog(catalog: &AttachedCatalogGraph) -> LineageResult {
    let mut out = LineageResult::default();
    for view in &catalog.views {
        let target = if view.schema.is_empty() {
            view.name.clone()
        } else {
            format!("{}.{}", view.schema, view.name)
        };
        if view.definition_gap {
            out.gaps.push(medulla_catalog_types::lineage::LineageGap {
                output: format!("{target}.*"),
                reason: "view definition over lineage.max_sql_bytes was not kept".to_string(),
            });
            continue;
        }
        match derive_lineage(&view.definition_sql, view.dialect, catalog, &target) {
            Ok(result) => {
                out.edges.extend(result.edges);
                out.gaps.extend(result.gaps);
            }
            Err(error) => out.gaps.push(medulla_catalog_types::lineage::LineageGap {
                output: format!("{target}.*"),
                reason: error.to_string(),
            }),
        }
    }
    out.edges.sort_by(|a, b| {
        (&a.target_column, &a.source_column).cmp(&(&b.target_column, &b.source_column))
    });
    out.edges.dedup();
    out.gaps.sort();
    out.gaps.dedup();
    out
}

/// One stored column edge as the drain wrote it: `source` is the target column.
pub(crate) struct StoredColumnEdge {
    pub(crate) target_column: String,
    pub(crate) source_column: String,
    pub(crate) props: serde_json::Value,
}

fn edge_of(stored: &StoredColumnEdge) -> Option<ColumnLineageEdge> {
    let transform: ColumnTransform =
        serde_json::from_value(stored.props.get("transform")?.clone()).ok()?;
    Some(ColumnLineageEdge {
        source_column: stored.source_column.clone(),
        target_column: stored.target_column.clone(),
        transform,
        query_digest: stored.props.get("query_digest")?.as_str()?.to_string(),
        parser_version: stored.props.get("parser_version")?.as_str()?.to_string(),
    })
}

/// Walk the stored edges breadth first from `column`, at most `max_depth` hops, and refuse
/// `LINEAGE_IMPACT_TOO_LARGE` when more than `max_columns` columns are reached.
pub(crate) fn walk_impact(
    stored: &[StoredColumnEdge],
    column: &str,
    direction: ImpactDirection,
    max_depth: u32,
    max_columns: usize,
) -> Result<Vec<ColumnLineageEdge>, String> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut adjacent: BTreeMap<&str, Vec<&StoredColumnEdge>> = BTreeMap::new();
    for edge in stored {
        let from = match direction {
            ImpactDirection::Downstream => edge.source_column.as_str(),
            ImpactDirection::Upstream => edge.target_column.as_str(),
        };
        adjacent.entry(from).or_default().push(edge);
    }
    let mut seen: BTreeSet<&str> = BTreeSet::from([column]);
    let mut frontier = vec![column];
    let mut found: BTreeSet<ColumnLineageEdge> = BTreeSet::new();
    for _ in 0..max_depth {
        let mut next = Vec::new();
        for from in frontier {
            for edge in adjacent.get(from).into_iter().flatten() {
                let to = match direction {
                    ImpactDirection::Downstream => edge.target_column.as_str(),
                    ImpactDirection::Upstream => edge.source_column.as_str(),
                };
                if let Some(found_edge) = edge_of(edge) {
                    found.insert(found_edge);
                }
                if seen.insert(to) {
                    if seen.len() > max_columns {
                        return Err(format!(
                            "LINEAGE_IMPACT_TOO_LARGE: more than {max_columns} columns"
                        ));
                    }
                    next.push(to);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(found.into_iter().collect())
}

/// The stored column edges of the graph: `DERIVES_FROM` edges between `col:` nodes.
fn stored_column_edges(core: &medulla_graph_core::graph::GraphCore) -> Vec<StoredColumnEdge> {
    let strip = |id: &str| id.strip_prefix("col:").map(str::to_string);
    core.get_edges_arc()
        .into_iter()
        .filter_map(|(target, source, props)| {
            let props: serde_json::Value = rmp_serde::from_slice(&props).ok()?;
            (props.get("relationship")?.as_str()? == COLUMN_DERIVES_FROM).then_some(())?;
            Some(StoredColumnEdge {
                target_column: strip(&target)?,
                source_column: strip(&source)?,
                props,
            })
        })
        .collect()
}

fn serve_impact(
    ctx: &QueryHandlerCtx<'_>,
    column: String,
    direction: ImpactDirection,
    max_depth: Option<u32>,
) -> Response {
    let knobs = medulla_server_api::knob_config::knobs();
    let cap = knobs.lineage_impact_max_depth as u32;
    let depth = max_depth.unwrap_or(cap.min(LINEAGE_IMPACT_MAX_DEPTH_DEFAULT));
    if depth == 0 || depth > cap {
        return Response::err(
            ctx.req_id,
            format!("LINEAGE_DEPTH_INVALID: max_depth must be between 1 and {cap}"),
        );
    }
    let core = match ctx.read_authority {
        Some(authority) => authority.project_core(ctx.core),
        None => ctx.core.clone(),
    };
    let max_columns = knobs.lineage_impact_max_columns as usize;
    match walk_impact(
        &stored_column_edges(&core),
        &column,
        direction,
        depth,
        max_columns,
    ) {
        Ok(edges) => Response::ok(
            ctx.req_id,
            ResultPayload::of::<ColumnLineage>(LineageResult {
                edges,
                gaps: Vec::new(),
            }),
        ),
        Err(refusal) => Response::err(ctx.req_id, refusal),
    }
}

/// Serve one `ColumnLineage` operation.
#[cfg(feature = "federation-sql")]
pub(crate) async fn handle_column_lineage(
    ctx: &QueryHandlerCtx<'_>,
    op: Box<ColumnLineageOp>,
) -> Response {
    match *op {
        ColumnLineageOp::DeriveFromSql {
            sql,
            dialect,
            target,
            catalog,
        } => {
            let catalog = catalog_of(catalog);
            if let Err(duplicate) = catalog.validate() {
                return Response::err(
                    ctx.req_id,
                    format!("LINEAGE_CATALOG_AMBIGUOUS: {duplicate}"),
                );
            }
            match derive_lineage(&sql, dialect, &catalog, &target) {
                Ok(result) => Response::ok(ctx.req_id, ResultPayload::of::<ColumnLineage>(result)),
                Err(refusal) => Response::err(ctx.req_id, refusal.to_string()),
            }
        }
        ColumnLineageOp::DeriveFromCatalog { source } => {
            use medulla_server_ops::api::query_txn::ForeignStateExt;
            let Some(carrier) = ctx.read_authority.and_then(|a| a.carrier()) else {
                return Response::err(
                    ctx.req_id,
                    "ACCESS_DENIED: column lineage of a source needs a verified caller",
                );
            };
            let read = ctx
                .state
                .read()
                .await
                .foreign_sources()
                .catalog_read(carrier, &source);
            match read {
                Ok(catalog) => Response::ok(
                    ctx.req_id,
                    ResultPayload::of::<ColumnLineage>(derive_catalog(&catalog)),
                ),
                Err(refusal) if refusal.starts_with("SCHEMA_DRIFT") => {
                    Response::err(ctx.req_id, refusal)
                }
                Err(refusal) => {
                    Response::err(ctx.req_id, format!("LINEAGE_CATALOG_MISSING: {refusal}"))
                }
            }
        }
        ColumnLineageOp::Impact {
            column,
            direction,
            max_depth,
        } => serve_impact(ctx, column, direction, max_depth),
    }
}

/// Without `federation-sql` the parser is not built; the method says so.
#[cfg(not(feature = "federation-sql"))]
pub(crate) async fn handle_column_lineage(
    ctx: &QueryHandlerCtx<'_>,
    op: Box<ColumnLineageOp>,
) -> Response {
    if let ColumnLineageOp::Impact {
        column,
        direction,
        max_depth,
    } = *op
    {
        return serve_impact(ctx, column, direction, max_depth);
    }
    Response::err(
        ctx.req_id,
        "ENGINE_UNAVAILABLE: column lineage needs the federation-sql build",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(source: &str, target: &str) -> StoredColumnEdge {
        StoredColumnEdge {
            target_column: target.into(),
            source_column: source.into(),
            props: serde_json::json!({
                "relationship": COLUMN_DERIVES_FROM,
                "transform": "identity",
                "query_digest": "d".repeat(64),
                "parser_version": "sqlparser-0.62",
            }),
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R763.3
    #[test]
    fn impact_walks_both_ways_within_the_depth_and_column_bounds() {
        let edges = [stored("a", "b"), stored("b", "c"), stored("c", "d")];
        let down = |depth| walk_impact(&edges, "a", ImpactDirection::Downstream, depth, 100);
        assert_eq!(down(1).unwrap().len(), 1);
        assert_eq!(down(2).unwrap().len(), 2);
        assert_eq!(down(8).unwrap().len(), 3);
        let up = walk_impact(&edges, "d", ImpactDirection::Upstream, 8, 100).unwrap();
        assert_eq!(up.len(), 3);
        assert_eq!(up[0].parser_version, "sqlparser-0.62");
        assert!(walk_impact(&edges, "a", ImpactDirection::Downstream, 8, 2)
            .unwrap_err()
            .starts_with("LINEAGE_IMPACT_TOO_LARGE"));
        assert!(
            walk_impact(&edges, "zzz", ImpactDirection::Downstream, 8, 2)
                .unwrap()
                .is_empty()
        );
    }
}
