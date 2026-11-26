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
use medulla_source_types::source_changeset::Changeset;
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
            "text": body,
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
                    "provider_rows": parts
                        .provider
                        .rows
                        .iter()
                        .map(|(row_repo, row)| format!("{row_repo}:{row}"))
                        .collect::<Vec<_>>()
                        .join(","),
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

/// The node id of the document node of one artifact file.
pub fn document_node_id(repo: &str, file: &str) -> String {
    format!("specdoc:{repo}:{file}")
}

/// Project one spec document into graph nodes (MEDULLA-SDLC-R009): one `SpecDocument` with
/// its digest and one `SpecSection` per heading with its source span (first and last line,
/// 1-based) and the digest of the section's bytes, joined by `hasSection` edges. The nodes
/// carry spans and digests only; the text stays in the repository blob, so there is no
/// second document copy. Each op is an `upsert_node` or `upsert_edge`, nodes first.
pub fn project(repo: &str, file: &str, text: &str) -> Vec<serde_json::Value> {
    let lines: Vec<&str> = text.lines().collect();
    let mut fence = false;
    let mut headings: Vec<(usize, usize, String)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.starts_with("```") {
            fence = !fence;
        }
        let level = line.bytes().take_while(|b| *b == b'#').count();
        if !fence && (1..=6).contains(&level) && line[level..].starts_with(' ') {
            headings.push((index + 1, level, line[level..].trim().to_string()));
        }
    }
    let document = document_node_id(repo, file);
    let mut nodes = vec![node(
        &document,
        serde_json::json!({
            "type": "SpecDocument",
            "repo": repo,
            "file": file,
            "digest": digest_of(text),
            "lines": lines.len().to_string(),
        }),
    )];
    let mut links = Vec::new();
    for (position, (start, level, title)) in headings.iter().enumerate() {
        let end = headings
            .get(position + 1)
            .map_or(lines.len(), |(next, _, _)| next - 1);
        let body = lines[start - 1..end].join("\n");
        let id = format!("specsection:{repo}:{file}#L{start}");
        nodes.push(node(
            &id,
            serde_json::json!({
                "type": "SpecSection",
                "repo": repo,
                "file": file,
                "title": title,
                "level": level.to_string(),
                "start_line": start.to_string(),
                "end_line": end.to_string(),
                "digest": digest_of(&body),
                "text": body,
            }),
        ));
        links.push(edge(&document, &id, "hasSection"));
    }
    nodes.extend(links);
    nodes
}

/// Refusal code of a `Phase <N>` plan heading whose `N` is not a template phase.
pub const PLAN_PHASE_UNKNOWN: &str = "PLAN_PHASE_UNKNOWN";

/// The phases of the plan template: `Phase 0: Research`, `Phase 1: Design` and
/// `Phase 2: Task approach`.
const PLAN_PHASES: [&str; 3] = ["0", "1", "2"];

/// The node id of one section of a plan document.
pub fn plan_section_node_id(repo: &str, file: &str, start_line: usize) -> String {
    format!("plansection:{repo}:{file}#L{start_line}")
}

/// Whether `file` names a plan document (`plan.md` or `<dir>/plan.md`).
fn is_plan_file(file: &str) -> bool {
    file == "plan.md" || file.ends_with("/plan.md")
}

/// The phase a plan heading title names: `Ok(Some(N))` for `Phase <N>: <name>`, `Ok(None)`
/// for any other title, and the refusal code for a `Phase <N>` title with an unknown `N`.
fn plan_phase(title: &str) -> Result<Option<&'static str>, ()> {
    let Some(rest) = title.strip_prefix("Phase ") else {
        return Ok(None);
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return Ok(None);
    }
    let Some(phase) = PLAN_PHASES.iter().find(|phase| **phase == digits) else {
        return Err(());
    };
    Ok(rest[digits.len()..].starts_with(':').then_some(*phase))
}

/// The refusal code of a `Principle:` key that resolves to no principle (R014.6.2).
pub const PRINCIPLE_UNRESOLVED: &str = "PRINCIPLE_UNRESOLVED";

/// Whether `file` is the constitution of a repository: `.specify/memory/constitution.md`.
fn is_constitution_file(file: &str) -> bool {
    file == ".specify/memory/constitution.md" || file.ends_with("/.specify/memory/constitution.md")
}

/// The node id of one constitution principle: `principle:{repo}:{key}`.
pub fn principle_node_id(repo: &str, key: &str) -> String {
    format!("principle:{repo}:{key}")
}

/// The first token after `Article ` or `Principle ` in a level 2 or 3 heading: the key of the
/// principle (`Article XXXI: Title` gives `XXXI`).
fn principle_key(heading: &str) -> Option<&str> {
    let title = heading.trim_start_matches('#').trim();
    let rest = title
        .strip_prefix("Article ")
        .or_else(|| title.strip_prefix("Principle "))?;
    let key = rest
        .split_whitespace()
        .next()?
        .trim_end_matches([':', '.', ')']);
    (!key.is_empty()).then_some(key)
}

/// Project a constitution into principle records (MEDULLA-SDLC-R014.6.2): one
/// `ConstitutionPrinciple` per level 2 or 3 heading (outside code fences) that starts with
/// `Article ` or `Principle `, with its `repo`, `key`, `title` (the whole heading text) and the
/// `digest` of its section.
pub fn project_principles(repo: &str, file: &str, text: &str) -> RequirementProjection {
    let lines: Vec<&str> = text.lines().collect();
    let mut fence = false;
    let mut headings: Vec<(usize, usize, bool)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.starts_with("```") {
            fence = !fence;
        }
        if !fence && line.starts_with('#') {
            let level = line.chars().take_while(|c| *c == '#').count();
            headings.push((index, level, matches!(level, 2 | 3)));
        }
    }
    let mut nodes = Vec::new();
    for (position, (index, level, candidate)) in headings.iter().enumerate() {
        let Some(key) = candidate.then(|| principle_key(lines[*index])).flatten() else {
            continue;
        };
        // The section runs to the next heading of the same or a higher level.
        let end = headings[position + 1..]
            .iter()
            .find(|(_, next, _)| next <= level)
            .map_or(lines.len(), |(next, _, _)| *next);
        let body = lines[*index..end].join("\n");
        nodes.push(node(
            &principle_node_id(repo, key),
            serde_json::json!({
                "type": "ConstitutionPrinciple",
                "repo": repo,
                "key": key,
                "title": lines[*index].trim_start_matches('#').trim(),
                "digest": digest_of(&body),
                "file": file,
            }),
        ));
    }
    let count = nodes.len() as u32;
    RequirementProjection {
        operations: nodes,
        nodes: count,
        edges: 0,
    }
}

/// The principle keys a requirement row's text cites: `Principle: <key>[, <key>]`, keys
/// separated by commas with backticks stripped. Reading stops at the first piece that is not a
/// key.
fn cited_principles(text: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("Principle:") {
        rest = &rest[at + "Principle:".len()..];
        let segment = rest.split(['\n', '|', ';']).next().unwrap_or_default();
        for piece in segment.split(',') {
            let token = piece
                .trim()
                .trim_matches('`')
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_matches(|c: char| !c.is_ascii_alphanumeric());
            let is_key = !token.is_empty()
                && token.len() <= 16
                && token.chars().all(|c| c.is_ascii_alphanumeric());
            if !is_key {
                break;
            }
            if !keys.iter().any(|held| held == token) {
                keys.push(token.to_string());
            }
        }
    }
    keys
}

/// The `governedBy` edges of the requirement nodes among `operations`, one per cited key that
/// resolves in `known_principles`. A key that resolves to none refuses `PRINCIPLE_UNRESOLVED`.
/// This is the only writer of `governedBy`.
fn governed_by_edges(
    repo: &str,
    operations: &[serde_json::Value],
    known_principles: &BTreeSet<String>,
) -> Result<Vec<serde_json::Value>, String> {
    let mut edges = Vec::new();
    for op in operations {
        if op["op"] != "upsert_node" || op["properties"]["type"] != "Requirement" {
            continue;
        }
        let (Some(id), Some(text)) = (op["id"].as_str(), op["properties"]["text"].as_str()) else {
            continue;
        };
        for key in cited_principles(text) {
            let target = principle_node_id(repo, &key);
            if !known_principles.contains(&target) {
                return Err(format!(
                    "{PRINCIPLE_UNRESOLVED}: {key} (cited by {id}) resolves to no principle"
                ));
            }
            edges.push(edge(id, &target, "governedBy"));
        }
    }
    Ok(edges)
}

/// Whether `file` names a tasks document (`tasks.md` or `<dir>/tasks.md`).
fn is_tasks_file(file: &str) -> bool {
    file == "tasks.md" || file.ends_with("/tasks.md")
}

/// The node id of one task line: `task:{repo}:{file}#{task_id}`.
pub fn task_node_id(repo: &str, file: &str, task_id: &str) -> String {
    format!("task:{repo}:{file}#{task_id}")
}

