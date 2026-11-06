//! Pattern binding: node labels, relationship hops and path bounds.

use std::collections::BTreeSet;

use medulla_types::virtual_graph_view::{GraphView, NodeLabelMapping, RelationshipTypeMapping};

use super::{unsupported, ViewLimits, ViewRefusal};
use crate::cypher::plan::{CompareOp, Direction, EdgePat, NodePat, Pattern, PropVal};

/// A node variable bound to its mapped label.
pub(super) struct Bound<'v> {
    pub(super) var: String,
    pub(super) mapping: &'v NodeLabelMapping,
}

/// One same-source join: it becomes one pushed statement with one mapping digest.
pub(super) struct Fragment<'v> {
    pub(super) from: String,
    pub(super) to: String,
    pub(super) join: &'v RelationshipTypeMapping,
}

/// A pushable `var.prop <op> literal` comparison.
pub(super) struct Pushed {
    pub(super) var: String,
    pub(super) prop: String,
    pub(super) op: &'static str,
    pub(super) literal: String,
}

/// The compile state of one view read.
pub(super) struct Compiler<'v> {
    pub(super) view: &'v GraphView,
    pub(super) limits: ViewLimits,
    pub(super) bound: Vec<Bound<'v>>,
    pub(super) fragments: Vec<Fragment<'v>>,
    pub(super) triples: Vec<String>,
    pub(super) filters: Vec<String>,
    pub(super) pushed: Vec<Pushed>,
    pub(super) props: BTreeSet<(String, String)>,
    pub(super) prefixes: BTreeSet<String>,
}

fn is_safe_term(term: &str) -> bool {
    !term.is_empty() && !term.contains(|c: char| c.is_whitespace() || "<>\"{}|\\^`".contains(c))
}

impl<'v> Compiler<'v> {
    pub(super) fn new(view: &'v GraphView, limits: ViewLimits) -> Self {
        Self {
            view,
            limits,
            bound: Vec::new(),
            fragments: Vec::new(),
            triples: Vec::new(),
            filters: Vec::new(),
            pushed: Vec::new(),
            props: BTreeSet::new(),
            prefixes: BTreeSet::new(),
        }
    }

    fn resolve(&self, label: &str) -> Result<&'v NodeLabelMapping, ViewRefusal> {
        self.view
            .resolve_label(label)
            .ok()
            .filter(|m| is_safe_term(&m.ontology_class))
            .ok_or_else(|| ViewRefusal::UnmappedLabel(label.to_string()))
    }

    /// The join of the hop's relationship type; quantified, undirected and untyped hops refuse.
    fn hop_join(&self, edge: &EdgePat) -> Result<&'v RelationshipTypeMapping, ViewRefusal> {
        if edge.group.is_some() {
            return Err(unsupported("quantified path pattern"));
        }
        if matches!(edge.direction, Direction::Both) {
            return Err(unsupported("undirected relationship"));
        }
        let rel = edge.rel_type.as_deref().unwrap_or("(untyped relationship)");
        self.view
            .relationship_types
            .iter()
            .find(|r| r.rel_type == rel)
            .ok_or_else(|| ViewRefusal::UnmappedLabel(rel.to_string()))
    }

    /// The (pattern-left label, pattern-right label) of a hop.
    fn endpoints(join: &'v RelationshipTypeMapping, dir: Direction) -> (&'v str, &'v str) {
        match dir {
            Direction::Left => (&join.to_label, &join.from_label),
            _ => (&join.from_label, &join.to_label),
        }
    }

    /// Bind the node (naming an anonymous one) and emit its class triple and inline properties.
    fn bind_node(
        &mut self,
        node: &NodePat,
        inferred: Option<&str>,
        idx: usize,
    ) -> Result<String, ViewRefusal> {
        let label = node.label.as_deref().or(inferred);
        let label =
            label.ok_or_else(|| ViewRefusal::UnmappedLabel("(unlabeled node)".to_string()))?;
        let mapping = self.resolve(label)?;
        let var = node.var.clone().unwrap_or_else(|| format!("_n{idx}"));
        let class = self.class_term(&mapping.ontology_class);
        self.triples.push(format!("?{var} a {class} ."));
        self.bound.push(Bound {
            var: var.clone(),
            mapping,
        });
        for (key, value) in node.props.iter().flatten() {
            let PropVal::Lit(lit) = value else {
                return Err(unsupported("parameter in property map"));
            };
            self.compare(&var, key, CompareOp::Eq, lit)?;
        }
        Ok(var)
    }

    /// Bind every node and hop of the single `MATCH` pattern.
    pub(super) fn bind_pattern(&mut self, pattern: &Pattern) -> Result<(), ViewRefusal> {
        let first = match pattern.hops.first() {
            Some((edge, _)) => Some(Self::endpoints(self.hop_join(edge)?, edge.direction).0),
            None => None,
        };
        let mut prev = self.bind_node(&pattern.start, first, 0)?;
        for (idx, (edge, node)) in pattern.hops.iter().enumerate() {
            let join = self.hop_join(edge)?;
            let target = Self::endpoints(join, edge.direction).1;
            let next = self.bind_node(node, Some(target), idx + 1)?;
            self.bind_hop(&prev, &next, edge, join)?;
            prev = next;
        }
        Ok(())
    }

    fn bind_hop(
        &mut self,
        left: &str,
        right: &str,
        edge: &EdgePat,
        join: &'v RelationshipTypeMapping,
    ) -> Result<(), ViewRefusal> {
        let (subject, object) = match edge.direction {
            Direction::Left => (right, left),
            _ => (left, right),
        };
        let class = self.resolve(&join.from_label)?.ontology_class.clone();
        let path = self.path_term(&class, &join.rel_type, edge.var_len)?;
        self.triples.push(format!("?{subject} {path} ?{object} ."));
        self.fragments.push(Fragment {
            from: subject.to_string(),
            to: object.to_string(),
            join,
        });
        Ok(())
    }

    /// The predicate path of a hop: one IRI, or an alternation bounded by the depth knob.
    fn path_term(
        &mut self,
        class: &str,
        rel: &str,
        var_len: Option<(usize, usize)>,
    ) -> Result<String, ViewRefusal> {
        let one = self.predicate_term(class, rel);
        let Some((lo, hi)) = var_len else {
            return Ok(one);
        };
        let token = format!("*{lo}..{hi}");
        if hi > self.limits.max_path_depth {
            return Err(ViewRefusal::PathDepthExceeded(token));
        }
        if lo == 0 || lo > hi {
            return Err(unsupported(token));
        }
        let steps: Vec<String> = (lo..=hi).map(|n| vec![one.as_str(); n].join("/")).collect();
        Ok(format!("({})", steps.join("|")))
    }
}
