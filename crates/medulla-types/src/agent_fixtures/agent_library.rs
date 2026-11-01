//! Agent-library write fixtures shared by the contract-wave samples.

use crate::agent_library::AgentLibraryMutationContext;
use crate::contract::Nonce;
use crate::test_support::wave::digest_text;

/// The mutation context every agent-library write carries.
pub fn mutation_context() -> AgentLibraryMutationContext {
    AgentLibraryMutationContext {
        request_id: 1,
        principal: "principal-a".to_string(),
        caller_principal: "caller-a".to_string(),
        attempt_nonce: Nonce::from_bytes([7; 32]),
        tenant_id: "tenant-a".to_string(),
        actor_scope: "scope-a".to_string(),
        purpose_id: "purpose-a".to_string(),
        policy_revision: "policy-1".to_string(),
        policy_digest: digest_text(0x22),
        policy_decision_id: "decision-1".to_string(),
        idempotency_key: "idempotency-1".to_string(),
        expected_revision: Some(1),
        trace_id: Some("trace-1".to_string()),
        created_at_ms: 1_700_000_000_000,
    }
}
