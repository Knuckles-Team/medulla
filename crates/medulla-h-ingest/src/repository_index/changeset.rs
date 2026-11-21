//! The code graph follows an accepted changeset (MEDULLA-SCM-R006).
//!
//! `reindex_changeset` decides, from the stored index state of the repository
//! graph, which files of one changeset re-enter the pipeline: only structurally
//! changed ones. It returns the write-set that replaces exactly those files'
//! nodes (through the one `lower` of `repository_index.rs`), the communities of
//! the affected call subgraph, the new fingerprints and the next generation. It
//! never reads or writes a symbol outside the affected set.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use medulla_compute::parser::resolve::{
    file_node, resolution_names, resolve, similarity_bands, unresolved_imports,
    HAS_UNRESOLVED_CALLS, HAS_UNRESOLVED_IMPORTS, UNRESOLVED_CALLS, UNRESOLVED_IMPORTS,
};
use medulla_compute::parser::tree_sitter::{is_parseable_path, parse_file};
use medulla_enrich::fingerprint::{classify, ChangeLevel, StructuralFingerprint};
use medulla_enrich::Id;
use medulla_ingestion_types::ingestion_wire::{
    ExtractedEdge, ExtractedNode, IndexDelta, IndexResult, ParseResult,
};
use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_changeset::{Changeset, PathOp};

use super::delta::{refuse_mismatched_base, RepositoryGeneration};
use super::enrich::fingerprint::{fingerprint_method, fingerprint_node_id};
use super::enrich::{communities, feature_methods};
use super::{lower, IndexWriteSet};

/// Node type of the generation record `codegen:<repo>`.
pub const GENERATION_TYPE: &str = "RepositoryGeneration";

/// Envelope id prefix of a changeset re-index write-set.
const ENVELOPE_PREFIX: &str = "repository-changeset:";

/// The id of the generation node of one repository.
pub fn generation_node_id(repo: &str) -> String {
    format!("codegen:{repo}")
}

/// The generation node of `repo`: the one record both a full index and a re-indexed
/// changeset write, so a full-index baseline accepts the next changeset.
pub fn generation_node(repo: &str, generation: &RepositoryGeneration) -> ExtractedNode {
    ExtractedNode {
        node_id: generation_node_id(repo),
        node_type: GENERATION_TYPE.to_string(),
        properties: [
            (
                "map_generation".to_string(),
                generation.map_generation.to_string(),
            ),
            (
                "base_revision".to_string(),
                generation.base_revision.clone(),
            ),
            ("complete".to_string(), generation.complete.to_string()),
        ]
        .into_iter()
        .collect(),
    }
}

/// The generation a node `codegen:<repo>` stores, from its decoded properties.
pub fn stored_generation(node: &serde_json::Value) -> RepositoryGeneration {
    let text = |key: &str| node.get(key).and_then(serde_json::Value::as_str);
    RepositoryGeneration {
        map_generation: text("map_generation")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
        base_revision: text("base_revision").unwrap_or_default().to_string(),
        complete: text("complete") == Some("true"),
    }
}

/// The generation a stored `codegen:<repo>` node blob holds.
pub fn read_generation(blob: &[u8]) -> Option<RepositoryGeneration> {
    medulla_types::msgpack::decode_property_value(blob)
        .ok()
        .map(|node| stored_generation(&node))
}

/// The generation after `stored`, chained from `base_revision`.
pub fn next_generation(
    stored: Option<&RepositoryGeneration>,
    base_revision: &str,
) -> RepositoryGeneration {
    RepositoryGeneration {
        map_generation: stored.map_or(1, |generation| generation.map_generation + 1),
        base_revision: base_revision.to_string(),
        complete: true,
    }
}

/// Stamp a full index of `repo` with its generation node, chained from
/// `base_revision`, so the next changeset of the repository finds the baseline it
/// chains from. Indexing the same base again writes the generation it already is,
/// so a repeated index stays a replay. Returns the generation written.
pub fn stamp_generation(
    result: &mut IndexResult,
    repo: &str,
    stored: Option<&RepositoryGeneration>,
    base_revision: &str,
) -> RepositoryGeneration {
    // The same base indexed again is a replay: it writes the generation it already is.
    let generation = match stored {
        Some(current) if current.complete && current.base_revision == base_revision => {
            current.clone()
        }
        _ => next_generation(stored, base_revision),
    };
    result.nodes.push(generation_node(repo, &generation));
    generation
}

/// One file version of a changeset: a path and its new bytes.
pub struct ChangesetFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// What the repository graph stores of the index, restricted to the changed
/// paths and the call components that touch them.
#[derive(Default)]
pub struct StoredIndex {
    /// The node `codegen:<repo>`; `None` when no index exists yet.
    pub generation: Option<RepositoryGeneration>,
    /// The stored fingerprints of the changed paths.
    pub prints: BTreeMap<Id, StructuralFingerprint>,
    /// Every node whose `file_path` equals a changed path: the file node, its
    /// symbols, tests and spans.
    pub nodes_by_path: BTreeMap<String, Vec<String>>,
    /// Every resolved `calls` edge that touches one of those nodes.
    pub calls: Vec<(String, String)>,
    /// The community label of each symbol in the call components that contain
    /// those nodes (the label of the `feature:<label>` node it is a member of).
    pub communities: BTreeMap<String, String>,
    /// Every edge, of any type, that touches one of those nodes, with the
    /// properties it was stored with.
    pub edges: Vec<StoredEdge>,
    /// The stored node at the far end of each of those edges.
    pub neighbours: BTreeMap<String, StoredNode>,
}

/// A stored node: its id and its text properties (the `type` key included).
#[derive(Clone, Debug, Default)]
pub struct StoredNode {
    pub id: String,
    pub properties: BTreeMap<String, String>,
}

/// A stored edge: its relationship and the properties it carries besides it.
#[derive(Clone, Debug)]
pub struct StoredEdge {
    pub source: String,
    pub target: String,
    pub edge_type: String,
    pub properties: BTreeMap<String, String>,
}

/// Indexed reads of the repository graph that depend on what the changed files
/// ask for, so they cannot be gathered before the files are parsed.
pub trait StoredLookup {
    /// The stored `SYMBOL` nodes whose `name` is `name`.
    fn definitions(&self, name: &str) -> Result<Vec<StoredNode>, String>;
    /// The path of every file node of the repository.
    fn file_paths(&self) -> Result<Vec<String>, String>;
    /// The stored `SYMBOL` nodes that share any of these signature band keys and
    /// hashes: the symbols a changed symbol can be similar to.
    fn similar_candidates(&self, bands: &[(String, String)]) -> Result<Vec<StoredNode>, String>;
    /// The stored `SYMBOL` nodes that hold call sites bound to no definition.
    fn unresolved_calls(&self) -> Result<Vec<StoredNode>, String>;
    /// The stored `FILE` nodes that hold import modules bound to no file.
    fn unresolved_imports(&self) -> Result<Vec<StoredNode>, String>;
}

