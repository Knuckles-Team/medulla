//! MEDULLA-REPO-INGEST-R006.3 — serve revision-bound structured code maps.
//!
//! `Method::UnifiedQuery` with a `code_map` field reaches [`serve`] from
//! `sql_read.rs::handle_unified_query`'s dispatch. The map is read from the
//! graph the scoped `IndexRepository` committed: a live `Branch` node names the
//! repository's current revision, `hasFileVersion` reaches the file versions of
//! that revision, `hasBlob` their content and `IMPLEMENTS` the symbols parsed
//! from it. A symbol keeps its id while its file content is unchanged, and a
//! reindex that moves a ref moves the map generation.
//!
//! Authorization runs first: a request without the current signed tenant
//! authority is refused before the graph is read, and the snapshot is narrowed
//! to the units the verified reader may see before any entry, count or cursor is
//! produced. The requested revision is compared with the current one on every
//! page; a mismatch or unknown revision refuses with `CODE_MAP_STALE`. A cursor
//! binds the map generation, the query digest and the auth scope.

use super::*;

use std::collections::{BTreeMap, BTreeSet};

use medulla_graph_core::graph::GraphView;
use medulla_types::code_map::{
    CodeMapEdge, CodeMapEntry, CodeMapIncomplete, CodeMapQuery, CodeMapResult, CodeRelation,
    CODE_MAP_INVALID, CODE_MAP_STALE,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const BRANCH_PREFIX: &str = "branch:";
const CURSOR_TAG: &str = "cm1";

type Props = Map<String, Value>;

fn sha256_label(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn short(label: &str) -> &str {
    let hex = label.trim_start_matches("sha256:");
    &hex[..hex.len().min(16)]
}

fn text<'a>(props: &'a Props, key: &str) -> &'a str {
    props.get(key).and_then(Value::as_str).unwrap_or("")
}

fn number(props: &Props, key: &str) -> u32 {
    text(props, key).parse().unwrap_or(0)
}

/// `(target, relationship)` of every distinct out-edge of `id`.
fn out_edges(view: &GraphView, id: &str) -> Vec<(String, String)> {
    let Some(&index) = view.node_map.get(id) else {
        return Vec::new();
    };
    let mut found = BTreeSet::new();
    for neighbour in view.graph.neighbors(index) {
        let target = &view.graph[neighbour];
        let key = (id.to_string(), target.clone());
        for blob in view.edge_properties.get(&key).into_iter().flatten() {
            if let Ok(Value::Object(props)) = medulla_types::msgpack::decode_property_value(blob) {
                found.insert((target.clone(), text(&props, "relationship").to_string()));
            }
        }
    }
    found.into_iter().collect()
}

fn targets(view: &GraphView, id: &str, relationship: &str) -> Vec<String> {
    out_edges(view, id)
        .into_iter()
        .filter(|(_, rel)| rel == relationship)
        .map(|(target, _)| target)
        .collect()
}

/// One live ref of the repository.
struct LiveRef {
    node_id: String,
    ref_name: String,
    revision: String,
}

fn live_refs(view: &GraphView, repo: &str) -> Vec<LiveRef> {
    let mut refs: Vec<LiveRef> = view
        .node_properties
        .keys()
        .filter(|id| id.starts_with(BRANCH_PREFIX))
        .filter_map(|id| {
            let props = view.node_row_object(id)?;
            (text(&props, "type") == "Branch"
                && text(&props, "repository_id") == repo
                && text(&props, "status") == "live")
                .then(|| LiveRef {
                    node_id: id.clone(),
                    ref_name: text(&props, "ref_name").to_string(),
                    revision: text(&props, "revision_id").to_string(),
                })
        })
        .collect();
    refs.sort_by(|a, b| a.ref_name.cmp(&b.ref_name));
    refs
}

/// The generation of the map: it moves whenever a ref of the repository moves.
fn map_generation(refs: &[LiveRef]) -> u64 {
    let mut hasher = Sha256::new();
    for live in refs {
        hasher.update(live.ref_name.as_bytes());
        hasher.update([0]);
        hasher.update(live.revision.as_bytes());
        hasher.update([b'\n']);
    }
    let digest = hasher.finalize();
    u64::from_be_bytes(digest[..8].try_into().unwrap_or([0; 8]))
}

/// Everything the index committed for one revision.
struct RevisionMap {
    entries: BTreeMap<String, CodeMapEntry>,
    /// `(source, target, relation)`.
    edges: BTreeSet<(String, String, CodeRelation)>,
    incomplete: BTreeSet<(String, CodeRelation, String)>,
    inventory_digest: String,
}

