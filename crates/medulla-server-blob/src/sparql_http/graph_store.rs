//! Graph Store HTTP routing and RDF conversion helpers.

use std::collections::HashMap;
use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_server_api::http1::HttpMessage;
use medulla_server_api::state::ServerState;

use super::{
    choose_ct, parse_form, percent_decode, serialize_graph, signed_request, DEFAULT_GRAPH_ENV,
    GRAPH_FORMS,
};
use medulla_server_api::api::dispatch_support::SPARQL_HTTP_UPDATE_EVENT;

/// W3C SPARQL 1.1 Graph Store HTTP Protocol: route reads and signed writes for one graph.
pub(super) async fn handle_graph_store(
    state: &Arc<tokio::sync::RwLock<ServerState>>,
    req: &HttpMessage,
    path: &str,
    query_string: &str,
) -> (&'static str, &'static str, String) {
    let params = parse_form(query_string);
    let Some(graph) = gsp_target(path, &params) else {
        return (
            "400 Bad Request",
            "text/plain",
            "graph store protocol: name the graph via /rdf-graphs/service?graph=<iri> \
             (or ?default) or /rdf-graphs/<name>"
                .to_string(),
        );
    };

    match req.method.as_str() {
        "GET" | "HEAD" => graph_read(state, req, &params, graph).await,
        "PUT" | "POST" => graph_write(state, req, graph).await,
        "DELETE" => graph_delete(state, req, graph).await,
        _ => (
            "405 Method Not Allowed",
            "text/plain",
            "graph store protocol: use GET/PUT/POST/DELETE/HEAD".to_string(),
        ),
    }
}

async fn graph_read(
    state: &Arc<tokio::sync::RwLock<ServerState>>,
    req: &HttpMessage,
    params: &HashMap<String, String>,
    graph: String,
) -> (&'static str, &'static str, String) {
    // Unlike writes below, this leg has no verified request-carrier mechanism yet.
    if medulla_server_api::carrier::unauthenticated_carrier_denied(None) {
        medulla_kernel::metrics::process().access_denied();
        return (
            "403 Forbidden",
            "text/plain",
            "ACCESS_DENIED: Graph Store HTTP GET/HEAD has no verified \
             request-carrier mechanism yet"
                .to_string(),
        );
    }
    let core = {
        let s = state.read().await;
        s.registry.get(&graph).map(|e| e.core.clone())
    };
    let Some(core) = core else {
        return (
            "404 Not Found",
            "text/plain",
            format!("no such graph: {graph}"),
        );
    };
    let ct = choose_ct(
        req.header("accept"),
        params
            .get("output")
            .or_else(|| params.get("format"))
            .map(|s| s.as_str()),
        GRAPH_FORMS,
    );
    let head_only = req.method == "HEAD";
    let out = tokio::task::spawn_blocking(move || {
        let triples = export_graph(&core, &graph)?;
        serialize_graph(ct, &triples)
    })
    .await;
    match out {
        Ok(Ok(body)) => ("200 OK", ct, if head_only { String::new() } else { body }),
        Ok(Err(e)) => ("500 Internal Server Error", "text/plain", e),
        Err(e) => (
            "500 Internal Server Error",
            "text/plain",
            format!("compute task failed: {e}"),
        ),
    }
}

async fn graph_write(
    state: &Arc<tokio::sync::RwLock<ServerState>>,
    req: &HttpMessage,
    graph: String,
) -> (&'static str, &'static str, String) {
    // PUT replaces the graph and POST merges into it; both build their update text with
    // the one function the served `RDF_GRAPH_WRITE_EVENT` uses (MEDULLA-FEDERATED-QUERY-R740.3).
    let mode = if req.method == "PUT" {
        RdfWriteMode::Replace
    } else {
        RdfWriteMode::Merge
    };
    let query = match rdf_graph_write_update(req.header("content-type"), &req.text(), &graph, mode)
    {
        Ok(query) => query,
        Err(refusal) => return (refusal.http_status(), "text/plain", refusal.message()),
    };
    let created = !state.read().await.registry.exists(&graph);
    let method = medulla_protocol_types::protocol::Method::ApplyMutation {
        event_type: SPARQL_HTTP_UPDATE_EVENT.to_string(),
        query,
    };
    let request = match signed_request(req, graph, method) {
        Ok(request) => request,
        Err(error) => return ("401 Unauthorized", "text/plain", error),
    };
    let response = medulla_server_api::api::reentry::dispatch(state, request).await;
    mutation_response(response.error, created)
}

