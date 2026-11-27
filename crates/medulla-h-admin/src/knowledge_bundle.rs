//! The knowledge bundle handlers (MEDULLA-DURABLE-KERNEL-R877.2, R877.3).
//!
//! `BundleExport` reads every part of every chosen graph from one snapshot of the caller's
//! row-projected view, so a hidden node never enters a bundle, and stores the bundle as one blob.
//! `BundleImport` is planned here and committed by the router: [`plan_import`] reads the blob and
//! verifies the manifest and every part before it writes a graph, so each refusal leaves the
//! target unchanged. It stores the documents by digest under the caller's blob scope and rewrites
//! each `cas:sha256:` reference to the target's own (a stored blob's digest covers its owner).
//! The router then creates each target graph, commits its triples through the `AddTriples`
//! gateway (RDF admission applies), commits its provenance rows and embeddings as one batch, and
//! deletes the graphs it created if any step is refused.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::sync::Arc;

use medulla_graph_core::graph::{GraphCore, GraphView};
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::CarrierAuthority;
use medulla_server_api::state::ServerState;
use medulla_server_blob::blob::engine_bodies::read_owned_blob;
use medulla_server_blob::blob::store::ChunkStore;
use medulla_server_blob::blob::stream::stream_blob_put_owned;
use medulla_server_blob::blob::BlobStateExt;
use medulla_types::knowledge_bundle::{
    digest_text, Bundle, BundleError, BundleExportReceipt, BundleImportReceipt, BundleManifest,
    BundleOnConflict, DocumentRef, EmbeddingRecord, EmbeddingsPart, GraphParts, ImportedGraph,
    PartRef, BUNDLE_DIGEST_MISMATCH, BUNDLE_DOCUMENT_BYTES_MAX, BUNDLE_EMBEDDING_MODEL_UNKNOWN,
    BUNDLE_GRAPHS_MAX, BUNDLE_GRAPH_EXISTS, BUNDLE_TOO_LARGE, BUNDLE_TRIPLES_MAX, BUNDLE_VERSION,
    CAS_PREFIX,
};
use serde_json::{json, Value};
use tokio::sync::RwLock;

/// The model id an embedding carries when its graph declares no embedding space.
pub const UNDECLARED_MODEL: &str = "undeclared";
/// Most bytes of the bundle blob an import reads: the documents, the triples and the manifest.
const BUNDLE_BLOB_MAX: usize = BUNDLE_DOCUMENT_BYTES_MAX as usize + (2 << 30);
/// The class of graph bookkeeping nodes a bundle never carries.
const NAMED_GRAPH_PREFIX: &str = "__named_graph__:";