/// The `N` of a heading `Phase <N>...`, as decimal digits.
fn heading_phase(heading: &str) -> Option<String> {
    let title = heading.trim_start_matches('#').trim();
    let rest = title.strip_prefix("Phase ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then_some(digits)
}

/// Project one tasks document into task records (MEDULLA-SDLC-R014.6.1). Each checklist line
/// the parser reads as a task with a T-id becomes one `TaskLine` node with its flags, row,
/// state, text, file and the `phase` of the nearest preceding `Phase <N>` heading of the same
/// file (absent when none precedes). A task whose row is in `known_rows` gets one `forRow`
/// edge to the requirement record; an unresolved row writes no edge. A line with no T-id is
/// skipped. Nothing about task state beyond the tick is stored. Nodes come before edges.
pub fn project_tasks(
    repo: &str,
    file: &str,
    text: &str,
    known_rows: &BTreeSet<String>,
) -> RequirementProjection {
    let (mut nodes, mut links) = (Vec::new(), Vec::new());
    let mut phase: Option<String> = None;
    let mut fence = false;
    for line in text.lines() {
        if line.starts_with("```") {
            fence = !fence;
        }
        if fence {
            continue;
        }
        if line.starts_with('#') {
            if let Some(found) = heading_phase(line) {
                phase = Some(found);
            }
            continue;
        }
        let trimmed = line.trim_start();
        let (ticked, rest) = if let Some(rest) = trimmed
            .strip_prefix("- [ ]")
            .or_else(|| trimmed.strip_prefix("* [ ]"))
        {
            (false, rest)
        } else if let Some(rest) = ["- [x]", "- [X]", "* [x]", "* [X]"]
            .iter()
            .find_map(|prefix| trimmed.strip_prefix(prefix))
        {
            (true, rest)
        } else {
            continue;
        };
        let Some(task) = medulla_compute::parser::spec_artifacts::parse_task_line(ticked, rest)
        else {
            continue;
        };
        let Some(task_id) = task.task_id.as_deref() else {
            continue;
        };
        let id = task_node_id(repo, file, task_id);
        let row = task.row.clone().unwrap_or_default();
        let mut properties = serde_json::json!({
            "type": "TaskLine",
            "repo": repo,
            "file": file,
            "taskId": task_id,
            "parallel": task.parallel.to_string(),
            "story": task.story.clone().unwrap_or_default(),
            "row": row,
            "ticked": task.ticked.to_string(),
            "text": task.text,
        });
        if let Some(phase) = &phase {
            properties["phase"] = phase.clone().into();
        }
        nodes.push(node(&id, properties));
        if !row.is_empty() {
            let target = requirement_node_id(repo, &row);
            if known_rows.contains(&target) {
                links.push(edge(&id, &target, "forRow"));
            }
        }
    }
    let (count_nodes, count_edges) = (nodes.len() as u32, links.len() as u32);
    nodes.extend(links);
    RequirementProjection {
        operations: nodes,
        nodes: count_nodes,
        edges: count_edges,
    }
}

/// Project one plan document into section records (MEDULLA-SDLC-R014.7): one `PlanSection`
/// per level-2 heading (outside code fences) with its span, the digest of its bytes and its
/// `phase`, and one `covers` edge to each requirement of `known_rows` that the section body
/// names in backticks. A `Phase <N>` heading with an unknown `N` refuses the whole call with
/// `PLAN_PHASE_UNKNOWN: <file>:<line>` and no operation. An ID outside `known_rows` (a row
/// of another repository) writes no edge. Nodes come before edges.
pub fn project_plans(
    repo: &str,
    file: &str,
    text: &str,
    known_rows: &BTreeSet<String>,
) -> Result<RequirementProjection, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut fence = false;
    let mut starts: Vec<(usize, &str)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.starts_with("```") {
            fence = !fence;
        }
        if !fence {
            if let Some(title) = line.strip_prefix("## ") {
                starts.push((index + 1, title.trim()));
            }
        }
    }
    let (mut nodes, mut links) = (Vec::new(), Vec::new());
    for (position, (start, title)) in starts.iter().enumerate() {
        let phase = plan_phase(title).map_err(|()| {
            format!(
                "{}: {file}:{start}",
                EngineErrorCode::PlanPhaseUnknown.as_str()
            )
        })?;
        let end = starts
            .get(position + 1)
            .map_or(lines.len(), |(next, _)| next - 1);
        let body = lines[start - 1..end].join("\n");
        let id = plan_section_node_id(repo, file, *start);
        let mut properties = serde_json::json!({
            "type": "PlanSection",
            "repo": repo,
            "file": file,
            "title": title,
            "start_line": start.to_string(),
            "end_line": end.to_string(),
            "digest": digest_of(&body),
            "text": body,
        });
        if let Some(phase) = phase {
            properties["phase"] = phase.into();
        }
        nodes.push(node(&id, properties));
        let covered: BTreeSet<String> = ticked(&body)
            .into_iter()
            .filter(|span| is_row_id(span))
            .map(|span| requirement_node_id(repo, span))
            .filter(|target| known_rows.contains(target))
            .collect();
        for target in covered {
            links.push(edge(&id, &target, "covers"));
        }
    }
    let (count_nodes, count_edges) = (nodes.len() as u32, links.len() as u32);
    nodes.extend(links);
    Ok(RequirementProjection {
        operations: nodes,
        nodes: count_nodes,
        edges: count_edges,
    })
}

/// The node id of one changeset of one repository.
pub fn changeset_node_id(repo: &str, changeset_id: &str) -> String {
    format!("changeset:{repo}:{changeset_id}")
}

/// The artifact entry (a [`StoredArtifact`] of the `stored` map) whose digest is the id
/// of the last changeset cited and whose records are the changeset node ids written.
pub const CHANGESETS_ARTIFACT: &str = "changesets";

/// Link each changeset to the requirement rows it delivers (MEDULLA-SDLC-R014.3).
///
/// Per changeset: one `Changeset` node `changeset:{repo}:{id}` (`repo`, `changeset_id`,
/// `created_at`) and one `citesRequirement` edge to the requirement node of every
/// `Spec:` trailer id. `known` holds requirement node ids. An id that is not in `known`
/// refuses the whole call with `REQUIREMENT_UNRESOLVED` and no operation is produced.
pub fn cite_requirements(
    repo: &str,
    changesets: &[Changeset],
    known: &BTreeSet<String>,
) -> Result<RequirementProjection, String> {
    let (mut nodes, mut links) = (Vec::new(), Vec::new());
    for changeset in changesets {
        let source = changeset_node_id(repo, &changeset.id);
        let mut properties = serde_json::json!({
            "type": "Changeset",
            "repo": repo,
            "changeset_id": changeset.id,
            "created_at": changeset.created_at.to_string(),
        });
        // The ids a `Spec-Withdraw` trailer withdrew, comma-joined: the served `SpecStatus`
        // replays them (MEDULLA-SDLC-R005.3).
        if !changeset.trailers.withdraw.is_empty() {
            properties["withdraws"] = changeset.trailers.withdraw.join(",").into();
        }
        nodes.push(node(&source, properties));
        for row_id in &changeset.trailers.spec_ids {
            let target = requirement_node_id(repo, row_id);
            if !known.contains(&target) {
                return Err(format!(
                    "{}: {row_id} (cited by changeset {}) resolves to no row",
                    EngineErrorCode::RequirementUnresolved.as_str(),
                    changeset.id
                ));
            }
            links.push(edge(&source, &target, "citesRequirement"));
        }
    }
    let (count_nodes, count_edges) = (nodes.len() as u32, links.len() as u32);
    nodes.extend(links);
    Ok(RequirementProjection {
        operations: nodes,
        nodes: count_nodes,
        edges: count_edges,
    })
}

/// The links of one requirement row that name other records: the rows it depends on and
/// the seam files it provides or consumes (MEDULLA-SDLC-R014.12).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpecRowLinks {
    pub row_id: String,
    pub depends_on: Vec<String>,
    pub seam_provider: Vec<String>,
    pub seam_consumer: Vec<String>,
}

fn links_of<'a>(
    links: &'a mut Vec<SpecRowLinks>,
    index: &mut BTreeMap<String, usize>,
    row_id: &str,
) -> &'a mut SpecRowLinks {
    let at = *index.entry(row_id.to_string()).or_insert_with(|| {
        links.push(SpecRowLinks {
            row_id: row_id.to_string(),
            ..SpecRowLinks::default()
        });
        links.len() - 1
    });
    &mut links[at]
}

/// The row id a requirements table line starts with: `| `ID` | ...`.
fn row_line_id(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix('|')?.trim_start();
    let id = rest.strip_prefix('`')?.split('`').next()?;
    is_row_id(id).then_some(id)
}

/// The tokens after every `Depends:` of `text`, each up to the next `|` or the end of
/// the line, split on `,` and whitespace with backticks stripped, kept when they are row ids.
fn depends_tokens(text: &str, out: &mut Vec<String>) {
    for line in text.lines() {
        let mut rest = line;
        while let Some(at) = rest.find("Depends:") {
            rest = &rest[at + "Depends:".len()..];
            let clause = rest.split('|').next().unwrap_or_default();
            for token in clause.split(|c: char| c == ',' || c.is_whitespace()) {
                let token = token.replace('`', "");
                let token = token.trim_matches(|c: char| matches!(c, '.' | ';' | '(' | ')'));
                if is_row_id(token) && !out.iter().any(|seen| seen == token) {
                    out.push(token.to_string());
                }
            }
        }
    }
}