async fn graph_delete(
    state: &Arc<tokio::sync::RwLock<ServerState>>,
    req: &HttpMessage,
    graph: String,
) -> (&'static str, &'static str, String) {
    if !state.read().await.registry.exists(&graph) {
        return (
            "404 Not Found",
            "text/plain",
            format!("no such graph: {graph}"),
        );
    }
    let method = medulla_protocol_types::protocol::Method::ApplyMutation {
        event_type: SPARQL_HTTP_UPDATE_EVENT.to_string(),
        query: "CLEAR DEFAULT".to_string(),
    };
    let request = match signed_request(req, graph, method) {
        Ok(request) => request,
        Err(error) => return ("401 Unauthorized", "text/plain", error),
    };
    let response = medulla_server_api::api::reentry::dispatch(state, request).await;
    mutation_response(response.error, false)
}

fn mutation_response(error: Option<String>, created: bool) -> (&'static str, &'static str, String) {
    match error {
        None if created => ("201 Created", "text/plain", String::new()),
        None => ("204 No Content", "text/plain", String::new()),
        Some(error) if error.starts_with("ACCESS_DENIED") => ("403 Forbidden", "text/plain", error),
        Some(error) => ("400 Bad Request", "text/plain", error),
    }
}

/// Resolve the Graph Store target from indirect query or direct path naming.
pub(super) fn gsp_target(path: &str, params: &HashMap<String, String>) -> Option<String> {
    if path == "/rdf-graphs/service" || path == "/rdf-graphs/service/" {
        if params.contains_key("default") {
            return Some(gsp_default_graph());
        }
        return params.get("graph").filter(|g| !g.is_empty()).cloned();
    }
    let name = path.strip_prefix("/rdf-graphs/")?;
    if name.is_empty() {
        return None;
    }
    Some(percent_decode(name))
}

pub(super) fn gsp_default_graph() -> String {
    std::env::var(DEFAULT_GRAPH_ENV).unwrap_or_else(|_| "__commons__".to_string())
}

/// Most triples one graph write may hold (MEDULLA-FEDERATED-QUERY-R740.1).
pub const RDF_BODY_MAX_TRIPLES: usize = 1_000_000;

/// Why a graph write body was refused (MEDULLA-FEDERATED-QUERY-R740.1). Every refusal
/// leaves the store unchanged: the body is parsed before any update is planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RdfBodyRefusal {
    /// The media type is none of the six the engine reads.
    MediaTypeUnsupported {
        /// The media type as sent, parameters removed.
        media_type: String,
    },
    /// The body does not parse as its media type.
    ParseFailed {
        /// The parser's error text.
        detail: String,
    },
    /// A quad names a graph other than the default graph and the target graph.
    DatasetGraphMismatch,
    /// The body holds more than [`RDF_BODY_MAX_TRIPLES`] triples.
    BodyTooLarge,
}

impl RdfBodyRefusal {
    /// The wire code of the refusal.
    pub fn code(&self) -> &'static str {
        use medulla_types::contract::ServerErrorCode as Code;
        match self {
            Self::MediaTypeUnsupported { .. } => Code::RdfMediaTypeUnsupported,
            Self::ParseFailed { .. } => Code::RdfParseFailed,
            Self::DatasetGraphMismatch => Code::RdfDatasetGraphMismatch,
            Self::BodyTooLarge => Code::RdfBodyTooLarge,
        }
        .as_str()
    }

    /// The HTTP status line of the refusal on the graph-store route.
    pub fn http_status(&self) -> &'static str {
        match self {
            Self::MediaTypeUnsupported { .. } => "415 Unsupported Media Type",
            Self::ParseFailed { .. } => "400 Bad Request",
            Self::DatasetGraphMismatch => "422 Unprocessable Entity",
            Self::BodyTooLarge => "413 Content Too Large",
        }
    }

    /// The error text `CODE: detail`; it names no triple of the body.
    pub fn message(&self) -> String {
        let detail = match self {
            Self::MediaTypeUnsupported { media_type } => {
                format!("media type `{media_type}` is not one of the six RDF media types")
            }
            Self::ParseFailed { detail } => detail.clone(),
            Self::DatasetGraphMismatch => {
                "a quad names a graph other than the default graph and the target graph".into()
            }
            Self::BodyTooLarge => {
                format!("the body holds more than {RDF_BODY_MAX_TRIPLES} triples")
            }
        };
        format!("{}: {detail}", self.code())
    }
}

