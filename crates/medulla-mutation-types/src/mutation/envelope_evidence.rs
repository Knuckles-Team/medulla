use super::envelope::MutationRequestEnvelope;
use crate::outbox::OutboxIntent;
use medulla_types::{contract::Digest256, contract::OpaqueId, contract::RequestedMutationResult};

impl MutationRequestEnvelope {
    pub fn requested_result(&self) -> &RequestedMutationResult {
        &self.requested_result
    }

    pub fn canonical_payload_digest(&self) -> Digest256 {
        self.canonical_payload_digest
    }

    pub fn authority_receipt_id(&self) -> &OpaqueId {
        &self.verified_authority.receipt_id
    }

    pub fn authority_evidence_digest(&self) -> Digest256 {
        self.verified_authority.evidence_digest
    }

    pub fn operation_replay_digest(&self) -> Digest256 {
        self.operation_replay_digest
    }

    pub fn nonce_replay_digest(&self) -> Digest256 {
        self.nonce_replay_digest
    }

    pub fn envelope_digest(&self) -> Digest256 {
        self.envelope_digest
    }

    pub fn outbox_intent(&self, index: usize) -> Option<&OutboxIntent> {
        self.outbox.as_slice().get(index)
    }
}
