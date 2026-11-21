//! The code graph follows an accepted changeset (MEDULLA-SCM-R006).
//!
//! `reindex_changeset` decides, from the stored index state of the repository
//! graph, which files of one changeset re-enter the pipeline: only structurally
//! changed ones. It returns the write-set that replaces exactly those files'
//! nodes (through the one `lower` of `repository_index.rs`), the communities of
//! the affected call subgraph, the new fingerprints and the next generation. It
//! never reads or writes a symbol outside the affected set.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use medulla_compute::parser::resolve::{file_node, resolve};
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
        methods.push(Method::RemoveNode {
            node_id: fingerprint_node_id(&changeset.repo, path)?,
        });
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
    let mut resolved = resolve(&inputs, &results);
    resolved
        .nodes
        .extend(inputs.iter().map(|(path, _)| file_node(path)));

    let new_ids: BTreeSet<String> = resolved
        .nodes
        .iter()
        .map(|node| node.node_id.clone())
        .collect();
    let new_symbols: BTreeSet<String> = resolved
        .nodes
        .iter()
        .filter(|node| node.node_type == "SYMBOL")
        .map(|node| node.node_id.clone())
        .collect();
    let new_calls: BTreeSet<(String, String)> = resolved
        .edges
        .iter()
        .filter(|edge| edge.edge_type == "calls")
        .map(|edge| (edge.source.clone(), edge.target.clone()))
        .collect();
    let alive = |id: &str| !removed.contains(id) || new_ids.contains(id);

    // A removed node takes its edges with it: a stored call between two nodes
    // that both survive is written again.
    let mut post_calls: BTreeSet<(String, String)> = new_calls.clone();
    for (source, target) in &stored.calls {
        if !alive(source) || !alive(target) {
            continue;
        }
        if (removed.contains(source) || removed.contains(target))
            && !new_calls.contains(&(source.clone(), target.clone()))
        {
            resolved.edges.push(ExtractedEdge {
                source: source.clone(),
                target: target.clone(),
                edge_type: "calls".to_string(),
                properties: Default::default(),
            });
        }
        post_calls.insert((source.clone(), target.clone()));
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
    let generation = RepositoryGeneration {
        map_generation: stored
            .generation
            .as_ref()
            .map_or(1, |generation| generation.map_generation + 1),
        base_revision: commit_id.to_string(),
        complete: true,
    };
    resolved.nodes.push(ExtractedNode {
        node_id: generation_node_id(&changeset.repo),
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
    });
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
        let landed = reindex_changeset(&first, "k1", std::slice::from_ref(&a), &StoredIndex::default())
            .unwrap();
        assert_eq!(landed.reparsed, vec!["a.py".to_string()]);
        assert_eq!(landed.generation.map_generation, 1);
        assert_eq!(landed.generation.base_revision, "k1");
        let stored = stored_after(&a, &landed);

        // A whitespace edit keeps the structure: nothing but the generation node is written.
        let spaced = file("a.py", "def alpha():\n\n    return 1\n");
        let cosmetic = changeset("c2", Some("k1"), vec![changed("a.py", PathOp::Modify)]);
        let outcome = reindex_changeset(&cosmetic, "k2", &[spaced], &stored).unwrap();
        assert!(outcome.reparsed.is_empty());
        assert_eq!(outcome.unchanged, vec!["a.py".to_string()]);
        assert!(removed_ids(&outcome).is_empty());
        assert_eq!(outcome.generation.map_generation, 2);

        // A new signature is structural: the file's stored nodes are removed.
        let renamed = file("a.py", "def beta():\n    return 1\n");
        let structural = changeset("c2", Some("k1"), vec![changed("a.py", PathOp::Modify)]);
        let outcome = reindex_changeset(&structural, "k2", &[renamed], &stored).unwrap();
        assert_eq!(outcome.reparsed, vec!["a.py".to_string()]);
        assert!(removed_ids(&outcome).contains("file:a.py"));

        // A changeset that does not chain from the stored base is refused.
        let stray = changeset("c3", Some("elsewhere"), vec![changed("a.py", PathOp::Modify)]);
        let refusal = reindex_changeset(&stray, "k3", &[file("a.py", "x = 1\n")], &stored)
            .err()
            .unwrap();
        assert!(refusal.starts_with("INDEX_BASE_MISMATCH"), "{refusal}");
    }
}
