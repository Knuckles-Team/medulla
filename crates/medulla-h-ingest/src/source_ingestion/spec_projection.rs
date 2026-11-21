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
use medulla_compute::parser::spec_artifacts::{is_row_id, SDD_NS};
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
}