/// A repository graph that holds no other symbol and no other file.
pub struct NoLookup;

impl StoredLookup for NoLookup {
    fn definitions(&self, _name: &str) -> Result<Vec<StoredNode>, String> {
        Ok(Vec::new())
    }

    fn file_paths(&self) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    fn similar_candidates(&self, _bands: &[(String, String)]) -> Result<Vec<StoredNode>, String> {
        Ok(Vec::new())
    }

    fn unresolved_calls(&self) -> Result<Vec<StoredNode>, String> {
        Ok(Vec::new())
    }

    fn unresolved_imports(&self) -> Result<Vec<StoredNode>, String> {
        Ok(Vec::new())
    }
}

fn extracted(node: &StoredNode) -> ExtractedNode {
    let mut properties: std::collections::HashMap<String, String> = node
        .properties
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let node_type = properties.remove("type").unwrap_or_default();
    ExtractedNode {
        node_id: node.id.clone(),
        node_type,
        properties,
    }
}

/// A stored symbol as a resolution input that defines and is similar but calls
/// nothing: its call edges are stored, not resolved again.
fn inert(mut node: ExtractedNode) -> ExtractedNode {
    node.properties.remove("calls");
    node.properties.remove("call_sites");
    node
}

/// A stored caller as a resolution input: the call sites it left unresolved and the
/// names of the calls whose target was removed, and nothing else it calls.
fn caller_node(node: &StoredNode, fed: &BTreeSet<String>) -> ExtractedNode {
    let mut input = extracted(node);
    let mut sites: BTreeSet<String> = input
        .properties
        .get(UNRESOLVED_CALLS)
        .map(|sites| split_list(sites))
        .unwrap_or_default()
        .into_iter()
        .collect();
    sites.extend(fed.iter().cloned());
    input.properties.remove("calls");
    input.properties.insert(
        "call_sites".to_string(),
        sites.into_iter().collect::<Vec<_>>().join(";"),
    );
    input
}

