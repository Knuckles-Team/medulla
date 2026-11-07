//! Reject reintroduction of obsolete structural-property fallbacks.
//!
//! Property blobs remain open JSON/MessagePack objects, so an application is free to store an
//! ordinary field named `type`. Engine code must not reinterpret that payload field as either an
//! edge relationship or a KnowledgeSet row kind, and must not restore the former `rel_type`
//! integration key. This gate checks the structural readers where that ambiguity used to exist
//! instead of banning legitimate user data named `type` repository-wide.

use std::path::Path;

use regex::Regex;

use crate::analysis::{module_tree, read};

pub const NAME: &str = "canonical-property-schema";
pub const SUMMARY: &str = "no structural reader falls back to an ordinary `type` payload field";

const STRUCTURAL_READERS: [&str; 20] = [
    "crates/medulla-graph-core/src/graph.rs",
    "crates/medulla-graphql/src/schema.rs",
    "crates/medulla-graphql/src/resolver.rs",
    "crates/medulla-cypher/src/cypher/exec.rs",
    "crates/medulla-cypher/src/cypher/proc.rs",
    "crates/medulla-exec/src/exec.rs",
    "crates/medulla-rdf/src/mapping.rs",
    "crates/medulla-rdf/src/update.rs",
    "crates/medulla-rdf/src/sparql.rs",
    "crates/medulla-reasoner/src/owl.rs",
    "crates/medulla-reasoner/src/reasoning.rs",
    "crates/medulla-epistemic/src/adapter.rs",
    "crates/medulla-epistemic/src/incremental.rs",
    "crates/medulla-epistemic/src/recompute.rs",
    "crates/medulla-jobs/src/claim.rs",
    "crates/medulla-h-mining/src/mining.rs",
    "crates/medulla-server-api/src/handlers/graphlearn.rs",
    "crates/medulla-h-graph/src/graph_ops.rs",
    "crates/medulla-h-txn/src/txn.rs",
    "crates/medulla-h-retrieve/src/reasoning_projection.rs",
];
/// The one structural reader whose source is split over declared child modules.
const MODULE_TREE_READER: &str = "crates/medulla-h-graph/src/graph_ops.rs";
const KNOWLEDGE_READER: &str = "crates/medulla-retrieval/src/knowledge.rs";
const CYPHER_READERS: [&str; 2] = [
    "crates/medulla-cypher/src/cypher/exec.rs",
    "crates/medulla-cypher/src/cypher/proc.rs",
];
const CYPHER_EXEC: &str = "crates/medulla-cypher/src/cypher/exec.rs";

const EDGE_TYPE_FALLBACKS: [&str; 10] = [
    r#"get\("relationship"\)\s*\.or_else\(\|\|\s*[A-Za-z_][A-Za-z0-9_]*\.get\("type"\)\)"#,
    r"`relationship`\s*/\s*`type`",
    r"`type`\s*/\s*`relationship`",
    r#"get\("rel_type"\)"#,
    r#"get\("relationship_type"\)"#,
    r#"get\("relation"\)"#,
    r#""rel_type"\s*:"#,
    r#""relationship_type"\s*:"#,
    r#""relation"\s*:"#,
    r#""type"\s*:\s*(?:predicate|new_prop|prop\b|p\b)"#,
];
const KNOWLEDGE_TYPE_FALLBACK: &str =
    r#"get\("node_type"\)\s*\.or_else\(\|\|\s*[A-Za-z_][A-Za-z0-9_]*\.get\("type"\)\)"#;

pub fn run(root: &Path, _args: &[String]) -> Result<(), String> {
    let failures = check(root)?;
    if failures.is_empty() {
        println!("canonical property schema gate: PASS");
        return Ok(());
    }
    Err(format!(
        "{}\n{} finding(s)",
        failures
            .iter()
            .map(|f| format!("- {f}"))
            .collect::<Vec<_>>()
            .join("\n"),
        failures.len()
    ))
}

/// Every finding for the tree at `root`.
pub fn check(root: &Path) -> Result<Vec<String>, String> {
    let mut failures = edge_relationship_failures(root)?;
    failures.extend(cypher_label_failures(root)?);
    failures.extend(edge_writer_failures(root)?);
    failures.extend(documentation_failures(root)?);
    Ok(failures)
}

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static regex")
}

/// Structural edge readers must never fall back to an ordinary `type` field.
fn edge_relationship_failures(root: &Path) -> Result<Vec<String>, String> {
    let mut failures = Vec::new();
    for path in STRUCTURAL_READERS {
        let source = if path == MODULE_TREE_READER {
            module_tree::read(root, path, false)?
        } else {
            read(root, path)?
        };
        for pattern in EDGE_TYPE_FALLBACKS {
            if regex(pattern).is_match(&source) {
                failures.push(format!(
                    "{path}: edge relationship must not fall back to ordinary `type`"
                ));
            }
        }
    }
    if regex(KNOWLEDGE_TYPE_FALLBACK).is_match(&read(root, KNOWLEDGE_READER)?) {
        failures.push(format!(
            "{KNOWLEDGE_READER}: KnowledgeSet kind must read only canonical `node_type`"
        ));
    }
    Ok(failures)
}