fn collect_files(view: &GraphView, branches: &[&LiveRef]) -> BTreeMap<String, (String, String)> {
    let mut files = BTreeMap::new();
    for branch in branches {
        for version in targets(view, &branch.node_id, "hasFileVersion") {
            if let Some(props) = view.node_row_object(&version) {
                files.insert(
                    version,
                    (
                        text(&props, "path").to_string(),
                        text(&props, "content_digest").to_string(),
                    ),
                );
            }
        }
    }
    files
}

fn symbol_entry(view: &GraphView, symbol: &str, path: &str) -> Option<CodeMapEntry> {
    let props = view.node_row_object(symbol)?;
    let line = number(&props, "line");
    Some(CodeMapEntry {
        id: symbol.to_string(),
        kind: text(&props, "symbol_type").to_string(),
        name: text(&props, "name").to_string(),
        path: path.to_string(),
        line,
        end_line: number(&props, "end_line").max(line),
    })
}

fn revision_map(view: &GraphView, branches: &[&LiveRef]) -> RevisionMap {
    let files = collect_files(view, branches);
    let mut by_path: Vec<(&String, &(String, String))> = files.iter().collect();
    by_path.sort_by(|a, b| (&a.1 .0, a.0).cmp(&(&b.1 .0, b.0)));

    let inventory: BTreeSet<String> = files
        .values()
        .map(|(path, digest)| format!("{path}\0{digest}\n"))
        .collect();
    let inventory_digest = sha256_label(inventory.into_iter().collect::<String>().as_bytes());

    let mut map = RevisionMap {
        entries: BTreeMap::new(),
        edges: BTreeSet::new(),
        incomplete: BTreeSet::new(),
        inventory_digest,
    };
    for (version, (path, _)) in &by_path {
        map.entries.insert(
            (*version).clone(),
            CodeMapEntry {
                id: (*version).clone(),
                kind: "file".into(),
                name: path.clone(),
                path: path.clone(),
                line: 0,
                end_line: 0,
            },
        );
        for blob in targets(view, version, "hasBlob") {
            for symbol in targets(view, &blob, "IMPLEMENTS") {
                if let Some(entry) = symbol_entry(view, &symbol, path) {
                    map.entries.entry(symbol.clone()).or_insert(entry);
                    map.edges
                        .insert(((*version).clone(), symbol, CodeRelation::Contains));
                }
            }
        }
    }
    add_calls(view, &mut map);
    add_dependencies(view, &files, &mut map);
    map
}

fn add_calls(view: &GraphView, map: &mut RevisionMap) {
    let symbols: Vec<String> = map
        .edges
        .iter()
        .filter(|(_, _, relation)| *relation == CodeRelation::Contains)
        .map(|(_, symbol, _)| symbol.clone())
        .collect();
    for symbol in symbols {
        let mut resolved_names = BTreeSet::new();
        for callee in targets(view, &symbol, "calls") {
            let name = map
                .entries
                .get(&callee)
                .map(|entry| entry.name.clone())
                .or_else(|| {
                    view.node_row_object(&callee)
                        .map(|props| text(&props, "name").to_string())
                })
                .unwrap_or_else(|| callee.clone());
            resolved_names.insert(name.clone());
            if map.entries.contains_key(&callee) {
                map.edges
                    .insert((symbol.clone(), callee, CodeRelation::Calls));
            } else {
                map.incomplete
                    .insert((symbol.clone(), CodeRelation::Calls, name));
            }
        }
        let raw = view
            .node_row_object(&symbol)
            .map(|props| text(&props, "calls").to_string())
            .unwrap_or_default();
        for name in raw.split(',').filter(|name| !name.is_empty()) {
            if !resolved_names.contains(name) {
                map.incomplete
                    .insert((symbol.clone(), CodeRelation::Calls, name.to_string()));
            }
        }
    }
}

fn add_dependencies(
    view: &GraphView,
    files: &BTreeMap<String, (String, String)>,
    map: &mut RevisionMap,
) {
    for version in files.keys() {
        for dependency in targets(view, version, "depends_on") {
            if files.contains_key(&dependency) {
                map.edges
                    .insert((version.clone(), dependency, CodeRelation::DependsOn));
            } else {
                let name = view
                    .node_row_object(&dependency)
                    .map(|props| text(&props, "path").to_string())
                    .unwrap_or(dependency);
                map.incomplete
                    .insert((version.clone(), CodeRelation::DependsOn, name));
            }
        }
    }
}

