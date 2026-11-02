//! medulla-result-types: Answer, result contract, verdict, epistemic node, argument and query predicate types.
//!
//! Level L1 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: federated-query-and-reasoning (result types).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

// MEDULLA-FEDERATED-QUERY-R083.1 — typed prior/retrieved candidates and the closed
// arbitration verdict value; support classification and policy live above.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod retrieval_arbitration;
// CONCEPT:MEDULLA-KG.compute.epistemic-operations-protocol — strict shared DTOs for
// RequestContext, mutation/ingestion, work, artifact, query, job, and trace outcomes.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod epistemic_operations;
// GOC-46 — hand-written wire DTOs (WorkItemClaimCapability*, CasWorkItemMetadata*)
// that the AU protocol catalog does not yet declare (needs generator support for a
// binary payload type). Kept OUT of `epistemic_operations` so that module can stay
// exactly generator-clean; see the module doc comment for the full finding.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod epistemic_operations_ext;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod epistemic_operations_manifest;
// EH-194 — typed `Claim`/`Evidence` graph nodes: the one owner of the shape every
// epistemic writer stores and every reader decodes. Pure serde, unconditional.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod epistemic_node;
// GOC-20 — the atomic WorkItem outcome/provenance bundle and durable run-event
// wire contract (BUG-015). Deliberately a NEW module (not folded into
// `mutation_batch`), same rationale as `commit_descriptor` above: the native
// fusion of this bundle into `Method::CommitWorkItemResult`'s transaction is
// `mutation_batch.rs` work, currently blocked on the FO-001 five-lane
// (GOC-03/04/19/20/35) file-ownership collision. See
// `plans/graph-os-completion-program/decisions/GOC-20-atomic-outcome-provenance.md`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod outcome_bundle;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod row_predicate;
// MEDULLA-FEDERATED-QUERY-R082.1 — typed calibration, bounded limits and closed
// stop-verdict vocabulary; decision precedence lives in medulla-plan.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod retrieval_stop;
// Result bodies of the `compute` contract domain, declared by
// `result_contract::compute`; gated per compute family like `wire`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod compute_result;
// EH-400 — declared freshness: `eg:volatilityClass`, per-class invalidation events and
// foreign-source watermarks (the `FreshnessFeed` wire shapes).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod freshness;
// MEDULLA-FEDERATED-QUERY-R700 — AIF argument maps and their Dung projection.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod argument_map;
// MEDULLA-FEDERATED-QUERY-R081 — retrieval sufficiency, selection, feedback and
// hierarchy contracts (incl. the R081.9.7.1 section hierarchy policy); the deciding functions live in `medulla_plan::retrieval_verdict`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod retrieval_verdict;
// MEDULLA-FEDERATED-QUERY-R085.1 — approved, read-only source query records.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod source_query;
