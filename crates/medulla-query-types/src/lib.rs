//! medulla-query-types: Query plan AST and wire DTO types
//!
//! Layer `contract` (MEDULLA-SDLC-R021). Depends on `medulla-types` only (R021.4 adds `medulla-identity-types`).

#![forbid(unsafe_code)]

// CONCEPT:MEDULLA-KG.compute.native-asr-whisper-provider — the native-ASR agent-facing wire op
// (`AsrOp`), gated `asr-native`. Lives here (not in `medulla-asr-whisper`), for the
// SAME reason `quantum.rs` lives here rather than in `medulla-quantum-core`: `medulla-types`
// is the bottom-of-DAG crate every wire consumer already depends on, while the
// heavy provider crate sits above the facade. Pure serde — no dep on `medulla-audio`/
// `medulla-asr-whisper`.
#[cfg(feature = "asr-native")]
pub mod asr_wire;
// MEDULLA-FEDERATED-QUERY-R608.1 — causal model fit verdict (`CausalVerify`).
pub mod causal_wire;
// MEDULLA-FEDERATED-QUERY-R006.1 — origin record of a temporal rule-derived fact.
pub mod derived_fact;
// MEDULLA-UNIFIED-DATA-PLANE-R651.1 — document delete request and receipt.
pub mod document_ops;
// MEDULLA-FEDERATED-QUERY-R086.100.1 — ordered n-ary inductive link prediction model.
pub mod graphlearn_nary;
// MEDULLA-FEDERATED-QUERY-R058.2.2.1 — the disclosed NL query result (executed UQL + rows).
pub mod nl_query;
// MEDULLA-DURABLE-KERNEL-R041 — the SQL plan cache key (statement digest + schema
// version) and its lookup decision. Pure, no dep beyond `contract::Digest256`.
pub mod plan_cache_key;
// CONCEPT:MEDULLA-KG.compute.quantum-agent-api — the agent-facing quantum control-plane
// wire op (`QuantumOp`), gated `quantum`. Lives here (not in `medulla-quantum-core`,
// which sits ABOVE this crate in the DAG) for the SAME reason `jobs`/`statechart`
// do: `protocol::Method::Quantum` carries it over the wire, and `protocol` is
// bottom-of-DAG. Pure serde — no dep on `medulla-quantum-core`.
#[cfg(feature = "quantum")]
pub mod quantum;
// MEDULLA-TYPED-PACKS-R803.1 — typed sensitive-data detector and detection records.
pub mod sensitive_detect;
pub mod solve_scalar;
pub mod telemetry_derive;
pub mod topology;
#[cfg(feature = "viz")]
pub mod viz;
// F3: RDF load/update/rule/shape-validation report bodies (medulla-rdf/medulla-shacl/medulla-shex results).
pub mod rdf_report;
// MEDULLA-UNIFIED-DATA-PLANE-R038.5.1 — the search-index capability of an operation-bound source.
pub mod search_source;
// EH-522 — the series-expression algebra (`DERIVE` columns, derived series). Pure serde.
pub mod series_expr;
// MEDULLA-BUSINESS-OPS-R002.7 — the wire form of the UQL `EVALUATE RULE` stage. Pure serde.
pub mod rule_expr;
// MEDULLA-UNIFIED-DATA-PLANE-R020.1 — typed federated warehouse/lake source-kind
// model (Snowflake, BigQuery, DuckDB, Iceberg) and its per-kind required-field
// refusal. Pure data; the registered `ForeignSourceSpec::Warehouse` carries its config.
pub mod warehouse_federation;
// MEDULLA-FEDERATED-QUERY-R738.17 — the lifecycle of one EXPLAIN/PROFILE stage report.
#[cfg(feature = "query")]
pub mod uql_machine;
// The query plan AST and wire DTOs embedded in feature-gated `protocol::Method` variants.
// Domain-specific DTOs sit in private child modules and are re-exported by `wire` itself.
pub mod wire;
