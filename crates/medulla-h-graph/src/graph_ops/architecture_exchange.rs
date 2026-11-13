//! Served ArchiMate exchange (MEDULLA-CQ-ORCH-R705.6, R705.9).
//!
//! `ArchitectureModelImport` is gateway-routed (`Route::ArchitectureExchange`):
//! the document is parsed (`parse_exchange`) under the registry size and depth
//! knobs, mapped onto the caller's visible graph (`exchange_mutations`) and the
//! nodes and edges commit as the one `BatchUpdate` through the staged gateway.
//! Every imported node carries `archimateIdentifier` and `importedFrom`, the
//! SHA-256 of the document bytes. A refusal writes nothing.
//! `ArchitectureModelExport` is a read over the caller's projected graph; it
//! answers the document, the document digest and the canonical model digest.

use super::terminal::GraphOpsContext;
use super::*;
use medulla_core::architecture_exchange::graph::{exchange_mutations, ExchangeMutation};
use medulla_core::architecture_exchange::read::parse_exchange;
use medulla_core::architecture_exchange::write::{collect_exchange, exchange_bytes};
use medulla_core::architecture_exchange::{ArchimateTypeMap, ExchangeView, ExportScope};
use medulla_core::process_exchange::xml_tree::XmlLimits;
use medulla_protocol_types::result_contract::graph::{
    ArchitectureModelExport, ArchitectureModelImport,
};
use medulla_server_api::knob_config::knobs;
use medulla_types::archimate_exchange::{
    sha256_hex, ArchimateExchange, ArchitectureModelExported, ArchitectureModelImported,
    ARCHIMATE_EXCHANGE_SCHEMA_VERSION,
};
use medulla_types::contract::ServerErrorCode;

/// The ontology-derived type map, or the refusal that it is not compiled in.
#[cfg(feature = "rdf")]
fn type_map() -> Result<&'static ArchimateTypeMap, String> {
    Ok(medulla_rdf::archimate_types::archimate_type_map())
}

#[cfg(not(feature = "rdf"))]
fn type_map() -> Result<&'static ArchimateTypeMap, String> {
    Err("METHOD_NOT_YET_SERVED: the architecture exchange needs the rdf feature".to_string())
}

/// The parse limits, from the resolved registry knobs.
pub(super) fn limits() -> XmlLimits {
    let configured = knobs();
    XmlLimits {
        max_bytes: configured.process_document_max_bytes,
        max_depth: configured.process_document_max_depth as u32,
    }
}

/// Parse the document and check the caller's expected schema version. The
/// refusal is the exchange error's own wire code.
pub(super) fn parse_checked(
    document: &str,
    expected_version: Option<u32>,
) -> Result<ArchimateExchange, String> {
    let doc = parse_exchange(document.as_bytes(), limits()).map_err(|error| error.to_string())?;
    if let Some(expected) = expected_version {
        if expected != u32::from(doc.schema_version) {
            return Err(format!(
                "{}: the document carries version {}, the caller expected {expected}",
                ServerErrorCode::ArchimateVersionMismatch.as_str(),
                doc.schema_version
            ));
        }
    }
    Ok(doc)
}

/// The batch operations of one import: one `upsert_node` per node, one
/// `upsert_edge` per link, in the order the mapping gives them.
pub(super) fn batch_operations(
    mutations: &[ExchangeMutation],
    document_digest: &str,
) -> (Vec<serde_json::Value>, u64, u64) {
    let (mut nodes, mut edges) = (0u64, 0u64);
    let operations = mutations
        .iter()
        .map(|mutation| match mutation {
            ExchangeMutation::UpsertNode { id, properties, .. } => {
                nodes += 1;
                let mut properties = properties.clone();
                if properties.contains_key("importedFrom") {
                    properties.insert("importedFrom".to_owned(), document_digest.to_owned());
                }
                serde_json::json!({"op": "upsert_node", "id": id, "properties": properties})
            }
            ExchangeMutation::Link {
                source,
                target,
                predicate,
            } => {
                edges += 1;
                serde_json::json!({
                    "op": "upsert_edge",
                    "source": source,
                    "target": target,
                    "properties": {"relationship": predicate},
                })
            }
        })
        .collect();
    (operations, nodes, edges)
}

/// Admit one import: the batch write and the answer, or the typed refusal.
fn admit_import(
    core: &GraphCore,
    document: &str,
    expected_version: Option<u32>,
) -> Result<(Method, ArchitectureModelImported), String> {
    let doc = parse_checked(document, expected_version)?;
    let types = type_map()?;
    let view = ExchangeView { core, types };
    let mutations = exchange_mutations(&view, &doc);
    let document_digest = sha256_hex(document.as_bytes());
    let (operations, nodes, edges) = batch_operations(&mutations, &document_digest);
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|error| format!("INVALID_ARGUMENT: import encoding failed: {error}"))?;
    let imported = ArchitectureModelImported {
        document_digest,
        model_digest: doc.canonical_digest(),
        nodes,
        edges,
    };
    Ok((Method::BatchUpdate { operations_msgpack }, imported))
}

/// The `Route::ArchitectureExchange` entry for `ArchitectureModelImport`.
pub(super) async fn handle_import(
    ctx: &MutationCtx<'_>,
    tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    let Method::ArchitectureModelImport {
        document,
        expected_version,
    } = method
    else {
        return None;
    };
    let _ = tenant_id;
    let (write, imported) = match admit_import(ctx.core, document, *expected_version) {
        Ok(admitted) => admitted,
        Err(error) => return Some(Response::err(ctx.req_id, error)),
    };
    let Method::BatchUpdate { operations_msgpack } = &write else {
        unreachable!("admit_import builds a BatchUpdate");
    };
    let operations = operations_msgpack.clone();
    let plan = MutationPlan::for_method(&write);
    Some(
        medulla_server_ops::api::ingest::graph::commit_gateway(ctx, &plan, &write, move |core| {
            medulla_graph_algos::algorithms::batch_update(core, &operations)
                .and_then(|_| ResultPayload::of::<ArchitectureModelImport>(imported))
        })
        .await,
    )
}

/// The export of the visible architecture nodes, as the served answer.
pub(super) fn export_model(
    core: &GraphCore,
    tenant: &str,
    graph: Option<String>,
) -> Result<ArchitectureModelExported, String> {
    let types = type_map()?;
    let view = ExchangeView { core, types };
    let scope = ExportScope {
        tenant: tenant.to_owned(),
        graph,
    };
    let doc = collect_exchange(&view, &scope);
    debug_assert_eq!(doc.schema_version, ARCHIMATE_EXCHANGE_SCHEMA_VERSION);
    let bytes = exchange_bytes(&doc);
    let document_digest = sha256_hex(&bytes);
    let document = String::from_utf8(bytes)
        .map_err(|error| format!("INVALID_ARGUMENT: export is not text: {error}"))?;
    Ok(ArchitectureModelExported {
        document,
        document_digest,
        model_digest: doc.canonical_digest(),
    })
}

/// Route the export read served from the graph.
pub(super) async fn try_handle_architecture_export(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::ArchitectureModelExport { scope } = method else {
        return ControlFlow::Continue(method);
    };
    let tenant = ctx
        .read_authority
        .carrier()
        .map(|carrier| carrier.tenant_scope().to_owned())
        .unwrap_or_default();
    let response = match export_model(ctx.core, &tenant, scope) {
        Ok(exported) => Response::ok(
            ctx.req_id,
            ResultPayload::of::<ArchitectureModelExport>(exported),
        ),
        Err(error) => Response::err(ctx.req_id, error),
    };
    ControlFlow::Break(response)
}
