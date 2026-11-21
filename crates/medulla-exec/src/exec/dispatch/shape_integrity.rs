//! Graph integrity validation as named shapes (MEDULLA-FEDERATED-QUERY-R005.700).
//!
//! The `VALIDATE SHAPE` stage runs an integrity shape (`integrity:<name>[:<key>]`,
//! see [`IntegrityShape`]) over the queried snapshot in one pass: every node and every
//! edge entry is visited once, so time is `O(N + E)`. Findings are kept in a bounded
//! ordered set, so memory is `O(F)` for `F` findings, capped per shape by
//! [`MAX_FINDINGS_PER_SHAPE`]; when the cap is hit the smallest-focus findings stay, so
//! the kept set never depends on hash order.
//!
//! The stage keeps the input rows that have a finding (`KEEP VIOLATING`) or none
//! (`KEEP CONFORMING`). A kept violating row carries its first finding as the text
//! columns `shape.shape_id`, `shape.severity`, `shape.message`, `shape.blocks` and
//! `shape.findings`. A graph larger than `max_shape_nodes` is not scanned: the input
//! passes through unchanged and every row carries `shape.coverage = gap:<checked>/<budget>`,
//! never a clean pass.

use std::collections::{BTreeMap, BTreeSet};

use medulla_graph_core::graph::GraphView;
use medulla_types::shape_report::{IntegrityShape, ShapeReport};
use medulla_query_types::wire::ShapeKeep;

use crate::exec::PlanCtx;
use crate::rowset::{RowSet, TextChannels};

/// Most findings one shape retains.
pub const MAX_FINDINGS_PER_SHAPE: usize = 10_000;

/// What one scan visited, so a test can assert one visit per element.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ScanVisits {
    pub nodes: usize,
    pub edges: usize,
}

/// Run `shape` over the snapshot behind `ctx` for the rows in `input`.
pub(super) fn run_named_shapes(
    ctx: &PlanCtx,
    input: RowSet,
    shape: &IntegrityShape,
    keep: ShapeKeep,
) -> Result<RowSet, String> {
    let budget = ctx.budget.max_shape_nodes;
    if ctx.view.node_map.len() > budget {
        return Ok(gap_rows(input, budget));
    }
    let (findings, _visits) = scan(ctx.view, shape);
    let by_focus = group_by_focus(&findings);
    let violating = keep == ShapeKeep::Violating;
    let kept: std::collections::HashSet<&str> = input
        .rows()
        .iter()
        .map(|row| row.id.as_str())
        .filter(|id| by_focus.contains_key(*id) == violating)
        .collect();
    let out = input.intersect_keep_order(&kept);
    let mut texts = TextChannels::new();
    if violating {
        for row in out.rows() {
            if let Some(found) = by_focus.get(row.id.as_str()) {
                texts.insert(row.id.clone(), columns(found));
            }
        }
    }
    Ok(out.with_texts(texts))
}

/// Rows of an unscanned (over-budget) graph: unchanged, each marked as a coverage gap.
fn gap_rows(input: RowSet, budget: usize) -> RowSet {
    let mut texts = TextChannels::new();
    for row in input.rows() {
        let mut columns = BTreeMap::new();
        columns.insert("shape.coverage".to_string(), format!("gap:0/{budget}"));
        texts.insert(row.id.clone(), columns);
    }
    input.with_texts(texts)
}

fn group_by_focus(findings: &BTreeSet<ShapeReport>) -> BTreeMap<&str, Vec<&ShapeReport>> {
    let mut by_focus: BTreeMap<&str, Vec<&ShapeReport>> = BTreeMap::new();
    for finding in findings {
        by_focus
            .entry(finding.focus.as_str())
            .or_default()
            .push(finding);
    }
    by_focus
}

