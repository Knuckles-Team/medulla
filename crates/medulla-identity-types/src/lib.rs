//! medulla-identity-types: Issuer, principal, scope, tenancy, ACL and authority types.
//!
//! Layer L1 (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates earlier in `[types_split].chain`. Budget: 25,000 lines of Rust. Specs that land here: engine-identity-and-scope (types).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

// MEDULLA-DECISION-ENGINE-R656.1 — the assistant profile and document-set wire model.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod assistant_profile;
// MEDULLA-UNIFIED-DATA-PLANE-R024.1 — typed one-at-a-time application admission
// stage machine, refusing to retire an ingest connector before parity is
// confirmed. Pure data; the admission runner, rollback path, and the
// per-application admission test are later children.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod platform_admission;
// MEDULLA-IDENTITY-R724.1 — the explicit state machine of `ApplicationAdmission.stage`.
#[allow(missing_docs)] // docs pending: new machine module
pub mod application_admission_machine;
// MEDULLA-TYPED-PACKS-R652.1 — the classification claim model.
pub mod classification;
// The engine-owned identity store: principals, credentials,
// sessions, roles/groups/IdPs, the auth-mode singleton and scope classes.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod identity;
// EH-404 -- just-in-time RBAC elevation: an `rbac.elevation` control lease the
// graph-access chokepoint consults (two-person, exact-scope, hard expiry).
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod rbac_elevation;
// MEDULLA-IDENTITY-R704.1 — the resource grant ledger in the RBAC policy image.
pub mod resource_grant;
