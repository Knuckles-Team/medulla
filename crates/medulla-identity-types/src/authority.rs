//! Immutable authority and replay evidence consumed by the mutation kernel.
//!
//! `VerifiedAuthority` is serializable evidence, not an executable capability.
//! The request boundary must validate it and mint a private admitted-session
//! token before `medulla-transaction` may execute an effect.

mod context;
mod evidence;
mod replay;

pub use self::context::{
    AuthorityContext, AuthorityScope, AUTHORITY_CONTEXT_SCHEMA_V1, AUTHORITY_PROTOCOL_V1,
};
pub use self::evidence::{SignedAuthorityEnvelopeEvidence, VerifiedAuthority};
pub use self::replay::{NonceReplayKey, OperationReplayIdentity, ReplayReceipt};
pub use medulla_types::{
    contract::AdmissionOutcome, contract::DecisionOutcome, contract::EffectState,
    contract::IngressSurface, contract::Operation, contract::PurposeKind, contract::ReplayStatus,
    contract::ScopeKind, contract::VerificationStatus,
};

#[cfg(test)]
mod tests;