/// The text columns of one focus row: its most severe finding, then its count.
fn columns(found: &[&ShapeReport]) -> BTreeMap<String, String> {
    let first = found
        .iter()
        .max_by(|a, b| {
            a.severity
                .cmp(&b.severity)
                .then_with(|| b.message.cmp(&a.message))
        })
        .expect("a focus row has at least one finding");
    let mut columns = BTreeMap::new();
    columns.insert("shape.shape_id".to_string(), first.shape_id.clone());
    columns.insert(
        "shape.severity".to_string(),
        first.severity.as_str().to_string(),
    );
    columns.insert("shape.message".to_string(), first.message.clone());
    columns.insert("shape.blocks".to_string(), first.blocks.to_string());
    columns.insert("shape.findings".to_string(), found.len().to_string());
    columns
}

/// Scan the snapshot once for `shape`. Returns the bounded findings and the visit counts.
pub(super) fn scan(
    view: &GraphView,
    shape: &IntegrityShape,
) -> (BTreeSet<ShapeReport>, ScanVisits) {
    let mut findings = BTreeSet::new();
    let mut visits = ScanVisits::default();
    let mut record = |focus: &str, message: String| {
        findings.insert(ShapeReport::new(shape, focus.to_string(), message));
        if findings.len() > MAX_FINDINGS_PER_SHAPE {
            findings.pop_last();
        }
    };
    match shape {
        IntegrityShape::DanglingEdge => {
            for (source, target) in view.edge_properties.keys() {
                visits.edges += 1;
                for (endpoint, other) in [(source, target), (target, source)] {
                    if !view.node_map.contains_key(endpoint) {
                        // The finding is about the surviving endpoint, or the source
                        // when both are gone: a row must be a node id.
                        let focus = if view.node_map.contains_key(other) {
                            other
                        } else {
                            source
                        };
                        record(
                            focus,
                            format!(
                                "edge `{source}` -> `{target}` names missing node `{endpoint}`"
                            ),
                        );
                    }
                }
            }
        }
        IntegrityShape::OrphanNode => {
            for (id, index) in &view.node_map {
                visits.nodes += 1;
                if view.graph.neighbors_undirected(*index).next().is_none() {
                    record(id, format!("node `{id}` has no incident edge"));
                }
            }
        }
        IntegrityShape::RequiredProperty { key } => {
            for id in view.node_map.keys() {
                visits.nodes += 1;
                let present = view
                    .node_row_object(id)
                    .and_then(|object| object.get(key).map(|value| !value.is_null()))
                    .unwrap_or(false);
                if !present {
                    record(id, format!("node `{id}` lacks required property `{key}`"));
                }
            }
        }
        IntegrityShape::InvalidReference { key } => {
            for id in view.node_map.keys() {
                visits.nodes += 1;
                let Some(object) = view.node_row_object(id) else {
                    continue;
                };
                for target in references(object.get(key)) {
                    if !view.node_map.contains_key(target) {
                        record(
                            id,
                            format!(
                                "property `{key}` of node `{id}` names missing node `{target}`"
                            ),
                        );
                    }
                }
            }
        }
    }
    (findings, visits)
}

