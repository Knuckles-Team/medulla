//! MEDULLA-TYPED-PACKS-R807: the connector conformance evidence record.
//!
//! One record per tenant, connector and immutable pack revision. The connector onboarding
//! import writes it in the same admitted commit as the pack head; a served read returns the
//! current record or refuses `CONFORMANCE_EVIDENCE_ABSENT`; an operator revokes it. Arbora
//! never writes the record (`ARBORA-OPS-R067.10` only reads it).
//!
//! The record's lifecycle is the explicit [`ConformanceEvidenceMachine`]: `Active` and the
//! terminal `Revoked`, one `Revoke` event. [`ConformanceEvidence::apply`] is the only
//! function that changes the state, and it writes `revoked_at` together with the state.
//! Expiry is not a state: it is `now >= expires_at`, computed at read time from the stored
//! deadline, so no clock ever writes the record.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use medulla_types::contract::{Digest256, ResourceId};
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// The persisted state of one evidence record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ConformanceEvidenceState {
    /// The evidence stands until it expires or is revoked.
    Active,
    /// An operator withdrew the evidence. Terminal.
    Revoked,
}

/// The closed event set of `ConformanceEvidenceMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConformanceEvidenceEvent {
    /// An operator withdraws the evidence.
    Revoke,
}

impl MachineState for ConformanceEvidenceState {
    fn stored(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
        }
    }
}

impl MachineEvent for ConformanceEvidenceEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Revoke => "Revoke",
        }
    }
}

/// `ConformanceEvidenceMachine`: the closed edge table. Every other pair is refused.
pub static CONFORMANCE_EVIDENCE_MACHINE: MachineDef<
    ConformanceEvidenceState,
    ConformanceEvidenceEvent,
    (),
> = MachineDef {
    name: "ConformanceEvidenceMachine",
    states: &[
        ConformanceEvidenceState::Active,
        ConformanceEvidenceState::Revoked,
    ],
    initial: ConformanceEvidenceState::Active,
    terminals: &[ConformanceEvidenceState::Revoked],
    events: &[ConformanceEvidenceEvent::Revoke],
    guards: &[],
    edges: &[Edge {
        id: "C01",
        from: ConformanceEvidenceState::Active,
        event: ConformanceEvidenceEvent::Revoke,
        guard: None,
        to: ConformanceEvidenceState::Revoked,
    }],
};

/// What the connector's pack was when it was onboarded, and how long that stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ConformanceEvidence {
    pub tenant_id: String,
    pub connector: ResourceId,
    /// The immutable pack revision (the import's `binding_revision`) this evidence covers.
    pub revision: u64,
    pub pack_digest: Digest256,
    /// One fingerprint per tool the pack serves, keyed by the tool's URI.
    pub tool_fingerprints: BTreeMap<String, Digest256>,
    /// Milliseconds since the epoch: when the import committed.
    pub issued_at: u64,
    /// Milliseconds since the epoch: the registry knob `conformance.evidence_ttl_ms` after
    /// `issued_at`.
    pub expires_at: u64,
    /// The one persisted state; only [`Self::apply`] changes it.
    pub state: ConformanceEvidenceState,
    /// Milliseconds since the epoch: when the revocation committed. Data written by the
    /// `Revoke` transition together with `state`; absent while the evidence is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<u64>,
}

impl ConformanceEvidence {
    /// A new record, in the machine's initial state.
    pub fn issue(
        tenant_id: String,
        connector: ResourceId,
        revision: u64,
        pack_digest: Digest256,
        tool_fingerprints: BTreeMap<String, Digest256>,
        issued_at: u64,
        ttl_ms: u64,
    ) -> Self {
        Self {
            tenant_id,
            connector,
            revision,
            pack_digest,
            tool_fingerprints,
            issued_at,
            expires_at: issued_at.saturating_add(ttl_ms),
            state: CONFORMANCE_EVIDENCE_MACHINE.initial,
            revoked_at: None,
        }
    }

    /// The one transition function of `ConformanceEvidenceMachine`. `at_ms` is the commit
    /// time a `Revoke` records.
    pub fn apply(
        &mut self,
        event: ConformanceEvidenceEvent,
        at_ms: u64,
    ) -> Result<(), TransitionRefused> {
        let edge = transition(&CONFORMANCE_EVIDENCE_MACHINE, self.state, event, |()| true)?;
        self.state = edge.to;
        match event {
            ConformanceEvidenceEvent::Revoke => self.revoked_at = Some(at_ms),
        }
        Ok(())
    }

    /// Whether the evidence stands at `now_ms`: active and not past its deadline.
    pub fn is_current(&self, now_ms: u64) -> bool {
        self.state == ConformanceEvidenceState::Active && now_ms < self.expires_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence() -> ConformanceEvidence {
        ConformanceEvidence::issue(
            "tenant-a".to_string(),
            ResourceId::new("connector-a".to_string()).expect("resource id"),
            1,
            Digest256::from_bytes([7; 32]),
            BTreeMap::new(),
            1_000,
            500,
        )
    }

    // spec: MEDULLA-TYPED-PACKS-R807
    #[test]
    fn every_legal_transition_applies() {
        for edge in CONFORMANCE_EVIDENCE_MACHINE.edges {
            let mut record = evidence();
            assert_eq!(record.state, edge.from);
            record.apply(edge.event, 1_200).expect(edge.id);
            assert_eq!(record.state, edge.to, "edge {}", edge.id);
            assert_eq!(record.revoked_at, Some(1_200));
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R807
    #[test]
    fn a_revoked_record_refuses_every_event_and_keeps_its_data() {
        let mut record = evidence();
        record
            .apply(ConformanceEvidenceEvent::Revoke, 1_200)
            .unwrap();
        let refused = record
            .apply(ConformanceEvidenceEvent::Revoke, 9_999)
            .unwrap_err();
        assert_eq!(refused.machine, "ConformanceEvidenceMachine");
        assert_eq!(refused.from, "revoked");
        assert_eq!(refused.event, "Revoke");
        assert_eq!(record.revoked_at, Some(1_200), "a refusal changes nothing");
    }

    // spec: MEDULLA-TYPED-PACKS-R807
    #[test]
    fn currency_is_active_and_before_the_deadline() {
        let mut record = evidence();
        assert!(record.is_current(1_499));
        assert!(!record.is_current(1_500));
        record
            .apply(ConformanceEvidenceEvent::Revoke, 1_100)
            .unwrap();
        assert!(!record.is_current(1_101));
    }
}
