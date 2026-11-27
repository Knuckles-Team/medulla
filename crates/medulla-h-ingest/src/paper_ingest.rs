//! Research paper ingest (MEDULLA-MEDIA-VOCAB-R002.5.1 to R002.5.5).
//!
//! A `SourceIngest` record whose resolved mapping class equals or extends `ResearchPaper` is a
//! paper. [`admit_paper`] checks it ([`validate`]), maps it to one `paper:<hash>` node ([`map`])
//! and writes its citation edges ([`cites`]); a refused paper writes nothing and its siblings in
//! the envelope still commit.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_protocol_types::protocol::Method;
use medulla_types::contract::Digest256;
use medulla_types::refusal_code::RefusalCode;

use crate::source_ingestion::documents::{class_closure, class_iri};

pub(crate) mod cites;
pub(crate) mod map;
pub(crate) mod validate;

/// The ontology class a paper record resolves to.
const PAPER_CLASS: &str = "ResearchPaper";

/// What one admitted paper writes: its node and the citation edges and stubs it adds.
pub(crate) struct PaperMapping {
    pub(crate) node_id: String,
    pub(crate) properties: serde_json::Map<String, serde_json::Value>,
    /// Stub nodes for cited papers the batch and the graph do not hold, each with its id.
    pub(crate) stubs: Vec<(String, Method)>,
    pub(crate) edges: Vec<Method>,
}

/// The node id of the paper with `canonical_id`: `paper:<lowercase hex SHA-256 of the id>`.
pub(crate) fn paper_node_id(canonical_id: &str) -> String {
    format!(
        "paper:{}",
        Digest256::sha256(canonical_id.as_bytes()).to_hex()
    )
}

type Cached = Option<(Digest256, Arc<BTreeSet<String>>)>;

/// The IRIs of `ResearchPaper` and every class that extends it, computed once per schema identity.
pub(crate) fn paper_classes(sources: &GraphSchemaSources) -> Result<Arc<BTreeSet<String>>, String> {
    static CACHE: Mutex<Cached> = Mutex::new(None);
    let digest = sources.composed_digest();
    let mut cache = CACHE.lock().map_err(|_| "paper class cache poisoned")?;
    if let Some((cached, set)) = cache.as_ref() {
        if *cached == digest {
            return Ok(Arc::clone(set));
        }
    }
    let documents: Vec<&str> = sources.ontologies().map(|(_, text)| text).collect();
    let set = Arc::new(class_closure(&documents, &class_iri(PAPER_CLASS))?);
    *cache = Some((digest, Arc::clone(&set)));
    Ok(set)
}

/// Whether a mapping's `ontology_class` names a paper class.
pub(crate) fn is_paper(classes: &BTreeSet<String>, ontology_class: &str) -> bool {
    classes.contains(&class_iri(ontology_class))
}

/// Check, map and cite one paper payload. `tenant` is the verified tenant; `known` holds the node
/// ids of the papers the batch and the graph already hold, so a cited paper in it gets no stub.
pub(crate) fn admit_paper(
    payload: &serde_json::Value,
    tenant: &str,
    known: &BTreeSet<String>,
) -> Result<PaperMapping, RefusalCode> {
    validate::validate_paper(payload, tenant)?;
    map::map_paper(payload, known)
}