/// The node ids a reference property names: a string, or the strings of an array.
fn references(value: Option<&serde_json::Value>) -> Vec<&str> {
    match value {
        Some(serde_json::Value::String(target)) => vec![target.as_str()],
        Some(serde_json::Value::Array(items)) => {
            items.iter().filter_map(serde_json::Value::as_str).collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use medulla_graph_core::graph::GraphCore;
    use medulla_semantic_index::semantic::SemanticStore;
    use medulla_types::shape_report::Severity;

    use super::*;

    fn props(value: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&value).unwrap()
    }

    /// a -- b (both named), c has no edge and no name, d names a missing node.
    /// A raw edge entry points at a node that does not exist.
    fn graph() -> GraphView {
        let core = GraphCore::new();
        core.add_node("a".into(), props(serde_json::json!({"name": "A"})));
        core.add_node(
            "b".into(),
            props(serde_json::json!({"name": "B", "owner": "a"})),
        );
        core.add_node("c".into(), props(serde_json::json!({})));
        core.add_node(
            "d".into(),
            props(serde_json::json!({"name": "D", "owner": "ghost"})),
        );
        core.add_edge(
            "a".into(),
            "b".into(),
            props(serde_json::json!({"relationship": "KNOWS"})),
        )
        .unwrap();
        core.add_edge(
            "b".into(),
            "d".into(),
            props(serde_json::json!({"relationship": "KNOWS"})),
        )
        .unwrap();
        let mut view = core.analysis_snapshot();
        view.edge_properties.insert(
            ("a".to_string(), "missing".to_string()),
            vec![Arc::new(props(
                serde_json::json!({"relationship": "KNOWS"}),
            ))],
        );
        view
    }

    fn run(shape: &str, keep: ShapeKeep, budget: Option<usize>) -> RowSet {
        let view = graph();
        let semantic = SemanticStore::new();
        let mut ctx = PlanCtx::new(&view, &semantic);
        if let Some(budget) = budget {
            ctx.budget.max_shape_nodes = budget;
        }
        let shape = IntegrityShape::parse(shape).unwrap().unwrap();
        let input = RowSet::from_ids(["a", "b", "c", "d"].map(String::from));
        run_named_shapes(&ctx, input, &shape, keep).unwrap()
    }

    fn ids(rows: &RowSet) -> Vec<&str> {
        rows.rows().iter().map(|row| row.id.as_str()).collect()
    }

    // spec: MEDULLA-FEDERATED-QUERY-R005.700
    #[test]
    fn finding_per_shape_and_bound() {
        // One violation per shape, with the shape's severity and blocking flag.
        let cases = [
            (
                "integrity:required_property:name",
                "c",
                Severity::Warn,
                false,
            ),
            ("integrity:orphan_node", "c", Severity::Info, false),
            ("integrity:dangling_edge", "a", Severity::Fatal, true),
            (
                "integrity:invalid_reference:owner",
                "d",
                Severity::Fatal,
                true,
            ),
        ];
        for (shape, focus, severity, blocks) in cases {
            let out = run(shape, ShapeKeep::Violating, None);
            assert_eq!(ids(&out), vec![focus], "{shape}");
            assert_eq!(out.text(focus, "shape.shape_id"), Some(shape));
            assert_eq!(out.text(focus, "shape.severity"), Some(severity.as_str()));
            assert_eq!(
                out.text(focus, "shape.blocks"),
                Some(blocks.to_string().as_str())
            );
            assert_eq!(out.text(focus, "shape.findings"), Some("1"));
            // The complement is the conforming rows, with no report columns.
            let conforming = run(shape, ShapeKeep::Conforming, None);
            assert_eq!(conforming.len(), 3, "{shape}");
            assert!(conforming.texts().is_empty());
        }

        // One visit per element: nodes once each, edge entries once each.
        let view = graph();
        let (_, visits) = scan(&view, &IntegrityShape::OrphanNode);
        assert_eq!(visits, ScanVisits { nodes: 4, edges: 0 });
        let (_, visits) = scan(&view, &IntegrityShape::DanglingEdge);
        assert_eq!(
            visits,
            ScanVisits {
                nodes: 0,
                edges: view.edge_properties.len()
            }
        );

        // The per-shape bound: findings past the cap are dropped, smallest focus kept.
        let core = GraphCore::new();
        for ordinal in 0..(MAX_FINDINGS_PER_SHAPE + 5) {
            core.add_node(format!("n{ordinal:06}"), props(serde_json::json!({})));
        }
        let big = core.analysis_snapshot();
        let (findings, _) = scan(&big, &IntegrityShape::OrphanNode);
        assert_eq!(findings.len(), MAX_FINDINGS_PER_SHAPE);
        assert_eq!(findings.first().unwrap().focus, "n000000");

        // A graph over the node budget is not scanned and never reads as clean.
        let out = run("integrity:orphan_node", ShapeKeep::Violating, Some(2));
        assert_eq!(out.len(), 4);
        assert_eq!(out.text("a", "shape.coverage"), Some("gap:0/2"));
    }
}
