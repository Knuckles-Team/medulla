//! `Method::SchemaContext` (MEDULLA-UNIFIED-DATA-PLANE-R029.2): what one table of an attached
//! source is, what joins to it, which ontology class and application it belongs to, who owns
//! it and what a change to it reaches.
//!
//! The answer is built only from state the engine already holds for the verified caller: the
//! baselined catalog of a source the caller registered (columns and joins), the caller's
//! registered graph views (the ontology class, counting only a view that reads exactly this
//! table), the registering agent (the owner) and the stored column edges (the impact, through
//! the same walk and `lineage.impact_*` knobs as `ColumnLineage` Impact). The lookup and its
//! refusal of an unknown table are `serve_schema_context`; no default answer is ever returned.

use super::*;
use medulla_catalog_types::schema_context::{
    qualified_name, serve_schema_context, SchemaContextCatalog, SchemaContextIntent,
    SchemaContextRequest, SCHEMA_CONTEXT_CLASS_AMBIGUOUS, SCHEMA_CONTEXT_SOURCE_UNKNOWN,
    SCHEMA_CONTEXT_TABLE_UNKNOWN, SCHEMA_CONTEXT_UNMAPPED,
};
use medulla_protocol_types::result_contract::query::SchemaContext;
use medulla_server_ops::api::query_txn::foreign_catalog::RegisteredGraphView;
use medulla_server_ops::api::query_txn::ForeignStateExt;

/// What the caller's registered graph views say about one table's ontology class.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ClassLookup {
    /// No view reads exactly this table.
    Unmapped,
    /// Exactly one class.
    One(String),
    /// More than one class maps this table.
    Ambiguous,
}

/// The ontology class of `source`'s table `schema.object` from `views`: only a view whose one
/// source is this table counts, because a view over several tables cannot say which label
/// belongs to which table.
pub(crate) fn class_of(
    views: &[RegisteredGraphView],
    source: &str,
    schema: &str,
    object: &str,
) -> ClassLookup {
    let qualified = qualified_name(schema, object);
    let mut classes = std::collections::BTreeSet::new();
    for registered in views {
        let reads_this_table = matches!(
            registered.sources.as_slice(),
            [only] if only.name == source && (only.table == object || only.table == qualified)
        );
        if !reads_this_table {
            continue;
        }
        for label in &registered.view.node_labels {
            classes.insert(label.ontology_class.clone());
        }
    }
    let mut classes = classes.into_iter();
    match (classes.next(), classes.next()) {
        (None, _) => ClassLookup::Unmapped,
        (Some(class), None) => ClassLookup::One(class),
        (Some(_), Some(_)) => ClassLookup::Ambiguous,
    }
}