fn refusal(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

fn bundle_refusal(error: BundleError) -> String {
    error.message()
}

/// `graph` opened for `agent_id` with read or write access, and the isolation layer.
async fn open_graph(
    state: &Arc<RwLock<ServerState>>,
    agent_id: &str,
    graph: &str,
    level: medulla_isolation::isolation::AccessLevel,
) -> Result<(Arc<GraphCore>, medulla_isolation::isolation::IsolationLayer), String> {
    medulla_server_api::api::reentry::ensure_graph_resident(state, graph).await;
    let guard = state.read().await;
    let entry = guard
        .registry
        .get(graph)
        .ok_or_else(|| refusal("PARAMETER_INVALID", format!("unknown graph {graph}")))?;
    medulla_server_auth::access::check_graph_access(
        &guard.isolation,
        Some(agent_id),
        graph,
        entry.graph_type,
        entry.owner.as_deref(),
        entry.importing.as_deref(),
        level,
    )?;
    Ok((Arc::clone(&entry.core), guard.isolation.clone()))
}

/// The blob store and the caller's blob owner scope.
async fn blob_store(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
) -> Result<(Arc<dyn ChunkStore>, String), String> {
    let store: Arc<dyn ChunkStore> = {
        let guard = state.read().await;
        match guard.blob_cursors() {
            Some(cursors) => Arc::clone(&cursors.store),
            None => return Err("Blob substrate disabled (no persist dir configured)".to_string()),
        }
    };
    let owner = CarrierAuthority::from_verified(verified)?
        .owner_scope()
        .to_string();
    Ok((store, owner))
}

/// Every `cas:sha256:<64 hex>` reference in `value`.
fn collect_cas_refs(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => {
            if let Some(hex) = text.strip_prefix(CAS_PREFIX) {
                if hex.len() == 64
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    out.insert(text.clone());
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect_cas_refs(item, out)),
        Value::Object(fields) => fields.values().for_each(|item| collect_cas_refs(item, out)),
        _ => {}
    }
}

/// Canonical N-Quads of `triples`: one statement per line, sorted and without repeats.
fn canonical_nquads(triples: &[medulla_rdf::oxrdf::Triple]) -> Result<(Vec<u8>, u64), String> {
    let text = medulla_rdf::mapping::to_nquads(triples, None)?;
    let mut lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines.sort_unstable();
    lines.dedup();
    let count = lines.len() as u64;
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    Ok((out.into_bytes(), count))
}

/// The property-graph rows of `view` that carry no RDF identity: nodes whose id is no RDF term
/// and edges with such an endpoint. They hold the lineage the triples cannot, and are sorted.
fn provenance_rows(view: &GraphView) -> Result<Vec<Value>, String> {
    let mut nodes: Vec<(&String, Value)> = Vec::new();
    for id in view.node_properties.keys() {
        if medulla_rdf::mapping::is_rdf_term(id) || id.starts_with(NAMED_GRAPH_PREFIX) {
            continue;
        }
        if let Some(row) = view.node_row_object(id) {
            nodes.push((id, Value::Object(row)));
        }
    }
    nodes.sort_by(|a, b| a.0.cmp(b.0));
    let mut rows: Vec<Value> = nodes
        .into_iter()
        .map(|(id, properties)| json!({"kind": "node", "id": id, "properties": properties}))
        .collect();
    let mut edges: Vec<(&String, &String, Value)> = Vec::new();
    for ((source, target), blobs) in &view.edge_properties {
        if medulla_rdf::mapping::is_rdf_term(source) && medulla_rdf::mapping::is_rdf_term(target) {
            continue;
        }
        for blob in blobs {
            let properties = medulla_types::msgpack::decode_property_value(blob.as_slice())
                .map_err(|_| "PARAMETER_INVALID: an edge row is unreadable".to_string())?;
            edges.push((source, target, properties));
        }
    }
    edges.sort_by(|a, b| (a.0, a.1, a.2.to_string()).cmp(&(b.0, b.1, b.2.to_string())));
    rows.extend(edges.into_iter().map(|(source, target, properties)| {
        json!({"kind": "edge", "source": source, "target": target, "properties": properties})
    }));
    Ok(rows)
}

/// The embeddings of the visible nodes of `view` and the space the graph declares.
fn embeddings_part(core: &GraphCore, view: &GraphView) -> Result<EmbeddingsPart, String> {
    let store = core.semantic_store.read();
    let space = store
        .space()
        .map(|space| serde_json::to_value(space).map_err(|error| error.to_string()))
        .transpose()?;
    let model_id = space
        .as_ref()
        .and_then(|space| space.get("id"))
        .and_then(Value::as_str)
        .unwrap_or(UNDECLARED_MODEL)
        .to_string();
    let mut ids: Vec<&String> = view.node_properties.keys().collect();
    ids.sort();
    let records = ids
        .into_iter()
        .filter_map(|id| {
            store.get_embedding(id).map(|values| EmbeddingRecord {
                vector_id: id.clone(),
                model_id: model_id.clone(),
                dimension: values.len() as u32,
                values,
            })
        })
        .collect();
    Ok(EmbeddingsPart { space, records })
}

/// The caller's row-projected snapshot of an opened graph.
fn projected_snapshot(
    core: &GraphCore,
    isolation: &medulla_isolation::isolation::IsolationLayer,
    agent_id: &str,
) -> GraphView {
    let view = core.analysis_snapshot();
    #[cfg(feature = "security")]
    let view = {
        let mut view = view;
        isolation.filter_view(agent_id, &mut view);
        view
    };
    #[cfg(not(feature = "security"))]
    let _ = (isolation, agent_id);
    view
}

fn part_ref(entry: &str, bytes: &[u8]) -> PartRef {
    PartRef {
        entry: entry.to_string(),
        sha256: digest_text(bytes),
        size: bytes.len() as u64,
    }
}

/// Serve `Method::BundleExport` (MEDULLA-DURABLE-KERNEL-R877.2).
pub async fn handle_bundle_export(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    graphs: Vec<String>,
    include_embeddings: bool,
    include_documents: bool,
) -> Response {
    match export_bundle(
        state,
        verified,
        graphs,
        include_embeddings,
        include_documents,
    )
    .await
    {
        Ok(receipt) => {
            match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::BundleExport>(
                receipt,
            ) {
                Ok(payload) => Response::ok(req_id, payload),
                Err(error) => Response::err(req_id, error),
            }
        }
        Err(message) => Response::err(req_id, message),
    }
}

/// Export the named graphs as one bundle, as the caller sees them. Each graph contributes its triples and provenance, plus its embeddings and documents when asked; a bundle of no graph, of more graphs or triples than a bundle holds, or naming a graph twice refuses.
pub async fn export_bundle(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    graphs: Vec<String>,
    include_embeddings: bool,
    include_documents: bool,
) -> Result<BundleExportReceipt, String> {
    if graphs.is_empty() {
        return Err(refusal(
            "PARAMETER_INVALID",
            "a bundle export names at least one graph",
        ));
    }
    if graphs.len() > BUNDLE_GRAPHS_MAX {
        return Err(refusal(
            BUNDLE_TOO_LARGE,
            format!(
                "{} graphs are more than the {BUNDLE_GRAPHS_MAX} one bundle holds",
                graphs.len()
            ),
        ));
    }
    if graphs.iter().collect::<BTreeSet<_>>().len() != graphs.len() {
        return Err(refusal("PARAMETER_INVALID", "a graph is named twice"));
    }
    let (store, owner) = blob_store(state, verified).await?;
    let agent_id = verified.agent_id().to_string();

    let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut parts: Vec<GraphParts> = Vec::with_capacity(graphs.len());
    let mut document_bytes: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut total_triples = 0u64;
    let mut total_document_bytes = 0u64;
    for (index, graph) in graphs.iter().enumerate() {
        let (core, isolation) = open_graph(
            state,
            &agent_id,
            graph,
            medulla_isolation::isolation::AccessLevel::Read,
        )
        .await?;
        // One snapshot of the caller's view holds every part of this graph.
        let view = projected_snapshot(&core, &isolation, &agent_id);
        let triples = medulla_rdf::mapping::export_view_triples(&view)?;
        let (triple_bytes, triple_count) = canonical_nquads(&triples)?;
        total_triples += triple_count;
        if total_triples > BUNDLE_TRIPLES_MAX {
            return Err(refusal(
                BUNDLE_TOO_LARGE,
                format!("more than {BUNDLE_TRIPLES_MAX} triples in one bundle"),
            ));
        }
        let provenance = provenance_rows(&view)?;
        let provenance_bytes = serde_json::to_vec(&provenance).map_err(|e| e.to_string())?;
        let base = format!("graphs/{index:04}");
        let triples_ref = part_ref(&format!("{base}/triples.nq"), &triple_bytes);
        let provenance_ref = part_ref(&format!("{base}/provenance.json"), &provenance_bytes);
        entries.insert(triples_ref.entry.clone(), triple_bytes);
        entries.insert(provenance_ref.entry.clone(), provenance_bytes);
        let embeddings_ref = if include_embeddings {
            let part = embeddings_part(&core, &view)?;
            let bytes = serde_json::to_vec(&part).map_err(|e| e.to_string())?;
            let reference = part_ref(&format!("{base}/embeddings.json"), &bytes);
            entries.insert(reference.entry.clone(), bytes);
            Some(reference)
        } else {
            None
        };
        let mut documents: Vec<DocumentRef> = Vec::new();
        if include_documents {
            let mut refs = BTreeSet::new();
            for id in view.node_properties.keys() {
                if let Some(row) = view.node_row_object(id) {
                    collect_cas_refs(&Value::Object(row), &mut refs);
                }
            }
            for cas_ref in refs {
                // A blob the caller cannot read never enters the bundle.
                let Ok(bytes) = read_owned_blob(&*store, &owner, &cas_ref, BUNDLE_BLOB_MAX) else {
                    continue;
                };
                let sha256 = digest_text(&bytes);
                if !document_bytes.contains_key(&sha256) {
                    total_document_bytes += bytes.len() as u64;
                    if total_document_bytes > BUNDLE_DOCUMENT_BYTES_MAX {
                        return Err(refusal(
                            BUNDLE_TOO_LARGE,
                            format!("more than {BUNDLE_DOCUMENT_BYTES_MAX} document bytes in one bundle"),
                        ));
                    }
                    document_bytes.insert(sha256.clone(), bytes.clone());
                }
                documents.push(DocumentRef {
                    cas_ref,
                    sha256,
                    size: bytes.len() as u64,
                });
            }
        }
        parts.push(GraphParts {
            graph: graph.clone(),
            triples: triples_ref,
            triple_count,
            embeddings: embeddings_ref,
            provenance: provenance_ref,
            documents,
        });
    }
    for part in &parts {
        for document in &part.documents {
            if let Some(bytes) = document_bytes.get(&document.sha256) {
                entries.insert(document.entry(), bytes.clone());
            }
        }
    }
    let manifest = BundleManifest {
        bundle_version: BUNDLE_VERSION,
        graphs,
        parts,
        created_at: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
        digest: String::new(),
    }
    .seal();
    manifest.check_limits().map_err(bundle_refusal)?;
    let archive = Bundle::pack(&manifest, &entries);
    let size_bytes = archive.len() as u64;
    let blob = stream_blob_put_owned(&*store, Cursor::new(archive), 0, &owner)?;
    Ok(BundleExportReceipt {
        bundle_ref: format!("{CAS_PREFIX}{}", blob.digest),
        manifest_digest: manifest.digest,
        size_bytes,
    })
}

/// One graph of an import, fully verified and ready to commit.
#[derive(Clone, Debug)]
pub struct PlannedGraph {
    /// The graph name in the bundle.
    pub source: String,
    /// The graph name in the target: the prefix and the source name.
    pub target: String,
    /// Whether the target exists and `on_conflict` is `skip`: the graph is left alone.
    pub skip: bool,
    /// N-Quads for the `AddTriples` gateway; empty when the graph holds no triple.
    pub ntriples: String,
    /// How many triples `ntriples` holds.
    pub triple_count: u64,
    /// The embedding space to declare before the embeddings, as the engine's own space record.
    pub space: Option<Value>,
    /// The `BatchUpdate` operations: provenance upserts, then embeddings.
    pub operations: Vec<Value>,
    /// How many provenance rows `operations` holds.
    pub provenance_count: u64,
    /// How many embeddings `operations` holds.
    pub embedding_count: u64,
}

/// A verified import, ready for the router to commit.
pub struct ImportPlan {
    /// One entry per bundle graph, in manifest order.
    pub graphs: Vec<PlannedGraph>,
    /// How many documents were stored under the caller's blob scope.
    pub documents_stored: u64,
}

/// Replace every bundle document reference in `value` by the target's own.
fn rewrite_refs(value: &mut Value, remap: &BTreeMap<String, String>) {
    match value {
        Value::String(text) => {
            if let Some(new) = remap.get(text.as_str()) {
                *text = new.clone();
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| rewrite_refs(item, remap)),
        Value::Object(fields) => fields
            .values_mut()
            .for_each(|item| rewrite_refs(item, remap)),
        _ => {}
    }
}

/// Read, verify and plan one import. Nothing is written: a refusal here leaves the target as it
/// was.
pub async fn plan_import(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    bundle_ref: &str,
    target_graph_prefix: &str,
    on_conflict: BundleOnConflict,
) -> Result<ImportPlan, String> {
    let (store, owner) = blob_store(state, verified).await?;
    let bytes =
        read_owned_blob(&*store, &owner, bundle_ref, BUNDLE_BLOB_MAX).map_err(
            |error| match error.strip_prefix("BODY_MISSING: ") {
                Some(detail) => refusal("BLOB_NOT_FOUND", detail),
                None => error,
            },
        )?;
    let bundle = Bundle::unpack(&bytes).map_err(bundle_refusal)?;
    drop(bytes);

    let mut graphs: Vec<PlannedGraph> = Vec::with_capacity(bundle.manifest.parts.len());
    let mut documents: Vec<(DocumentRef, Vec<u8>)> = Vec::new();
    let mut seen_documents = BTreeSet::new();
    for part in &bundle.manifest.parts {
        let target = format!("{target_graph_prefix}{}", part.graph);
        medulla_core::registry::validate_graph_name(&target)?;
        let exists = {
            medulla_server_api::api::reentry::ensure_graph_resident(state, &target).await;
            state.read().await.registry.exists(&target)
        };
        if exists && on_conflict == BundleOnConflict::Refuse {
            return Err(refusal(
                BUNDLE_GRAPH_EXISTS,
                format!("the target graph {target} exists and on_conflict is refuse"),
            ));
        }
        let triples = bundle.entry(&part.triples.entry).map_err(bundle_refusal)?;
        let ntriples = String::from_utf8(triples.to_vec())
            .map_err(|_| refusal("PARAMETER_INVALID", "the triples part is not UTF-8"))?;
        let counted = ntriples
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count() as u64;
        if counted != part.triple_count {
            return Err(refusal(
                BUNDLE_DIGEST_MISMATCH,
                format!(
                    "the triples part of {} holds {counted} triples, not {}",
                    part.graph, part.triple_count
                ),
            ));
        }
        let mut operations: Vec<Value> = Vec::new();
        let provenance: Vec<Value> = serde_json::from_slice(
            bundle
                .entry(&part.provenance.entry)
                .map_err(bundle_refusal)?,
        )
        .map_err(|error| {
            refusal(
                "PARAMETER_INVALID",
                format!("the provenance part is malformed: {error}"),
            )
        })?;
        for row in &provenance {
            operations.push(match row.get("kind").and_then(Value::as_str) {
                Some("node") => {
                    json!({"op": "upsert_node", "id": row["id"], "properties": row["properties"]})
                }
                Some("edge") => json!({
                    "op": "upsert_edge", "source": row["source"], "target": row["target"],
                    "properties": row["properties"],
                }),
                _ => {
                    return Err(refusal(
                        "PARAMETER_INVALID",
                        "a provenance row is neither a node nor an edge",
                    ))
                }
            });
        }
        let provenance_count = provenance.len() as u64;
        let (space, embedding_count) = match &part.embeddings {
            Some(reference) => {
                let embeddings: EmbeddingsPart =
                    serde_json::from_slice(bundle.entry(&reference.entry).map_err(bundle_refusal)?)
                        .map_err(|error| {
                            refusal(
                                "PARAMETER_INVALID",
                                format!("the embeddings part is malformed: {error}"),
                            )
                        })?;
                let known = embeddings
                    .space
                    .as_ref()
                    .and_then(|space| space.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or(UNDECLARED_MODEL)
                    .to_string();
                for record in &embeddings.records {
                    if record.model_id != known {
                        return Err(refusal(
                            BUNDLE_EMBEDDING_MODEL_UNKNOWN,
                            format!(
                                "the embedding of {} names model {}, the graph declares {known}",
                                record.vector_id, record.model_id
                            ),
                        ));
                    }
                    if record.values.len() != record.dimension as usize {
                        return Err(refusal(
                            "PARAMETER_INVALID",
                            format!(
                                "the embedding of {} is not {} wide",
                                record.vector_id, record.dimension
                            ),
                        ));
                    }
                    operations.push(json!({
                        "op": "add_embedding", "id": record.vector_id, "embedding": record.values,
                    }));
                }
                (embeddings.space, embeddings.records.len() as u64)
            }
            None => (None, 0),
        };
        if !exists {
            for document in &part.documents {
                if seen_documents.insert(document.cas_ref.clone()) {
                    let bytes = bundle
                        .entry(&document.entry())
                        .map_err(bundle_refusal)?
                        .to_vec();
                    documents.push((document.clone(), bytes));
                }
            }
        }
        graphs.push(PlannedGraph {
            source: part.graph.clone(),
            skip: exists,
            target,
            ntriples,
            triple_count: part.triple_count,
            space,
            operations,
            provenance_count,
            embedding_count,
        });
    }
    // Every check has passed: store the documents and point the rows at the target's blobs.
    let mut remap: BTreeMap<String, String> = BTreeMap::new();
    for (document, bytes) in &documents {
        let blob = stream_blob_put_owned(&*store, Cursor::new(bytes.as_slice()), 0, &owner)?;
        remap.insert(
            document.cas_ref.clone(),
            format!("{CAS_PREFIX}{}", blob.digest),
        );
    }
    if !remap.is_empty() {
        for graph in graphs.iter_mut().filter(|graph| !graph.skip) {
            for (old, new) in &remap {
                graph.ntriples = graph.ntriples.replace(old.as_str(), new);
            }
            for operation in &mut graph.operations {
                rewrite_refs(operation, &remap);
            }
        }
    }
    Ok(ImportPlan {
        graphs,
        documents_stored: documents.len() as u64,
    })
}

impl ImportPlan {
    /// What the import wrote, once every graph committed.
    pub fn receipt(&self) -> BundleImportReceipt {
        let written = self.graphs.iter().filter(|graph| !graph.skip);
        BundleImportReceipt {
            graphs: self
                .graphs
                .iter()
                .map(|graph| ImportedGraph {
                    graph: graph.target.clone(),
                    skipped: graph.skip,
                })
                .collect(),
            triples: written.clone().map(|graph| graph.triple_count).sum(),
            embeddings: written.clone().map(|graph| graph.embedding_count).sum(),
            provenance: written.map(|graph| graph.provenance_count).sum(),
            documents: self.documents_stored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::knowledge_bundle::{BUNDLE_VERSION_UNSUPPORTED, MANIFEST_ENTRY};

    const TENANT: &str = "tenant-bundle";
    const SOURCE: &str = "__commons__";

    fn verified() -> VerifiedRequestContext {
        VerifiedRequestContext::verified_for_test_with_scopes("bundle-agent", TENANT, &["*"])
    }

    async fn served() -> (Arc<RwLock<ServerState>>, tempfile::TempDir) {
        served_with(ServerState::test_isolation("bundle-agent")).await
    }

    async fn served_with(
        isolation: medulla_isolation::isolation::IsolationLayer,
    ) -> (Arc<RwLock<ServerState>>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut server = ServerState::new_for_test("bundle-test-secret", isolation);
        server.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        server.set_blob_cursors(Some(medulla_server_ops::api::ingest::cursors_for_test(
            dir.path().to_str().unwrap(),
        )));
        (Arc::new(RwLock::new(server)), dir)
    }

    /// The source graph: two RDF triples, one embedding, one property-graph document node.
    async fn seed(state: &Arc<RwLock<ServerState>>, document_ref: &str) {
        let core = state
            .read()
            .await
            .registry
            .get(SOURCE)
            .unwrap()
            .core
            .clone();
        let triples = medulla_rdf::mapping::parse_ntriples(
            "<urn:bundle:a> <urn:bundle:title> \"alpha\" .\n\
             <urn:bundle:a> <urn:bundle:next> <urn:bundle:b> .\n",
        )
        .unwrap();
        medulla_rdf::mapping::load_triples(
            &core,
            &mut medulla_rdf::mapping::IriStore::default(),
            SOURCE,
            triples,
        )
        .unwrap();
        core.semantic_store
            .write()
            .add_embedding("<urn:bundle:a>".to_string(), vec![1.0, 0.0])
            .unwrap();
        core.add_node(
            "bundle-doc".to_string(),
            rmp_serde::to_vec_named(&json!({"type": "Document", "content_ref": document_ref}))
                .unwrap(),
        );
    }

    async fn export(state: &Arc<RwLock<ServerState>>) -> BundleExportReceipt {
        export_bundle(state, &verified(), vec![SOURCE.to_string()], true, true)
            .await
            .expect("an export answers")
    }

    // spec: MEDULLA-DURABLE-KERNEL-R877.2
    #[tokio::test]
    async fn export_then_plan_carries_every_part_and_remaps_documents() {
        let (state, _dir) = served().await;
        let (store, owner) = blob_store(&state, &verified()).await.unwrap();
        let blob = stream_blob_put_owned(
            &*store,
            Cursor::new(b"source document".as_slice()),
            0,
            &owner,
        )
        .unwrap();
        let document_ref = format!("{CAS_PREFIX}{}", blob.digest);
        seed(&state, &document_ref).await;

        let receipt = export(&state).await;
        assert!(receipt.bundle_ref.starts_with(CAS_PREFIX));
        let plan = plan_import(
            &state,
            &verified(),
            &receipt.bundle_ref,
            "copy-",
            BundleOnConflict::Refuse,
        )
        .await
        .expect("a fresh export plans");
        let graph = &plan.graphs[0];
        assert_eq!(graph.target, format!("copy-{SOURCE}"));
        assert!(!graph.skip);
        assert_eq!(graph.triple_count, 2);
        assert_eq!(graph.embedding_count, 1);
        assert_eq!(graph.provenance_count, 1);
        assert_eq!(plan.documents_stored, 1);
        // The document row now points at the blob stored under the target's own scope.
        let row = graph
            .operations
            .iter()
            .find(|operation| operation["id"] == "bundle-doc")
            .expect("the document node is carried as a provenance row");
        let stored = row["properties"]["content_ref"].as_str().unwrap();
        assert!(stored.starts_with(CAS_PREFIX));
        let body = read_owned_blob(&*store, &owner, stored, 1 << 20).unwrap();
        assert_eq!(body, b"source document");
        assert_eq!(plan.receipt().triples, 2);
    }

    // spec: MEDULLA-DURABLE-KERNEL-R877.3
    #[tokio::test]
    async fn each_refusal_names_its_code_and_writes_nothing() {
        let (state, _dir) = served().await;
        seed(&state, "not a reference").await;
        let receipt = export(&state).await;
        let (store, owner) = blob_store(&state, &verified()).await.unwrap();
        let put = |bytes: Vec<u8>| {
            let blob = stream_blob_put_owned(&*store, Cursor::new(bytes), 0, &owner).unwrap();
            format!("{CAS_PREFIX}{}", blob.digest)
        };
        let plan = |bundle_ref: String, prefix: &'static str, conflict| {
            let state = state.clone();
            async move { plan_import(&state, &verified(), &bundle_ref, prefix, conflict).await }
        };
        let archive = read_owned_blob(&*store, &owner, &receipt.bundle_ref, 1 << 24).unwrap();

        // BUNDLE_GRAPH_EXISTS: the target already holds the graph and on_conflict is refuse.
        let error = plan(receipt.bundle_ref.clone(), "", BundleOnConflict::Refuse)
            .await
            .err()
            .unwrap();
        assert!(error.starts_with(BUNDLE_GRAPH_EXISTS), "{error}");
        // With skip the graph is left alone and nothing is stored.
        let skipped = plan(receipt.bundle_ref.clone(), "", BundleOnConflict::Skip)
            .await
            .ok()
            .unwrap();
        assert!(skipped.graphs[0].skip);
        assert_eq!(skipped.documents_stored, 0);

        // BUNDLE_DIGEST_MISMATCH: one changed byte of a part.
        let bundle = Bundle::unpack(&archive).unwrap();
        let mut entries = bundle.entries.clone();
        let triples = &bundle.manifest.parts[0].triples.entry;
        entries.get_mut(triples).unwrap()[0] ^= 1;
        let corrupt = put(Bundle::pack(&bundle.manifest, &entries));
        let error = plan(corrupt, "x-", BundleOnConflict::Refuse)
            .await
            .err()
            .unwrap();
        assert!(error.starts_with(BUNDLE_DIGEST_MISMATCH), "{error}");

        // BUNDLE_VERSION_UNSUPPORTED: a manifest of another version.
        let mut future = bundle.manifest.clone();
        future.bundle_version = BUNDLE_VERSION + 1;
        let unknown = put(Bundle::pack(&future, &bundle.entries));
        let error = plan(unknown, "x-", BundleOnConflict::Refuse)
            .await
            .err()
            .unwrap();
        assert!(error.starts_with(BUNDLE_VERSION_UNSUPPORTED), "{error}");

        // BUNDLE_TOO_LARGE: more triples than one bundle holds.
        let mut large = bundle.manifest.clone();
        large.parts[0].triple_count = BUNDLE_TRIPLES_MAX + 1;
        let large = put(Bundle::pack(&large.seal(), &bundle.entries));
        let error = plan(large, "x-", BundleOnConflict::Refuse)
            .await
            .err()
            .unwrap();
        assert!(error.starts_with(BUNDLE_TOO_LARGE), "{error}");

        // BUNDLE_EMBEDDING_MODEL_UNKNOWN: a vector whose model is not the graph's declared one.
        let embeddings = bundle.manifest.parts[0].embeddings.clone().unwrap();
        let mut part: EmbeddingsPart =
            serde_json::from_slice(&bundle.entries[&embeddings.entry]).unwrap();
        part.records[0].model_id = "another-model".to_string();
        let bytes = serde_json::to_vec(&part).unwrap();
        let mut manifest = bundle.manifest.clone();
        manifest.parts[0].embeddings = Some(part_ref(&embeddings.entry, &bytes));
        let mut entries = bundle.entries.clone();
        entries.insert(embeddings.entry.clone(), bytes);
        let foreign = put(Bundle::pack(&manifest.seal(), &entries));
        let error = plan(foreign, "x-", BundleOnConflict::Refuse)
            .await
            .err()
            .unwrap();
        assert!(error.starts_with(BUNDLE_EMBEDDING_MODEL_UNKNOWN), "{error}");

        // An export of an unknown graph, or of too many, writes no blob.
        let error = export_bundle(
            &state,
            &verified(),
            vec!["absent".to_string()],
            false,
            false,
        )
        .await
        .err()
        .unwrap();
        assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
        let many: Vec<String> = (0..=BUNDLE_GRAPHS_MAX).map(|n| format!("g{n}")).collect();
        let error = export_bundle(&state, &verified(), many, false, false)
            .await
            .err()
            .unwrap();
        assert!(error.starts_with(BUNDLE_TOO_LARGE), "{error}");
        assert_eq!(MANIFEST_ENTRY, "manifest.json");
    }

    /// An isolation layer whose only agent is an ordinary one: it sees the rows it owns and the
    /// public ones, never another principal's private row.
    #[cfg(feature = "security")]
    fn limited_isolation(agent: &str) -> medulla_isolation::isolation::IsolationLayer {
        let mut isolation = medulla_isolation::isolation::IsolationLayer::new();
        isolation.register_agent(medulla_identity_types::acl::AgentIdentity {
            agent_id: agent.to_string(),
            role: medulla_identity_types::acl::AgentRole::Agent,
            teams: Vec::new(),
            roles: Vec::new(),
        });
        isolation
    }

    // spec: MEDULLA-DURABLE-KERNEL-R877.2
    #[cfg(feature = "security")]
    #[tokio::test]
    async fn export_hides_rls_nodes_and_import_is_atomic() {
        let (state, _dir) = served_with(limited_isolation("limited-agent")).await;
        let caller =
            VerifiedRequestContext::verified_for_test_with_scopes("limited-agent", TENANT, &["*"]);
        let core = state
            .read()
            .await
            .registry
            .get(SOURCE)
            .unwrap()
            .core
            .clone();
        let row = |owner: &str, visibility: &str| {
            rmp_serde::to_vec_named(&json!({
                "type": "Note", "_owner": owner, "_visibility": visibility, "text": "row",
            }))
            .unwrap()
        };
        core.add_node("note-mine".to_string(), row("limited-agent", "private"));
        core.add_node("note-theirs".to_string(), row("another-agent", "private"));
        {
            let mut store = core.semantic_store.write();
            store
                .add_embedding("note-mine".to_string(), vec![1.0, 0.0])
                .unwrap();
            store
                .add_embedding("note-theirs".to_string(), vec![0.0, 1.0])
                .unwrap();
        }

        let receipt = export_bundle(&state, &caller, vec![SOURCE.to_string()], true, true)
            .await
            .expect("an ordinary caller exports what it can read");
        let (store, owner) = blob_store(&state, &caller).await.unwrap();
        let archive = read_owned_blob(&*store, &owner, &receipt.bundle_ref, 1 << 24).unwrap();
        let bundle = Bundle::unpack(&archive).unwrap();
        let part = &bundle.manifest.parts[0];
        // A hidden row never enters the bundle: not as a provenance row, not as an embedding.
        let provenance = String::from_utf8(bundle.entries[&part.provenance.entry].clone()).unwrap();
        assert!(provenance.contains("note-mine"), "{provenance}");
        assert!(!provenance.contains("note-theirs"), "{provenance}");
        let embeddings: EmbeddingsPart =
            serde_json::from_slice(&bundle.entries[&part.embeddings.as_ref().unwrap().entry])
                .unwrap();
        let ids: Vec<&str> = embeddings
            .records
            .iter()
            .map(|r| r.vector_id.as_str())
            .collect();
        assert_eq!(ids, ["note-mine"]);

        // The import is atomic: a bundle with one bad part plans nothing, and no target graph
        // exists afterwards.
        let mut bad: EmbeddingsPart = embeddings.clone();
        bad.records[0].model_id = "another-model".to_string();
        let bytes = serde_json::to_vec(&bad).unwrap();
        let reference = part.embeddings.clone().unwrap();
        let mut manifest = bundle.manifest.clone();
        manifest.parts[0].embeddings = Some(part_ref(&reference.entry, &bytes));
        let mut entries = bundle.entries.clone();
        entries.insert(reference.entry.clone(), bytes);
        let blob = stream_blob_put_owned(
            &*store,
            Cursor::new(Bundle::pack(&manifest.seal(), &entries)),
            0,
            &owner,
        )
        .unwrap();
        let error = plan_import(
            &state,
            &caller,
            &format!("{CAS_PREFIX}{}", blob.digest),
            "atomic-",
            BundleOnConflict::Refuse,
        )
        .await
        .err()
        .expect("a bad part refuses the whole import");
        assert!(error.starts_with(BUNDLE_EMBEDDING_MODEL_UNKNOWN), "{error}");
        assert!(!state
            .read()
            .await
            .registry
            .exists(&format!("atomic-{SOURCE}")));
    }

    // spec: MEDULLA-DURABLE-KERNEL-R877.2
    #[test]
    fn cas_references_are_found_in_nested_rows() {
        let digest = "ab".repeat(32);
        let row = json!({
            "name": "doc",
            "body": {"ref": format!("{CAS_PREFIX}{digest}"), "other": ["x", format!("{CAS_PREFIX}short")]},
        });
        let mut found = BTreeSet::new();
        collect_cas_refs(&row, &mut found);
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            [format!("{CAS_PREFIX}{digest}")]
        );
    }
}
