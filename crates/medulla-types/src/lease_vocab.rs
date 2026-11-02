//! Control-lease bounds shared by the lease, elevation and governed-change
//! vocabularies, so none of them reaches into another for a constant.

/// Longest `hard_expires_at_ms - issued_at_ms` a lease may span (24 hours).
pub const MAX_CONTROL_LEASE_SPAN_MS: u64 = 24 * 60 * 60 * 1000;
/// The reserved kind of a just-in-time RBAC elevation (EH-404,
/// `crate::rbac_elevation`). Only the two-person `RbacElevation` flow issues
/// one; `IssueControlLease` refuses it.
pub const RBAC_ELEVATION_KIND: &str = "rbac.elevation";
