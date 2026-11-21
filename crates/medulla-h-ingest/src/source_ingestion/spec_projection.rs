//! Spec rows as requirement records (MEDULLA-SDLC-R012.2).
//!
//! `project_requirements` turns the parser output of `parse_spec_dir`
//! (MEDULLA-SDLC-R014.1) into one requirement node per requirements row, with its
//! ID, owner repository, title and derived state (MEDULLA-SDLC-R005). The
//! `Caller:` chain of a row becomes `implementedBy` edges to code symbols, its
//! `Depends:` IDs become `dependsOn` edges and its proving test becomes a
//! `provedBy` edge to the test symbol. An ID that resolves to no row refuses with
//! `REQUIREMENT_UNRESOLVED` and nothing is written. `commit_requirements` writes the
//! projection as one `BatchUpdate` through `commit_gateway`.

use std::collections::{BTreeMap, BTreeSet};

use medulla_catalog_types::ontology_inspection::{RdfObject, RdfTriple};
use medulla_compute::parser::spec_artifacts::{is_row_id, parse_spec_file, SDD_NS};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_server_mutation::mutation::{MutationCtx, MutationPlan};
use medulla_types::contract::EngineErrorCode;

/// Refusal code of a row ID that resolves to no row.
pub const REQUIREMENT_UNRESOLVED: &str = "REQUIREMENT_UNRESOLVED";

/// State of a row that the caller has not derived a state for.
const DEFAULT_STATE: &str = "SPECIFIED";

/// The graph write of one projection: `upsert_node` and `upsert_edge` operations.
#[derive(Clone, Debug, PartialEq)]
pub struct RequirementProjection {
    pub operations: Vec<serde_json::Value>,
    pub nodes: u32,
    pub edges: u32,
}

/// The node id of one requirement of one repository.
pub fn requirement_node_id(repo: &str, row_id: &str) -> String {
    format!("requirement:{repo}:{row_id}")
}

fn object_text(object: &RdfObject) -> &str {
    match object {
        RdfObject::Iri { iri } => iri,
        RdfObject::Literal { lexical, .. } => lexical,
    }
}

fn property<'a>(triples: &'a [RdfTriple], subject: &str, name: &str) -> Option<&'a str> {
    let predicate = format!("{SDD_NS}{name}");
    triples
        .iter()
        .find(|t| t.subject == subject && t.predicate == predicate)
        .map(|t| object_text(&t.object))
}

/// Backticked spans of `text`, in order.
fn ticked(text: &str) -> Vec<&str> {
    text.split('`').skip(1).step_by(2).collect()
}

/// The text of `body` after the last occurrence of any marker, up to the first stop marker.
fn section<'a>(body: &'a str, markers: &[&str], stops: &[&str]) -> Option<&'a str> {
    let (start, len) = markers
        .iter()
        .filter_map(|m| body.rfind(m).map(|at| (at, m.len())))
        .max_by_key(|(at, _)| *at)?;
    let rest = &body[start + len..];
    let end = stops
        .iter()
        .filter_map(|s| rest.find(s))
        .min()
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

/// `crates/a/b.rs:142` names the symbol `crates/a/b.rs`.
fn symbol_name(span: &str) -> &str {
    match span.rsplit_once(':') {
        Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => span,
    }
}

fn caller_symbols(body: &str) -> BTreeSet<String> {
    let stops = ["Shape:", "Depends:", "Depends on:", "Budget:", "Bench:"];
    section(body, &["Caller chain:", "Caller:"], &stops)
        .map(|chain| {
            ticked(chain)
                .into_iter()
                .map(symbol_name)
                .filter(|s| !s.is_empty() && !s.contains(' ') && !is_row_id(s))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The row IDs named after `Depends:` or `Depends on:`; a short `R014.5` takes the prefix of `own`.
fn dependencies(body: &str, own: &str) -> BTreeSet<String> {
    let prefix = own.rsplit_once("-R").map(|(prefix, _)| prefix);
    let Some(text) = section(body, &["Depends on:", "Depends:"], &[]) else {
        return BTreeSet::new();
    };
    text.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';'))
        .map(|t| t.trim_matches(|c: char| matches!(c, '`' | '.' | '(' | ')')))
        .filter_map(|token| {
            if is_row_id(token) {
                return Some(token.to_string());
            }
            let short = token.strip_prefix('R').filter(|n| {
                !n.is_empty()
                    && n.split('.')
                        .all(|p| p.bytes().all(|b| b.is_ascii_digit()) && !p.is_empty())
            });
            short
                .zip(prefix)
                .map(|(_, prefix)| format!("{prefix}-{token}"))
        })
        .collect()
}

/// The proving tests of a row: backticked `path::name` spans of the acceptance cell.
fn proving_tests(acceptance: &str) -> BTreeSet<String> {
    ticked(acceptance)
        .into_iter()
        .filter(|s| s.contains("::"))
        .map(str::to_string)
        .collect()
}

fn node(id: &str, properties: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"op": "upsert_node", "id": id, "properties": properties})
}

fn edge(source: &str, target: &str, relationship: &str) -> serde_json::Value {
    serde_json::json!({
        "op": "upsert_edge",
        "source": source,
        "target": target,
        "properties": {"relationship": relationship},
    })
}