/// Parse the row links of the spec files of one repository tree (`(path, bytes)`).
///
/// A row is a requirements table line `| `ID` | ... |`; its text runs to the next table
/// line or blank line, so a `Depends:` line under the row belongs to it. A file under
/// `contracts/` whose lines are `Provider: <text>` and `Consumer: <text>` records the seam
/// side on the row id that is the last token of the line accepted by `is_row_id`. Lines
/// that parse to nothing are ignored.
pub fn parse_row_links(files: &[(String, Vec<u8>)]) -> Vec<SpecRowLinks> {
    let mut links: Vec<SpecRowLinks> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for (path, bytes) in files {
        if !is_spec_artifact(path) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        if path.contains("/contracts/") || path.starts_with("contracts/") {
            for line in text.lines() {
                let line = line.trim();
                let provider = line.starts_with("Provider:");
                if !provider && !line.starts_with("Consumer:") {
                    continue;
                }
                let row = line
                    .split_whitespace()
                    .rev()
                    .map(|t| t.trim_matches(|c: char| matches!(c, '`' | '.' | ',' | ';')))
                    .find(|t| is_row_id(t));
                if let Some(row) = row {
                    let entry = links_of(&mut links, &mut index, row);
                    let side = if provider {
                        &mut entry.seam_provider
                    } else {
                        &mut entry.seam_consumer
                    };
                    if !side.contains(path) {
                        side.push(path.clone());
                    }
                }
            }
            continue;
        }
        let mut current: Option<(String, String)> = None;
        let flush = |current: &mut Option<(String, String)>,
                         links: &mut Vec<SpecRowLinks>,
                         index: &mut BTreeMap<String, usize>| {
            if let Some((row, body)) = current.take() {
                let mut depends = Vec::new();
                depends_tokens(&body, &mut depends);
                let entry = links_of(links, index, &row);
                for dependency in depends {
                    if !entry.depends_on.contains(&dependency) {
                        entry.depends_on.push(dependency);
                    }
                }
            }
        };
        for line in text.lines() {
            if let Some(id) = row_line_id(line) {
                flush(&mut current, &mut links, &mut index);
                current = Some((id.to_string(), line.to_string()));
            } else if line.trim().is_empty() || line.trim_start().starts_with('|') {
                flush(&mut current, &mut links, &mut index);
            } else if let Some((_, body)) = current.as_mut() {
                body.push('\n');
                body.push_str(line);
            }
        }
        flush(&mut current, &mut links, &mut index);
    }
    links
}

/// Whether the pipelines spec-status gate scans `path` for test bindings: every file but
/// the spec and docs trees, markdown and JSON.
fn is_binding_source(path: &str) -> bool {
    !(path.starts_with("specs/")
        || path.starts_with("docs/")
        || path.ends_with(".md")
        || path.ends_with(".json"))
}

/// The row ids named in `body`: tokens of identifier characters, dots and hyphens that are row ids.
fn row_ids_in(body: &str) -> impl Iterator<Item = &str> {
    body.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
        .map(|token| token.trim_end_matches('.'))
        .filter(|token| is_row_id(token))
}

/// The test binding markers of a repository tree, in the two forms the spec standard's
/// status gate reads (`pipelines_hooks/specs/bindings.py`): a `// spec: ID[, ID...]` or
/// `# spec: ID[, ID...]` comment, and a `mark.spec(...)` marker naming ids.
pub fn parse_test_bindings(
    files: &[(String, Vec<u8>)],
) -> Vec<medulla_core::lifecycle::derive::TestBinding> {
    let mut bound: BTreeSet<(String, String)> = BTreeSet::new();
    for (path, bytes) in files {
        if !is_binding_source(path) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        for line in text.lines() {
            let mut ids: Vec<&str> = Vec::new();
            for (at, _) in line.match_indices("spec:") {
                let before = line[..at].trim_end();
                if !(before.ends_with("//") || before.ends_with('#')) {
                    continue;
                }
                // The comma-separated id list that follows the marker; it stops at the first
                // token that is not a row id.
                let mut rest = line[at + "spec:".len()..].trim_start();
                loop {
                    let end = rest
                        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
                        .unwrap_or(rest.len());
                    let token = rest[..end].trim_end_matches('.');
                    if !is_row_id(token) {
                        break;
                    }
                    ids.push(token);
                    match rest[end..].trim_start().strip_prefix(',') {
                        Some(next) => rest = next.trim_start(),
                        None => break,
                    }
                }
            }
            for (at, _) in line.match_indices("mark.spec(") {
                let body = &line[at + "mark.spec(".len()..];
                ids.extend(row_ids_in(body.split(')').next().unwrap_or_default()));
            }
            for id in ids {
                bound.insert((id.to_string(), path.clone()));
            }
        }
    }
    bound
        .into_iter()
        .map(
            |(requirement, test)| medulla_core::lifecycle::derive::TestBinding {
                requirement,
                test,
            },
        )
        .collect()
}

/// The node id of a bound test file of one repository.
pub fn test_file_node_id(repo: &str, path: &str) -> String {
    format!("testfile:{repo}:{path}")
}

/// The node id of one seam file of one repository.
pub fn seam_node_id(repo: &str, path: &str) -> String {
    format!("seam:{repo}:{path}")
}

/// The inputs of [`link_history_to_rows`] (MEDULLA-SDLC-R014.12).
pub struct SpecHistoryInput<'a> {
    pub repo: &'a str,
    /// The default-branch head commit id.
    pub rev: &'a str,
    /// The repository's row ids, in order.
    pub ids: &'a [String],
    pub links: &'a [SpecRowLinks],
    /// The whole default-branch chain, oldest first.
    pub chain: &'a [Changeset],
    /// The delta slice: the only changesets whose `implementedByCommit` and `commitHash`
    /// operations are emitted. On a first ingest it equals `chain`.
    pub new_changesets: &'a [Changeset],
    pub bindings: &'a [medulla_core::lifecycle::derive::TestBinding],
    /// The test file paths on the default branch at the head.
    pub existing_test_paths: &'a BTreeSet<String>,
}

/// Link rows to commits, tests, dependencies and seams, and derive each row status
/// (MEDULLA-SDLC-R014.12).
///
/// A `Spec:` trailer gives one `implementedByCommit` edge per row id, from the requirement
/// node to the changeset node, and the changeset node gains `commitHash`; a native
/// changeset (no git origin) gets neither. A bound test path gives a `boundTest` edge to
/// the `testfile:` node, and a `verifiedBy` edge too when the path exists at the head. Each
/// row gets one `rowStatus` property from `derive_spec_status`. A `Depends:` id that is a
/// row of the repository gives a `dependsOnRow` edge; a seam side gives `seamProvider` or
/// `seamConsumer`. A trailer id that is no row refuses with `REQUIREMENT_UNRESOLVED` and
/// no operation is produced.
pub fn link_history_to_rows(input: &SpecHistoryInput<'_>) -> Result<RequirementProjection, String> {
    let rows: BTreeSet<&str> = input.ids.iter().map(String::as_str).collect();
    let (mut nodes, mut links) = (Vec::new(), Vec::new());
    for changeset in input.new_changesets {
        let source = changeset_node_id(input.repo, &changeset.id);
        let mut properties = serde_json::json!({
            "type": "Changeset",
            "repo": input.repo,
            "changeset_id": changeset.id,
        });
        let commit = changeset.git.as_ref().map(|git| git.commit_id.as_str());
        if let Some(commit) = commit {
            properties["commitHash"] = commit.into();
        }
        for row_id in &changeset.trailers.spec_ids {
            if !rows.contains(row_id.as_str()) {
                return Err(format!(
                    "{}: {row_id} (cited by changeset {}) resolves to no row",
                    EngineErrorCode::RequirementUnresolved.as_str(),
                    changeset.id
                ));
            }
            if commit.is_some() {
                links.push(edge(
                    &requirement_node_id(input.repo, row_id),
                    &source,
                    "implementedByCommit",
                ));
            }
        }
        if commit.is_some() {
            nodes.push(node(&source, properties));
        }
    }
    let mut test_nodes: BTreeSet<&str> = BTreeSet::new();
    for binding in input.bindings {
        if !rows.contains(binding.requirement.as_str()) {
            continue;
        }
        let source = requirement_node_id(input.repo, &binding.requirement);
        let target = test_file_node_id(input.repo, &binding.test);
        if test_nodes.insert(binding.test.as_str()) {
            nodes.push(node(
                &target,
                serde_json::json!({"type": "TestFile", "path": binding.test, "repo": input.repo}),
            ));
        }
        links.push(edge(&source, &target, "boundTest"));
        if input.existing_test_paths.contains(&binding.test) {
            links.push(edge(&source, &target, "verifiedBy"));
        }
    }
    let status = medulla_core::lifecycle::status::derive_spec_status(
        input.repo,
        input.rev,
        input.ids,
        input.chain,
        input.bindings,
    );
    for row in &status.rows {
        let state = serde_json::to_value(row.state).map_err(|error| error.to_string())?;
        nodes.push(node(
            &requirement_node_id(input.repo, &row.id),
            serde_json::json!({
                "type": "Requirement",
                "rowId": row.id,
                "repo": input.repo,
                "rowStatus": state,
            }),
        ));
    }
    let mut seam_nodes: BTreeSet<&str> = BTreeSet::new();
    for row_links in input.links {
        if !rows.contains(row_links.row_id.as_str()) {
            continue;
        }
        let source = requirement_node_id(input.repo, &row_links.row_id);
        for dependency in &row_links.depends_on {
            if rows.contains(dependency.as_str()) {
                links.push(edge(
                    &source,
                    &requirement_node_id(input.repo, dependency),
                    "dependsOnRow",
                ));
            }
        }
        for (paths, relationship) in [
            (&row_links.seam_provider, "seamProvider"),
            (&row_links.seam_consumer, "seamConsumer"),
        ] {
            for path in paths {
                let target = seam_node_id(input.repo, path);
                if seam_nodes.insert(path.as_str()) {
                    nodes.push(node(
                        &target,
                        serde_json::json!({"type": "SeamSide", "path": path, "repo": input.repo}),
                    ));
                }
                links.push(edge(&source, &target, relationship));
            }
        }
    }
    let (count_nodes, count_edges) = (nodes.len() as u32, links.len() as u32);
    nodes.extend(links);
    Ok(RequirementProjection {
        operations: nodes,
        nodes: count_nodes,
        edges: count_edges,
    })
}

