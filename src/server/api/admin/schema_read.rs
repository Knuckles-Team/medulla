//! Read side of GraphSchema: the disclosed schema sources of a graph and their list view.

use std::sync::Arc;

use crate::graph::GraphCore;
use crate::protocol::{Response, ResultPayload};

/// The schema-source set a schema read may disclose: the REQUEST graph's own
/// keyed sources, and nothing else. `GraphSchemaList` and `GraphSchemaClasses`
/// both read through this one accessor, so the vocabulary read can never show a
/// source key the list would not (EH-389 visibility ruling, 2026-09-24).
pub fn disclosed_sources(core: &GraphCore) -> Arc<crate::graph::GraphSchemaSources> {
    core.schema_sources()
}

/// List the request graph's schema sources and its composed digest.
pub async fn handle_list(req_id: u64, graph_name: &str, core: &Arc<GraphCore>) -> Response {
    list_response(req_id, list(graph_name, core))
}

fn list_response(
    req_id: u64,
    result: Result<medulla_catalog_types::graph_schema::GraphSchemaSourcesView, String>,
) -> Response {
    match result {
        Ok(body) => Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::reasoning::GraphSchemaList>(
                body,
            ),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// The sources view of `core`'s schema for `graph_name`.
pub fn list(
    graph_name: &str,
    core: &GraphCore,
) -> Result<medulla_catalog_types::graph_schema::GraphSchemaSourcesView, String> {
    use medulla_catalog_types::{
        graph_schema::GraphSchemaSourceView, graph_schema::GraphSchemaSourcesView,
        graph_schema::GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
    };
    let sources = disclosed_sources(core);
    let project =
        |source_id: &str, source: &crate::graph::GraphSchemaSource| -> GraphSchemaSourceView {
            GraphSchemaSourceView {
                source_id: source_id.to_string(),
                origin: origin_view(&source.origin),
                shapes_sha256: source.shapes_sha256.map(|digest| digest.to_hex()),
                ontology_sha256: source.ontology_sha256.map(|digest| digest.to_hex()),
                shapes_bytes: source.shapes_ttl.as_deref().map_or(0, str::len) as u64,
                ontology_bytes: source.ontology_ttl.as_deref().map_or(0, str::len) as u64,
                attached_at_ms: source.attached_at_ms,
            }
        };
    let core_sources = sources
        .core
        .iter()
        .map(|(source_id, source)| project(source_id, source))
        .collect();
    let dynamic_sources = sources
        .dynamic
        .iter()
        .map(|(source_id, source)| project(source_id, source))
        .collect();
    Ok(GraphSchemaSourcesView {
        schema_version: GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
        graph: graph_name.to_string(),
        core_catalog_digest: crate::graph::current_core_set_digest().to_hex(),
        composed_digest: sources.composed_digest().to_hex(),
        core_sources: medulla_types::contract::BoundedVec::new(core_sources)?,
        dynamic_sources: medulla_types::contract::BoundedVec::new(dynamic_sources)?,
    })
}

/// The wire view of one source's authority origin.
fn origin_view(
    origin: &crate::graph::SchemaSourceOrigin,
) -> medulla_catalog_types::graph_schema::SchemaSourceOriginView {
    use crate::graph::SchemaSourceOrigin as Origin;
    use medulla_catalog_types::graph_schema::SchemaSourceOriginView as View;
    match origin {
        Origin::Core {
            module,
            version,
            set_digest,
        } => View::Core {
            module: module.clone(),
            version: *version,
            set_digest: set_digest.to_hex(),
        },
        Origin::Operator => View::Operator,
        Origin::Admin { name } => View::Admin { name: name.clone() },
        Origin::Pack {
            connector,
            record_id,
        } => View::Pack {
            connector: connector.clone(),
            record_id: record_id.clone(),
        },
        Origin::Ingestion { mapping, revision } => View::Ingestion {
            mapping: mapping.clone(),
            revision: *revision,
        },
        Origin::Approved {
            name,
            approval_lease_id,
        } => View::Approved {
            name: name.clone(),
            approval_lease_id: approval_lease_id.clone(),
        },
    }
}