/// Project the requirement rows of `triples` (parser output of one repository `repo`).
///
/// `states` maps a row ID to its derived state (MEDULLA-SDLC-R005); a row without one is
/// `SPECIFIED`. `external` maps the IDs of rows declared outside `triples` to their node
/// ids. A `Depends:` ID found in neither refuses with `REQUIREMENT_UNRESOLVED` naming the
/// ID and the row that cites it, and no operation is produced.
pub fn project_requirements(
    repo: &str,
    triples: &[RdfTriple],
    states: &BTreeMap<String, String>,
    external: &BTreeMap<String, String>,
) -> Result<RequirementProjection, String> {
    project_requirements_in(repo, triples, states, external, Unresolved::Refuse)
}

/// What to do with a `Depends:` ID that resolves to no row of the projected tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unresolved {
    /// Refuse with `REQUIREMENT_UNRESOLVED` and write nothing.
    Refuse,
    /// Write no `dependsOn` edge for it. The repository index sees one repository's files,
    /// so a row of another repository cannot resolve there.
    Skip,
}

/// [`project_requirements`] with the choice of what an unresolved ID does.
pub fn project_requirements_in(
    repo: &str,
    triples: &[RdfTriple],
    states: &BTreeMap<String, String>,
    external: &BTreeMap<String, String>,
    unresolved: Unresolved,
) -> Result<RequirementProjection, String> {
    let requirement_class = format!("{SDD_NS}Requirement");
    let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    let subjects: BTreeSet<&str> = triples
        .iter()
        .filter(|t| t.predicate == rdf_type && object_text(&t.object) == requirement_class)
        .map(|t| t.subject.as_str())
        .collect();
    let ids: BTreeMap<&str, &str> = subjects
        .iter()
        .filter_map(|s| property(triples, s, "rowId").map(|id| (id, *s)))
        .collect();

    let (mut operations, mut nodes, mut edges) = (Vec::new(), 0u32, 0u32);
    let mut stubs: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for (id, subject) in &ids {
        let node_id = requirement_node_id(repo, id);
        let body = property(triples, subject, "body").unwrap_or_default();
        let state = states.get(*id).map_or(DEFAULT_STATE, String::as_str);
        let mut properties = serde_json::json!({
            "type": "Requirement",
            "rowId": id,
            "repo": repo,
            "title": property(triples, subject, "title").unwrap_or_default(),
            "state": state,
        });
        if let Some(file) = property(triples, subject, "declaredIn") {
            properties["declaredIn"] = file.into();
        }
        operations.push(node(&node_id, properties));
        nodes += 1;

        for dependency in dependencies(body, id) {
            let target = if ids.contains_key(dependency.as_str()) {
                requirement_node_id(repo, &dependency)
            } else if let Some(target) = external.get(&dependency) {
                target.clone()
            } else if unresolved == Unresolved::Skip {
                continue;
            } else {
                return Err(format!(
                    "{}: {dependency} (cited by {id}) resolves to no row",
                    EngineErrorCode::RequirementUnresolved.as_str()
                ));
            };
            operations.push(edge(&node_id, &target, "dependsOn"));
            edges += 1;
        }
        for symbol in caller_symbols(body) {
            let target = format!("symbol:{symbol}");
            stubs.entry(target.clone()).or_insert_with(|| {
                node(
                    &target,
                    serde_json::json!({"type": "CodeSymbolRef", "name": symbol}),
                )
            });
            operations.push(edge(&node_id, &target, "implementedBy"));
            edges += 1;
        }
        let acceptance = property(triples, subject, "acceptance").unwrap_or_default();
        for test in proving_tests(acceptance) {
            let target = format!("test:{test}");
            stubs.entry(target.clone()).or_insert_with(|| {
                node(
                    &target,
                    serde_json::json!({"type": "TestSymbolRef", "name": test}),
                )
            });
            operations.push(edge(&node_id, &target, "provedBy"));
            edges += 1;
        }
    }
    // Symbol and test stubs go first so every edge endpoint exists when the edge applies.
    let mut ordered: Vec<serde_json::Value> = stubs.into_values().collect();
    nodes += ordered.len() as u32;
    ordered.extend(operations);
    // Requirement nodes before their edges: stable partition by operation kind.
    let (mut writes, links): (Vec<_>, Vec<_>) = ordered
        .into_iter()
        .partition(|op| op["op"] == "upsert_node");
    writes.extend(links);
    Ok(RequirementProjection {
        operations: writes,
        nodes,
        edges,
    })
}

/// Refusal code of a contract row that no spec holds.
pub const CONTRACT_ROW_UNRESOLVED: &str = "CONTRACT_ROW_UNRESOLVED";

/// The node id of the contract record of one seam of one repository.
pub fn contract_node_id(repo: &str, seam: &str) -> String {
    format!("contract:{repo}:{seam}")
}

/// One side of a seam: a repository and the rows it holds on that side.
#[derive(Default)]
struct SeamSide {
    repos: BTreeSet<String>,
    rows: BTreeSet<(String, String)>,
}

impl SeamSide {
    /// `Provider: <repo> <ROW-ID>` header line.
    fn header(&mut self, rest: &str) {
        let mut words = rest.split_whitespace();
        if let (Some(repo), Some(row)) = (words.next(), words.next()) {
            let row = row.trim_matches(|c: char| matches!(c, '`' | '.' | ','));
            if is_row_id(row) {
                self.repos.insert(repo.to_string());
                self.rows.insert((repo.to_string(), row.to_string()));
            }
        }
    }

