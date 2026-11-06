//! The spatial FILTER index scan (MEDULLA-FEDERATED-QUERY-R709.7, D112).
//!
//! `FILTER(geof:F(?w, "<wkt>"^^geo:wktLiteral))` (or `geof:distance(?w, C, u) <= d`)
//! over a basic graph pattern that binds `?w` as the object of one triple pattern
//! is planned as an index scan: that triple pattern is matched only on the subject
//! nodes the spatial index returns for the constant's window, and the exact kernel
//! test then runs on those rows alone. The plan is taken only when the dataset's
//! spatial binding reflects the snapshot read (version-checked) and the active
//! graph is the default graph; otherwise the filter scans row by row as before.
//! Both paths return the same rows.

use std::collections::HashSet;

use spargebra::algebra::Function;

use super::*;
use crate::spatial_index::{index_window, Narrowing, SpatialAccessor};

/// The RDF geometry predicate the graph spatial index keeps.
const GEO_AS_WKT: &str = "http://www.opengis.net/ont/geosparql#asWKT";

/// A spatial FILTER the index can narrow: the function's local name, the bound
/// variable and the constant operand.
pub(super) struct SpatialFilter<'e> {
    pub(super) local: &'e str,
    variable: &'e str,
    operand: &'e Expression,
    bound: Option<f64>,
    constant: &'e str,
}

fn geof_local(function: &Function) -> Option<&str> {
    match function {
        Function::Custom(iri) => iri.as_str().strip_prefix(crate::geosparql::GEOF_NS),
        _ => None,
    }
}

/// A planar unit: `distance` narrows only in coordinate units, never metres.
fn planar_units(units: Option<&Expression>) -> bool {
    match units {
        None => true,
        Some(Expression::NamedNode(iri)) => !iri.as_str().to_ascii_lowercase().contains("met"),
        Some(_) => false,
    }
}

/// `geof:F(?w, C, …)` → `(F, ?w, C)`.
fn call_parts(expr: &Expression) -> Option<(&str, &Expression, &str, &str)> {
    let Expression::FunctionCall(function, args) = expr else {
        return None;
    };
    let local = geof_local(function)?;
    if local == "distance" && !planar_units(args.get(2)) {
        return None;
    }
    let operand = args.first()?;
    let (Expression::Variable(variable), Expression::Literal(constant)) = (operand, args.get(1)?)
    else {
        return None;
    };
    Some((local, operand, variable.as_str(), constant.value()))
}

fn numeric(expr: &Expression) -> Option<f64> {
    match expr {
        Expression::Literal(literal) => literal.value().parse().ok(),
        _ => None,
    }
}

impl<'e> SpatialFilter<'e> {
    /// The narrowable spatial filter `expr` states, if any.
    pub(super) fn of(expr: &'e Expression) -> Option<Self> {
        let (call, bound) = match expr {
            Expression::LessOrEqual(left, right) | Expression::Less(left, right) => {
                (left.as_ref(), Some(numeric(right)?))
            }
            other => (other, None),
        };
        let (local, operand, variable, constant) = call_parts(call)?;
        if (local == "distance") != bound.is_some() {
            return None;
        }
        Some(Self {
            local,
            variable,
            operand,
            bound,
            constant,
        })
    }

    /// The index of the `?s geo:asWKT ?w` triple pattern in `patterns` that binds
    /// the filter variable: the index keeps exactly that predicate's literals.
    fn anchor(&self, patterns: &[TriplePattern]) -> Option<usize> {
        patterns.iter().position(|tp| {
            let indexed =
                matches!(&tp.predicate, NamedNodePattern::NamedNode(p) if p.as_str() == GEO_AS_WKT);
            let binds =
                matches!(&tp.object, TermPattern::Variable(v) if v.as_str() == self.variable);
            indexed && binds
        })
    }

    /// A row the complement accepts must still carry a valid geometry operand, as
    /// the scan would have checked it (R709.4).
    fn operand_valid(&self, ctx: &Ctx, sol: &Solution) -> bool {
        expr_str(ctx, self.operand, sol)
            .is_some_and(|lexical| crate::geosparql::geom_from_checked(&lexical).is_ok())
    }
}

/// The accessor a query on `ctx` may scan, when it reflects the snapshot read.
fn current_accessor<'a>(ctx: &Ctx<'a>) -> Option<&'a dyn SpatialAccessor> {
    std::ptr::eq(ctx.active, ctx.ds.default)
        .then(|| ctx.ds.spatial_accessor())
        .flatten()
}