/// The `;`-joined entries of a stored list property.
fn split_list(list: &str) -> Vec<String> {
    list.split(';')
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// The receiver and callee names of encoded call sites (`receiver:callee:argc`).
fn site_names(sites: &str) -> Vec<String> {
    split_list(sites)
        .iter()
        .flat_map(|site| {
            site.splitn(3, ':')
                .take(2)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// `node` with the properties named by `keys` taken from `now` (or dropped when `now`
/// lacks them), or `None` when that changes nothing.
fn with_unresolved(
    node: &StoredNode,
    keys: [&str; 2],
    now: &std::collections::HashMap<String, String>,
) -> Option<ExtractedNode> {
    let before = extracted(node);
    let mut after = extracted(node);
    for key in keys {
        match now.get(key) {
            Some(value) => {
                after.properties.insert(key.to_string(), value.clone());
            }
            None => {
                after.properties.remove(key);
            }
        }
    }
    (after.properties != before.properties).then_some(after)
}

/// The outcome of re-indexing one changeset.
pub struct ReindexOutcome {
    pub write_set: IndexWriteSet,
    pub generation: RepositoryGeneration,
    pub reparsed: Vec<String>,
    pub unchanged: Vec<String>,
    pub deleted: Vec<String>,
    pub communities_recomputed: usize,
}

/// One parsed file: its bytes' parse (or the failure) and the fingerprint.
struct Parsed {
    path: String,
    result: Result<ParseResult, String>,
}

fn symbol_properties(node: &ExtractedNode) -> BTreeMap<String, String> {
    node.properties
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// The fingerprint of a parsed file under the one definition of the committed
/// rows, or `None` when its path is not a valid identifier.
fn print_of(parsed: &Parsed, bytes: &[u8]) -> Option<StructuralFingerprint> {
    let path_id = Id::new(parsed.path.clone()).ok()?;
    Some(match &parsed.result {
        Ok(result) => {
            let properties: Vec<(String, BTreeMap<String, String>)> = result
                .nodes
                .iter()
                .filter(|node| node.node_type == "SYMBOL")
                .map(|node| (node.node_id.clone(), symbol_properties(node)))
                .collect();
            super::enrich::fingerprint::symbol_fingerprint(
                path_id,
                properties
                    .iter()
                    .map(|(id, properties)| (id.as_str(), properties))
                    .collect(),
            )
        }
        Err(error) => medulla_enrich::fingerprint::fingerprint(path_id, bytes, Err(error.as_str())),
    })
}

/// Re-index the structurally changed files of `changeset`. `commit_id` is the
/// branch commit that carries it: the next changeset's first parent, so it is the
/// base the new generation chains from.
pub fn reindex_changeset(
    changeset: &Changeset,
    commit_id: &str,
    files: &[ChangesetFile],
    stored: &StoredIndex,
    lookup: &dyn StoredLookup,
) -> Result<ReindexOutcome, String> {
    // (1) The base check, before anything is parsed or written. No generation is the
    // empty base: the first changeset of a repository has no parent.
    let expected = changeset.parents.first().cloned().unwrap_or_default();
    let current = stored.generation.clone().unwrap_or(RepositoryGeneration {
        map_generation: 0,
        base_revision: String::new(),
        complete: true,
    });
    refuse_mismatched_base(
        &IndexDelta {
            base_revision: expected,
            target_revision: commit_id.to_string(),
            ..Default::default()
        },
        Some(&current),
    )?;

    // (2) Classify every changed path, in order.
    let bytes_of: BTreeMap<&str, &[u8]> = files
        .iter()
        .map(|file| (file.path.as_str(), file.bytes.as_slice()))
        .collect();
    let mut deleted = Vec::new();
    let mut unchanged = Vec::new();
    let mut reparsed = Vec::new();
    let mut parsed: Vec<Parsed> = Vec::new();
    let mut prints: Vec<StructuralFingerprint> = Vec::new();
    for changed in &changeset.changed_paths {
        match changed.op {
            PathOp::Delete => deleted.push(changed.path.clone()),
            PathOp::Add | PathOp::Modify | PathOp::Rename => {
                let bytes = bytes_of.get(changed.path.as_str()).ok_or_else(|| {
                    format!(
                        "CHANGESET_INVALID: no bytes were supplied for {}",
                        changed.path
                    )
                })?;
                // A path with no grammar holds no code: nothing to index.
                if !is_parseable_path(&changed.path) {
                    unchanged.push(changed.path.clone());
                    continue;
                }
                let file = Parsed {
                    path: changed.path.clone(),
                    result: parse_file(&changed.path, bytes),
                };
                let print = print_of(&file, bytes);
                let level = match &print {
                    Some(new) => {
                        let old = Id::new(changed.path.clone())
                            .ok()
                            .and_then(|id| stored.prints.get(&id));
                        classify(old, new).0
                    }
                    None => ChangeLevel::Structural,
                };
                match level {
                    ChangeLevel::None | ChangeLevel::Cosmetic => {
                        unchanged.push(changed.path.clone());
                    }
                    ChangeLevel::Structural => {
                        reparsed.push(changed.path.clone());
                        prints.extend(print);
                        parsed.push(file);
                    }
                }
            }
        }
    }

    // (3) The nodes of every reparsed and deleted path leave, and the reparsed
    // paths are lowered again, so every id is the per-occurrence id a full index
    // gives.
    let touched: BTreeSet<&str> = reparsed
        .iter()
        .chain(&deleted)
        .map(String::as_str)
        .collect();
    let removed: BTreeSet<String> = touched
        .iter()
        .filter_map(|path| stored.nodes_by_path.get(*path))
        .flatten()
        .cloned()
        .collect();
    let mut methods: Vec<Method> = removed
        .iter()
        .map(|id| Method::RemoveNode {
            node_id: id.clone(),
        })
        .collect();
    for path in &deleted {
        // The fingerprint node carries the path, so it is usually removed above.
        let fingerprint = fingerprint_node_id(&changeset.repo, path)?;
        if !removed.contains(&fingerprint) {
            methods.push(Method::RemoveNode {
                node_id: fingerprint,
            });
        }
    }

    let inputs: Vec<(String, Vec<u8>)> = parsed
        .iter()
        .map(|file| {
            (
                file.path.clone(),
                bytes_of
                    .get(file.path.as_str())
                    .map(|bytes| bytes.to_vec())
                    .unwrap_or_default(),
            )
        })
        .collect();
    let results: Vec<ParseResult> = parsed
        .iter()
        .map(|file| match &file.result {
            Ok(result) => ParseResult {
                nodes: result.nodes.iter().map(clone_node).collect(),
                edges: result.edges.iter().map(clone_edge).collect(),
                symbols_extracted: result.symbols_extracted,
            },
            Err(_) => ParseResult {
                nodes: Vec::new(),
                edges: Vec::new(),
                symbols_extracted: 0,
            },
        })
        .collect();
    // Resolution sees the changed files beside the stored symbols they bind to, the
    // stored callers whose edges the removal broke, and the stored references that were
    // unresolved and now name a changed or new symbol or path, so a call into an
    // unchanged file resolves and an edge from an unchanged file is re-resolved, not
    // dropped.
    let fresh_ids: BTreeSet<String> = results
        .iter()
        .flat_map(|result| result.nodes.iter().map(|node| node.node_id.clone()))
        .chain(inputs.iter().map(|(path, _)| format!("file:{path}")))
        .collect();
    let alive = |id: &str| !removed.contains(id) || fresh_ids.contains(id);
    // A similarity edge that touches a changed node is recomputed, not carried over.
    let cross: Vec<&StoredEdge> = stored
        .edges
        .iter()
        .filter(|edge| edge.edge_type != "similar_to")
        .filter(|edge| removed.contains(&edge.source) != removed.contains(&edge.target))
        .collect();
    // Stored callers and importers: the node and what is fed to the resolver for it.
    let mut callers: BTreeMap<String, (StoredNode, BTreeSet<String>)> = BTreeMap::new();
    let mut importers: BTreeMap<String, (StoredNode, BTreeSet<String>)> = BTreeMap::new();
    let mut context: BTreeMap<String, StoredNode> = BTreeMap::new();
    for edge in &cross {
        let stored_source = !removed.contains(&edge.source);
        let outer = if stored_source {
            &edge.source
        } else {
            &edge.target
        };
        let Some(node) = stored.neighbours.get(outer) else {
            continue;
        };
        match (edge.edge_type.as_str(), stored_source) {
            ("calls", true) => {
                let fed = callers
                    .entry(outer.clone())
                    .or_insert_with(|| (node.clone(), BTreeSet::new()));
                // The stored symbol keeps the callee name only: a call whose target is
                // gone is fed back as that name, so it can bind again or stay unresolved.
                if !alive(&edge.target) {
                    if let Some(name) = edge.properties.get("name") {
                        fed.1.insert(format!(":{name}:"));
                    }
                }
            }
            ("depends_on", true) => {
                let fed = importers
                    .entry(outer.clone())
                    .or_insert_with(|| (node.clone(), BTreeSet::new()));
                if let Some(module) = edge.properties.get("module") {
                    fed.1.insert(module.clone());
                }
            }
            _ => {
                context.insert(outer.clone(), node.clone());
            }
        }
    }
    // The symbols the changed files define and the paths they add decide which stored
    // unresolved references can bind now.
    let defined: BTreeSet<String> = results
        .iter()
        .flat_map(|result| result.nodes.iter())
        .filter(|node| node.node_type == "SYMBOL")
        .filter_map(|node| node.properties.get("name").cloned())
        .collect();
    if !defined.is_empty() {
        for node in lookup.unresolved_calls()? {
            let names = node
                .properties
                .get(UNRESOLVED_CALLS)
                .map(|sites| site_names(sites))
                .unwrap_or_default();
            if !removed.contains(&node.id) && names.iter().any(|name| defined.contains(name)) {
                callers
                    .entry(node.id.clone())
                    .or_insert_with(|| (node, BTreeSet::new()));
            }
        }
    }
    let new_paths = reparsed.iter().any(|path| {
        let file_id = format!("file:{path}");
        !stored
            .nodes_by_path
            .get(path)
            .is_some_and(|ids| ids.contains(&file_id))
    });
    if new_paths {
        for node in lookup.unresolved_imports()? {
            if !removed.contains(&node.id) {
                importers
                    .entry(node.id.clone())
                    .or_insert_with(|| (node, BTreeSet::new()));
            }
        }
    }
    for id in callers.keys().chain(importers.keys()) {
        context.remove(id);
    }
    // The stored symbols that collide with a changed symbol's signature bands.
    for node in results.iter().flat_map(|result| result.nodes.iter()) {
        let Some(bands) = similarity_bands(node) else {
            continue;
        };
        for candidate in lookup.similar_candidates(&bands)? {
            if !removed.contains(&candidate.id)
                && !callers.contains_key(&candidate.id)
                && !importers.contains_key(&candidate.id)
            {
                context.entry(candidate.id.clone()).or_insert(candidate);
            }
        }
    }
    let mut asked: BTreeSet<String> = BTreeSet::new();
    let mut pending: Vec<ExtractedNode> = results
        .iter()
        .flat_map(|result| result.nodes.iter().map(clone_node))
        .chain(context.values().map(|node| inert(extracted(node))))
        .chain(
            callers
                .values()
                .map(|(node, fed)| caller_node(node, fed)),
        )
        .collect();
    while !pending.is_empty() {
        let wanted: Vec<String> = resolution_names(&pending)
            .into_iter()
            .filter(|name| !asked.contains(name))
            .collect();
        pending = Vec::new();
        for name in wanted {
            for definition in lookup.definitions(&name)? {
                if !removed.contains(&definition.id)
                    && !context.contains_key(&definition.id)
                    && !callers.contains_key(&definition.id)
                {
                    pending.push(inert(extracted(&definition)));
                    context.insert(definition.id.clone(), definition);
                }
            }
            asked.insert(name);
        }
    }
    let raw_imports: Vec<ExtractedEdge> = importers
        .iter()
        .flat_map(|(id, (node, fed))| {
            let stored_modules = node
                .properties
                .get(UNRESOLVED_IMPORTS)
                .map(|modules| split_list(modules))
                .unwrap_or_default();
            stored_modules
                .into_iter()
                .chain(fed.iter().cloned())
                .collect::<BTreeSet<String>>()
                .into_iter()
                .map(|module| ExtractedEdge {
                    source: id.clone(),
                    target: module,
                    edge_type: "depends_on_raw".to_string(),
                    properties: Default::default(),
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let mut all_inputs = inputs.clone();
    let reparsed_paths: BTreeSet<&str> = inputs.iter().map(|(path, _)| path.as_str()).collect();
    let deleted_paths: BTreeSet<&str> = deleted.iter().map(String::as_str).collect();
    for path in lookup.file_paths()? {
        if !reparsed_paths.contains(path.as_str()) && !deleted_paths.contains(path.as_str()) {
            all_inputs.push((path, Vec::new()));
        }
    }
    let mut all_results = results;
    all_results.push(ParseResult {
        nodes: context
            .values()
            .filter(|node| node.properties.get("type").map(String::as_str) == Some("SYMBOL"))
            .map(|node| inert(extracted(node)))
            .chain(
                callers
                    .values()
                    .map(|(node, fed)| caller_node(node, fed)),
            )
            .collect(),
        edges: raw_imports,
        symbols_extracted: 0,
    });
    let mut resolved = resolve(&all_inputs, &all_results);
    let unresolved = unresolved_imports(&all_inputs, &all_results);
    // A stored caller or importer that held an unresolved reference is written again
    // with what is still unresolved.
    let mut rewritten: Vec<ExtractedNode> = Vec::new();
    for (id, (node, _)) in &callers {
        if let Some(now) = resolved.nodes.iter().find(|written| &written.node_id == id) {
            rewritten.extend(with_unresolved(
                node,
                [UNRESOLVED_CALLS, HAS_UNRESOLVED_CALLS],
                &now.properties,
            ));
        }
    }
    for (id, (node, _)) in &importers {
        let path = id.strip_prefix("file:").unwrap_or(id);
        let now = file_node(path, unresolved.get(path));
        rewritten.extend(with_unresolved(
            node,
            [UNRESOLVED_IMPORTS, HAS_UNRESOLVED_IMPORTS],
            &now.properties,
        ));
    }
    // Only the changed files' nodes are new.
    resolved
        .nodes
        .retain(|node| fresh_ids.contains(&node.node_id));
    resolved.nodes.extend(
        inputs
            .iter()
            .map(|(path, _)| file_node(path, unresolved.get(path))),
    );

    let new_ids: BTreeSet<String> = resolved
        .nodes
        .iter()
        .map(|node| node.node_id.clone())
        .collect();
    // Only an edge that touches a re-indexed node is written, and the resolved edges
    // of a stored caller or importer: the edges among the stored nodes stay as they are.
    resolved.edges.retain(|edge| {
        new_ids.contains(&edge.source)
            || new_ids.contains(&edge.target)
            || (matches!(edge.edge_type.as_str(), "calls" | "depends_on")
                && (callers.contains_key(&edge.source) || importers.contains_key(&edge.source)))
    });
    let new_symbols: BTreeSet<String> = resolved
        .nodes
        .iter()
        .filter(|node| node.node_type == "SYMBOL")
        .map(|node| node.node_id.clone())
        .collect();
    resolved.nodes.extend(rewritten);

    // A stored edge between a removed node and a node that survives is written again.
    // From a changed caller the resolver's edge is the answer and a stored edge it did
    // not write is kept as it was. From a stored caller the stored edge wins: the
    // stored symbol holds the callee names but not the receivers or argument counts,
    // so the resolver binds it on less than the original parse saw and its strategy
    // and confidence are the weaker reading.
    for edge in &cross {
        if !alive(&edge.source) || !alive(&edge.target) {
            continue;
        }
        let stored_caller = !removed.contains(&edge.source);
        let same = |written: &ExtractedEdge| {
            written.source == edge.source
                && written.target == edge.target
                && written.edge_type == edge.edge_type
        };
        let written = resolved.edges.iter().any(same);
        if written && !stored_caller {
            continue;
        }
        resolved.edges.retain(|written| !same(written));
        resolved.edges.push(ExtractedEdge {
            source: edge.source.clone(),
            target: edge.target.clone(),
            edge_type: edge.edge_type.clone(),
            properties: edge
                .properties
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        });
    }
    let new_calls: BTreeSet<(String, String)> = resolved
        .edges
        .iter()
        .filter(|edge| edge.edge_type == "calls")
        .map(|edge| (edge.source.clone(), edge.target.clone()))
        .collect();
    let mut post_calls: BTreeSet<(String, String)> = new_calls.clone();
    for (source, target) in &stored.calls {
        if alive(source) && alive(target) {
            post_calls.insert((source.clone(), target.clone()));
        }
    }

    // (4) Communities on the affected subgraph only. The pre-state components that
    // hold a removed node, and the post-state components that hold a new symbol.
    let pre_component = component(
        removed.iter().cloned(),
        stored.calls.iter().map(|(a, b)| (a.as_str(), b.as_str())),
    );
    let seeds = pre_component
        .iter()
        .filter(|id| alive(id.as_str()))
        .cloned()
        .chain(new_symbols.iter().cloned());
    let affected = component(
        seeds,
        post_calls.iter().map(|(a, b)| (a.as_str(), b.as_str())),
    );
    let symbols: BTreeSet<String> = affected
        .iter()
        .filter(|id| alive(id.as_str()))
        .cloned()
        .collect();
    let calls_in: Vec<(String, String)> = post_calls
        .iter()
        .filter(|(source, target)| symbols.contains(source) && symbols.contains(target))
        .cloned()
        .collect();
    let assignment = communities(&symbols, &calls_in, &changeset.id)?;
    let stale_labels: BTreeSet<&String> = pre_component
        .iter()
        .chain(&affected)
        .filter_map(|id| stored.communities.get(id))
        .collect();
    for label in stale_labels {
        methods.push(Method::RemoveNode {
            node_id: format!("feature:{label}"),
        });
    }
    let written: BTreeSet<&String> = assignment.values().collect();
    let communities_recomputed = written.len();

    // (5) The next generation advances for every accepted changeset.
    let generation = next_generation(stored.generation.as_ref(), commit_id);
    resolved.nodes.push(generation_node(&changeset.repo, &generation));
    let lowered: IndexResult = resolved;
    methods.extend(lower(&lowered)?.methods);
    for print in &prints {
        methods.push(fingerprint_method(&changeset.repo, print)?);
    }
    methods.extend(feature_methods(&assignment)?);

    Ok(ReindexOutcome {
        write_set: IndexWriteSet::from_methods(
            ENVELOPE_PREFIX,
            b"eg/repository-changeset-batch",
            methods,
        )?,
        generation,
        reparsed,
        unchanged,
        deleted,
        communities_recomputed,
    })
}

fn clone_node(node: &ExtractedNode) -> ExtractedNode {
    ExtractedNode {
        node_id: node.node_id.clone(),
        node_type: node.node_type.clone(),
        properties: node.properties.clone(),
    }
}

fn clone_edge(edge: &ExtractedEdge) -> ExtractedEdge {
    ExtractedEdge {
        source: edge.source.clone(),
        target: edge.target.clone(),
        edge_type: edge.edge_type.clone(),
        properties: edge.properties.clone(),
    }
}

/// The nodes of the connected components (edges taken undirected) that contain
/// any of `seeds`, seeds included.
fn component<'a>(
    seeds: impl Iterator<Item = String>,
    edges: impl Iterator<Item = (&'a str, &'a str)>,
) -> BTreeSet<String> {
    let mut adjacent: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (source, target) in edges {
        adjacent.entry(source).or_default().push(target);
        adjacent.entry(target).or_default().push(source);
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for seed in seeds {
        if seen.insert(seed.clone()) {
            queue.push_back(seed);
        }
    }
    while let Some(node) = queue.pop_front() {
        for next in adjacent.get(node.as_str()).into_iter().flatten() {
            if seen.insert((*next).to_string()) {
                queue.push_back((*next).to_string());
            }
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_source_types::source_changeset::{ChangedPath, Trailers};
    use std::collections::VecDeque;

    fn changed(path: &str, op: PathOp) -> ChangedPath {
        ChangedPath {
            path: path.into(),
            op,
            old_blob: None,
            new_blob: None,
            mode: 0o100644,
        }
    }

    fn changeset(id: &str, parent: Option<&str>, paths: Vec<ChangedPath>) -> Changeset {
        Changeset {
            id: id.into(),
            repo: "repo".into(),
            parents: parent.map(str::to_string).into_iter().collect(),
            author_principal: "alice".into(),
            message: "change".into(),
            changed_paths: paths,
            created_at: 1,
            trailers: Trailers::default(),
            agent_identity: None,
            run_trace_id: None,
            git: None,
        }
    }

    fn file(path: &str, text: &str) -> ChangesetFile {
        ChangesetFile {
            path: path.into(),
            bytes: text.as_bytes().to_vec(),
        }
    }

    fn removed_ids(outcome: &ReindexOutcome) -> BTreeSet<String> {
        outcome
            .write_set
            .methods
            .iter()
            .filter_map(|method| match method {
                Method::RemoveNode { node_id } => Some(node_id.clone()),
                _ => None,
            })
            .collect()
    }

    /// The stored state a first changeset of three files leaves behind.
    fn stored_after(first: &ChangesetFile, outcome: &ReindexOutcome) -> StoredIndex {
        let mut stored = StoredIndex {
            generation: Some(outcome.generation.clone()),
            ..Default::default()
        };
        let print = {
            let parsed = Parsed {
                path: first.path.clone(),
                result: parse_file(&first.path, &first.bytes),
            };
            print_of(&parsed, &first.bytes).unwrap()
        };
        stored.prints.insert(print.path.clone(), print);
        stored
            .nodes_by_path
            .insert(first.path.clone(), vec![format!("file:{}", first.path)]);
        stored
    }

    // spec: MEDULLA-SCM-R006
    #[test]
    fn only_structural_paths_reparse_and_communities_stay_local() {
        let a = file("a.py", "def alpha():\n    return 1\n");
        let first = changeset("c1", None, vec![changed("a.py", PathOp::Add)]);
        let landed = reindex_changeset(&first, "k1", std::slice::from_ref(&a), &StoredIndex::default(), &NoLookup)
            .unwrap();
        assert_eq!(landed.reparsed, vec!["a.py".to_string()]);
        assert_eq!(landed.generation.map_generation, 1);
        assert_eq!(landed.generation.base_revision, "k1");
        let stored = stored_after(&a, &landed);

        // A whitespace edit keeps the structure: nothing but the generation node is written.
        let spaced = file("a.py", "def alpha():\n\n    return 1\n");
        let cosmetic = changeset("c2", Some("k1"), vec![changed("a.py", PathOp::Modify)]);
        let outcome = reindex_changeset(&cosmetic, "k2", &[spaced], &stored, &NoLookup).unwrap();
        assert!(outcome.reparsed.is_empty());
        assert_eq!(outcome.unchanged, vec!["a.py".to_string()]);
        assert!(removed_ids(&outcome).is_empty());
        assert_eq!(outcome.generation.map_generation, 2);

        // A new signature is structural: the file's stored nodes are removed.
        let renamed = file("a.py", "def beta():\n    return 1\n");
        let structural = changeset("c2", Some("k1"), vec![changed("a.py", PathOp::Modify)]);
        let outcome = reindex_changeset(&structural, "k2", &[renamed], &stored, &NoLookup).unwrap();
        assert_eq!(outcome.reparsed, vec!["a.py".to_string()]);
        assert!(removed_ids(&outcome).contains("file:a.py"));

        // A changeset that does not chain from the stored base is refused.
        let stray = changeset("c3", Some("elsewhere"), vec![changed("a.py", PathOp::Modify)]);
        let refusal = reindex_changeset(&stray, "k3", &[file("a.py", "x = 1\n")], &stored, &NoLookup)
            .err()
            .unwrap();
        assert!(refusal.starts_with("INDEX_BASE_MISMATCH"), "{refusal}");
    }

    /// The stored form of an index of `files`: its nodes with their `type`, its
    /// edges with the properties they carry besides the relationship.
    fn indexed(files: &[(&str, &str)]) -> (Vec<StoredNode>, Vec<StoredEdge>) {
        let owned: Vec<(String, Vec<u8>)> = files
            .iter()
            .map(|(path, text)| (path.to_string(), text.as_bytes().to_vec()))
            .collect();
        let result = medulla_compute::parser::resolve::index_repository(&owned);
        let nodes = result
            .nodes
            .iter()
            .map(|node| {
                let mut properties: BTreeMap<String, String> = node
                    .properties
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                properties.insert("type".to_string(), node.node_type.clone());
                StoredNode {
                    id: node.node_id.clone(),
                    properties,
                }
            })
            .collect();
        let edges = result
            .edges
            .iter()
            .map(|edge| StoredEdge {
                source: edge.source.clone(),
                target: edge.target.clone(),
                edge_type: edge.edge_type.clone(),
                properties: edge
                    .properties
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            })
            .collect();
        (nodes, edges)
    }

    struct Memory {
        nodes: Vec<StoredNode>,
        files: Vec<String>,
    }

    impl StoredLookup for Memory {
        fn definitions(&self, name: &str) -> Result<Vec<StoredNode>, String> {
            Ok(self
                .nodes
                .iter()
                .filter(|node| {
                    node.properties.get("type").map(String::as_str) == Some("SYMBOL")
                        && node.properties.get("name").map(String::as_str) == Some(name)
                })
                .cloned()
                .collect())
        }

        fn file_paths(&self) -> Result<Vec<String>, String> {
            Ok(self.files.clone())
        }

        fn similar_candidates(
            &self,
            bands: &[(String, String)],
        ) -> Result<Vec<StoredNode>, String> {
            Ok(self.marked(|node| {
                bands
                    .iter()
                    .any(|(key, value)| node.properties.get(key) == Some(value))
            }))
        }

        fn unresolved_calls(&self) -> Result<Vec<StoredNode>, String> {
            Ok(self.marked(|node| node.properties.contains_key(HAS_UNRESOLVED_CALLS)))
        }

        fn unresolved_imports(&self) -> Result<Vec<StoredNode>, String> {
            Ok(self.marked(|node| node.properties.contains_key(HAS_UNRESOLVED_IMPORTS)))
        }
    }

    impl Memory {
        fn marked(&self, keep: impl Fn(&StoredNode) -> bool) -> Vec<StoredNode> {
            self.nodes.iter().filter(|node| keep(node)).cloned().collect()
        }
    }

    fn written_edges(outcome: &ReindexOutcome) -> Vec<(String, String, BTreeMap<String, String>)> {
        outcome
            .write_set
            .methods
            .iter()
            .filter_map(|method| match method {
                Method::AddEdge {
                    source_id,
                    target_id,
                    properties_msgpack,
                } => Some((
                    source_id.clone(),
                    target_id.clone(),
                    rmp_serde::from_slice(properties_msgpack).unwrap(),
                )),
                _ => None,
            })
            .collect()
    }

    fn symbol_id(nodes: &[StoredNode], name: &str) -> String {
        nodes
            .iter()
            .find(|node| node.properties.get("name").map(String::as_str) == Some(name))
            .map(|node| node.id.clone())
            .unwrap()
    }

    // spec: MEDULLA-SCM-R006
    #[test]
    fn a_full_index_baseline_accepts_the_next_changeset() {
        let mut result = IndexResult::default();
        let stamped = stamp_generation(&mut result, "repo", None, "k1");
        assert_eq!(stamped.map_generation, 1);
        let node = result
            .nodes
            .iter()
            .find(|node| node.node_id == generation_node_id("repo"))
            .expect("the full index writes the generation node");
        assert_eq!(node.node_type, GENERATION_TYPE);
        assert_eq!(node.properties.get("base_revision").map(String::as_str), Some("k1"));

        // A changeset whose parent is the baseline is accepted, not refused.
        let stored = StoredIndex {
            generation: Some(stamped),
            ..Default::default()
        };
        let next = changeset("c2", Some("k1"), vec![changed("a.py", PathOp::Add)]);
        let outcome = reindex_changeset(
            &next,
            "k2",
            &[file("a.py", "def alpha():\n    return 1\n")],
            &stored,
            &NoLookup,
        )
        .unwrap();
        assert_eq!(outcome.generation.map_generation, 2);

        // A second full index advances the stored generation.
        let again = stamp_generation(&mut result, "repo", Some(&outcome.generation), "k3");
        assert_eq!(again.map_generation, 3);
    }

    // spec: MEDULLA-SCM-R006
    #[test]
    fn a_new_call_into_an_unchanged_file_resolves_with_its_properties() {
        let (nodes, _) = indexed(&[("b.py", "def helper():\n    return 1\n")]);
        let helper = symbol_id(&nodes, "helper");
        let lookup = Memory {
            nodes,
            files: vec!["b.py".to_string()],
        };
        let first = changeset("c1", None, vec![changed("a.py", PathOp::Add)]);
        let outcome = reindex_changeset(
            &first,
            "k1",
            &[file("a.py", "def alpha():\n    return helper()\n")],
            &StoredIndex::default(),
            &lookup,
        )
        .unwrap();
        let call = written_edges(&outcome)
            .into_iter()
            .find(|(_, target, properties)| {
                target == &helper
                    && properties.get("relationship").map(String::as_str) == Some("calls")
            })
            .expect("the call into the unchanged file resolves");
        assert!(call.2.contains_key("strategy"), "{:?}", call.2);
        assert!(call.2.contains_key("confidence"), "{:?}", call.2);
        // The unchanged file's symbols are not written again.
        assert!(!outcome
            .write_set
            .methods
            .iter()
            .any(|method| matches!(method, Method::AddNode { node_id, .. } if node_id == &helper)));
    }

    // spec: MEDULLA-SCM-R006
    #[test]
    fn edges_from_an_unchanged_file_survive_a_removal_with_their_properties() {
        let before = [
            ("a.py", "def alpha():\n    return 1\n"),
            ("b.py", "def use():\n    return alpha()\n\nclass Child(Base):\n    pass\n"),
        ];
        let (nodes, edges) = indexed(&before);
        let alpha = symbol_id(&nodes, "alpha");
        let user = symbol_id(&nodes, "use");
        let original = edges
            .iter()
            .find(|edge| edge.source == user && edge.target == alpha && edge.edge_type == "calls")
            .expect("use calls alpha")
            .clone();

        // The stored state of a.py's nodes and every edge that touches them.
        let in_a: Vec<String> = nodes
            .iter()
            .filter(|node| node.properties.get("file_path").map(String::as_str) == Some("a.py"))
            .map(|node| node.id.clone())
            .collect();
        let touching: Vec<StoredEdge> = edges
            .iter()
            .filter(|edge| in_a.contains(&edge.source) || in_a.contains(&edge.target))
            .cloned()
            .collect();
        let mut stored = StoredIndex {
            generation: Some(RepositoryGeneration {
                map_generation: 1,
                base_revision: "k1".into(),
                complete: true,
            }),
            edges: touching,
            ..Default::default()
        };
        stored.nodes_by_path.insert("a.py".into(), in_a.clone());
        stored.calls.push((user.clone(), alpha.clone()));
        stored.neighbours.insert(
            user.clone(),
            nodes.iter().find(|node| node.id == user).unwrap().clone(),
        );
        let a_file = file("a.py", before[0].1);
        stored.prints.insert(
            Id::new("a.py".to_string()).unwrap(),
            print_of(
                &Parsed {
                    path: "a.py".into(),
                    result: parse_file("a.py", &a_file.bytes),
                },
                &a_file.bytes,
            )
            .unwrap(),
        );
        let lookup = Memory {
            nodes: nodes.clone(),
            files: vec!["a.py".into(), "b.py".into()],
        };

        // a.py gains a function: structural, so its nodes are removed and written again.
        let modify = changeset("c2", Some("k1"), vec![changed("a.py", PathOp::Modify)]);
        let outcome = reindex_changeset(
            &modify,
            "k2",
            &[file("a.py", "def alpha():\n    return 1\n\ndef extra():\n    return 2\n")],
            &stored,
            &lookup,
        )
        .unwrap();
        assert!(removed_ids(&outcome).contains(&alpha));
        let kept = written_edges(&outcome)
            .into_iter()
            .find(|(source, target, properties)| {
                source == &user
                    && target == &alpha
                    && properties.get("relationship").map(String::as_str) == Some("calls")
            })
            .expect("the call from the unchanged file is written again, not dropped");
        for (key, value) in &original.properties {
            assert_eq!(kept.2.get(key), Some(value), "property {key}");
        }
    }

    /// The graph a write-set leaves behind: nodes by id and edges by
    /// (source, target, relationship), each with its text properties.
    #[derive(Default, Clone)]
    struct Graph {
        nodes: BTreeMap<String, BTreeMap<String, String>>,
        edges: BTreeMap<(String, String, String), BTreeMap<String, String>>,
    }

    impl Graph {
        fn apply(&mut self, outcome: &ReindexOutcome) {
            for method in &outcome.write_set.methods {
                match method {
                    Method::AddNode {
                        node_id,
                        properties_msgpack,
                    } => {
                        let properties: BTreeMap<String, String> =
                            rmp_serde::from_slice(properties_msgpack).unwrap();
                        self.nodes.insert(node_id.clone(), properties);
                    }
                    Method::AddEdge {
                        source_id,
                        target_id,
                        properties_msgpack,
                    } => {
                        let mut properties: BTreeMap<String, String> =
                            rmp_serde::from_slice(properties_msgpack).unwrap();
                        let relationship = properties.remove("relationship").unwrap();
                        self.edges.insert(
                            (source_id.clone(), target_id.clone(), relationship),
                            properties,
                        );
                    }
                    Method::RemoveNode { node_id } => {
                        self.nodes.remove(node_id);
                        self.edges
                            .retain(|(source, target, _), _| source != node_id && target != node_id);
                    }
                    other => panic!("a re-index write-set holds no {other:?}"),
                }
            }
        }

        fn node(&self, id: &str) -> StoredNode {
            StoredNode {
                id: id.to_string(),
                properties: self.nodes[id].clone(),
            }
        }

        fn marked(&self, keep: impl Fn(&BTreeMap<String, String>) -> bool) -> Vec<StoredNode> {
            self.nodes
                .iter()
                .filter(|(_, properties)| keep(properties))
                .map(|(id, _)| self.node(id))
                .collect()
        }

        /// The stored index of `repo` a changeset reads: the same reads the router makes.
        fn stored_index(&self, changeset: &Changeset) -> StoredIndex {
            let paths: BTreeSet<&str> = changeset
                .changed_paths
                .iter()
                .map(|changed| changed.path.as_str())
                .collect();
            let generation = self.nodes.get(&generation_node_id("repo")).map(|properties| {
                stored_generation(&serde_json::to_value(properties).unwrap())
            });
            let mut stored = StoredIndex {
                generation,
                ..Default::default()
            };
            for path in &paths {
                let id = fingerprint_node_id("repo", path).unwrap();
                if let Some(print) = self
                    .nodes
                    .get(&id)
                    .and_then(|properties| {
                        super::enrich::fingerprint::stored_fingerprint(
                            &serde_json::to_value(properties).unwrap(),
                        )
                    })
                {
                    stored.prints.insert(print.path.clone(), print);
                }
                let ids: Vec<String> = self
                    .nodes
                    .iter()
                    .filter(|(_, properties)| {
                        properties.get("file_path").map(String::as_str) == Some(*path)
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                if !ids.is_empty() {
                    stored.nodes_by_path.insert(path.to_string(), ids);
                }
            }
            let held: BTreeSet<&String> = stored.nodes_by_path.values().flatten().collect();
            for ((source, target, relationship), properties) in &self.edges {
                if held.contains(source) || held.contains(target) {
                    stored.edges.push(StoredEdge {
                        source: source.clone(),
                        target: target.clone(),
                        edge_type: relationship.clone(),
                        properties: properties.clone(),
                    });
                    for far in [source, target] {
                        if !held.contains(far) {
                            stored.neighbours.insert(far.clone(), self.node(far));
                        }
                    }
                }
            }
            let mut component: BTreeSet<String> = held.iter().map(|id| (*id).clone()).collect();
            let mut queue: VecDeque<String> = component.iter().cloned().collect();
            let mut calls = BTreeSet::new();
            while let Some(node) = queue.pop_front() {
                for ((source, target, relationship), _) in &self.edges {
                    if relationship == "calls" && (source == &node || target == &node) {
                        calls.insert((source.clone(), target.clone()));
                        let far = if source == &node { target } else { source };
                        if component.insert(far.clone()) {
                            queue.push_back(far.clone());
                        }
                    }
                    if relationship == "memberOfFeature" && source == &node {
                        if let Some(label) = target.strip_prefix("feature:") {
                            stored.communities.insert(node.clone(), label.to_string());
                        }
                    }
                }
            }
            stored.calls = calls.into_iter().collect();
            stored
        }

        /// The code graph: everything but the generation record and the communities,
        /// which the changeset recomputes on the affected subgraph only.
        fn code(&self) -> (BTreeMap<String, BTreeMap<String, String>>, BTreeMap<(String, String, String), BTreeMap<String, String>>) {
            let nodes = self
                .nodes
                .iter()
                .filter(|(id, _)| {
                    **id != generation_node_id("repo") && !id.starts_with("feature:")
                })
                .map(|(id, properties)| (id.clone(), properties.clone()))
                .collect();
            let edges = self
                .edges
                .iter()
                .filter(|((_, _, relationship), _)| relationship != "memberOfFeature")
                .map(|(key, properties)| (key.clone(), properties.clone()))
                .collect();
            (nodes, edges)
        }
    }

    impl StoredLookup for Graph {
        fn definitions(&self, name: &str) -> Result<Vec<StoredNode>, String> {
            Ok(self.marked(|properties| {
                properties.get("type").map(String::as_str) == Some("SYMBOL")
                    && properties.get("name").map(String::as_str) == Some(name)
            }))
        }

        fn file_paths(&self) -> Result<Vec<String>, String> {
            Ok(self
                .nodes
                .iter()
                .filter(|(_, properties)| {
                    properties.get("type").map(String::as_str) == Some("FILE")
                })
                .filter_map(|(id, _)| id.strip_prefix("file:").map(str::to_string))
                .collect())
        }

        fn similar_candidates(&self, bands: &[(String, String)]) -> Result<Vec<StoredNode>, String> {
            Ok(self.marked(|properties| {
                properties.get("type").map(String::as_str) == Some("SYMBOL")
                    && bands
                        .iter()
                        .any(|(key, value)| properties.get(key) == Some(value))
            }))
        }

        fn unresolved_calls(&self) -> Result<Vec<StoredNode>, String> {
            Ok(self.marked(|properties| properties.contains_key(HAS_UNRESOLVED_CALLS)))
        }

        fn unresolved_imports(&self) -> Result<Vec<StoredNode>, String> {
            Ok(self.marked(|properties| properties.contains_key(HAS_UNRESOLVED_IMPORTS)))
        }
    }

    /// Land one changeset on `graph` the way the router does: read the stored index,
    /// re-index, write.
    fn land(graph: &mut Graph, id: &str, parent: Option<&str>, commit: &str, steps: &[(&str, PathOp, Option<&str>)]) {
        let paths = steps
            .iter()
            .map(|(path, op, _)| changed(path, *op))
            .collect();
        let changeset = changeset(id, parent, paths);
        let files: Vec<ChangesetFile> = steps
            .iter()
            .filter_map(|(path, _, text)| text.map(|text| file(path, text)))
            .collect();
        let stored = graph.stored_index(&changeset);
        let outcome = reindex_changeset(&changeset, commit, &files, &stored, &*graph).unwrap();
        graph.apply(&outcome);
    }

    // spec: MEDULLA-SCM-R006
    #[test]
    fn a_changeset_reindex_equals_a_full_reindex_of_the_resulting_tree() {
        let alpha = "def alpha(x):\n    total = 0\n    for i in x:\n        total = total + i\n    return total\n";
        let beta = "def beta(y):\n    acc = 0\n    for j in y:\n        acc = acc + j\n    return acc\n";
        let util = format!("def helper():\n    return 1\n\n{alpha}");
        let util_extended = format!("{util}\ndef extra():\n    return 3\n");
        let main = "import lib\n\ndef run():\n    helper()\n    later()\n    return external()\n";
        let lib = format!("def later():\n    return 2\n\n{beta}");
        let solo = "def solo():\n    return 4\n";

        // The tree grows one changeset at a time, each on the graph the one before left.
        let mut incremental = Graph::default();
        land(
            &mut incremental,
            "c1",
            None,
            "k1",
            &[
                ("util.py", PathOp::Add, Some(util.as_str())),
                ("main.py", PathOp::Add, Some(main)),
            ],
        );
        // lib.py adds the definition of a call and the file of an import that were
        // unresolved, and a clone of a symbol of util.py.
        land(
            &mut incremental,
            "c2",
            Some("k1"),
            "k2",
            &[("lib.py", PathOp::Add, Some(lib.as_str()))],
        );
        // util.py changes structurally while main.py, which calls into it, does not.
        land(
            &mut incremental,
            "c3",
            Some("k2"),
            "k3",
            &[
                ("util.py", PathOp::Modify, Some(util_extended.as_str())),
                ("solo.py", PathOp::Add, Some(solo)),
            ],
        );
        land(
            &mut incremental,
            "c4",
            Some("k3"),
            "k4",
            &[("solo.py", PathOp::Delete, None)],
        );

        // A full index of the resulting tree.
        let mut full = Graph::default();
        land(
            &mut full,
            "full",
            None,
            "k4",
            &[
                ("util.py", PathOp::Add, Some(util_extended.as_str())),
                ("main.py", PathOp::Add, Some(main)),
                ("lib.py", PathOp::Add, Some(lib.as_str())),
            ],
        );

        let (inc_nodes, inc_edges) = incremental.code();
        let (full_nodes, full_edges) = full.code();
        assert_eq!(inc_nodes, full_nodes, "the node sets differ");
        assert_eq!(inc_edges, full_edges, "the edge sets differ");
        // The scenario reached what it is meant to: the call and the import that were
        // unresolved bound, the clone was linked, and the removed file left nothing.
        let relationships: BTreeSet<&str> = full_edges
            .keys()
            .map(|(_, _, relationship)| relationship.as_str())
            .collect();
        assert!(relationships.contains("calls"));
        assert!(relationships.contains("depends_on"));
        assert!(relationships.contains("similar_to"));
        assert!(!incremental.nodes.contains_key("file:solo.py"));
        assert_eq!(
            incremental.nodes["file:main.py"].get(UNRESOLVED_IMPORTS),
            None,
            "the import that bound is no longer unresolved"
        );
    }
}
