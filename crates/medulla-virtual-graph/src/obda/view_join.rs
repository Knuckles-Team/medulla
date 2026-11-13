//! Same-source joins of a graph-view query, shipped as one source statement
//! (MEDULLA-UNIFIED-DATA-PLANE-R038.6.2, MEDULLA-UNIFIED-DATA-PLANE-R006).
//!
//! A relationship of a graph view joins the tables of two mapped labels. When both
//! tables belong to one database, [`apply_view_joins`] replaces the in-memory join of
//! two scans with one derived triples map whose source runs the join as one statement.

use std::collections::BTreeSet;
use std::sync::Arc;

use medulla_types::virtual_graph_view::ViewJoin;

use super::{
    ForeignRow, ObdaFilter, ObdaSource, ObdaSourceRegistry, ObjectMap, TriplesMap, VirtualGraph,
};

/// Prefix of the right-table columns in the rows a join statement returns.
pub const JOIN_RIGHT_PREFIX: &str = "rhs__";

/// The database and table behind one source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceIdentity {
    pub source_id: String,
    pub table: String,
}

/// One join statement: the source's own table (left) joined with `right_table`.
/// Output rows carry the left columns by name and the right columns as
/// `rhs__<column>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinScan {
    pub right_table: String,
    pub on_left: String,
    pub on_right: String,
}

/// A source that answers every scan with the join statement of its left source.
struct JoinSource {
    left: Arc<dyn ObdaSource>,
    join: JoinScan,
}

impl ObdaSource for JoinSource {
    fn scan(
        &self,
        needed: &BTreeSet<String>,
        _filters: &[ObdaFilter],
    ) -> Result<Vec<ForeignRow>, String> {
        self.left
            .scan_join(&self.join, needed)
            .unwrap_or_else(|| Err("obda: the source cannot run a join statement".into()))
    }
}

/// Rewrite every `{col}` of `template` to `{<prefix>col}`.
fn prefix_template_columns(template: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(template.len() + prefix.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            if chars.peek() == Some(&'{') {
                chars.next();
                out.push_str("{{");
                continue;
            }
            out.push('{');
            out.push_str(prefix);
        } else {
            out.push(c);
        }
    }
    out
}

fn map_index(vg: &VirtualGraph, class: &str) -> Option<usize> {
    vg.triples_maps
        .iter()
        .position(|map| map.subject_class.as_deref() == Some(class))
}

/// For each `join` whose two endpoint maps read tables of one database, add a derived
/// triples map that produces the relationship from ONE join statement, and drop the
/// relationship predicate from the from-endpoint map so it is not scanned twice. A join
/// across databases, or over a source that cannot join, is left to the evaluator: the
/// from-endpoint map must then declare the relationship predicate itself. Returns the
/// number of joins pushed.
pub fn apply_view_joins(
    vg: &mut VirtualGraph,
    registry: &mut ObdaSourceRegistry,
    joins: &[ViewJoin],
) -> Result<usize, String> {
    let mut pushed = 0;
    for join in joins {
        let (Some(from_idx), Some(to_idx)) = (
            map_index(vg, &join.from_class),
            map_index(vg, &join.to_class),
        ) else {
            continue;
        };
        let (from_map, to_map) = (&vg.triples_maps[from_idx], &vg.triples_maps[to_idx]);
        let (Some(left), Some(right)) = (
            registry.get(&from_map.logical_source).cloned(),
            registry.get(&to_map.logical_source).cloned(),
        ) else {
            continue;
        };
        let (Some(left_id), Some(right_id)) = (left.identity(), right.identity()) else {
            continue;
        };
        if left_id.source_id != right_id.source_id {
            continue;
        }
        let scan = JoinScan {
            right_table: right_id.table,
            on_left: join.on_from.clone(),
            on_right: join.on_to.clone(),
        };
        let name = format!(
            "view_join:{}:{}:{}",
            from_map.logical_source, to_map.logical_source, join.rel_predicate
        );
        let derived = TriplesMap::new(name.clone(), from_map.subject_template.clone()).add(
            join.rel_predicate.clone(),
            ObjectMap::Template(prefix_template_columns(
                &to_map.subject_template,
                JOIN_RIGHT_PREFIX,
            )),
        );
        registry.register(name, Arc::new(JoinSource { left, join: scan }));
        vg.triples_maps[from_idx]
            .predicate_object_maps
            .retain(|(predicate, _)| predicate != &join.rel_predicate);
        vg.triples_maps.push(derived);
        pushed += 1;
    }
    Ok(pushed)
}
