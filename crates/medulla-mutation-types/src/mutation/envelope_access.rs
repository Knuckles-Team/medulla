use super::effects::MutationEffect;
use super::envelope::MutationRequestEnvelope;
use super::outbox_decode::ProvenanceBinding;
use super::targets::MutationPrecondition;
use super::{MAX_MUTATION_PRECONDITIONS, MAX_PROVENANCE_REFS};
use crate::outbox::OutboxIntent;
use medulla_identity_types::{
    authority::AuthorityScope, authority::NonceReplayKey, authority::OperationReplayIdentity,
    authority::VerifiedAuthority,
};
use medulla_types::{
    contract::BoundedVec, contract::OpaqueId, contract::ResourceId, contract::MAX_MUTATION_EFFECTS,
    contract::MAX_OUTBOX_INTENTS,
};

impl MutationRequestEnvelope {
    pub fn schema_version(&self) -> &ResourceId {
        &self.schema_version
    }

    pub fn mutation_id(&self) -> &OpaqueId {
        &self.mutation_id
    }

    pub fn scope(&self) -> &AuthorityScope {
        &self.scope
    }

    pub fn verified_authority(&self) -> &VerifiedAuthority {
        &self.verified_authority
    }

    pub fn operation_identity(&self) -> &OperationReplayIdentity {
        &self.operation_identity
    }

    pub fn nonce_replay_key(&self) -> &NonceReplayKey {
        &self.nonce_replay_key
    }

    pub fn preconditions(&self) -> &BoundedVec<MutationPrecondition, MAX_MUTATION_PRECONDITIONS> {
        &self.preconditions
    }

    pub fn effects(&self) -> &BoundedVec<MutationEffect, MAX_MUTATION_EFFECTS> {
        &self.effects
    }

    pub fn outbox(&self) -> &BoundedVec<OutboxIntent, MAX_OUTBOX_INTENTS> {
        &self.outbox
    }

    pub fn provenance(&self) -> &BoundedVec<ProvenanceBinding, MAX_PROVENANCE_REFS> {
        &self.provenance
    }
}