/// Entries the request selects, best first.
fn select(map: &RevisionMap, query: &CodeMapQuery) -> Vec<String> {
    let tokens: Vec<String> = query
        .behavior_text
        .iter()
        .flat_map(|text| text.split(|c: char| !(c.is_alphanumeric() || c == '_')))
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut scored: Vec<(usize, &CodeMapEntry)> = map
        .entries
        .values()
        .filter(|entry| {
            query
                .symbol
                .as_deref()
                .map_or(true, |symbol| entry.name == symbol)
        })
        .filter_map(|entry| {
            if tokens.is_empty() {
                return Some((0, entry));
            }
            let haystack = format!("{} {} {}", entry.name, entry.kind, entry.path).to_lowercase();
            let hits = tokens
                .iter()
                .filter(|token| haystack.contains(token.as_str()))
                .count();
            (hits > 0).then_some((hits, entry))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| (&a.1.path, a.1.line, &a.1.id).cmp(&(&b.1.path, b.1.line, &b.1.id)))
    });
    scored
        .into_iter()
        .map(|(_, entry)| entry.id.clone())
        .collect()
}

fn cursor_for(generation: u64, query_digest: &str, auth_digest: &str, offset: usize) -> String {
    format!(
        "{CURSOR_TAG}.{generation}.{}.{}.{offset}",
        short(query_digest),
        short(auth_digest)
    )
}

/// The offset a cursor names, once it is proven minted for this generation,
/// query and reader.
fn cursor_offset(
    cursor: &str,
    generation: u64,
    query_digest: &str,
    auth_digest: &str,
) -> Result<usize, String> {
    let invalid = || format!("{CODE_MAP_INVALID}: cursor is not valid for this request");
    let parts: Vec<&str> = cursor.split('.').collect();
    let [tag, minted_generation, query, auth, offset] = parts[..] else {
        return Err(invalid());
    };
    if tag != CURSOR_TAG {
        return Err(invalid());
    }
    if minted_generation != generation.to_string() {
        return Err(format!(
            "{CODE_MAP_STALE}: cursor was minted for another map generation"
        ));
    }
    if query != short(query_digest) || auth != short(auth_digest) {
        return Err(invalid());
    }
    offset.parse().map_err(|_| invalid())
}

/// Build one page of the map for `query` over `view`.
fn build(
    view: &GraphView,
    query: &CodeMapQuery,
    auth_digest: &str,
) -> Result<CodeMapResult, String> {
    query.validate()?;
    let refs = live_refs(view, &query.repo);
    let current: Vec<&LiveRef> = refs
        .iter()
        .filter(|live| live.revision == query.source_revision)
        .collect();
    if current.is_empty() {
        let live: Vec<&str> = refs.iter().map(|live| live.revision.as_str()).collect();
        return Err(format!(
            "{CODE_MAP_STALE}: revision {} is not current for the repository (current: [{}])",
            query.source_revision,
            live.join(", ")
        ));
    }
    let generation = map_generation(&refs);
    let mut digested = query.clone();
    digested.cursor = None;
    let query_digest =
        sha256_label(&serde_json::to_vec(&digested).map_err(|error| format!("INTERNAL: {error}"))?);
    let offset = match query.cursor.as_deref() {
        Some(cursor) => cursor_offset(cursor, generation, &query_digest, auth_digest)?,
        None => 0,
    };

    let map = revision_map(view, &current);
    let selected = select(&map, query);
    let limit = query.page_limit as usize;
    let page: Vec<&String> = selected.iter().skip(offset).take(limit).collect();
    let in_page: BTreeSet<&str> = page.iter().map(|id| id.as_str()).collect();
    let wanted = query.wanted_relations();
    let next = offset + page.len();

    Ok(CodeMapResult {
        map_generation: generation,
        inventory_digest: map.inventory_digest.clone(),
        query_digest: query_digest.clone(),
        auth_scope_digest: auth_digest.to_string(),
        entries: page.iter().map(|id| map.entries[*id].clone()).collect(),
        edges: map
            .edges
            .iter()
            .filter(|(source, _, relation)| {
                in_page.contains(source.as_str()) && wanted.contains(relation)
            })
            .map(|(source, target, relation)| CodeMapEdge {
                source: source.clone(),
                target: target.clone(),
                relation: *relation,
            })
            .collect(),
        incomplete: map
            .incomplete
            .iter()
            .filter(|(source, relation, _)| {
                in_page.contains(source.as_str()) && wanted.contains(relation)
            })
            .map(|(source, relation, name)| CodeMapIncomplete {
                source: source.clone(),
                relation: *relation,
                name: name.clone(),
            })
            .collect(),
        next_cursor: (next < selected.len())
            .then(|| cursor_for(generation, &query_digest, auth_digest, next)),
    })
}

