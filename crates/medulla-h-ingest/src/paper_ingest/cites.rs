//! Citation edges of one paper record (MEDULLA-MEDIA-VOCAB-R002.5.3).

use std::collections::BTreeSet;

use medulla_protocol_types::protocol::Method;
use serde_json::Value;

use super::paper_node_id;

fn encode(value: &Value) -> Vec<u8> {
    // A serde_json value always encodes.
    rmp_serde::to_vec_named(value).unwrap_or_default()
}

/// The node ids of the papers `payload` cites by a declaration: a blank `cited_canonical_id` or an
/// `unidentified:` one names none.
pub(crate) fn cited_node_ids(payload: &Value) -> Vec<String> {
    payload
        .get("citation_edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|declaration| {
            declaration
                .get("cited_canonical_id")
                .and_then(Value::as_str)
        })
        .filter(|id| !id.trim().is_empty() && !id.starts_with("unidentified:"))
        .map(paper_node_id)
        .collect()
}

/// One `:cites` edge per declaration from the citing paper to the cited one, with the compact JSON
/// of its `source_evidence` as the `evidence` property, and a stub node (`type` `ResearchPaper`,
/// `canonical_id`, `stub` true) for a cited paper `known` does not hold. A `citation_count` writes
/// no edge. Returns `(stubs, edges)`.
pub(crate) fn write_citations(
    citing: &str,
    payload: &Value,
    known: &BTreeSet<String>,
) -> (Vec<(String, Method)>, Vec<Method>) {
    let mut stubs: Vec<(String, Method)> = Vec::new();
    let mut edges = Vec::new();
    let mut seen = BTreeSet::new();
    for declaration in payload
        .get("citation_edges")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(cited) = declaration
            .get("cited_canonical_id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty() && !id.starts_with("unidentified:"))
        else {
            continue;
        };
        let target = paper_node_id(cited);
        if !seen.insert(target.clone()) {
            continue;
        }
        if !known.contains(&target) {
            stubs.push((
                target.clone(),
                Method::AddNode {
                    node_id: target.clone(),
                    properties_msgpack: encode(&serde_json::json!({
                        "type": "ResearchPaper",
                        "canonical_id": cited,
                        "stub": true,
                    })),
                },
            ));
        }
        let evidence = declaration
            .get("source_evidence")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        edges.push(Method::AddEdge {
            source_id: citing.to_string(),
            target_id: target,
            properties_msgpack: encode(&serde_json::json!({
                "relationship": "cites",
                "evidence": serde_json::to_string(&evidence).unwrap_or_default(),
            })),
        });
    }
    (stubs, edges)
}
