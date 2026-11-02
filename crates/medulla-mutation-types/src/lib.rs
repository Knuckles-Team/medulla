//! medulla-mutation-types: Mutation, batch, change envelope, commit descriptor, outbox, lease and consensus record types.
//!
//! Level L1 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: durable-graph-kernel (types).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![allow(dead_code)] // moved helper code, as the facade crate did
#![allow(unused_imports)] // glob imports carried over from the facade; the visibility pass prunes them
#![forbid(unsafe_code)]
#![deny(missing_docs)]

// CONCEPT:AU-ORCH.scheduling — GOC-21 distributed CapacityCell/CapacityLease
// wire contract + pure fencing/admission algorithm (cross-host capacity
// authority for the AU fair scheduler). Unconditional, like `acl`/`jobs`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod capacity_lease;
// GOC-03 — the cross-domain commit-descriptor/read-barrier currency shared by
// graph, modality, vector, blob/refcount, time-series, evidence, table/lake, and
// terminal-analytics-outcome participants. Deliberately a NEW module (not folded
// into `mutation_batch`): `MutationBatch`/`MutationProjectionCursor` remain the
// per-surface request/outbox envelope; `CommitDescriptor` is the one commit
// identity every domain's participant registers against. See
// `plans/graph-os-completion-program/lanes/GOC-03-cross-domain-commit-currency.md`.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod commit_descriptor;
// RF-RULING-004 — current-only parent/participant transaction coordination DTOs.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod consensus;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mutation;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mutation_outbox;
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod mutation_scope;
// RF-RULING-004 — mutation-kernel-owned outbox intent, delivery, and cursor DTOs.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod outbox;
// EH-558 — sealed record classes: content-addressed rows generic writes may not change.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod sealed_record;
// EH-560 -- governed changes: reserved control-lease kinds whose approval EG
// proves two-person (approver != proposer, exact digest, single use).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod governed_change;
// EH-406 -- error-budget adaptive (AIMD) throttling on a capacity cell:
// automatic steps only narrow or give back what they narrowed.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod capacity_throttle;
// RF-ADR-009 D18 — EG-owned versioned source change sets and append-only
// write-back/reconciliation receipts. Connector transports remain in the SDK.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod write_back;
