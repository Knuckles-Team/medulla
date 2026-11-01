//! medulla-graph-core: In-memory property graph, its indexes, paths, projections and JSONPath.
//!
//! Level L3 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: durable-graph-kernel (graph).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(feature = "contract")]
mod contract;

/// CONCEPT:MEDULLA-KG.storage.bloom-negative-lookup-guard — lock-free per-graph Bloom
/// filter guarding the durable read-through negative-lookup path. Pure-Rust,
/// always compiled (no feature gate — the default/Pi build already links it).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod bloom;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod broker_keys;
#[cfg(feature = "cold-tier")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod cold_tier;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod graph;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod index;
/// CONCEPT:MEDULLA-KG.compute.jsonpath-evaluator — pure-Rust JSONPath evaluator + Postgres-`@>` containment.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod jsonpath;
/// CONCEPT:MEDULLA-KG.storage.path-index-store — durable JSONPath index persistence. The dep-free
/// `PathIndexPersistence` seam + in-memory default are always compiled; the
/// redb-backed `RedbPathIndexStore` is gated behind the `path-persist` feature.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod path_persist;
// One test-only composition root for the kernel-owned stores this crate opens.
// Never compiled outside `cfg(test)`.
/// The bounded per-actor, version-and-generation-keyed cache mechanism the two RLS read
/// caches (`rls_projection_cache`, `rls_view_cache`) share; each of those modules keeps
/// its own rationale and names this type. Gated with them.
#[cfg(feature = "security")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub(crate) mod per_actor_cache;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod read_through;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod rls;
/// CONCEPT:MEDULLA-KG.sharding.row-level-security (D-OP-1 / D-OB-20) — bounded per-actor
/// cache for `GraphReadAuthority::project_core`'s RLS projection, so a repeat read
/// by the same actor at an unchanged graph version reuses the materialized
/// projection instead of rebuilding it (sort+clone every visible node/edge, copy
/// every visible embedding) on every single call. Gated with `security` — it is
/// only ever consulted from the `security`-gated half of `project_core`.
#[cfg(feature = "security")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub(crate) mod rls_projection_cache;
/// CONCEPT:MEDULLA-KG.sharding.row-level-security (perf/cold-query-floor-analysis) — bounded
/// per-actor cache of the RLS-FILTERED `GraphView` `Method::CypherQuery`'s cache-miss
/// path builds, structurally a sibling of `rls_projection_cache` (same per-actor +
/// whole-image-`generation` idiom) but caching the lighter-weight `GraphView` instead of
/// a second whole `GraphCore`. See `crate::rls_view_cache` for the full rationale —
/// UNCOMPILED proposed fix, not yet wired into any caller.
#[cfg(feature = "security")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub(crate) mod rls_view_cache;
/// CONCEPT:MEDULLA-KG.compute.scene-graph-primitives — scene-graph / 3D world-model primitives (poses, transform
/// composition, AABBs). Pure deterministic math; the `GraphCore` scene methods
/// (`add_scene_object`, `world_transform`, spatial relations) live in `graph`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod scene;
#[cfg(all(test, feature = "path-persist"))]
pub(crate) mod test_scope_grant;
#[cfg(test)]
pub(crate) mod test_threads;
#[cfg(test)]
pub(crate) static PROP_ENV_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