    /// `Repository: <repo>. Rows: `A`, `B` (notes).` line of a side section.
    fn section_line(&mut self, line: &str) {
        let Some(rest) = line.strip_prefix("Repository:") else {
            return;
        };
        let repo = rest.split('.').next().unwrap_or_default().trim();
        if repo.is_empty() {
            return;
        }
        self.repos.insert(repo.to_string());
        for span in ticked(rest) {
            if is_row_id(span) {
                self.rows.insert((repo.to_string(), span.to_string()));
            }
        }
    }

    fn join_repos(&self) -> String {
        self.repos.iter().cloned().collect::<Vec<_>>().join(",")
    }
}

/// The parts of one hand-written contract file `specs/<family>/contracts/<seam>.md`.
struct ContractParts {
    provider: SeamSide,
    consumer: SeamSide,
    wire_shape: String,
    refusal_codes: BTreeSet<String>,
    types: BTreeSet<String>,
    vectors: BTreeSet<String>,
}

fn parse_contract(text: &str) -> ContractParts {
    let mut parts = ContractParts {
        provider: SeamSide::default(),
        consumer: SeamSide::default(),
        wire_shape: String::new(),
        refusal_codes: BTreeSet::new(),
        types: BTreeSet::new(),
        vectors: BTreeSet::new(),
    };
    let mut heading = String::new();
    for line in text.lines().map(str::trim_end) {
        if let Some(title) = line.strip_prefix("## ") {
            heading = title.trim().to_lowercase();
            continue;
        }
        if let Some(rest) = line.strip_prefix("Provider:") {
            parts.provider.header(rest);
        } else if let Some(rest) = line.strip_prefix("Consumer:") {
            parts.consumer.header(rest);
        }
        match heading.as_str() {
            "provider" => parts.provider.section_line(line),
            "consumer" => parts.consumer.section_line(line),
            "wire shape" => {
                parts.wire_shape.push_str(line);
                parts.wire_shape.push('\n');
            }
            "refusal codes" => {
                if let Some(code) = line
                    .strip_prefix("- ")
                    .and_then(|bullet| ticked(bullet).into_iter().next())
                {
                    parts.refusal_codes.insert(code.to_string());
                }
            }
            "authoritative types" | "authoritative code types" => {
                if line.starts_with("- ") {
                    parts.types.extend(
                        ticked(line)
                            .into_iter()
                            .filter(|s| s.contains('/'))
                            .map(str::to_string),
                    );
                }
            }
            "test vectors" => {
                if line.starts_with("- ") {
                    parts.vectors.extend(
                        ticked(line)
                            .into_iter()
                            .filter(|s| s.contains("::"))
                            .map(str::to_string),
                    );
                }
            }
            _ => {}
        }
    }
    parts.wire_shape = parts.wire_shape.trim().to_string();
    parts
}

/// Project the hand-written contract files of repository `repo` (MEDULLA-SDLC-R014.4).
///
/// `files` is `(file name, text)` of each `contracts/<seam>.md`. Each becomes one
/// Contract record that names the provider and consumer repositories and rows, lists the
/// wire shape and refusal codes and links the authoritative types and test vectors. Each
/// named row gets a `crossesSeam` edge from its requirement node to the contract, so a
/// requirement resolves to the owning contract and both repositories. `known_rows` holds the
/// requirement node ids that exist; a contract row outside it refuses with
/// `CONTRACT_ROW_UNRESOLVED` and no operation is produced. The engine generates no contract.
pub fn project_contracts(
    repo: &str,
    files: &[(&str, &str)],
    known_rows: &BTreeSet<String>,
) -> Result<RequirementProjection, String> {
    let mut contracts: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut links: BTreeSet<(String, String)> = BTreeSet::new();
    for (name, text) in files {
        let seam = name.rsplit('/').next().unwrap_or(name);
        let seam = seam.strip_suffix(".md").unwrap_or(seam);
        let id = contract_node_id(repo, seam);
        let parts = parse_contract(text);
        for (row_repo, row) in parts.provider.rows.iter().chain(parts.consumer.rows.iter()) {
            let requirement = requirement_node_id(row_repo, row);
            if !known_rows.contains(&requirement) {
                return Err(format!(
                    "{CONTRACT_ROW_UNRESOLVED}: {row} (named by contract {seam}) resolves to no row"
                ));
            }
            links.insert((requirement, id.clone()));
        }
        let join = |set: &BTreeSet<String>| set.iter().cloned().collect::<Vec<_>>().join(",");
        contracts.insert(
            id.clone(),
            node(
                &id,
                serde_json::json!({
                    "type": "Contract",
                    "repo": repo,
                    "seam": seam,
                    "declaredIn": name,
                    "provider_repos": parts.provider.join_repos(),
                    "consumer_repos": parts.consumer.join_repos(),
                    "wire_shape": parts.wire_shape,
                    "refusal_codes": join(&parts.refusal_codes),
                    "types": join(&parts.types),
                    "vectors": join(&parts.vectors),
                }),
            ),
        );
    }
    let nodes = contracts.len() as u32;
    let edges = links.len() as u32;
    let mut operations: Vec<serde_json::Value> = contracts.into_values().collect();
    operations.extend(
        links
            .iter()
            .map(|(requirement, contract)| edge(requirement, contract, "crossesSeam")),
    );
    Ok(RequirementProjection {
        operations,
        nodes,
        edges,
    })
}

/// Refusal code of a bundle plan row that no spec holds.
pub const BUNDLE_ROW_UNRESOLVED: &str = "BUNDLE_ROW_UNRESOLVED";

/// The node id of one release bundle.
pub fn bundle_node_id(bundle: &str) -> String {
    format!("bundle:{bundle}")
}

