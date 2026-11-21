//! The authority a source-query approval is issued under (MEDULLA-FEDERATED-QUERY-R085.2).
//! One function derives it for issue and for verification, from the verified request context
//! only; a wire value is never read.

use crate::authority_context::VerifiedRequestContext;
use crate::carrier::CarrierAuthority;
use medulla_types::contract::Digest256;

/// The identity and digests the admission of a source query binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceQueryAuthority {
    pub tenant_ref: String,
    pub owner_ref: String,
    pub actor_ref: String,
    pub authority_digest: String,
    pub policy_digest: String,
    pub identity_digest: String,
}

/// Derive the source-query authority of `verified` for `graph`.
pub fn source_query_authority(
    verified: &VerifiedRequestContext,
    graph: &str,
) -> Result<SourceQueryAuthority, String> {
    let carrier = CarrierAuthority::from_verified(verified)?;
    let authority_digest = Digest256::framed(
        b"eg/source-query-authority/v1",
        &[carrier.authority_digest().as_bytes(), graph.as_bytes()],
    )?;
    let policy_digest = Digest256::framed(
        b"eg/source-query-policy/v1",
        &[verified.claims().policy_version.as_bytes()],
    )?;
    let principal_persistence_id = verified.principal_persistence_id();
    let delegation_digest = carrier.delegation_digest();
    let identity_digest = Digest256::framed(
        b"eg/source-query-identity/v1",
        &[
            verified.tenant().as_bytes(),
            principal_persistence_id.as_bytes(),
            verified.agent_id().as_bytes(),
            delegation_digest.as_bytes(),
        ],
    )?;
    Ok(SourceQueryAuthority {
        tenant_ref: verified.tenant().to_string(),
        owner_ref: carrier.owner_scope().to_string(),
        actor_ref: carrier.actor_scope().to_string(),
        authority_digest: authority_digest.to_hex(),
        policy_digest: policy_digest.to_hex(),
        identity_digest: identity_digest.to_hex(),
    })
}
