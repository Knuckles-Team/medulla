//! # Spatial index access for SPARQL (MEDULLA-FEDERATED-QUERY-R709.7)
//!
//! The graph's one spatial index is the maintained per-graph `GraphSpatialIndex`
//! (D112): it lives with the graph's other secondary indexes and is updated in the
//! same commit as the triples. This crate does not own an index. It defines the
//! [`SpatialAccessor`] seam (bbox query → subject node ids, plus the commit version
//! the index reflects); the index's own crate implements it, and the SPARQL
//! dataset carries one through [`SpatialBinding`].
//!
//! A binding is used only when the index version equals the snapshot version the
//! query reads ([`SpatialBinding::current`]); otherwise the plan scans, so a write
//! landing between snapshot and query never changes a result.
//!
//! [`index_window`] is the narrowing rule of the planner: the relations whose true
//! rows must meet the constant's box (`sfWithin`, `sfIntersects`, `sfContains`,
//! `sfOverlaps`, `sfEquals`, `sfTouches`, `sfCrosses`) take that box as the window,
//! and `distance` with a constant bound widens it by the bound. `sfDisjoint` takes the
//! complement: a row whose box misses the window is true without a test.

use std::sync::Arc;

use medulla_geo::geometry::Bbox;

use crate::geosparql::{geom_from_checked, GeoRefusal};

/// Bbox access to a graph's maintained spatial index over its WKT literals.
pub trait SpatialAccessor: Send + Sync {
    /// The graph commit version the index reflects.
    fn version(&self) -> u64;
    /// The subject node ids whose literal box meets `window` (touching edges count).
    fn candidates(&self, window: &Bbox) -> Vec<String>;
}

/// A spatial accessor bound to the snapshot version a query reads.
#[derive(Clone)]
pub struct SpatialBinding {
    accessor: Arc<dyn SpatialAccessor>,
    snapshot_version: u64,
}

impl SpatialBinding {
    /// Bind `accessor` to the snapshot at `snapshot_version`.
    pub fn new(accessor: Arc<dyn SpatialAccessor>, snapshot_version: u64) -> Self {
        Self {
            accessor,
            snapshot_version,
        }
    }

    /// The accessor, when the index reflects exactly the snapshot being read.
    pub fn current(&self) -> Option<&dyn SpatialAccessor> {
        (self.accessor.version() == self.snapshot_version).then_some(self.accessor.as_ref())
    }
}

/// How the index narrows a spatial function's rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Narrowing {
    /// A true row's box meets the window: only candidates take the exact test.
    Meets,
    /// `sfDisjoint`: a row whose box misses the window is true without a test, and
    /// only the candidates take the exact test.
    Complement,
}

/// How `local` narrows, or `None` when it cannot (unknown, or `distance` without a
/// constant bound).
pub fn narrowing(local: &str, bound: Option<f64>) -> Option<Narrowing> {
    match local {
        "distance" => bound.map(|_| Narrowing::Meets),
        "sfDisjoint" => Some(Narrowing::Complement),
        other => crate::geosparql::relation_fn(other).map(|_| Narrowing::Meets),
    }
}

/// The window an index scan takes candidates from for `local(literal, constant)`:
/// the constant's box, widened by the bound for `distance`. `None` when the
/// function cannot narrow; `Err` is the R709.4 refusal of the constant.
pub fn index_window(
    local: &str,
    bound: Option<f64>,
    constant_wkt: &str,
) -> Result<Option<(Narrowing, Bbox)>, GeoRefusal> {
    let Some(narrowing) = narrowing(local, bound) else {
        return Ok(None);
    };
    let constant = geom_from_checked(constant_wkt)?;
    let b = constant.bbox().ok_or(GeoRefusal::LiteralInvalid)?;
    let d = bound.unwrap_or(0.0);
    let window = Bbox::new(b.minx - d, b.miny - d, b.maxx + d, b.maxy + d);
    Ok(Some((narrowing, window)))
}