/// The digest of the verified reader the page was cut for.
fn auth_scope_digest(ctx: &QueryHandlerCtx<'_>) -> String {
    let caller = medulla_server_api::read_scope::ReadCaller::from_authority(
        ctx.read_authority,
        ctx.graph_name,
    );
    let principal = &caller.principal;
    sha256_label(
        format!(
            "{}\0{}\0{}",
            principal.id, principal.tenant, principal.placement
        )
        .as_bytes(),
    )
}

/// Serve one code-map page. Authorization is the first step: nothing is read,
/// counted or cursor-minted for a principal without current signed authority.
pub(super) async fn serve(
    ctx: &QueryHandlerCtx<'_>,
    plan: &medulla_exec::algebra::Plan,
    query: &CodeMapQuery,
) -> Response {
    let req_id = ctx.req_id;
    if let Err(denied) = required_sql_authority(req_id, ctx.read_authority) {
        return denied;
    }
    if !plan.ops.is_empty() {
        return Response::err(
            req_id,
            format!("{CODE_MAP_INVALID}: a code_map request carries an empty plan"),
        );
    }
    if let Err(refusal) = query.validate() {
        return Response::err(req_id, refusal);
    }
    let (snap, version) = sql_read_snapshot(
        ctx.core,
        #[cfg(feature = "security")]
        ctx.caller,
        #[cfg(feature = "security")]
        ctx.rls,
    );
    let scope = ReadScope::resolve(ctx.state, ctx.read_authority, ctx.graph_name).await;
    let snap = match scope.narrow(req_id, version, snap, &["code_map".to_string()]) {
        Ok(snap) => snap,
        Err(refusal) => return refusal,
    };
    let auth_digest = auth_scope_digest(ctx);
    let query = query.clone();
    let built = compute_off_lock(req_id, move || build(&snap, &query, &auth_digest)).await;
    match built {
        Ok(Ok(page)) => match ResultPayload::of_ref::<query_results::UnifiedQueryCodeMap>(&page) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        },
        Ok(Err(refusal)) => Response::err(req_id, refusal),
        Err(response) => response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_and_binds_generation_query_and_reader() {
        let (query, auth) = (sha256_label(b"q"), sha256_label(b"a"));
        let cursor = cursor_for(7, &query, &auth, 25);
        assert_eq!(cursor_offset(&cursor, 7, &query, &auth), Ok(25));
        let stale = cursor_offset(&cursor, 8, &query, &auth).unwrap_err();
        assert!(stale.starts_with(CODE_MAP_STALE), "{stale}");
        let other_query = cursor_offset(&cursor, 7, &sha256_label(b"z"), &auth).unwrap_err();
        assert!(other_query.starts_with(CODE_MAP_INVALID), "{other_query}");
        let other_reader = cursor_offset(&cursor, 7, &query, &sha256_label(b"z")).unwrap_err();
        assert!(other_reader.starts_with(CODE_MAP_INVALID), "{other_reader}");
        assert!(cursor_offset("garbage", 7, &query, &auth).is_err());
    }

    fn entry(id: &str, name: &str, path: &str, line: u32) -> CodeMapEntry {
        CodeMapEntry {
            id: id.into(),
            kind: "function".into(),
            name: name.into(),
            path: path.into(),
            line,
            end_line: line,
        }
    }

    fn map_of(entries: Vec<CodeMapEntry>) -> RevisionMap {
        RevisionMap {
            entries: entries.into_iter().map(|e| (e.id.clone(), e)).collect(),
            edges: BTreeSet::new(),
            incomplete: BTreeSet::new(),
            inventory_digest: sha256_label(b""),
        }
    }

    fn query(behavior: Option<&str>, symbol: Option<&str>) -> CodeMapQuery {
        CodeMapQuery {
            repo: "r".into(),
            source_revision: "a".repeat(40),
            behavior_text: behavior.map(str::to_string),
            symbol: symbol.map(str::to_string),
            relations: vec![],
            page_limit: 10,
            cursor: None,
        }
    }

    #[test]
    fn behavior_text_ranks_matching_entries_first_and_drops_the_rest() {
        let map = map_of(vec![
            entry("s1", "parse_invoice", "billing/parse.py", 3),
            entry("s2", "render_page", "ui/render.py", 9),
            entry("s3", "invoice_total", "billing/total.py", 1),
        ]);
        let picked = select(&map, &query(Some("parse invoice"), None));
        assert_eq!(picked, vec!["s1".to_string(), "s3".to_string()]);
        assert_eq!(select(&map, &query(None, Some("render_page"))), vec!["s2"]);
        assert_eq!(select(&map, &query(None, None)).len(), 3);
    }
}