/// One generated bundle plan under `releases/` (MEDULLA-SDLC-R018.1): a JSON file.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BundlePlan {
    bundle: String,
    /// Row ids per repository.
    rows_by_repo: BTreeMap<String, Vec<String>>,
    /// The dependency closure of the bundle, as row ids.
    closure: Vec<String>,
    /// Repository names in train order.
    train_order: Vec<String>,
    scenario: String,
    /// Row ids of the open tasks.
    open_tasks: Vec<String>,
}

/// Project the generated bundle plans of `releases/` (MEDULLA-SDLC-R018.1).
///
/// `plans` is `(file name, JSON text)` of each plan. Each becomes one `ReleaseBundle`
/// record that carries the closure, the train order, the scenario and the open tasks, and
/// each row of `rows_by_repo` gets an `inBundle` edge from its requirement node to the
/// bundle. `known_rows` holds the requirement node ids that exist. A plan that names a row
/// no spec holds, in `rows_by_repo`, `closure` or `open_tasks`, refuses with
/// `BUNDLE_ROW_UNRESOLVED` and no operation is produced.
pub fn project_bundles(
    plans: &[(&str, &str)],
    known_rows: &BTreeSet<String>,
) -> Result<RequirementProjection, String> {
    let known_ids: BTreeSet<&str> = known_rows
        .iter()
        .filter_map(|id| id.rsplit(':').next())
        .collect();
    let mut bundles: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut links: BTreeSet<(String, String)> = BTreeSet::new();
    for (name, text) in plans {
        let plan: BundlePlan = serde_json::from_str(text).map_err(|error| {
            format!(
                "{}: bundle plan {name}: {error}",
                EngineErrorCode::InvalidArgument.as_str()
            )
        })?;
        let id = bundle_node_id(&plan.bundle);
        let unresolved = |row: &str| {
            format!(
                "{BUNDLE_ROW_UNRESOLVED}: {row} (named by bundle {}) resolves to no row",
                plan.bundle
            )
        };
        for (repo, rows) in &plan.rows_by_repo {
            for row in rows {
                let requirement = requirement_node_id(repo, row);
                if !known_rows.contains(&requirement) {
                    return Err(unresolved(row));
                }
                links.insert((requirement, id.clone()));
            }
        }
        for row in plan.closure.iter().chain(plan.open_tasks.iter()) {
            if !known_ids.contains(row.as_str()) {
                return Err(unresolved(row));
            }
        }
        bundles.insert(
            id.clone(),
            node(
                &id,
                serde_json::json!({
                    "type": "ReleaseBundle",
                    "bundle": plan.bundle,
                    "scenario": plan.scenario,
                    "declaredIn": name,
                    "train_order": plan.train_order.join(","),
                    "closure": plan.closure.join(","),
                    "open_tasks": plan.open_tasks.join(","),
                }),
            ),
        );
    }
    let nodes = bundles.len() as u32;
    let edges = links.len() as u32;
    let mut operations: Vec<serde_json::Value> = bundles.into_values().collect();
    operations.extend(
        links
            .iter()
            .map(|(requirement, bundle)| edge(requirement, bundle, "inBundle")),
    );
    Ok(RequirementProjection {
        operations,
        nodes,
        edges,
    })
}

/// What the engine already holds of one artifact file: the digest it was ingested
/// under and the ids of the requirement records it produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoredArtifact {
    pub digest: String,
    pub records: Vec<String>,
}

/// The graph write of one incremental re-ingest.
#[derive(Clone, Debug, PartialEq)]
pub struct ReingestPlan {
    pub operations: Vec<serde_json::Value>,
    /// Files whose digest changed (or are new), in order.
    pub reingested: Vec<String>,
    /// Files that left the tree; their records are removed.
    pub removed: Vec<String>,
    /// Files whose digest is unchanged and were not parsed.
    pub unchanged: u32,
}

/// The node that holds the digest of one artifact file of one repository.
pub fn artifact_node_id(repo: &str, file: &str) -> String {
    format!("specfile:{repo}:{file}")
}

