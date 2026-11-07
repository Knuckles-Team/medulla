//! medulla-query-types: Query plan AST and wire DTO types
//!
//! Layer `contract` (MEDULLA-SDLC-R021). Depends on `medulla-types` only.

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
pub mod telemetry_derive;
pub mod topology;
#[cfg(feature = "viz")]
pub mod viz;
