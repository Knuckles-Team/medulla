//! Read side of GraphSchema: the disclosed schema sources of a graph and their list view.

use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;

/// The schema-source set a schema read may disclose: the REQUEST graph's own
/// keyed sources, and nothing else. `GraphSchemaList` and `GraphSchemaClasses`
/// both read through this one accessor, so the vocabulary read can never show a
/// source key the list would not (EH-389 visibility ruling, 2026-09-24).
pub fn disclosed_sources(core: &GraphCore) -> Arc<medulla_graph_core::graph::GraphSchemaSources> {
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
        graph_schema::GraphSchemaSourcesView, graph_schema::GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
    };
    let sources = disclosed_sources(core);
    let project = project_source;
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
        core_catalog_digest: medulla_graph_core::graph::current_core_set_digest().to_hex(),
        composed_digest: sources.composed_digest().to_hex(),
        core_sources: medulla_types::contract::BoundedVec::new(core_sources)?,
        dynamic_sources: medulla_types::contract::BoundedVec::new(dynamic_sources)?,
    })
}

/// The projection of one attached source: its key, origin, digests and document sizes.
fn project_source(
    source_id: &str,
    source: &medulla_graph_core::graph::GraphSchemaSource,
) -> medulla_catalog_types::graph_schema::GraphSchemaSourceView {
    medulla_catalog_types::graph_schema::GraphSchemaSourceView {
        source_id: source_id.to_string(),
        origin: origin_view(&source.origin),
        shapes_sha256: source.shapes_sha256.map(|digest| digest.to_hex()),
        ontology_sha256: source.ontology_sha256.map(|digest| digest.to_hex()),
        shapes_bytes: source.shapes_ttl.as_deref().map_or(0, str::len) as u64,
        ontology_bytes: source.ontology_ttl.as_deref().map_or(0, str::len) as u64,
        attached_at_ms: source.attached_at_ms,
    }
}

/// One schema source of the request graph with its stored documents
/// (MEDULLA-TYPED-PACKS-R813). The source is looked up in the same disclosed set the list
/// reads, so a key the list would not show refuses `SCHEMA_SOURCE_NOT_FOUND`.
pub async fn handle_get(
    req_id: u64,
    graph_name: &str,
    core: &Arc<GraphCore>,
    source_id: &str,
) -> Response {
    match get(graph_name, core, source_id) {
        Ok(body) => Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::reasoning::GraphSchemaGet>(
                body,
            ),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// The documents view of `source_id` in `core`'s schema for `graph_name`.
pub fn get(
    graph_name: &str,
    core: &GraphCore,
    source_id: &str,
) -> Result<medulla_catalog_types::graph_schema::GraphSchemaSourceDocumentsView, String> {
    use medulla_catalog_types::graph_schema::{
        GraphSchemaErrorCode, GraphSchemaSourceDocumentsView, GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
    };
    let sources = disclosed_sources(core);
    let source = sources
        .core
        .get(source_id)
        .or_else(|| sources.dynamic.get(source_id))
        .ok_or_else(|| {
            format!(
                "{}: the graph carries no schema source '{source_id}'",
                GraphSchemaErrorCode::SourceNotFound.as_str()
            )
        })?;
    Ok(GraphSchemaSourceDocumentsView {
        schema_version: GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
        graph: graph_name.to_string(),
        composed_digest: sources.composed_digest().to_hex(),
        source: project_source(source_id, source),
        shapes_ttl: source.shapes_ttl.as_deref().map(str::to_string),
        ontology_ttl: source.ontology_ttl.as_deref().map(str::to_string),
    })
}

/// The wire view of one source's authority origin.
fn origin_view(
    origin: &medulla_graph_core::graph::SchemaSourceOrigin,
) -> medulla_catalog_types::graph_schema::SchemaSourceOriginView {
    use medulla_catalog_types::graph_schema::SchemaSourceOriginView as View;
    use medulla_graph_core::graph::SchemaSourceOrigin as Origin;
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
            ..
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
            shadow_report_digest,
        } => View::Approved {
            name: name.clone(),
            approval_lease_id: approval_lease_id.clone(),
            shadow_report_digest: shadow_report_digest.clone(),
        },
        Origin::Ontology {
            connector,
            ontology_iri,
            revision,
            importer,
            history,
        } => View::Ontology {
            connector: connector.clone(),
            ontology_iri: ontology_iri.clone(),
            revision: *revision,
            importer: importer.clone(),
            history: history.clone(),
        },
    }
}

#[cfg(test)]
mod get_tests {
    use super::*;
    use medulla_graph_core::graph::{GraphSchemaSource, GraphSchemaSources, SchemaSourceOrigin};
    use std::collections::BTreeMap;

    fn core_with_admin_source() -> Arc<GraphCore> {
        let core = Arc::new(GraphCore::new());
        let source = GraphSchemaSource::new(
            SchemaSourceOrigin::Admin {
                name: "alpha".to_string(),
            },
            None,
            Some(Arc::from(
                "<https://example.org/a#C> a <http://www.w3.org/2002/07/owl#Class> .\n",
            )),
            7,
        )
        .expect("a bounded admin source");
        let dynamic = BTreeMap::from([("admin:alpha".to_string(), source)]);
        core.install_schema_sources(Arc::new(
            GraphSchemaSources::with_dynamic(dynamic).expect("composes with the core"),
        ));
        core
    }

    // spec: MEDULLA-TYPED-PACKS-R813
    #[test]
    fn get_answers_the_stored_documents_and_the_list_projection_of_one_source() {
        let core = core_with_admin_source();
        let found = get("g", &core, "admin:alpha").expect("the source is attached");
        assert_eq!(found.graph, "g");
        assert_eq!(found.source.attached_at_ms, 7);
        assert!(found.ontology_ttl.as_deref().unwrap().contains("owl#Class"));
        let listed = list("g", &core).expect("lists");
        assert_eq!(found.composed_digest, listed.composed_digest);
        assert!(listed.dynamic_sources.as_slice().contains(&found.source));
    }

    // spec: MEDULLA-TYPED-PACKS-R813
    #[test]
    fn get_refuses_an_unattached_key_by_code() {
        let core = core_with_admin_source();
        let error = get("g", &core, "admin:missing").unwrap_err();
        assert!(error.starts_with("SCHEMA_SOURCE_NOT_FOUND"), "{error}");
    }
}