/// The media type of a `Content-Type` value: parameters removed, lower case.
fn media_type_essence(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// The triples of the quads that name the default graph or `graph`; any other graph name
/// is a mismatch.
fn quads_in_graph(
    quads: Vec<medulla_rdf::oxrdf::Quad>,
    graph: &str,
) -> Result<Vec<medulla_rdf::oxrdf::Triple>, RdfBodyRefusal> {
    use medulla_rdf::oxrdf::{GraphName, Triple};
    let mut triples = Vec::with_capacity(quads.len());
    for quad in quads {
        match &quad.graph_name {
            GraphName::DefaultGraph => {}
            GraphName::NamedNode(name) if name.as_str() == graph => {}
            _ => return Err(RdfBodyRefusal::DatasetGraphMismatch),
        }
        triples.push(Triple::new(quad.subject, quad.predicate, quad.object));
    }
    Ok(triples)
}

fn parse_failed(error: String) -> RdfBodyRefusal {
    RdfBodyRefusal::ParseFailed { detail: error }
}

/// Parse one graph write body by its exact media type (MEDULLA-FEDERATED-QUERY-R740.1).
///
/// The parser is chosen from the media type with its parameters removed: N-Triples,
/// Turtle, N-Quads, TriG, JSON-LD and, with the `rdf-xml` feature, RDF/XML. A quad
/// parser returns triples only when every quad names the default graph or `graph`.
pub fn parse_rdf_body(
    content_type: &str,
    body: &str,
    graph: &str,
) -> Result<Vec<medulla_rdf::oxrdf::Triple>, RdfBodyRefusal> {
    use medulla_rdf::mapping;
    let media_type = media_type_essence(content_type);
    let triples = match media_type.as_str() {
        "application/n-triples" => mapping::parse_ntriples(body).map_err(parse_failed)?,
        "text/turtle" => {
            mapping::parse_turtle_bounded(body, RDF_BODY_MAX_TRIPLES).map_err(|error| {
                if error.contains("exceeds maximum") {
                    RdfBodyRefusal::BodyTooLarge
                } else {
                    parse_failed(error)
                }
            })?
        }
        "application/n-quads" => {
            quads_in_graph(mapping::parse_nquads(body).map_err(parse_failed)?, graph)?
        }
        "application/trig" => {
            quads_in_graph(mapping::parse_trig(body).map_err(parse_failed)?, graph)?
        }
        "application/ld+json" => quads_in_graph(
            medulla_rdf::jsonld::from_jsonld(body).map_err(parse_failed)?,
            graph,
        )?,
        #[cfg(feature = "rdf-xml")]
        "application/rdf+xml" => mapping::parse_rdfxml(body).map_err(parse_failed)?,
        _ => return Err(RdfBodyRefusal::MediaTypeUnsupported { media_type }),
    };
    if triples.len() > RDF_BODY_MAX_TRIPLES {
        return Err(RdfBodyRefusal::BodyTooLarge);
    }
    Ok(triples)
}

/// How a graph write treats the triples already in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdfWriteMode {
    /// Clear the graph, then insert the body (HTTP `PUT`).
    Replace,
    /// Insert the body next to what the graph holds (HTTP `POST`).
    Merge,
}

/// The update text of one graph write: the one function behind both the graph-store route
/// and the served `RDF_GRAPH_WRITE_EVENT` (MEDULLA-FEDERATED-QUERY-R740.3). The text
/// commits through the one SPARQL update coordinator.
pub fn rdf_graph_write_update(
    content_type: &str,
    body: &str,
    graph: &str,
    mode: RdfWriteMode,
) -> Result<String, RdfBodyRefusal> {
    let triples = parse_rdf_body(content_type, body, graph)?;
    let ntriples = medulla_rdf::mapping::to_ntriples(&triples).map_err(parse_failed)?;
    Ok(match mode {
        RdfWriteMode::Replace => format!("CLEAR DEFAULT; INSERT DATA {{\n{ntriples}}}"),
        RdfWriteMode::Merge => format!("INSERT DATA {{\n{ntriples}}}"),
    })
}

/// The update text of one `RDF_GRAPH_WRITE_EVENT`: its `query` is one JSON object
/// `{media_type, mode, body}` with `mode` `replace` or `merge`. An envelope that is not that
/// object refuses `RDF_PARSE_FAILED`.
pub fn rdf_graph_write_event_update(query: &str, graph: &str) -> Result<String, RdfBodyRefusal> {
    let envelope: serde_json::Value = serde_json::from_str(query)
        .map_err(|error| parse_failed(format!("the write envelope is not JSON: {error}")))?;
    let field = |name: &str| {
        envelope
            .get(name)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| parse_failed(format!("the write envelope has no string `{name}`")))
    };
    let mode = match field("mode")? {
        "replace" => RdfWriteMode::Replace,
        "merge" => RdfWriteMode::Merge,
        other => {
            return Err(parse_failed(format!(
                "the write mode `{other}` is neither `replace` nor `merge`"
            )))
        }
    };
    rdf_graph_write_update(field("media_type")?, field("body")?, graph, mode)
}

pub(super) fn export_graph(
    core: &GraphCore,
    name: &str,
) -> Result<Vec<medulla_rdf::oxrdf::Triple>, String> {
    medulla_rdf::mapping::export_triples(core, name)
}