/// The planned index scan of `filter` over `inner`, or `None` when the plan does
/// not apply.
pub(super) fn plan<'p>(
    ctx: &Ctx,
    filter: &SpatialFilter<'_>,
    inner: &'p GraphPattern,
) -> Option<(
    &'p [TriplePattern],
    usize,
    (Narrowing, medulla_geo::geometry::Bbox),
)> {
    current_accessor(ctx)?;
    let GraphPattern::Bgp { patterns } = inner else {
        return None;
    };
    let anchor = filter.anchor(patterns)?;
    let window = index_window(filter.local, filter.bound, filter.constant).ok()??;
    Some((patterns.as_slice(), anchor, window))
}

/// Match `tp` on the given subject nodes only.
fn match_nodes<'i>(
    ctx: &Ctx,
    tp: &TriplePattern,
    ids: impl Iterator<Item = &'i String>,
) -> Vec<Solution> {
    let mut out = Vec::new();
    for id in ids {
        let Some(blob) = ctx.active.node_properties.get(id) else {
            continue;
        };
        let Ok(value) = medulla_types::msgpack::decode_property_value(blob.as_slice()) else {
            continue;
        };
        let Some(obj) = value.as_object() else {
            continue;
        };
        match_triple_pattern_literals(ctx, tp, obj, &ctx.proj.node_iri(id), &mut out);
    }
    out
}

fn join(acc: Vec<Solution>, matches: &[Solution]) -> Vec<Solution> {
    let mut next = Vec::new();
    for base in &acc {
        next.extend(matches.iter().filter_map(|m| merge(base, m)));
    }
    next
}

/// Join `patterns` with the anchor pattern's matches replaced by `anchored`.
fn join_with(
    ctx: &Ctx,
    patterns: &[TriplePattern],
    anchor: usize,
    anchored: &[Solution],
) -> Vec<Solution> {
    let mut acc = vec![Solution::new()];
    for (at, tp) in patterns.iter().enumerate() {
        acc = if at == anchor {
            join(acc, anchored)
        } else {
            join(acc, &match_triple_pattern(ctx, tp))
        };
        if acc.is_empty() {
            break;
        }
    }
    acc
}

/// Evaluate `inner` as the index scan of `expr`, or `None` to scan as before.
/// Returns `(tested, accepted)`: `tested` are the BGP rows whose anchor subject is
/// an index candidate, which the caller still runs through the exact FILTER;
/// `accepted` are, for `sfDisjoint` only, the rows whose box misses the window and
/// so hold without a test.
pub(super) fn try_index_scan(
    ctx: &Ctx,
    expr: &Expression,
    inner: &GraphPattern,
) -> Option<(Vec<Solution>, Vec<Solution>)> {
    let filter = SpatialFilter::of(expr)?;
    let (patterns, anchor, (narrowing, window)) = plan(ctx, &filter, inner)?;
    let candidates: HashSet<String> = current_accessor(ctx)?
        .candidates(&window)
        .into_iter()
        .collect();
    let tp = &patterns[anchor];
    let near = match_nodes(ctx, tp, candidates.iter());
    let tested = join_with(ctx, patterns, anchor, &near);
    let accepted = match narrowing {
        Narrowing::Meets => Vec::new(),
        Narrowing::Complement => {
            let far_ids = ctx.active.node_properties.keys();
            let far = match_nodes(ctx, tp, far_ids.filter(|id| !candidates.contains(*id)));
            let mut rows = join_with(ctx, patterns, anchor, &far);
            rows.retain(|sol| filter.operand_valid(ctx, sol));
            rows
        }
    };
    Some((tested, accepted))
}

/// Every spatial function a FILTER of `pattern` calls that the index scan serves.
pub(super) fn indexed_functions<'p>(ctx: &Ctx, pattern: &'p GraphPattern, out: &mut Vec<&'p str>) {
    for child in children(pattern) {
        indexed_functions(ctx, child, out);
    }
    if let GraphPattern::Filter { expr, inner } = pattern {
        if let Some(filter) = SpatialFilter::of(expr) {
            if plan(ctx, &filter, inner).is_some() {
                out.push(filter.local);
            }
        }
    }
}

fn children(pattern: &GraphPattern) -> Vec<&GraphPattern> {
    match pattern {
        GraphPattern::Filter { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Extend { inner, .. }
        | GraphPattern::Group { inner, .. } => vec![inner.as_ref()],
        GraphPattern::Join { left, right }
        | GraphPattern::LeftJoin { left, right, .. }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right } => vec![left.as_ref(), right.as_ref()],
        _ => Vec::new(),
    }
}