fn digest_of(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Incremental ingest by file digest (MEDULLA-SDLC-R014.2).
///
/// `files` is `(file name, text)` of the artifact tree now; `stored` is what the engine
/// holds per file name. Only a file whose digest differs from the stored one is parsed
/// (`parse_spec_file`) and projected (`project_requirements`); its digest and record ids go
/// on the file node as properties. A record a changed file no longer produces is removed. A
/// stored file that left the tree loses its file node and all its records. A parse or
/// projection refusal produces no operation at all.
pub fn reingest_changed_artifacts(
    repo: &str,
    files: &[(&str, &str)],
    stored: &BTreeMap<String, StoredArtifact>,
    states: &BTreeMap<String, String>,
    external: &BTreeMap<String, String>,
) -> Result<ReingestPlan, String> {
    reingest_in(repo, files, stored, states, external, Unresolved::Refuse)
}

fn reingest_in(
    repo: &str,
    files: &[(&str, &str)],
    stored: &BTreeMap<String, StoredArtifact>,
    states: &BTreeMap<String, String>,
    external: &BTreeMap<String, String>,
    unresolved: Unresolved,
) -> Result<ReingestPlan, String> {
    let mut removals: Vec<serde_json::Value> = Vec::new();
    let mut writes: Vec<serde_json::Value> = Vec::new();
    let mut links: Vec<serde_json::Value> = Vec::new();
    let (mut reingested, mut removed, mut unchanged) = (Vec::new(), Vec::new(), 0u32);
    let present: BTreeSet<&str> = files.iter().map(|(name, _)| *name).collect();
    for (name, old) in stored {
        if !present.contains(name.as_str()) {
            removed.push(name.clone());
            removals.extend(
                old.records
                    .iter()
                    .map(|id| serde_json::json!({"op": "remove_node", "id": id})),
            );
            removals
                .push(serde_json::json!({"op": "remove_node", "id": artifact_node_id(repo, name)}));
        }
    }
    for (name, text) in files {
        let digest = digest_of(text);
        let old = stored.get(*name);
        if old.is_some_and(|old| old.digest == digest) {
            unchanged += 1;
            continue;
        }
        let triples = parse_spec_file(name, text).map_err(|refusal| refusal.to_string())?;
        let projection = project_requirements_in(repo, &triples, states, external, unresolved)?;
        let mut records = Vec::new();
        for op in projection.operations {
            if op["op"] == "upsert_node" {
                if op["properties"]["type"] == "Requirement" {
                    records.push(op["id"].as_str().unwrap_or_default().to_string());
                }
                writes.push(op);
            } else {
                links.push(op);
            }
        }
        if let Some(old) = old {
            removals.extend(
                old.records
                    .iter()
                    .filter(|id| !records.contains(id))
                    .map(|id| serde_json::json!({"op": "remove_node", "id": id})),
            );
        }
        writes.push(node(
            &artifact_node_id(repo, name),
            serde_json::json!({
                "type": "SpecArtifactFile",
                "repo": repo,
                "file": name,
                "digest": digest,
                "records": records.join(","),
            }),
        ));
        reingested.push(name.to_string());
    }
    let mut operations = removals;
    operations.extend(writes);
    operations.extend(links);
    Ok(ReingestPlan {
        operations,
        reingested,
        removed,
        unchanged,
    })
}

/// A spec artifact file of a repository tree: Markdown under a `specs/` directory.
fn is_spec_artifact(path: &str) -> bool {
    path.ends_with(".md") && (path.starts_with("specs/") || path.contains("/specs/"))
}

/// The artifact files the engine already holds for `repo`, read off the `SpecArtifactFile`
/// nodes that [`reingest_changed_artifacts`] wrote.
pub fn stored_artifacts(
    core: &medulla_graph_core::graph::GraphCore,
    repo: &str,
) -> BTreeMap<String, StoredArtifact> {
    const MAX_ARTIFACT_FILES: usize = 1_000_000;
    core.get_nodes_by_label("SpecArtifactFile", MAX_ARTIFACT_FILES)
        .into_iter()
        .filter_map(|(_, properties)| {
            let node = medulla_types::msgpack::decode_property_value(&properties).ok()?;
            let text = |key: &str| node.get(key).and_then(|v| v.as_str()).map(str::to_owned);
            (text("repo")? == repo).then(|| {
                (
                    text("file").unwrap_or_default(),
                    StoredArtifact {
                        digest: text("digest").unwrap_or_default(),
                        records: text("records")
                            .unwrap_or_default()
                            .split(',')
                            .filter(|id| !id.is_empty())
                            .map(str::to_owned)
                            .collect(),
                    },
                )
            })
        })
        .collect()
}

/// The `SourceIngest` and `IndexRepository` worker loop's entry for spec artifacts
/// (MEDULLA-SDLC-R014.2): called with the files of one scoped repository batch, it
/// re-ingests only the spec artifact files whose digest differs from `stored` and lowers
/// the records into `result`, so they commit in the same change envelope as the rest of
/// the index. A path that repeats in `files` is read once, at its first occurrence. A
/// stored file that is absent from this batch is not removed here; the batch need not
/// carry every file, and the returned plan names `removed` for a caller that holds the
/// full tree.
pub fn project_spec_artifacts(
    repo: &str,
    files: &[(String, Vec<u8>)],
    stored: &BTreeMap<String, StoredArtifact>,
    result: &mut medulla_types::ingestion_wire::IndexResult,
) -> Result<ReingestPlan, String> {
    let mut seen = BTreeSet::new();
    let texts: Vec<(&str, &str)> = files
        .iter()
        .filter(|(path, _)| is_spec_artifact(path) && seen.insert(path.as_str()))
        .filter_map(|(path, bytes)| Some((path.as_str(), std::str::from_utf8(bytes).ok()?)))
        .collect();
    if texts.is_empty() {
        return Ok(ReingestPlan {
            operations: Vec::new(),
            reingested: Vec::new(),
            removed: Vec::new(),
            unchanged: 0,
        });
    }
    // Only the batch's files are compared: a stored file the batch does not carry stays.
    let carried: BTreeMap<String, StoredArtifact> = stored
        .iter()
        .filter(|(name, _)| seen.contains(name.as_str()))
        .map(|(name, old)| (name.clone(), old.clone()))
        .collect();
    let plan = reingest_in(
        repo,
        &texts,
        &carried,
        &BTreeMap::new(),
        &BTreeMap::new(),
        Unresolved::Skip,
    )?;
    for op in &plan.operations {
        match op["op"].as_str() {
            Some("upsert_node") => result.nodes.push(index_node(op)),
            Some("upsert_edge") => result.edges.push(index_edge(op)),
            _ => {}
        }
    }
    Ok(plan)
}

fn index_node(op: &serde_json::Value) -> medulla_types::ingestion_wire::ExtractedNode {
    let properties: std::collections::HashMap<String, String> = op["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
        .collect();
    medulla_types::ingestion_wire::ExtractedNode {
        node_id: op["id"].as_str().unwrap_or_default().to_owned(),
        node_type: properties.get("type").cloned().unwrap_or_default(),
        properties,
    }
}

fn index_edge(op: &serde_json::Value) -> medulla_types::ingestion_wire::ExtractedEdge {
    medulla_types::ingestion_wire::ExtractedEdge {
        source: op["source"].as_str().unwrap_or_default().to_owned(),
        target: op["target"].as_str().unwrap_or_default().to_owned(),
        edge_type: op["properties"]["relationship"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        properties: std::collections::HashMap::new(),
    }
}

/// Write one projection as the single `BatchUpdate` through the mutation gateway.
pub async fn commit_requirements(
    ctx: &MutationCtx<'_>,
    projection: &RequirementProjection,
) -> Response {
    let operations_msgpack = match rmp_serde::to_vec_named(&projection.operations) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Response::err(
                ctx.req_id,
                format!("INVALID_ARGUMENT: requirement projection encoding failed: {error}"),
            )
        }
    };
    let write = Method::BatchUpdate { operations_msgpack };
    let Method::BatchUpdate { operations_msgpack } = &write else {
        unreachable!("write was just built as a BatchUpdate");
    };
    let operations = operations_msgpack.clone();
    let plan = MutationPlan::for_method(&write);
    medulla_server_ops::api::ingest::graph::commit_gateway(ctx, &plan, &write, move |core| {
        let summary = medulla_graph_algos::algorithms::batch_update(core, &operations)?;
        medulla_protocol_types::result_contract::transactions::BatchUpdateReport::decode(&summary)
            .and_then(
                ResultPayload::of::<
                    medulla_protocol_types::result_contract::transactions::BatchUpdate,
                >,
            )
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_compute::parser::spec_artifacts::parse_spec_file;

    const REQUIREMENTS: &str = "# Requirements\n\n\
| ID | Requirement | Acceptance |\n|----|----|----|\n\
| `DEMO-R001` | **Base.** Caller chain: `serve_base` (R002) → `crates/demo/src/base.rs:12`. Shape: new. | Test: `tests/demo/test_base.py::test_base` (L3). |\n\
| `DEMO-R002` | **Second.** Caller chain: `serve_second` → `Second::run`. Depends: DEMO-R001, OTHER-R009. | Test: `crates/demo/tests/second.rs::runs` (L2). |\n\
| `DEMO-R003` | **Third.** Caller chain: `serve_third`. Depends: R002. | Test: `tests/demo/test_third.py::test_third` (L3). |\n";

    fn triples() -> Vec<RdfTriple> {
        parse_spec_file("requirements.md", REQUIREMENTS).expect("fixture parses")
    }

    fn external() -> BTreeMap<String, String> {
        BTreeMap::from([(
            "OTHER-R009".to_string(),
            "requirement:other:OTHER-R009".to_string(),
        )])
    }

    fn edges_of(p: &RequirementProjection, source: &str, relationship: &str) -> Vec<String> {
        p.operations
            .iter()
            .filter(|op| {
                op["op"] == "upsert_edge"
                    && op["source"] == source
                    && op["properties"]["relationship"] == relationship
            })
            .map(|op| op["target"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn spec_rows_become_requirement_nodes_with_edges() {
        let states = BTreeMap::from([("DEMO-R001".to_string(), "VERIFIED".to_string())]);
        let p = project_requirements("demo", &triples(), &states, &external()).expect("projects");
        for id in ["DEMO-R001", "DEMO-R002", "DEMO-R003"] {
            let node_id = requirement_node_id("demo", id);
            let op = p
                .operations
                .iter()
                .find(|op| op["op"] == "upsert_node" && op["id"] == node_id.as_str())
                .expect("requirement node");
            let props = &op["properties"];
            println!(
                "REQ_NODE\t{id}\t{}\t{}\t{}",
                props["repo"].as_str().unwrap(),
                props["title"].as_str().unwrap(),
                props["state"].as_str().unwrap()
            );
            for rel in ["dependsOn", "implementedBy", "provedBy"] {
                for target in edges_of(&p, &node_id, rel) {
                    println!("REQ_EDGE\t{id}\t{rel}\t{target}");
                }
            }
        }
        let r2 = requirement_node_id("demo", "DEMO-R002");
        assert_eq!(
            edges_of(&p, &r2, "dependsOn"),
            vec![
                requirement_node_id("demo", "DEMO-R001"),
                "requirement:other:OTHER-R009".to_string()
            ]
        );
        assert_eq!(
            edges_of(&p, &requirement_node_id("demo", "DEMO-R003"), "dependsOn"),
            vec![r2.clone()]
        );
        assert_eq!(
            edges_of(
                &p,
                &requirement_node_id("demo", "DEMO-R001"),
                "implementedBy"
            ),
            vec!["symbol:crates/demo/src/base.rs", "symbol:serve_base"]
        );
        assert_eq!(
            edges_of(&p, &r2, "provedBy"),
            vec!["test:crates/demo/tests/second.rs::runs"]
        );
        let r1 = p
            .operations
            .iter()
            .find(|op| op["id"] == requirement_node_id("demo", "DEMO-R001").as_str())
            .unwrap();
        assert_eq!(r1["properties"]["state"], "VERIFIED");
        assert_eq!(r1["properties"]["title"], "Base.");
        assert_eq!(
            p.operations
                .iter()
                .position(|op| op["op"] == "upsert_edge")
                .unwrap(),
            p.nodes as usize,
            "every node precedes every edge"
        );
        // The same bytes give the same projection.
        assert_eq!(
            p,
            project_requirements("demo", &triples(), &states, &external()).unwrap()
        );
    }

    #[test]
    fn unknown_depends_id_refuses_and_writes_nothing() {
        let refusal = project_requirements("demo", &triples(), &BTreeMap::new(), &BTreeMap::new())
            .expect_err("OTHER-R009 resolves to no row");
        assert!(refusal.starts_with(REQUIREMENT_UNRESOLVED), "{refusal}");
        assert!(refusal.contains("OTHER-R009") && refusal.contains("DEMO-R002"));
    }

    const CONTRACT: &str = "# Seam contract: demo\n\n\
Provider: demo DEMO-R001\nConsumer: other OTHER-R009\n\n\
## Provider\n\nRepository: demo. Rows: `DEMO-R001`, `DEMO-R002`.\n\n\
## Wire shape\n\n| Direction | Record |\n|---|---|\n| a to b | `Req` |\n\n\
## Refusal codes\n\n- `DEMO_REFUSED`: no.\n\n\
## Authoritative types\n\n- `crates/demo/src/lib.rs`: `Req`.\n\n\
## Test vectors\n\n- `tests/demo/test_demo.py::test_demo` holds the vector.\n";

    fn known() -> BTreeSet<String> {
        ["demo:DEMO-R001", "demo:DEMO-R002", "other:OTHER-R009"]
            .iter()
            .map(|id| {
                let (repo, row) = id.split_once(':').unwrap();
                requirement_node_id(repo, row)
            })
            .collect()
    }

    // spec: MEDULLA-SDLC-R014.4
    #[test]
    fn contract_files_form_the_seam_graph() {
        let files = [("contracts/demo-seam.md", CONTRACT)];
        let p = project_contracts("demo", &files, &known()).expect("projects");
        let id = contract_node_id("demo", "demo-seam");
        let node = &p.operations[0];
        assert_eq!(node["id"], id.as_str());
        assert_eq!(node["properties"]["type"], "Contract");
        println!(
            "CONTRACT\t{}\t{}\t{}",
            node["properties"]["provider_repos"].as_str().unwrap(),
            node["properties"]["consumer_repos"].as_str().unwrap(),
            node["properties"]["refusal_codes"].as_str().unwrap()
        );
        for op in p.operations.iter().filter(|op| op["op"] == "upsert_edge") {
            assert_eq!(op["target"], id.as_str());
            assert_eq!(op["properties"]["relationship"], "crossesSeam");
            println!("SEAM_EDGE\t{}", op["source"].as_str().unwrap());
        }
        assert_eq!(p.nodes, 1);
        assert_eq!(p.edges, 3);
        assert_eq!(node["properties"]["types"], "crates/demo/src/lib.rs");
        assert_eq!(
            node["properties"]["vectors"],
            "tests/demo/test_demo.py::test_demo"
        );
        assert!(node["properties"]["wire_shape"]
            .as_str()
            .unwrap()
            .contains("| a to b |"));
    }

    // spec: MEDULLA-SDLC-R014.4
    #[test]
    fn contract_with_unknown_row_refuses() {
        let files = [("contracts/demo-seam.md", CONTRACT)];
        let refusal =
            project_contracts("demo", &files, &BTreeSet::new()).expect_err("no row is known");
        assert!(refusal.starts_with(CONTRACT_ROW_UNRESOLVED), "{refusal}");
    }

    const PLAN: &str = r#"{
  "bundle": "P1",
  "rows_by_repo": {"demo": ["DEMO-R001", "DEMO-R002"], "other": ["OTHER-R009"]},
  "closure": ["DEMO-R001", "DEMO-R002", "OTHER-R009"],
  "train_order": ["other", "demo"],
  "scenario": "DEMO-SCENARIO-R001",
  "open_tasks": ["DEMO-R002"]
}"#;

    // spec: MEDULLA-SDLC-R018.1
    #[test]
    fn bundle_plan_becomes_records_with_in_bundle_edges() {
        let p = project_bundles(&[("releases/P1.json", PLAN)], &known()).expect("projects");
        let id = bundle_node_id("P1");
        let node = &p.operations[0];
        assert_eq!(node["id"], id.as_str());
        assert_eq!(node["properties"]["type"], "ReleaseBundle");
        assert_eq!(node["properties"]["train_order"], "other,demo");
        assert_eq!(node["properties"]["open_tasks"], "DEMO-R002");
        println!(
            "BUNDLE\t{}\t{}\t{}",
            node["properties"]["scenario"].as_str().unwrap(),
            node["properties"]["train_order"].as_str().unwrap(),
            node["properties"]["closure"].as_str().unwrap()
        );
        for op in p.operations.iter().filter(|op| op["op"] == "upsert_edge") {
            assert_eq!(op["target"], id.as_str());
            assert_eq!(op["properties"]["relationship"], "inBundle");
            println!("IN_BUNDLE\t{}", op["source"].as_str().unwrap());
        }
        assert_eq!((p.nodes, p.edges), (1, 3));
    }

    // spec: MEDULLA-SDLC-R018.1
    #[test]
    fn bundle_plan_with_unknown_row_refuses() {
        let plan = PLAN.replace("\"OTHER-R009\"]},", "\"OTHER-R999\"]},");
        let refusal = project_bundles(&[("releases/P1.json", &plan)], &known())
            .expect_err("OTHER-R999 resolves to no row");
        assert!(refusal.starts_with(BUNDLE_ROW_UNRESOLVED), "{refusal}");
        assert!(refusal.contains("OTHER-R999"));
    }

    fn artifact_files() -> Vec<(&'static str, String)> {
        let second = REQUIREMENTS
            .replace("DEMO-R", "MORE-R")
            .replace("Depends: MORE-R001, OTHER-R009.", "")
            .replace("Depends: R002.", "");
        vec![
            ("requirements.md", REQUIREMENTS.to_string()),
            ("more/requirements.md", second),
        ]
    }

    fn as_refs(files: &[(&'static str, String)]) -> Vec<(&'static str, &str)> {
        files.iter().map(|(n, t)| (*n, t.as_str())).collect()
    }

    /// The stored state a plan leaves behind, read back off its file nodes.
    fn stored_after(plan: &ReingestPlan, into: &mut BTreeMap<String, StoredArtifact>) {
        for op in &plan.operations {
            if op["op"] == "upsert_node" && op["properties"]["type"] == "SpecArtifactFile" {
                let props = &op["properties"];
                into.insert(
                    props["file"].as_str().unwrap().to_string(),
                    StoredArtifact {
                        digest: props["digest"].as_str().unwrap().to_string(),
                        records: props["records"]
                            .as_str()
                            .unwrap()
                            .split(',')
                            .filter(|id| !id.is_empty())
                            .map(str::to_string)
                            .collect(),
                    },
                );
            }
        }
    }

    // spec: MEDULLA-SDLC-R014.2
    #[test]
    fn one_file_change_reingests_only_that_file() {
        let mut files = artifact_files();
        let mut stored = BTreeMap::new();
        let external = external();
        let first = reingest_changed_artifacts(
            "demo",
            &as_refs(&files),
            &stored,
            &BTreeMap::new(),
            &external,
        )
        .expect("first ingest");
        assert_eq!(
            first.reingested,
            ["more/requirements.md", "requirements.md"]
        );
        stored_after(&first, &mut stored);

        // Nothing changed: nothing is parsed and nothing is written.
        let idle = reingest_changed_artifacts(
            "demo",
            &as_refs(&files),
            &stored,
            &BTreeMap::new(),
            &external,
        )
        .expect("idle");
        assert!(idle.operations.is_empty() && idle.reingested.is_empty());
        assert_eq!(idle.unchanged, 2);

        // One file changes: only it re-ingests, within the budget.
        files[1].1 = files[1].1.replace("**Second.**", "**Second row.**");
        let started = std::time::Instant::now();
        let change = reingest_changed_artifacts(
            "demo",
            &as_refs(&files),
            &stored,
            &BTreeMap::new(),
            &external,
        )
        .expect("change");
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        println!("REINGEST\t{}", change.reingested.join(","));
        println!("UNCHANGED\t{}", change.unchanged);
        assert_eq!(change.reingested, ["more/requirements.md"]);
        assert_eq!(change.unchanged, 1);
        stored_after(&change, &mut stored);

        // A file that leaves the tree loses its file node and its records.
        files.remove(1);
        let gone = reingest_changed_artifacts(
            "demo",
            &as_refs(&files),
            &stored,
            &BTreeMap::new(),
            &external,
        )
        .expect("removal");
        println!("REMOVED\t{}", gone.removed.join(","));
        assert_eq!(gone.removed, ["more/requirements.md"]);
        let removed_ids: BTreeSet<&str> = gone
            .operations
            .iter()
            .filter(|op| op["op"] == "remove_node")
            .map(|op| op["id"].as_str().unwrap())
            .collect();
        assert!(removed_ids.contains("specfile:demo:more/requirements.md"));
        assert!(removed_ids.contains("requirement:demo:MORE-R002"));
        assert!(gone.reingested.is_empty());
    }

    // spec: MEDULLA-SDLC-R014.2
    #[test]
    fn index_batch_lowers_only_changed_spec_files_into_the_result() {
        use medulla_types::ingestion_wire::IndexResult;

        let files = vec![
            (
                "specs/a/requirements.md".to_string(),
                REQUIREMENTS.as_bytes().to_vec(),
            ),
            ("src/lib.rs".to_string(), b"fn main() {}".to_vec()),
        ];
        let mut first = IndexResult::default();
        let plan = project_spec_artifacts("demo", &files, &BTreeMap::new(), &mut first)
            .expect("first batch");
        assert_eq!(plan.reingested, ["specs/a/requirements.md"]);
        // The DEMO-R002 `Depends: OTHER-R009` names another repository; it is skipped.
        assert!(first
            .nodes
            .iter()
            .any(|n| n.node_id == "requirement:demo:DEMO-R002"));
        assert!(first.edges.iter().any(|e| e.edge_type == "dependsOn"
            && e.source == "requirement:demo:DEMO-R003"
            && e.target == "requirement:demo:DEMO-R002"));
        assert!(!first.edges.iter().any(|e| e.target.contains("OTHER-R009")));
        println!("INDEX_NODES\t{}", first.nodes.len());

        let mut stored = BTreeMap::new();
        stored_after(&plan, &mut stored);
        let mut again = IndexResult::default();
        let idle = project_spec_artifacts("demo", &files, &stored, &mut again).expect("idle");
        assert!(idle.reingested.is_empty());
        assert!(again.nodes.is_empty() && again.edges.is_empty());
    }
}
