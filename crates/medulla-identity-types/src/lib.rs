//! medulla-identity-types: Issuer, principal, scope, tenancy, ACL and authority types.
//!
//! Level L1 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: engine-identity-and-scope (types).
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// RF-RULING-004 — canonical serializable authority/replay evidence for the future
// medulla-transaction admission boundary. This module grants no executable capability.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod authority;
// MEDULLA-SRC-AUTH-R007.1 — typed audit entries for source selection evidence.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod authority_audit;
// MEDULLA-UNIFIED-DATA-PLANE-R024.1 — typed one-at-a-time application admission
// stage machine, refusing to retire an ingest connector before parity is
// confirmed. Pure data; the admission runner, rollback path, and the
// per-application admission test are later children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod platform_admission;
// MEDULLA-SCM-R013 — graph-native symbol edits.
#[cfg(any(test, feature = "test-support"))]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod identity_fixtures;
// MEDULLA-IDENTITY-R610 — tenant parent chains, secured reads and ownership claims.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod tenancy;
// The engine-owned identity store: principals, credentials,
// sessions, roles/groups/IdPs, the auth-mode singleton and scope classes.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod identity;
// EH-404 -- just-in-time RBAC elevation: an `rbac.elevation` control lease the
// graph-access chokepoint consults (two-person, exact-scope, hard expiry).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod rbac_elevation;
