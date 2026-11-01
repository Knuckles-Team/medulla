//! medulla-query — the SQL/Cypher query surface (CONCEPT:MEDULLA-KG.query.read-only-sql-query).
//!
//! Read-only relational query over a single graph's nodes. ALL DataFusion/Arrow
//! code lives behind the `sql` cargo feature so a default build links none of it
//! (the engine stays lean for a Raspberry Pi).
//!
//! The `cypher` feature (CONCEPT:MEDULLA-KG.query.dep-free-behind) is a DEP-FREE Cypher subset — a
//! hand-written recursive-descent parser compiled to the engine's own primitives
//! (the medulla-core label index, `vf2_subgraph_match`, petgraph BFS). It pulls NO
//! DataFusion, so it ships in the lean Pi build.
//!
//! ## ModalityContract (CONCEPT:E4) — deliberately NOT retrofitted
//! `medulla-query` is a pure QUERY-EXECUTOR surface: it parses SQL/Cypher and compiles it to
//! scans/BFS over `medulla-core`'s graph primitives, producing result rows — it owns no
//! persisted modality VALUE type of its own (its public items are query planners,
//! `exec_sql*` entry points, and column/plan descriptors, all transient). The stored
//! objects a query READS already carry `ModalityContract` in their own crates
//! (`medulla-core::NodeChange`, `medulla-tensor`, `medulla-tsdb::SeriesMeta`, `medulla-rdf::owl::ProofNode`,
//! …). Per the `medulla-modality` README's own note that "pure protocol/executor crates with
//! no modality VALUE type of their own — `ModalityContract` may simply not apply, which
//! is a legitimate outcome, not a gap", this crate is a documented SKIP.


