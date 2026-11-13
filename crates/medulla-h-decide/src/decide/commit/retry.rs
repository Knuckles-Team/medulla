//! MEDULLA-DECISION-ENGINE-R113.3.3: the one retry a capacity denial allows.
//!
//! A denied topology commit stages the durable allowance of its record id on its Agent Library
//! reservation, and the coordinator's ABORT applies it (R113.3.1.1, R113.3.2); a
//! follow-up commit that names that denied decision consumes it before it is accepted
//! (R113.3.2). The take is its own admitted Agent Library write, so it carries its
//! own attempt nonce and idempotency key, derived from the request's so a transport retry of the
//! same request replays instead of conflicting.

use medulla_agent_types::agent_library::AgentLibraryMutationContext;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_record::retry_allowance::RetryAllowanceError;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::contract::Nonce;
use sha2::{Digest, Sha256};

/// Hash domain of an allowance write's derived attempt nonce.
const NONCE_DOMAIN: &[u8] = b"medulla/decision-retry-allowance-nonce/v1\0";

/// The write context of one allowance event (`issue` or `take`), bound to the verified request.
pub(in crate::decide) fn allowance_context(
    store: &AgentLibraryStore,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request_context: &AgentLibraryMutationContext,
    action: &str,
) -> Result<AgentLibraryMutationContext, String> {
    let mut bound =
        medulla_persistence_records::agent_library::context::bind_agent_library_context(
            store,
            req_id,
            verified,
            request_context.clone(),
            &format!("decision-retry-allowance:{action}"),
            true,
        )?;
    bound.idempotency_key = format!("{}:retry-{action}", bound.idempotency_key);
    let mut hasher = Sha256::new();
    hasher.update(NONCE_DOMAIN);
    hasher.update(bound.attempt_nonce.as_bytes());
    hasher.update(action.as_bytes());
    bound.attempt_nonce = Nonce::from_bytes(hasher.finalize().into());
    Ok(bound)
}

/// Consume the allowance of `denied_decision_id` for the verified tenant. A second use is
/// `RETRY_ALLOWANCE_EXHAUSTED`; a denied decision the tenant was never granted an allowance for
/// (another tenant's, or one that does not exist) is the same `ACCESS_DENIED`, so the refusal
/// never confirms that a row exists.
pub(in crate::decide) fn consume(
    store: &AgentLibraryStore,
    context: &AgentLibraryMutationContext,
    denied_decision_id: &str,
) -> Result<(), String> {
    store
        .consume_denial_allowance(context, denied_decision_id)
        .map_err(|error| match error {
            RetryAllowanceError::NotIssued => {
                "ACCESS_DENIED: no retry allowance was granted for the named denied decision"
                    .to_string()
            }
            other => other.to_string(),
        })
}