/// The artifact entry whose records are `row=STATE`: the `rowStatus` last written per row,
/// which a delta pass compares with the status it derives (MEDULLA-SDLC-R014.12).
pub const ROW_STATUS_ARTIFACT: &str = "rowstatus";

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
    // First pass: parse and project every changed file, so a plan document can see the
    // requirement records of the whole changed tree (MEDULLA-SDLC-R014.7).
    let mut changed: Vec<(&str, &str, String, RequirementProjection)> = Vec::new();
    for (name, text) in files {
        let digest = digest_of(text);
        if stored.get(*name).is_some_and(|old| old.digest == digest) {
            unchanged += 1;
            continue;
        }
        let triples = parse_spec_file(name, text).map_err(|refusal| refusal.to_string())?;
        let projection = project_requirements_in(repo, &triples, states, external, unresolved)?;
        changed.push((name, text, digest, projection));
    }
    let mut known_rows: BTreeSet<String> = external.values().cloned().collect();
    let requirement_prefix = requirement_node_id(repo, "");
    for (name, old) in stored {
        if !changed.iter().any(|(changed_name, ..)| *changed_name == name.as_str()) {
            known_rows.extend(
                old.records
                    .iter()
                    .filter(|id| id.starts_with(&requirement_prefix))
                    .cloned(),
            );
        }
    }
    // The principles of the repository: those its stored artifacts hold and those the changed
    // constitution defines (MEDULLA-SDLC-R014.6.2).
    let principle_prefix = principle_node_id(repo, "");
    let mut known_principles: BTreeSet<String> = BTreeSet::new();
    for (name, old) in stored {
        if !changed.iter().any(|(changed_name, ..)| *changed_name == name.as_str()) {
            known_principles.extend(
                old.records
                    .iter()
                    .filter(|id| id.starts_with(&principle_prefix))
                    .cloned(),
            );
        }
    }
    for (name, text, _, _) in &changed {
        if is_constitution_file(name) {
            known_principles.extend(
                project_principles(repo, name, text)
                    .operations
                    .iter()
                    .filter_map(|op| op["id"].as_str().map(str::to_string)),
            );
        }
    }
    for (_, _, _, projection) in &changed {
        known_rows.extend(
            projection
                .operations
                .iter()
                .filter(|op| op["op"] == "upsert_node" && op["properties"]["type"] == "Requirement")
                .filter_map(|op| op["id"].as_str().map(str::to_string)),
        );
    }
    for (name, text, digest, projection) in changed {
        let old = stored.get(name);
        let plan_operations = if is_plan_file(name) {
            project_plans(repo, name, text, &known_rows)?.operations
        } else {
            Vec::new()
        };
        let mut task_operations = if is_tasks_file(name) {
            project_tasks(repo, name, text, &known_rows).operations
        } else {
            Vec::new()
        };
        if is_constitution_file(name) {
            task_operations.extend(project_principles(repo, name, text).operations);
        }
        task_operations.extend(governed_by_edges(repo, &projection.operations, &known_principles)?);
        let mut records = Vec::new();
        for op in projection
            .operations
            .into_iter()
            .chain(project(repo, name, text))
            .chain(plan_operations)
            .chain(task_operations)
        {
            if op["op"] == "upsert_node" {
                if matches!(
                    op["properties"]["type"].as_str(),
                    Some(
                    "Requirement" | "SpecDocument" | "SpecSection" | "PlanSection" | "TaskLine"
                        | "ConstitutionPrinciple"
                )
                ) {
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
    (path.ends_with(".md") && (path.starts_with("specs/") || path.contains("/specs/")))
        || is_constitution_file(path)
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

/// The classification of the spec files of one batch (MEDULLA-ORCH-VOCAB-R012.4).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClassifiedSpecFiles {
    /// One `upsert_node` per classified or unknown file.
    pub operations: Vec<serde_json::Value>,
    /// The paths no rule matched, in batch order.
    pub unknown: Vec<String>,
}

/// The id of the node that records how a spec file was classified.
pub fn spec_kind_node_id(repo: &str, path: &str) -> String {
    format!("speckind:{repo}:{path}")
}

/// Classify every spec file of a batch by the named rules of `classify_spec_file_by_rule`
/// (MEDULLA-ORCH-VOCAB-R012.4, R012.5). A considered file is a `.md` file that satisfies
/// [`is_spec_artifact`] or has a `.specify` path segment; a path that repeats is read once at
/// its first occurrence and a file that is not UTF-8 is skipped. A file with a kind becomes a
/// node of its class with `repo`, `path`, `classifiedByRule` and `blob_digest`. A file that
/// matches no rule is written as a plain `Document` with no kind and no rule, and its path is
/// listed in `unknown`: an unknown file is not guessed.
pub fn classify_spec_files(repo: &str, files: &[(String, Vec<u8>)]) -> ClassifiedSpecFiles {
    use medulla_core::spec_folder::{classify_spec_file_by_rule, parse_front_matter};

    let mut seen = BTreeSet::new();
    let mut out = ClassifiedSpecFiles::default();
    for (path, bytes) in files {
        let considered = path.ends_with(".md")
            && (is_spec_artifact(path) || path.split('/').any(|segment| segment == ".specify"));
        if !considered || !seen.insert(path.as_str()) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        let blob_digest = digest_of(text);
        let id = spec_kind_node_id(repo, path);
        match classify_spec_file_by_rule(path, &parse_front_matter(text)) {
            Ok((kind, rule)) => {
                let class = kind.class_name();
                out.operations.push(node(
                    &id,
                    serde_json::json!({
                        "type": class,
                        "repo": repo,
                        "path": path,
                        "classifiedByRule": rule,
                        "blob_digest": blob_digest,
                    }),
                ));
            }
            Err(_) => {
                out.operations.push(node(
                    &id,
                    serde_json::json!({
                        "type": "Document",
                        "repo": repo,
                        "path": path,
                        "blob_digest": blob_digest,
                    }),
                ));
                out.unknown.push(path.clone());
            }
        }
    }
    out
}

/// Append the classification to the index result: its nodes, and for each unknown file the
/// `SPEC_FILE_UNKNOWN_KIND` diagnostic on that file's outcome. An outcome that is absent or
/// whose diagnostics are full is skipped, and the verdict never changes.
fn append_classified(
    classified: &ClassifiedSpecFiles,
    result: &mut medulla_ingestion_types::ingestion_wire::IndexResult,
) {
    use medulla_ingestion_types::ingestion_wire::IndexDiagnostic;
    use medulla_types::contract::BoundedVec;

    for op in &classified.operations {
        result.nodes.push(index_node(op));
    }
    for path in &classified.unknown {
        let Some(outcome) = result
            .file_outcomes
            .iter_mut()
            .find(|outcome| outcome.file_path == *path)
        else {
            continue;
        };
        let mut diagnostics = outcome.diagnostics.as_slice().to_vec();
        diagnostics.push(IndexDiagnostic {
            code: "SPEC_FILE_UNKNOWN_KIND".to_owned(),
            message: path.clone(),
        });
        if let Ok(bounded) = BoundedVec::new(diagnostics) {
            outcome.diagnostics = bounded;
        }
    }
}

/// Append the policy candidates of the files `classify_spec_files` typed `SpecPolicy`
/// (MEDULLA-ORCH-VOCAB-R012.7.2): one `PolicyCandidate` node per clause, `proposed` or
/// `inactive`. Nothing is activated and no `ProposeRule` is written.
fn append_policy_candidates(
    repo: &str,
    commit: &str,
    files: &[(String, Vec<u8>)],
    classified: &ClassifiedSpecFiles,
    result: &mut medulla_ingestion_types::ingestion_wire::IndexResult,
) {
    let policy_paths: Vec<String> = classified
        .operations
        .iter()
        .filter(|op| op["properties"]["type"] == "SpecPolicy")
        .filter_map(|op| op["properties"]["path"].as_str().map(str::to_owned))
        .collect();
    for op in super::policy_candidates::synthesize_policy_candidates(
        repo,
        commit,
        files,
        &policy_paths,
    ) {
        result
            .nodes
            .push(super::policy_candidates::candidate_index_node(&op));
    }
}

/// The `SourceIngest` and `IndexRepository` worker loop's entry for spec artifacts
/// (MEDULLA-SDLC-R014.2): called with the files of one scoped repository batch, it
/// re-ingests only the spec artifact files whose digest differs from `stored` and lowers
/// the records into `result`, so they commit in the same change envelope as the rest of
/// the index. A path that repeats in `files` is read once, at its first occurrence. A
/// stored file that is absent from this batch is not removed here; the batch need not
/// carry every file, and the returned plan names `removed` for a caller that holds the
/// full tree. `chain` is the whole default-branch changeset chain, oldest first, and
/// `changesets` its delta after the last cited changeset (MEDULLA-SDLC-R014.12).
pub fn project_spec_artifacts(
    repo: &str,
    files: &[(String, Vec<u8>)],
    stored: &BTreeMap<String, StoredArtifact>,
    chain: &[Changeset],
    changesets: &[Changeset],
    branch: &str,
    commit: &str,
    result: &mut medulla_ingestion_types::ingestion_wire::IndexResult,
) -> Result<ReingestPlan, String> {
    let mut seen = BTreeSet::new();
    let texts: Vec<(&str, &str)> = files
        .iter()
        .filter(|(path, _)| is_spec_artifact(path) && seen.insert(path.as_str()))
        .filter_map(|(path, bytes)| Some((path.as_str(), std::str::from_utf8(bytes).ok()?)))
        .collect();
    let classified = classify_spec_files(repo, files);
    if texts.is_empty() && changesets.is_empty() {
        append_classified(&classified, result);
        append_policy_candidates(repo, commit, files, &classified, result);
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
    let mut plan = if texts.is_empty() {
        ReingestPlan {
            operations: Vec::new(),
            reingested: Vec::new(),
            removed: Vec::new(),
            unchanged: 0,
        }
    } else {
        reingest_in(
            repo,
            &texts,
            &carried,
            &BTreeMap::new(),
            &BTreeMap::new(),
            Unresolved::Skip,
        )?
    };
    // The requirement node ids of this plan plus the ones already stored.
    let requirement_prefix = requirement_node_id(repo, "");
    let mut known: BTreeSet<String> = stored
        .values()
        .flat_map(|artifact| artifact.records.iter())
        .filter(|id| id.starts_with(&requirement_prefix))
        .cloned()
        .collect();
    known.extend(
        plan.operations
            .iter()
            .filter(|op| op["op"] == "upsert_node" && op["properties"]["type"] == "Requirement")
            .filter_map(|op| op["id"].as_str().map(str::to_string)),
    );
    if let Some(last) = changesets.last() {
        let cited = cite_requirements(repo, changesets, &known)?;
        let mut records: Vec<String> = stored
            .get(CHANGESETS_ARTIFACT)
            .map(|old| old.records.clone())
            .unwrap_or_default();
        let mut writes = Vec::new();
        let mut links = Vec::new();
        for mut op in cited.operations {
            if op["op"] == "upsert_node" {
                op["properties"]["branch"] = branch.into();
                let id = op["id"].as_str().unwrap_or_default().to_string();
                if !records.contains(&id) {
                    records.push(id);
                }
                writes.push(op);
            } else {
                links.push(op);
            }
        }
        writes.push(node(
            &artifact_node_id(repo, CHANGESETS_ARTIFACT),
            serde_json::json!({
                "type": "SpecArtifactFile",
                "repo": repo,
                "file": CHANGESETS_ARTIFACT,
                "digest": last.id,
                "records": records.join(","),
            }),
        ));
        // Changeset nodes join the plan's nodes; their edges follow every node.
        let mut ordered: Vec<serde_json::Value> = plan
            .operations
            .iter()
            .filter(|op| op["op"] == "upsert_node")
            .cloned()
            .collect();
        ordered.extend(writes);
        ordered.extend(plan.operations.iter().filter(|op| op["op"] != "upsert_node").cloned());
        ordered.extend(links);
        plan.operations = ordered;
    }
    // MEDULLA-SDLC-R014.12: rows link to commits, tests, dependencies and seams, and each
    // row carries its derived status.
    let ids: Vec<String> = known
        .iter()
        .filter_map(|id| id.strip_prefix(&requirement_prefix).map(str::to_string))
        .collect();
    let links = parse_row_links(files);
    let bindings = parse_test_bindings(files);
    let existing_test_paths: BTreeSet<String> = files
        .iter()
        .map(|(path, _)| path)
        .filter(|path| is_binding_source(path))
        .cloned()
        .collect();
    let history = link_history_to_rows(&SpecHistoryInput {
        repo,
        rev: commit,
        ids: &ids,
        links: &links,
        chain,
        new_changesets: changesets,
        bindings: &bindings,
        existing_test_paths: &existing_test_paths,
    })?;
    // A delta pass re-emits the history of a row only when its file was rewritten, its
    // derived status differs from the stored one, or a new changeset cites it.
    let rewritten: BTreeSet<&str> = plan
        .operations
        .iter()
        .filter(|op| op["op"] == "upsert_node" && op["properties"]["type"] == "Requirement")
        .filter_map(|op| op["id"].as_str())
        .collect();
    let stored_status: BTreeMap<&str, &str> = stored
        .get(ROW_STATUS_ARTIFACT)
        .map(|artifact| {
            artifact
                .records
                .iter()
                .filter_map(|record| record.split_once('='))
                .collect()
        })
        .unwrap_or_default();
    let mut touched: BTreeSet<String> = rewritten.iter().map(|id| id.to_string()).collect();
    touched.extend(
        changesets
            .iter()
            .flat_map(|changeset| changeset.trailers.spec_ids.iter())
            .map(|row| requirement_node_id(repo, row)),
    );
    let mut statuses: Vec<String> = Vec::new();
    for op in &history.operations {
        let properties = &op["properties"];
        if let (Some(row), Some(state)) = (properties["rowId"].as_str(), properties["rowStatus"].as_str()) {
            statuses.push(format!("{row}={state}"));
            if stored_status.get(row) != Some(&state) {
                touched.insert(requirement_node_id(repo, row));
            }
        }
    }
    let mut kept: Vec<serde_json::Value> = Vec::new();
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for op in &history.operations {
        if op["op"] == "upsert_edge" {
            if op["source"].as_str().is_some_and(|source| touched.contains(source)) {
                referenced.extend(op["target"].as_str().map(str::to_string));
                kept.push(op.clone());
            }
        }
    }
    let mut history_ops: Vec<serde_json::Value> = Vec::new();
    for op in &history.operations {
        if op["op"] != "upsert_node" {
            continue;
        }
        let keep = match op["properties"]["type"].as_str() {
            Some("Requirement") => op["id"].as_str().is_some_and(|id| touched.contains(id)),
            Some("TestFile" | "SeamSide") => op["id"].as_str().is_some_and(|id| referenced.contains(id)),
            _ => true,
        };
        if keep {
            history_ops.push(op.clone());
        }
    }
    if history_ops.iter().any(|op| op["properties"]["type"] == "Requirement") {
        history_ops.push(node(
            &artifact_node_id(repo, ROW_STATUS_ARTIFACT),
            serde_json::json!({
                "type": "SpecArtifactFile",
                "repo": repo,
                "file": ROW_STATUS_ARTIFACT,
                "digest": digest_of(&statuses.join(",")),
                "records": statuses.join(","),
            }),
        ));
    }
    history_ops.extend(kept);
    // The index envelope commits a node with `AddNode`, which replaces the stored property
    // set. A Requirement written only for its history therefore carries the whole set
    // its file projects, so the row's text and other properties survive.
    if history_ops.iter().any(|op| {
        op["properties"]["type"] == "Requirement"
            && op["id"].as_str().is_some_and(|id| !rewritten.contains(id))
    }) {
        let mut full: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        for (name, text) in &texts {
            let triples = parse_spec_file(name, text).map_err(|refusal| refusal.to_string())?;
            let projection = project_requirements_in(
                repo,
                &triples,
                &BTreeMap::new(),
                &BTreeMap::new(),
                Unresolved::Skip,
            )?;
            for op in projection.operations {
                if op["op"] == "upsert_node" && op["properties"]["type"] == "Requirement" {
                    if let Some(id) = op["id"].as_str() {
                        full.insert(id.to_string(), op["properties"].clone());
                    }
                }
            }
        }
        for op in &mut history_ops {
            if op["properties"]["type"] != "Requirement" {
                continue;
            }
            let Some(id) = op["id"].as_str().map(str::to_string) else {
                continue;
            };
            if rewritten.contains(id.as_str()) {
                continue;
            }
            if let (Some(base), Some(extra)) = (full.get(&id), op["properties"].as_object().cloned()) {
                let mut merged = base.clone();
                for (key, value) in extra {
                    merged[key] = value;
                }
                op["properties"] = merged;
            }
        }
    }
    merge_history(&mut plan.operations, history_ops);
    for op in &plan.operations {
        match op["op"].as_str() {
            Some("upsert_node") => result.nodes.push(index_node(op)),
            Some("upsert_edge") => result.edges.push(index_edge(op)),
            _ => {}
        }
    }
    append_classified(&classified, result);
    append_policy_candidates(repo, commit, files, &classified, result);
    Ok(plan)
}

/// Fold the history operations into the plan: a node the plan already writes gains the
/// new properties in place, so a Requirement or Changeset node is written once.
fn merge_history(plan: &mut Vec<serde_json::Value>, history: Vec<serde_json::Value>) {
    let (history_nodes, history_edges): (Vec<_>, Vec<_>) = history
        .into_iter()
        .partition(|op| op["op"] == "upsert_node");
    let mut extra_nodes = Vec::new();
    for op in history_nodes {
        let existing = plan
            .iter_mut()
            .find(|candidate| candidate["op"] == "upsert_node" && candidate["id"] == op["id"]);
        match (existing, op["properties"].as_object()) {
            (Some(existing), Some(properties)) => {
                for (key, value) in properties {
                    existing["properties"][key] = value.clone();
                }
            }
            _ => extra_nodes.push(op),
        }
    }
    let first_edge = plan
        .iter()
        .position(|op| op["op"] != "upsert_node")
        .unwrap_or(plan.len());
    plan.splice(first_edge..first_edge, extra_nodes);
    plan.extend(history_edges);
}

fn index_node(op: &serde_json::Value) -> medulla_ingestion_types::ingestion_wire::ExtractedNode {
    let properties: std::collections::HashMap<String, String> = op["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
        .collect();
    medulla_ingestion_types::ingestion_wire::ExtractedNode {
        node_id: op["id"].as_str().unwrap_or_default().to_owned(),
        node_type: properties.get("type").cloned().unwrap_or_default(),
        properties,
    }
}

fn index_edge(op: &serde_json::Value) -> medulla_ingestion_types::ingestion_wire::ExtractedEdge {
    medulla_ingestion_types::ingestion_wire::ExtractedEdge {
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

    fn as_refs<'a>(files: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
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
        use medulla_ingestion_types::ingestion_wire::IndexResult;

        let files = vec![
            (
                "specs/a/requirements.md".to_string(),
                REQUIREMENTS.as_bytes().to_vec(),
            ),
            ("src/lib.rs".to_string(), b"fn main() {}".to_vec()),
        ];
        let mut first = IndexResult::default();
        let plan =
            project_spec_artifacts("demo", &files, &BTreeMap::new(), &[], &[], "main", "c0ffee", &mut first)
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
        let idle = project_spec_artifacts("demo", &files, &stored, &[], &[], "main", "c0ffee", &mut again)
            .expect("idle");
        assert!(idle.reingested.is_empty());
        assert!(again.nodes.is_empty() && again.edges.is_empty());
    }

    // spec: MEDULLA-SDLC-R009
    #[test]
    fn spec_document_projects_sections_with_spans() {
        let text =
            "# Title\n\nintro\n\n## Rows\n\n```\n# not a heading\n```\nbody\n\n## Tasks\nlast\n";
        let ops = project("demo", "specs/a/plan.md", text);
        let doc = &ops[0];
        assert_eq!(doc["id"], "specdoc:demo:specs/a/plan.md");
        assert_eq!(doc["properties"]["type"], "SpecDocument");
        assert_eq!(doc["properties"]["digest"], digest_of(text).as_str());
        let sections: Vec<_> = ops
            .iter()
            .filter(|op| op["properties"]["type"] == "SpecSection")
            .collect();
        for section in &sections {
            let p = &section["properties"];
            println!(
                "SECTION\t{}\t{}\t{}\t{}",
                p["title"].as_str().unwrap().replace(' ', "_"),
                p["level"].as_str().unwrap(),
                p["start_line"].as_str().unwrap(),
                p["end_line"].as_str().unwrap()
            );
        }
        let spans: Vec<(String, String)> = sections
            .iter()
            .map(|s| {
                (
                    s["properties"]["start_line"].as_str().unwrap().to_string(),
                    s["properties"]["end_line"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            spans,
            [("1", "4"), ("5", "11"), ("12", "13")].map(|(a, b)| (a.to_string(), b.to_string()))
        );
        let edges = ops.iter().filter(|op| op["op"] == "upsert_edge").count();
        assert_eq!(edges, 3);
        assert_eq!(
            project("demo", "specs/a/plan.md", text),
            ops,
            "deterministic"
        );
    }

    // spec: MEDULLA-SDLC-R014.6.1
    #[test]
    fn tasks_project_to_nodes_with_phase_and_row_edge() {
        let tasks = "# Tasks\n\n- [ ] T000 [P] DEMO-R009 before any phase\n\n\
## Phase 1: Setup\n- [ ] T001 [P] [US1] DEMO-R001 Wire it\n- [x] T002 DEMO-R404 Gone\n\
- [ ] DEMO-R001 a row-only task has no T-id\n\n## Phase 2: Build\n- [X] T003 plain text\n";
        let known = BTreeSet::from([requirement_node_id("demo", "DEMO-R001")]);
        let p = project_tasks("demo", "specs/x/tasks.md", tasks, &known);
        let nodes: Vec<&serde_json::Value> = p
            .operations
            .iter()
            .filter(|op| op["op"] == "upsert_node")
            .collect();
        let ids: Vec<&str> = nodes.iter().map(|n| n["id"].as_str().unwrap()).collect();
        assert_eq!(
            ids,
            [
                "task:demo:specs/x/tasks.md#T000",
                "task:demo:specs/x/tasks.md#T001",
                "task:demo:specs/x/tasks.md#T002",
                "task:demo:specs/x/tasks.md#T003",
            ]
        );
        assert!(nodes[0]["properties"].get("phase").is_none());
        let t1 = &nodes[1]["properties"];
        assert_eq!(t1["type"], "TaskLine");
        assert_eq!(t1["taskId"], "T001");
        assert_eq!(t1["parallel"], "true");
        assert_eq!(t1["story"], "US1");
        assert_eq!(t1["row"], "DEMO-R001");
        assert_eq!(t1["ticked"], "false");
        assert_eq!(t1["text"], "Wire it");
        assert_eq!(t1["file"], "specs/x/tasks.md");
        assert_eq!(t1["phase"], "1");
        assert_eq!(nodes[2]["properties"]["ticked"], "true");
        assert_eq!(nodes[3]["properties"]["phase"], "2");
        assert_eq!(nodes[3]["properties"]["row"], "");
        // Only the task whose row resolves gets a `forRow` edge.
        assert_eq!(
            edges_of(&p, "task:demo:specs/x/tasks.md#T001", "forRow"),
            vec![requirement_node_id("demo", "DEMO-R001")]
        );
        assert_eq!((p.nodes, p.edges), (4, 1));
        // A re-projection of the same text is byte-equal, so an unchanged digest rewrites nothing.
        assert_eq!(project_tasks("demo", "specs/x/tasks.md", tasks, &known).operations, p.operations);
    }

    // spec: MEDULLA-SDLC-R014.6.2
    #[test]
    fn principles_project_and_rows_link_through_governed_by() {
        let constitution = "# Constitution\n\n## Article XXXI: Gates\nBody.\n\n\
### Principle VII — Specs first\nMore.\n\n## Governance\nNot a principle.\n\
```\n## Article IX: in a fence\n```\n";
        let p = project_principles("demo", ".specify/memory/constitution.md", constitution);
        let nodes: Vec<&serde_json::Value> = p.operations.iter().collect();
        let ids: Vec<&str> = nodes.iter().map(|n| n["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["principle:demo:XXXI", "principle:demo:VII"]);
        assert_eq!(nodes[0]["properties"]["type"], "ConstitutionPrinciple");
        assert_eq!(nodes[0]["properties"]["key"], "XXXI");
        assert_eq!(nodes[0]["properties"]["title"], "Article XXXI: Gates");
        assert_eq!(nodes[0]["properties"]["repo"], "demo");
        assert_eq!(nodes[0]["properties"]["digest"].as_str().unwrap().len(), 64);
        assert_eq!((p.nodes, p.edges), (2, 0));

        // Keys are comma separated, backticks stripped; a repeat counts once.
        assert_eq!(
            cited_principles("Does x. Principle: `XXXI`, VII, XXXI. More text."),
            ["XXXI", "VII"]
        );
        assert!(cited_principles("no citation here").is_empty());

        let known = BTreeSet::from([
            principle_node_id("demo", "XXXI"),
            principle_node_id("demo", "VII"),
        ]);
        let row = |text: &str| {
            vec![node(
                &requirement_node_id("demo", "DEMO-R001"),
                serde_json::json!({"type": "Requirement", "text": text}),
            )]
        };
        let edges = governed_by_edges("demo", &row("Principle: XXXI, VII"), &known).expect("resolves");
        let pairs: Vec<(&str, &str)> = edges
            .iter()
            .map(|e| (e["source"].as_str().unwrap(), e["target"].as_str().unwrap()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("requirement:demo:DEMO-R001", "principle:demo:XXXI"),
                ("requirement:demo:DEMO-R001", "principle:demo:VII")
            ]
        );
        assert!(edges.iter().all(|e| e["properties"]["relationship"] == "governedBy"));
        let refusal = governed_by_edges("demo", &row("Principle: XXXI, LX"), &known).unwrap_err();
        assert!(refusal.starts_with("PRINCIPLE_UNRESOLVED: LX"));
    }

    // spec: MEDULLA-SDLC-R014.7
    #[test]
    fn plan_sections_carry_phase_and_cover_rows_and_unknown_phase_refuses() {
        let plan = "# Plan\n\n## Technical context\nUses `DEMO-R001`.\n\n\
## Phase 1: Design\nCovers `DEMO-R002` and `OTHER-R009` and `DEMO-R002` again.\n\
```\n## Phase 9: in a fence\n```\n";
        let known = BTreeSet::from([
            requirement_node_id("demo", "DEMO-R001"),
            requirement_node_id("demo", "DEMO-R002"),
        ]);
        let p = project_plans("demo", "specs/x/plan.md", plan, &known).expect("projects");
        let sections: Vec<&serde_json::Value> = p
            .operations
            .iter()
            .filter(|op| op["op"] == "upsert_node")
            .collect();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0]["id"], "plansection:demo:specs/x/plan.md#L3");
        assert_eq!(sections[0]["properties"]["type"], "PlanSection");
        assert_eq!(sections[0]["properties"]["title"], "Technical context");
        assert!(sections[0]["properties"].get("phase").is_none());
        assert_eq!(sections[1]["properties"]["phase"], "1");
        assert_eq!(sections[1]["properties"]["start_line"], "6");
        assert_eq!(
            edges_of(&p, "plansection:demo:specs/x/plan.md#L3", "covers"),
            vec![requirement_node_id("demo", "DEMO-R001")]
        );
        assert_eq!(
            edges_of(&p, "plansection:demo:specs/x/plan.md#L6", "covers"),
            vec![requirement_node_id("demo", "DEMO-R002")]
        );
        assert_eq!((p.nodes, p.edges), (2, 2));

        let bad = "## Phase 7: Nope\n";
        let refusal = project_plans("demo", "specs/x/plan.md", bad, &known).unwrap_err();
        assert_eq!(refusal, "PLAN_PHASE_UNKNOWN: specs/x/plan.md:1");
    }

    // spec: MEDULLA-SDLC-R014.7
    #[test]
    fn a_changed_plan_file_reingests_its_sections_with_covers_edges() {
        let plan = "## Phase 0: Research\nSee `DEMO-R001`.\n";
        let files = [
            ("specs/a/requirements.md", REQUIREMENTS),
            ("specs/a/plan.md", plan),
        ];
        let first = reingest_changed_artifacts(
            "demo",
            &files,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &external(),
        )
        .expect("reingests");
        let section = "plansection:demo:specs/a/plan.md#L1";
        assert!(first
            .operations
            .iter()
            .any(|op| op["op"] == "upsert_node" && op["id"] == section));
        assert!(first.operations.iter().any(|op| op["op"] == "upsert_edge"
            && op["source"] == section
            && op["target"] == requirement_node_id("demo", "DEMO-R001").as_str()
            && op["properties"]["relationship"] == "covers"));
        let file_node = first
            .operations
            .iter()
            .find(|op| op["id"] == artifact_node_id("demo", "specs/a/plan.md").as_str())
            .expect("plan file node");
        assert!(file_node["properties"]["records"]
            .as_str()
            .unwrap()
            .split(',')
            .any(|id| id == section));
    }

    /// MEDULLA-SDLC-R014.12: binding markers are exactly the forms of the spec-status gate.
    #[test]
    fn test_bindings_use_the_gate_forms() {
        let files: Vec<(String, Vec<u8>)> = vec![
            ("src/a.rs".into(), b"// spec: DEMO-R001, DEMO-R002\n".to_vec()),
            ("tests/b.py".into(), b"#spec: DEMO-R003\n".to_vec()),
            ("tests/c.py".into(), b"@pytest.mark.spec(\"DEMO-R004\", \"DEMO-R005\")\n".to_vec()),
            // Not forms of the gate: a bare `spec:` word, the `spec(\"..\")` call, markdown, JSON.
            ("tests/d.py".into(), b"x = \"spec: DEMO-R006\"\nspec(\"DEMO-R007\")\n".to_vec()),
            ("specs/a/e.rs".into(), b"// spec: DEMO-R008\n".to_vec()),
            ("notes.md".into(), b"// spec: DEMO-R009\n".to_vec()),
            ("data.json".into(), b"// spec: DEMO-R010\n".to_vec()),
        ];
        let pairs: Vec<(String, String)> = parse_test_bindings(&files)
            .into_iter()
            .map(|b| (b.requirement, b.test))
            .collect();
        let expect = |id: &str, path: &str| (id.to_string(), path.to_string());
        assert_eq!(
            pairs,
            vec![
                expect("DEMO-R001", "src/a.rs"),
                expect("DEMO-R002", "src/a.rs"),
                expect("DEMO-R003", "tests/b.py"),
                expect("DEMO-R004", "tests/c.py"),
                expect("DEMO-R005", "tests/c.py"),
            ]
        );
    }

    fn changeset(id: &str, created_at: u64, spec_ids: &[&str]) -> Changeset {
        Changeset {
            id: id.to_string(),
            repo: "demo".to_string(),
            parents: Vec::new(),
            author_principal: "dev".to_string(),
            message: "change".to_string(),
            changed_paths: Vec::new(),
            created_at,
            trailers: medulla_source_types::source_changeset::Trailers {
                spec_ids: spec_ids.iter().map(|id| id.to_string()).collect(),
                ..Default::default()
            },
            agent_identity: None,
            run_trace_id: None,
            git: None,
            symbol_edit: None,
        }
    }

    // spec: MEDULLA-SDLC-R014.12
    #[test]
    fn history_links_rows_to_commits_tests_and_dependencies() {
        use medulla_source_types::source_changeset::{GitObjectFormat, GitOrigin};

        let requirements = format!("{REQUIREMENTS}Depends: DEMO-R001\n");
        let files: Vec<(String, Vec<u8>)> = vec![
            ("specs/a/requirements.md".into(), requirements.into_bytes()),
            (
                "specs/a/contracts/seam.md".into(),
                b"# Seam\nProvider: demo DEMO-R001\nConsumer: other OTHER-R002\nConsumer: demo DEMO-R003.\n"
                    .to_vec(),
            ),
            (
                "tests/demo/test_base.py".into(),
                b"@pytest.mark.spec(\"DEMO-R001\")\n".to_vec(),
            ),
            (
                "tests/demo/test_third.py".into(),
                b"# spec: DEMO-R003\n".to_vec(),
            ),
        ];
        let links = parse_row_links(&files);
        let of = |id: &str| links.iter().find(|l| l.row_id == id).expect("row links");
        assert_eq!(of("DEMO-R002").depends_on, ["DEMO-R001", "OTHER-R009"]);
        assert_eq!(of("DEMO-R003").depends_on, ["DEMO-R001"]);
        assert_eq!(of("DEMO-R001").seam_provider, ["specs/a/contracts/seam.md"]);
        assert_eq!(of("DEMO-R003").seam_consumer, ["specs/a/contracts/seam.md"]);
        // A row of another repository is recorded but never linked: it is no row here.
        assert_eq!(of("OTHER-R002").seam_consumer, ["specs/a/contracts/seam.md"]);

        let bindings = parse_test_bindings(&files);
        assert_eq!(bindings.len(), 2);
        let ids: Vec<String> = ["DEMO-R001", "DEMO-R002", "DEMO-R003"]
            .map(String::from)
            .to_vec();
        let mut git_changeset = changeset("c1", 1700, &["DEMO-R001"]);
        git_changeset.git = Some(GitOrigin {
            object_format: GitObjectFormat::Sha1,
            commit_id: "a".repeat(40),
            tree_id: "b".repeat(40),
            parent_commit_ids: Vec::new(),
            side_pack_digest: None,
            raw_headers: String::new(),
        });
        let chain = [git_changeset, changeset("c2", 1800, &["DEMO-R002"])];
        // Only the first test file is present at the head.
        let existing: BTreeSet<String> = BTreeSet::from(["tests/demo/test_base.py".to_string()]);
        let input = SpecHistoryInput {
            repo: "demo",
            rev: "c2",
            ids: &ids,
            links: &links,
            chain: &chain,
            new_changesets: &chain,
            bindings: &bindings,
            existing_test_paths: &existing,
        };
        let p = link_history_to_rows(&input).expect("links");
        let r1 = requirement_node_id("demo", "DEMO-R001");
        let r2 = requirement_node_id("demo", "DEMO-R002");
        assert_eq!(
            edges_of(&p, &r1, "implementedByCommit"),
            [changeset_node_id("demo", "c1")]
        );
        // A native changeset gets no commitHash and no implementedByCommit edge.
        assert!(edges_of(&p, &r2, "implementedByCommit").is_empty());
        let commit_node = |id: &str| {
            p.operations
                .iter()
                .find(|op| op["id"] == changeset_node_id("demo", id).as_str())
        };
        assert_eq!(
            commit_node("c1").expect("git changeset node")["properties"]["commitHash"],
            "a".repeat(40).as_str()
        );
        assert!(commit_node("c2").is_none());
        assert_eq!(
            edges_of(&p, &r1, "boundTest"),
            ["testfile:demo:tests/demo/test_base.py"]
        );
        assert_eq!(
            edges_of(&p, &r1, "verifiedBy"),
            ["testfile:demo:tests/demo/test_base.py"]
        );
        let r3 = requirement_node_id("demo", "DEMO-R003");
        assert_eq!(
            edges_of(&p, &r3, "boundTest"),
            ["testfile:demo:tests/demo/test_third.py"]
        );
        assert!(edges_of(&p, &r3, "verifiedBy").is_empty());
        assert_eq!(edges_of(&p, &r3, "dependsOnRow"), [r1.clone()]);
        assert_eq!(edges_of(&p, &r2, "dependsOnRow"), [r1.clone()]);
        assert_eq!(
            edges_of(&p, &r1, "seamProvider"),
            ["seam:demo:specs/a/contracts/seam.md"]
        );
        let status = |id: &str| {
            p.operations
                .iter()
                .find(|op| op["id"] == requirement_node_id("demo", id).as_str() && op["op"] == "upsert_node")
                .and_then(|op| op["properties"]["rowStatus"].as_str())
                .map(str::to_string)
        };
        assert_eq!(status("DEMO-R001").as_deref(), Some("VERIFIED"));
        assert_eq!(status("DEMO-R002").as_deref(), Some("LANDED"));
        assert_eq!(status("DEMO-R003").as_deref(), Some("SPECIFIED"));
        println!("HISTORY\t{}\t{}", p.nodes, p.edges);

        // A trailer id that is no row refuses and produces nothing.
        let unknown = [changeset("c3", 1900, &["DEMO-R999"])];
        let refused = link_history_to_rows(&SpecHistoryInput {
            new_changesets: &unknown,
            chain: &unknown,
            ..input
        })
        .unwrap_err();
        assert!(refused.starts_with("REQUIREMENT_UNRESOLVED: DEMO-R999"));
    }

    // spec: MEDULLA-SDLC-R014.12
    #[test]
    fn history_delta_emits_only_new_commits() {
        use medulla_ingestion_types::ingestion_wire::IndexResult;
        use medulla_source_types::source_changeset::{GitObjectFormat, GitOrigin};

        let git = |id: &str, at: u64, rows: &[&str]| {
            let mut c = changeset(id, at, rows);
            c.git = Some(GitOrigin {
                object_format: GitObjectFormat::Sha1,
                commit_id: format!("{id:0>40}"),
                tree_id: "b".repeat(40),
                parent_commit_ids: Vec::new(),
                side_pack_digest: None,
                raw_headers: String::new(),
            });
            c
        };
        let files = vec![(
            "specs/a/requirements.md".to_string(),
            REQUIREMENTS.as_bytes().to_vec(),
        )];
        let first_chain = [git("c1", 1700, &["DEMO-R001"])];
        let mut first = IndexResult::default();
        let plan = project_spec_artifacts(
            "demo",
            &files,
            &BTreeMap::new(),
            &first_chain,
            &first_chain,
            "main",
            "head1",
            &mut first,
        )
        .expect("first ingest");
        assert!(first.edges.iter().any(|e| e.edge_type == "implementedByCommit"
            && e.target == "changeset:demo:c1"));
        let mut stored = BTreeMap::new();
        stored_after(&plan, &mut stored);
        assert!(stored.contains_key(ROW_STATUS_ARTIFACT));

        // The next pass reads the chain, but only c2 is new.
        let chain = [first_chain[0].clone(), git("c2", 1800, &["DEMO-R002"])];
        let mut delta = IndexResult::default();
        let started = std::time::Instant::now();
        project_spec_artifacts(
            "demo",
            &files,
            &stored,
            &chain,
            &chain[1..],
            "main",
            "head2",
            &mut delta,
        )
        .expect("delta");
        let elapsed = started.elapsed();
        assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
        let commits: Vec<&str> = delta
            .edges
            .iter()
            .filter(|e| e.edge_type == "implementedByCommit")
            .map(|e| e.target.as_str())
            .collect();
        assert_eq!(commits, ["changeset:demo:c2"]);
        let statuses: Vec<(&str, &str)> = delta
            .nodes
            .iter()
            .filter_map(|n| {
                Some((n.node_id.as_str(), n.properties.get("rowStatus")?.as_str()))
            })
            .collect();
        // Only DEMO-R002 changed status; DEMO-R001 and DEMO-R003 are not rewritten.
        assert_eq!(statuses, [("requirement:demo:DEMO-R002", "LANDED")]);
        // The index envelope replaces a node's property set, so the status-only rewrite
        // carries every property the row held: nothing but `rowStatus` differs.
        let property_set = |result: &IndexResult| {
            let mut properties = result
                .nodes
                .iter()
                .find(|n| n.node_id == "requirement:demo:DEMO-R002")
                .expect("requirement node")
                .properties
                .clone();
            properties.remove("rowStatus");
            properties
        };
        assert!(property_set(&delta).contains_key("text"));
        assert_eq!(property_set(&delta), property_set(&first));
        println!("HISTORY_DELTA\t{}\t{}", commits.len(), elapsed.as_micros());
    }

    // spec: MEDULLA-SDLC-R014.3
    #[test]
    fn cite_requirements_links_trailers_and_refuses_unknown_ids() {
        let known = BTreeSet::from([
            requirement_node_id("demo", "DEMO-R001"),
            requirement_node_id("demo", "DEMO-R002"),
        ]);
        let changesets = [
            changeset("c1", 1700, &["DEMO-R001", "DEMO-R002"]),
            changeset("c2", 1800, &[]),
        ];
        let p = cite_requirements("demo", &changesets, &known).expect("cites");
        let first = changeset_node_id("demo", "c1");
        assert_eq!(first, "changeset:demo:c1");
        let nodes: Vec<&serde_json::Value> = p
            .operations
            .iter()
            .filter(|op| op["op"] == "upsert_node")
            .collect();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0]["properties"]["type"], "Changeset");
        assert_eq!(nodes[0]["properties"]["repo"], "demo");
        assert_eq!(nodes[0]["properties"]["changeset_id"], "c1");
        assert_eq!(nodes[0]["properties"]["created_at"], "1700");
        assert_eq!(
            edges_of(&p, &first, "citesRequirement"),
            vec![
                requirement_node_id("demo", "DEMO-R001"),
                requirement_node_id("demo", "DEMO-R002")
            ]
        );
        assert!(edges_of(&p, &changeset_node_id("demo", "c2"), "citesRequirement").is_empty());
        assert_eq!((p.nodes, p.edges), (2, 2));

        let unknown = [changeset("c3", 1900, &["DEMO-R001", "DEMO-R999"])];
        let refusal = cite_requirements("demo", &unknown, &known).unwrap_err();
        assert_eq!(
            refusal,
            "REQUIREMENT_UNRESOLVED: DEMO-R999 (cited by changeset c3) resolves to no row"
        );
    }

    // spec: MEDULLA-SDLC-R014.3
    #[test]
    fn index_batch_cites_requirements_once_per_changeset() {
        use medulla_ingestion_types::ingestion_wire::IndexResult;

        let files = vec![(
            "specs/a/requirements.md".to_string(),
            REQUIREMENTS.as_bytes().to_vec(),
        )];
        let changesets = [changeset("c1", 1700, &["DEMO-R001"])];
        let mut first = IndexResult::default();
        let plan = project_spec_artifacts(
            "demo",
            &files,
            &BTreeMap::new(),
            &changesets,
            &changesets,
            "main",
            "c0ffee",
            &mut first,
        )
        .expect("first batch");
        let cited = first
            .edges
            .iter()
            .find(|e| e.edge_type == "citesRequirement")
            .expect("one citation edge");
        assert_eq!(cited.source, "changeset:demo:c1");
        assert_eq!(cited.target, "requirement:demo:DEMO-R001");
        println!("CITE\t{}\t{}", cited.source, cited.target);
        let changeset_node = first
            .nodes
            .iter()
            .find(|n| n.node_id == "changeset:demo:c1")
            .expect("changeset node");
        assert_eq!(changeset_node.properties.get("branch").map(String::as_str), Some("main"));

        // The route hands over only the changesets after the last cited one.
        let mut stored = BTreeMap::new();
        stored_after(&plan, &mut stored);
        assert_eq!(stored[CHANGESETS_ARTIFACT].digest, "c1");
        let mut again = IndexResult::default();
        project_spec_artifacts(
            "demo",
            &files,
            &stored,
            &changesets,
            &[],
            "main",
            "c0ffee",
            &mut again,
        )
        .expect("idle");
        assert!(again.nodes.is_empty() && again.edges.is_empty());

        // An id that resolves to no row refuses and writes nothing.
        let unknown = [changeset("c2", 1800, &["DEMO-R999"])];
        let mut refused = IndexResult::default();
        let error = project_spec_artifacts("demo", &files, &stored, &unknown, &unknown, "main", "c0ffee", &mut refused)
            .unwrap_err();
        assert!(error.starts_with("REQUIREMENT_UNRESOLVED: DEMO-R999"));
        assert!(refused.nodes.is_empty() && refused.edges.is_empty());
        println!("CITE_REFUSED\t{error}");
    }

    // spec: MEDULLA-ORCH-VOCAB-R012.4
    #[test]
    fn unknown_spec_file_is_plain_document_with_diagnostic() {
        use medulla_ingestion_types::ingestion_wire::{
            IndexFileOutcome, IndexFileVerdict, IndexResult,
        };
        use medulla_types::contract::BoundedVec;

        let files: Vec<(String, Vec<u8>)> = vec![
            ("specs/x/notes.md".into(), b"just notes\n".to_vec()),
            ("specs/x/tasks.md".into(), b"- [ ] T1 do it\n".to_vec()),
            ("specs/x/spec.md".into(), b"# Spec\n".to_vec()),
            (".specify/memory/constitution.md".into(), b"# Rules\n".to_vec()),
            ("specs/x/goal.md".into(), b"---\ngoal: ship\n---\n".to_vec()),
            ("specs/x/memory/lesson.md".into(), b"lesson\n".to_vec()),
            // Read once, at the first occurrence; not UTF-8 is skipped; not a spec file ignored.
            ("specs/x/spec.md".into(), b"# Other\n".to_vec()),
            ("specs/x/binary.md".into(), vec![0xff, 0xfe]),
            ("src/lib.rs".into(), b"fn main() {}".to_vec()),
        ];
        let classified = classify_spec_files("demo", &files);
        assert_eq!(classified.unknown, vec!["specs/x/notes.md".to_string()]);
        let by_path = |path: &str| {
            classified
                .operations
                .iter()
                .find(|op| op["id"] == spec_kind_node_id("demo", path).as_str())
                .unwrap_or_else(|| panic!("{path} is written"))
        };
        assert_eq!(classified.operations.len(), 6);
        let plain = &by_path("specs/x/notes.md")["properties"];
        assert_eq!(plain["type"], "Document");
        assert!(plain.get("classifiedByRule").is_none());
        assert_eq!(plain["path"], "specs/x/notes.md");
        for (path, class, rule) in [
            ("specs/x/tasks.md", "SpecTask", "tasks-file"),
            ("specs/x/spec.md", "Specification", "spec-document"),
            (".specify/memory/constitution.md", "SpecPolicy", "constitution-file"),
            ("specs/x/goal.md", "SpecGoal", "goal-front-matter"),
            ("specs/x/memory/lesson.md", "SpecMemory", "memory-folder"),
        ] {
            let properties = &by_path(path)["properties"];
            assert_eq!(properties["type"], class, "{path}");
            assert_eq!(properties["classifiedByRule"], rule, "{path}");
            assert_eq!(properties["repo"], "demo");
            let recorded = properties["blob_digest"].as_str().unwrap();
            assert!(!recorded.starts_with("sha256:"), "{path}: bare hex, one digest format");
            assert_eq!(recorded.len(), 64, "{path}");
        }
        // The first occurrence of a repeated path wins.
        assert_eq!(
            by_path("specs/x/spec.md")["properties"]["blob_digest"],
            digest_of("# Spec\n").as_str()
        );

        // Through the projection: the unknown file's outcome gains the diagnostic.
        let outcome = |path: &str| IndexFileOutcome {
            file_path: path.to_string(),
            verdict: IndexFileVerdict::Success,
            content_digest: String::new(),
            parser_capability_digest: String::new(),
            diagnostics: BoundedVec::new(Vec::new()).unwrap(),
        };
        let mut result = IndexResult::default();
        result.file_outcomes = vec![
            outcome("specs/x/notes.md"),
            outcome(".specify/memory/constitution.md"),
        ];
        let batch: Vec<(String, Vec<u8>)> = files
            .iter()
            .filter(|(path, _)| path == "specs/x/notes.md" || path.starts_with(".specify/"))
            .cloned()
            .collect();
        project_spec_artifacts("demo", &batch, &BTreeMap::new(), &[], &[], "main", "c0ffee", &mut result)
            .expect("projects");
        let diagnostics = result.file_outcomes[0].diagnostics.as_slice();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "SPEC_FILE_UNKNOWN_KIND");
        assert_eq!(diagnostics[0].message, "specs/x/notes.md");
        assert!(result.file_outcomes[1].diagnostics.as_slice().is_empty());
        assert_eq!(result.file_outcomes[0].verdict, IndexFileVerdict::Success);
        assert!(result
            .nodes
            .iter()
            .any(|node| node.node_id
                == spec_kind_node_id("demo", ".specify/memory/constitution.md")
                && node.node_type == "SpecPolicy"));
    }
}
