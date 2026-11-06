//! # Per-graph spatial index over geo literals (MEDULLA-FEDERATED-QUERY-R709.7)
//!
//! One R-tree per graph over the bounding boxes of its `geo:wktLiteral` objects, built with
//! the box and tree types of `medulla-geo` (EPSG:4326 only). [`SpatialIndex::apply`] is the
//! write side: the graph commit passes the literals an insert adds and a delete removes, and
//! the tree is rebuilt in the same call, so the index never disagrees with the graph. A
//! literal that fails R709.4 is not indexed and the whole apply refuses with that
//! [`GeoRefusal`] before any change.
//!
//! The query side is [`eval_relation_over`]: with an index it takes the candidate set from the
//! tree and runs the exact `medulla-geo` kernel test on the candidates only; without one it
//! scans every literal. Both paths return the same rows.

use std::collections::BTreeMap;

use medulla_geo::geometry::Bbox;
use medulla_geo::rtree::RTree;
use medulla_geo::{predicates, Geometry};

use crate::geosparql::{geom_from_checked, GeoRefusal};

/// The identity of one geo literal term within its graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TermId(pub u64);

/// The bounding box of a checked geometry, or the R709.4 refusal for an empty one.
fn bbox_of(geometry: &Geometry) -> Result<Bbox, GeoRefusal> {
    geometry.bbox().ok_or(GeoRefusal::LiteralInvalid)
}

/// One graph's R-tree over the boxes of its WKT literals.
pub struct SpatialIndex {
    /// The graph the index belongs to.
    pub graph: String,
    boxes: BTreeMap<TermId, Bbox>,
    /// Tree item ids are positions in `order`.
    order: Vec<TermId>,
    tree: RTree,
}

impl SpatialIndex {
    /// An empty index for `graph`.
    pub fn new(graph: impl Into<String>) -> Self {
        Self {
            graph: graph.into(),
            boxes: BTreeMap::new(),
            order: Vec::new(),
            tree: RTree::build(&[]),
        }
    }

    /// Number of indexed literals.
    pub fn len(&self) -> usize {
        self.boxes.len()
    }

    /// Whether no literal is indexed.
    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }

    /// Apply one commit: remove the `deletes`, add the `inserts` (a `(term, lexical)` pair
    /// whose term is already present is replaced). Every insert is checked first; one that
    /// fails R709.4 refuses the call and leaves the index unchanged.
    pub fn apply(
        &mut self,
        inserts: &[(TermId, &str)],
        deletes: &[TermId],
    ) -> Result<(), GeoRefusal> {
        let mut staged = Vec::with_capacity(inserts.len());
        for (term, lexical) in inserts {
            staged.push((*term, bbox_of(&geom_from_checked(lexical)?)?));
        }
        for term in deletes {
            self.boxes.remove(term);
        }
        for (term, bbox) in staged {
            self.boxes.insert(term, bbox);
        }
        self.rebuild();
        Ok(())
    }

    fn rebuild(&mut self) {
        self.order = self.boxes.keys().copied().collect();
        let items: Vec<(usize, Bbox)> = self
            .order
            .iter()
            .enumerate()
            .map(|(pos, term)| (pos, self.boxes[term]))
            .collect();
        self.tree = RTree::build(&items);
    }

    /// The terms whose box intersects `window` (touching edges count), in term order.
    pub fn candidates(&self, window: &Bbox) -> Vec<TermId> {
        let mut out: Vec<TermId> = self
            .tree
            .query_bbox(window)
            .into_iter()
            .map(|pos| self.order[pos])
            .collect();
        out.sort_unstable();
        out
    }
}

/// How a spatial function narrows its candidates.
enum Narrowing {
    /// A true row needs the literal's box to meet the constant's box.
    Meets,
    /// A true row may be box-disjoint (`sfDisjoint`): the complement is true without a test.
    Complement,
    /// A true row lies within `bound` of the constant (`distance`).
    Within(f64),
}

/// A relation's test of `(literal, constant)` and how the tree narrows it; `None` for a
/// function that is not a boolean spatial relation over a constant.
fn plan(
    local: &str,
    bound: Option<f64>,
) -> Option<(Narrowing, Box<dyn Fn(&Geometry, &Geometry) -> bool>)> {
    if local == "distance" {
        let bound = bound?;
        return Some((
            Narrowing::Within(bound),
            Box::new(move |a, b| predicates::distance(a, b) <= bound),
        ));
    }
    let narrowing = if local == "sfDisjoint" {
        Narrowing::Complement
    } else {
        Narrowing::Meets
    };
    let relation = crate::geosparql::relation_fn(local)?;
    Some((narrowing, Box::new(move |a, b| relation(a, b))))
}

/// The terms among `literals` for which `local(literal, constant)` holds (`distance` holds
/// when the planar distance is within `bound`). With `index` present the candidates come from
/// the tree and only they take the exact kernel test; with `None` every literal is scanned.
/// `Ok(None)` is an unknown function; `Err` an operand refused by R709.4. Rows are in term
/// order on both paths.
pub fn eval_relation_over(
    local: &str,
    bound: Option<f64>,
    constant_wkt: &str,
    literals: &[(TermId, String)],
    index: Option<&SpatialIndex>,
) -> Result<Option<Vec<TermId>>, GeoRefusal> {
    let Some((narrowing, test)) = plan(local, bound) else {
        return Ok(None);
    };
    let constant = geom_from_checked(constant_wkt)?;
    let by_term: BTreeMap<TermId, &str> = literals.iter().map(|(t, s)| (*t, s.as_str())).collect();
    let holds = |term: TermId| -> bool {
        by_term
            .get(&term)
            .and_then(|lexical| geom_from_checked(lexical).ok())
            .is_some_and(|g| test(&g, &constant))
    };
    let mut rows: Vec<TermId> = match index {
        None => by_term.keys().copied().filter(|t| holds(*t)).collect(),
        Some(index) => {
            let window = match (bbox_of(&constant), &narrowing) {
                (Ok(b), Narrowing::Within(d)) => {
                    Bbox::new(b.minx - d, b.miny - d, b.maxx + d, b.maxy + d)
                }
                (Ok(b), _) => b,
                (Err(refusal), _) => return Err(refusal),
            };
            let near = index.candidates(&window);
            match narrowing {
                Narrowing::Complement => {
                    let near_set: std::collections::BTreeSet<TermId> =
                        near.iter().copied().collect();
                    let mut far: Vec<TermId> = by_term
                        .keys()
                        .copied()
                        .filter(|t| !near_set.contains(t))
                        .collect();
                    far.extend(near.into_iter().filter(|t| holds(*t)));
                    far
                }
                _ => near.into_iter().filter(|t| holds(*t)).collect(),
            }
        }
    };
    rows.sort_unstable();
    Ok(Some(rows))
}
