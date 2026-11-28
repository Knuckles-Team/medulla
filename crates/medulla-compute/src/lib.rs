//! medulla-compute — the compute domains layered above the graph core: always-on graph
//! `algorithms` + `ast`/`parser` (their tree-sitter parts gated by `ast`), plus the
//! feature-gated `finance`, `datascience`, and `reasoning` domains. Depends on
//! `medulla-types` and `medulla-core`, never on `medulla-server`. It re-exports nothing from
//! the lower crates: a caller names the owning crate.

// CONCEPT:MEDULLA-KG.compute.graph-data-science-algorithms — standalone graph data-science algorithms (Neo4j GDS parity).
// Pure-Rust, deterministic, generic over node id; decoupled from the live engine
// graph so it is unit-testable in isolation. Always-on (no heavy deps).
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod ast;

#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod parser;

// Decide layer packages A2/A5 — the assembly decision function: step-1b
// eliminations, the typed model over the legal remainder, the solve with its
// no-good loop, why-not re-solves and the sealed, replayable record.
#[cfg(feature = "decide")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod assemble;