/// Cypher labels are a strict current-only seam: MATCH/CREATE and db.labels never reinterpret an
/// ordinary `type`/`label` payload field as a structural label.
fn cypher_label_failures(root: &Path) -> Result<Vec<String>, String> {
    let mut failures = Vec::new();
    let label_read = regex(r#"\.get\("(?:type|label)"\)"#);
    for path in CYPHER_READERS {
        let text = read(root, path)?;
        let production = text.split("#[cfg(test)]").next().unwrap_or("");
        if label_read.is_match(production) {
            failures.push(format!(
                "{path}: Cypher structural label readers must use only `node_type`/`labels`"
            ));
        }
    }
    let exec = read(root, CYPHER_EXEC)?;
    for required in [
        "materialize_node(view, id)",
        r#"obj.insert("id".to_string(), Value::String(node_id.to_string()));"#,
        r#"obj.insert("node_type".to_string(), Value::Null);"#,
        r#"props.insert("node_type".to_string(), Value::String(label.clone()));"#,
    ] {
        if !exec.contains(required) {
            failures.push(format!(
                "{CYPHER_EXEC}: missing canonical Cypher node projection/write marker `{required}`"
            ));
        }
    }
    Ok(failures)
}

/// Edge writers and the typed edge itself expose only `relationship`.
fn edge_writer_failures(root: &Path) -> Result<Vec<String>, String> {
    let mut failures = Vec::new();
    let writer = read(root, "crates/medulla-graphql/src/mutation.rs")?;
    if !writer.contains(r#"obj.insert("relationship".to_string(), Value::String(rel.clone()));"#) {
        failures.push(
            "crates/medulla-graphql/src/mutation.rs: GraphQL edge writer must stamp `relationship`"
                .into(),
        );
    }
    let typed = read(root, "crates/medulla-types/src/types.rs")?;
    if !typed.contains("pub relationship: String,")
        || typed.contains("pub relationship_type:")
        || !typed.contains("#[serde(deny_unknown_fields)]\npub struct EdgeData")
    {
        failures.push(
            "crates/medulla-types/src/types.rs: EdgeData must expose only canonical `relationship`"
                .into(),
        );
    }
    Ok(failures)
}

/// The contract page exists, is navigable, and names every canonical term.
fn documentation_failures(root: &Path) -> Result<Vec<String>, String> {
    let mut failures = Vec::new();
    let page = "docs/architecture/canonical-property-schema.md";
    let docs = read(root, page)?;
    if !read(root, "mkdocs.yml")?.contains("architecture/canonical-property-schema.md") {
        failures.push("mkdocs.yml: canonical property schema page is not in navigation".into());
    }
    for term in ["`relationship`", "`node_type`", "`rel_type`"] {
        if !docs.contains(term) {
            failures.push(format!("{page}: missing contract term {term}"));
        }
    }
    Ok(failures)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;

    fn fallback_findings(source: &str) -> usize {
        EDGE_TYPE_FALLBACKS
            .iter()
            .filter(|p| regex(p).is_match(source))
            .count()
    }

    #[test]
    fn every_edge_fallback_shape_is_detected() {
        for source in [
            r#"m.get("relationship").or_else(|| m.get("type"))"#,
            "`relationship` / `type`",
            "`type` / `relationship`",
            r#"m.get("rel_type")"#,
            r#"m.get("relationship_type")"#,
            r#"m.get("relation")"#,
            r#"{"rel_type": x}"#,
            r#"{"relationship_type": x}"#,
            r#"{"relation": x}"#,
            r#"{"type": predicate}"#,
        ] {
            assert_eq!(fallback_findings(source), 1, "{source}");
        }
        assert_eq!(fallback_findings(r#"m.get("relationship")"#), 0);
    }

    #[test]
    fn the_knowledge_fallback_is_detected() {
        assert!(regex(KNOWLEDGE_TYPE_FALLBACK)
            .is_match(r#"v.get("node_type").or_else(|| v.get("type"))"#));
        assert!(!regex(KNOWLEDGE_TYPE_FALLBACK).is_match(r#"v.get("node_type")"#));
    }

    #[test]
    fn cypher_label_reads_in_tests_are_ignored() {
        let t = Tree::new(&[
            (
                CYPHER_READERS[0],
                "fn a() {}\n#[cfg(test)]\nmod t { fn b(v: V) { v.get(\"type\"); } }\n",
            ),
            (CYPHER_READERS[1], "fn a(v: V) { v.get(\"label\"); }\n"),
        ]);
        let failures = cypher_label_failures(t.path());
        let failures = failures.unwrap();
        assert!(failures
            .iter()
            .any(|f| f.starts_with(CYPHER_READERS[1]) && f.contains("must use only")));
        assert!(!failures
            .iter()
            .any(|f| f.starts_with(CYPHER_READERS[0]) && f.contains("must use only")));
    }

    #[test]
    fn docs_must_name_every_term_and_be_navigable() {
        let t = Tree::new(&[
            (
                "docs/architecture/canonical-property-schema.md",
                "`relationship` `node_type`",
            ),
            ("mkdocs.yml", "nav: []"),
        ]);
        let failures = documentation_failures(t.path()).unwrap();
        assert_eq!(failures.len(), 2, "{failures:?}");
    }

    #[test]
    fn passes_on_the_current_tree() {
        let findings = check(&crate::workspace_root()).unwrap();
        assert!(findings.is_empty(), "{findings:?}");
    }
}