/// Serve one `SchemaContext` call for the verified caller.
pub(crate) async fn handle_schema_context(
    ctx: &QueryHandlerCtx<'_>,
    source_id: String,
    schema: String,
    object: String,
    intent: SchemaContextIntent,
) -> Response {
    let Some(carrier) = ctx.read_authority.and_then(|authority| authority.carrier()) else {
        return Response::err(
            ctx.req_id,
            "ACCESS_DENIED: schema context of a source needs a verified caller",
        );
    };
    let (graph, views) = {
        let state = ctx.state.read().await;
        let sources = state.foreign_sources();
        match sources.catalog_read(carrier, &source_id) {
            Ok(graph) => (graph, sources.views().registered_for(carrier)),
            Err(refusal) if refusal.contains("unknown foreign source") => {
                return Response::err(
                    ctx.req_id,
                    format!(
                        "{SCHEMA_CONTEXT_SOURCE_UNKNOWN}: no source '{source_id}' is registered"
                    ),
                )
            }
            Err(refusal) => return Response::err(ctx.req_id, refusal),
        }
    };
    // The owner is the agent that registered the source: only the owner's own sources resolve.
    let catalog = SchemaContextCatalog::from_attached(&graph, &source_id, carrier.agent_id());
    let table = qualified_name(&schema, &object);
    let mut answer = match serve_schema_context(&catalog, &SchemaContextRequest { table }) {
        Ok(answer) => answer,
        Err(error) => {
            return Response::err(
                ctx.req_id,
                format!("{SCHEMA_CONTEXT_TABLE_UNKNOWN}: {error}"),
            )
        }
    };
    match (class_of(&views, &source_id, &schema, &object), intent) {
        (ClassLookup::One(class), _) => answer.ontology_class = Some(class),
        (ClassLookup::Unmapped, SchemaContextIntent::Ontology) => {
            return Response::err(
                ctx.req_id,
                format!(
                    "{SCHEMA_CONTEXT_UNMAPPED}: no registered graph view maps '{}' to a class",
                    answer.table
                ),
            )
        }
        (ClassLookup::Ambiguous, SchemaContextIntent::Ontology) => {
            return Response::err(
                ctx.req_id,
                format!(
                    "{SCHEMA_CONTEXT_CLASS_AMBIGUOUS}: more than one class maps '{}'",
                    answer.table
                ),
            )
        }
        _ => {}
    }
    let columns: Vec<String> = answer
        .columns
        .iter()
        .map(|column| format!("{}.{}", answer.table, column.name))
        .collect();
    match super::column_lineage::downstream_of(ctx, &columns) {
        Ok(reached) if reached.is_empty() => {
            answer.change_impact = "no downstream column reads this table".to_string();
        }
        Ok(reached) => {
            answer.change_impact = format!(
                "{} downstream columns: {}",
                reached.len(),
                reached.join(", ")
            );
        }
        Err(refusal) => return Response::err(ctx.req_id, refusal),
    }
    Response::ok(ctx.req_id, ResultPayload::of::<SchemaContext>(answer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use {
        medulla_catalog_source_types::virtual_graph_view::GraphView,
        medulla_catalog_source_types::virtual_graph_view::NodeLabelMapping,
        medulla_catalog_source_types::virtual_graph_view::ViewSource,
    };

    fn view(source: &str, table: &str, classes: &[&str]) -> RegisteredGraphView {
        RegisteredGraphView {
            view: GraphView {
                mapping_digest: String::new(),
                node_labels: classes
                    .iter()
                    .map(|class| NodeLabelMapping {
                        label: "L".to_string(),
                        ontology_class: (*class).to_string(),
                        mapping_id: "m".to_string(),
                    })
                    .collect(),
                relationship_types: Vec::new(),
            },
            mapping: String::new(),
            tables: vec![table.to_string()],
            sources: vec![ViewSource {
                name: source.to_string(),
                dsn: String::new(),
                table: table.to_string(),
            }],
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R029.2
    #[test]
    fn only_a_view_reading_exactly_this_table_names_its_class() {
        let views = vec![view("gramps", "person", &["gramps:Person"])];
        assert_eq!(
            class_of(&views, "gramps", "main", "person"),
            ClassLookup::One("gramps:Person".to_string())
        );
        assert_eq!(
            class_of(&views, "gramps", "main", "family"),
            ClassLookup::Unmapped
        );
        assert_eq!(
            class_of(&views, "other", "main", "person"),
            ClassLookup::Unmapped
        );
        let mut two_tables = view("gramps", "person", &["gramps:Person"]);
        two_tables.sources.push(ViewSource {
            name: "gramps".to_string(),
            dsn: String::new(),
            table: "family".to_string(),
        });
        assert_eq!(
            class_of(&[two_tables], "gramps", "main", "person"),
            ClassLookup::Unmapped
        );
        let ambiguous = vec![
            view("gramps", "person", &["gramps:Person"]),
            view("gramps", "person", &["foaf:Person"]),
        ];
        assert_eq!(
            class_of(&ambiguous, "gramps", "main", "person"),
            ClassLookup::Ambiguous
        );
    }
}
