//! Typed, durable contract for Raft membership shrink.
//!
//! Openraft's `change_membership` primitive is deliberately small: it can remove
//! a voter, but it cannot prove that work has drained, a replacement learner is
//! caught up, leadership has moved, or that the post-change topology still
//! satisfies the operator's quorum/failure-domain/headroom/PDB policy. This
//! module owns that missing admission contract. It is a pure state machine; the
//! [`super::multi::MultiRaft`] actuator persists each journal transition through
//! the placement graph and invokes openraft only after `SafetyChecked`.

use std::collections::BTreeSet;

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefusalReason,
    TransitionRefused,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{GroupId, NodeId};

pub const MEMBERSHIP_SHRINK_SCHEMA_VERSION: u16 = 1;
const MAX_VOTERS: usize = 64;
const MAX_EVIDENCE_REF: usize = 256;

/// Explicit, restart-visible shrink phases. A phase is never inferred from a
/// live membership read: the journal is the recovery authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipShrinkPhase {
    Proposed,
    DrainRequested,
    Drained,
    LearnerCaughtUp,
    LeadershipTransferred,
    SafetyChecked,
    RemovalCommitted,
    Completed,
    Aborted,
}

// MEDULLA-PLACEMENT-R783.2 — the shrink edge table; `advance` and `abort` fire its events.
// MembershipShrinkMachine: the row's edge table. The ontology test compares it with state-machines-v1.ttl.
/// An event of `MembershipShrinkMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MembershipShrinkEvent {
    /// The `RequestDrain` event.
    RequestDrain,
    /// The `ConfirmDrained` event.
    ConfirmDrained,
    /// The `ConfirmLearnerCaughtUp` event.
    ConfirmLearnerCaughtUp,
    /// The `ConfirmLeadershipTransferred` event.
    ConfirmLeadershipTransferred,
    /// The `ConfirmSafety` event.
    ConfirmSafety,
    /// The `CommitRemoval` event.
    CommitRemoval,
    /// The `Complete` event.
    Complete,
    /// The `Abort` event.
    Abort,
}

impl MachineState for MembershipShrinkPhase {
    fn stored(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::DrainRequested => "drain_requested",
            Self::Drained => "drained",
            Self::LearnerCaughtUp => "learner_caught_up",
            Self::LeadershipTransferred => "leadership_transferred",
            Self::SafetyChecked => "safety_checked",
            Self::RemovalCommitted => "removal_committed",
            Self::Completed => "completed",
            Self::Aborted => "aborted",
        }
    }
}

impl MachineEvent for MembershipShrinkEvent {
    fn name(self) -> &'static str {
        match self {
            Self::RequestDrain => "RequestDrain",
            Self::ConfirmDrained => "ConfirmDrained",
            Self::ConfirmLearnerCaughtUp => "ConfirmLearnerCaughtUp",
            Self::ConfirmLeadershipTransferred => "ConfirmLeadershipTransferred",
            Self::ConfirmSafety => "ConfirmSafety",
            Self::CommitRemoval => "CommitRemoval",
            Self::Complete => "Complete",
            Self::Abort => "Abort",
        }
    }
}

/// Guard of `MembershipShrinkMachine`: the evidence shows the node drained.
pub const MEMBERSHIP_SHRINK_GUARD_NODE_DRAINED: &str = "NodeDrained";
/// Guard of `MembershipShrinkMachine`: the evidence shows the learner caught up.
pub const MEMBERSHIP_SHRINK_GUARD_LEARNER_CAUGHT_UP: &str = "LearnerCaughtUp";
/// Guard of `MembershipShrinkMachine`: the evidence shows leadership left the target.
pub const MEMBERSHIP_SHRINK_GUARD_LEADERSHIP_LEFT_TARGET: &str = "LeadershipLeftTarget";
/// Guard of `MembershipShrinkMachine`: quorum, failure domain, headroom and disruption budget are preserved.
pub const MEMBERSHIP_SHRINK_GUARD_SAFETY_PRESERVED: &str = "SafetyPreserved";
/// Guard of `MembershipShrinkMachine`: the evidence shows the removal committed with the remaining voters.
pub const MEMBERSHIP_SHRINK_GUARD_REMOVAL_COMMITTED: &str = "RemovalCommitted";
/// Guard of `MembershipShrinkMachine`: the evidence shows the target absent.
pub const MEMBERSHIP_SHRINK_GUARD_TARGET_ABSENT: &str = "TargetAbsent";
/// Guard of `MembershipShrinkMachine`: the reason is non-empty, bounded and has no control character.
pub const MEMBERSHIP_SHRINK_GUARD_REASON_BOUNDED: &str = "ReasonBounded";

/// `MembershipShrinkMachine`: the closed edge table. Every other pair of state and event is refused.
pub static MEMBERSHIP_SHRINK_MACHINE: MachineDef<
    MembershipShrinkPhase,
    MembershipShrinkEvent,
    &'static str,
> = MachineDef {
    name: "MembershipShrinkMachine",
    states: &[
        MembershipShrinkPhase::Proposed,
        MembershipShrinkPhase::DrainRequested,
        MembershipShrinkPhase::Drained,
        MembershipShrinkPhase::LearnerCaughtUp,
        MembershipShrinkPhase::LeadershipTransferred,
        MembershipShrinkPhase::SafetyChecked,
        MembershipShrinkPhase::RemovalCommitted,
        MembershipShrinkPhase::Completed,
        MembershipShrinkPhase::Aborted,
    ],
    initial: MembershipShrinkPhase::Proposed,
    terminals: &[
        MembershipShrinkPhase::Completed,
        MembershipShrinkPhase::Aborted,
    ],
    events: &[
        MembershipShrinkEvent::RequestDrain,
        MembershipShrinkEvent::ConfirmDrained,
        MembershipShrinkEvent::ConfirmLearnerCaughtUp,
        MembershipShrinkEvent::ConfirmLeadershipTransferred,
        MembershipShrinkEvent::ConfirmSafety,
        MembershipShrinkEvent::CommitRemoval,
        MembershipShrinkEvent::Complete,
        MembershipShrinkEvent::Abort,
    ],
    guards: &[
        MEMBERSHIP_SHRINK_GUARD_NODE_DRAINED,
        MEMBERSHIP_SHRINK_GUARD_LEARNER_CAUGHT_UP,
        MEMBERSHIP_SHRINK_GUARD_LEADERSHIP_LEFT_TARGET,
        MEMBERSHIP_SHRINK_GUARD_SAFETY_PRESERVED,
        MEMBERSHIP_SHRINK_GUARD_REMOVAL_COMMITTED,
        MEMBERSHIP_SHRINK_GUARD_TARGET_ABSENT,
        MEMBERSHIP_SHRINK_GUARD_REASON_BOUNDED,
    ],
    edges: &[
        Edge {
            id: "S01",
            from: MembershipShrinkPhase::Proposed,
            event: MembershipShrinkEvent::RequestDrain,
            guard: None,
            to: MembershipShrinkPhase::DrainRequested,
        },
        Edge {
            id: "S02",
            from: MembershipShrinkPhase::DrainRequested,
            event: MembershipShrinkEvent::ConfirmDrained,
            guard: Some(MEMBERSHIP_SHRINK_GUARD_NODE_DRAINED),
            to: MembershipShrinkPhase::Drained,
        },
        Edge {
            id: "S03",
            from: MembershipShrinkPhase::Drained,
            event: MembershipShrinkEvent::ConfirmLearnerCaughtUp,
            guard: Some(MEMBERSHIP_SHRINK_GUARD_LEARNER_CAUGHT_UP),
            to: MembershipShrinkPhase::LearnerCaughtUp,
        },
        Edge {
            id: "S04",
            from: MembershipShrinkPhase::LearnerCaughtUp,
            event: MembershipShrinkEvent::ConfirmLeadershipTransferred,
            guard: Some(MEMBERSHIP_SHRINK_GUARD_LEADERSHIP_LEFT_TARGET),
            to: MembershipShrinkPhase::LeadershipTransferred,
        },
        Edge {
            id: "S05",
            from: MembershipShrinkPhase::LeadershipTransferred,
            event: MembershipShrinkEvent::ConfirmSafety,
            guard: Some(MEMBERSHIP_SHRINK_GUARD_SAFETY_PRESERVED),
            to: MembershipShrinkPhase::SafetyChecked,
        },
        Edge {
            id: "S06",
            from: MembershipShrinkPhase::SafetyChecked,
            event: MembershipShrinkEvent::CommitRemoval,
            guard: Some(MEMBERSHIP_SHRINK_GUARD_REMOVAL_COMMITTED),
            to: MembershipShrinkPhase::RemovalCommitted,
        },
        Edge {
            id: "S07",
            from: MembershipShrinkPhase::RemovalCommitted,
            event: MembershipShrinkEvent::Complete,
            guard: Some(MEMBERSHIP_SHRINK_GUARD_TARGET_ABSENT),
            to: MembershipShrinkPhase::Completed,
        },
        Edge {
            id: "S08",
            from: MembershipShrinkPhase::Proposed,
            event: MembershipShrinkEvent::Abort,
            guard: Some(MEMBERSHIP_SHRINK_GUARD_REASON_BOUNDED),
            to: MembershipShrinkPhase::Aborted,
        },
        Edge {
            id: "S09",
            from: MembershipShrinkPhase::DrainRequested,
            event: MembershipShrinkEvent::Abort,
            guard: Some(MEMBERSHIP_SHRINK_GUARD_REASON_BOUNDED),
            to: MembershipShrinkPhase::Aborted,
        },
        Edge {
            id: "S10",
            from: MembershipShrinkPhase::Drained,
            event: MembershipShrinkEvent::Abort,
            guard: Some(MEMBERSHIP_SHRINK_GUARD_REASON_BOUNDED),
            to: MembershipShrinkPhase::Aborted,
        },
        Edge {
            id: "S11",
            from: MembershipShrinkPhase::LearnerCaughtUp,
            event: MembershipShrinkEvent::Abort,
            guard: Some(MEMBERSHIP_SHRINK_GUARD_REASON_BOUNDED),
            to: MembershipShrinkPhase::Aborted,
        },
        Edge {
            id: "S12",
            from: MembershipShrinkPhase::LeadershipTransferred,
            event: MembershipShrinkEvent::Abort,
            guard: Some(MEMBERSHIP_SHRINK_GUARD_REASON_BOUNDED),
            to: MembershipShrinkPhase::Aborted,
        },
        Edge {
            id: "S13",
            from: MembershipShrinkPhase::SafetyChecked,
            event: MembershipShrinkEvent::Abort,
            guard: Some(MEMBERSHIP_SHRINK_GUARD_REASON_BOUNDED),
            to: MembershipShrinkPhase::Aborted,
        },
        Edge {
            id: "S14",
            from: MembershipShrinkPhase::RemovalCommitted,
            event: MembershipShrinkEvent::Abort,
            guard: Some(MEMBERSHIP_SHRINK_GUARD_REASON_BOUNDED),
            to: MembershipShrinkPhase::Aborted,
        },
    ],
};

impl MembershipShrinkPhase {
    /// The one transition function of `MembershipShrinkMachine`. `guard_holds` answers each guard
    /// by name; an unguarded edge always holds.
    pub fn apply(
        self,
        event: MembershipShrinkEvent,
        guard_holds: impl Fn(&'static str) -> bool,
    ) -> Result<Self, TransitionRefused> {
        transition(&MEMBERSHIP_SHRINK_MACHINE, self, event, guard_holds).map(|edge| edge.to)
    }
}

impl MembershipShrinkPhase {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Aborted)
    }
}

/// Bounded observations supplied by the lifecycle/controller adapter. The
/// adapter must bind these observations to the exact term and voter set in the
/// journal; a bare `true` from a previous membership incarnation is not enough.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipShrinkEvidence {
    pub observed_term: u64,
    pub observed_voters: Vec<NodeId>,
    pub observed_learner: Option<NodeId>,
    pub observed_leader: Option<NodeId>,
    pub learner_caught_up: bool,
    pub drained: bool,
    pub leadership_transferred: bool,
    pub quorum_preserved: bool,
    pub failure_domain_preserved: bool,
    pub headroom_preserved: bool,
    pub pdb_preserved: bool,
    pub membership_change_committed: bool,
    pub target_absent: bool,
    /// Opaque bounded evidence reference; raw metrics and workload payloads do
    /// not belong in the membership journal.
    pub evidence_ref: String,
}

impl MembershipShrinkEvidence {
    pub fn validate(&self) -> Result<(), String> {
        validate_voters(&self.observed_voters)?;
        if self.evidence_ref.is_empty() || self.evidence_ref.len() > MAX_EVIDENCE_REF {
            return Err("membership shrink evidence reference is invalid".to_string());
        }
        if self
            .evidence_ref
            .bytes()
            .any(|byte| byte.is_ascii_control())
        {
            return Err("membership shrink evidence reference contains control bytes".to_string());
        }
        Ok(())
    }
}

/// Durable shrink intent/progress. The immutable expected and remaining voter
/// sets make restart/replay deterministic and prevent a later membership view
/// from silently changing the operation's target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipShrinkJournal {
    pub schema_version: u16,
    pub operation_id: String,
    pub group_id: GroupId,
    pub target: NodeId,
    pub learner: NodeId,
    pub expected_term: u64,
    pub expected_voters: Vec<NodeId>,
    pub remaining_voters: Vec<NodeId>,
    pub phase: MembershipShrinkPhase,
    pub evidence: Option<MembershipShrinkEvidence>,
    pub abort_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShrinkRecoveryAction {
    Resume,
    Complete,
    Abort,
}

fn validate_voters(voters: &[NodeId]) -> Result<(), String> {
    if voters.is_empty()
        || voters.len() > MAX_VOTERS
        || voters.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err("membership voter set must be sorted, unique, and bounded".to_string());
    }
    Ok(())
}

fn operation_id(
    group_id: GroupId,
    target: NodeId,
    learner: NodeId,
    expected_term: u64,
    expected_voters: &[NodeId],
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"epistemic-graph/membership-shrink/v1\0");
    digest.update(group_id.to_be_bytes());
    digest.update(target.to_be_bytes());
    digest.update(learner.to_be_bytes());
    digest.update(expected_term.to_be_bytes());
    for voter in expected_voters {
        digest.update(voter.to_be_bytes());
    }
    hex::encode(digest.finalize())
}

impl MembershipShrinkJournal {
    pub fn new(
        group_id: GroupId,
        target: NodeId,
        learner: NodeId,
        expected_term: u64,
        mut expected_voters: Vec<NodeId>,
    ) -> Result<Self, String> {
        expected_voters.sort_unstable();
        expected_voters.dedup();
        validate_voters(&expected_voters)?;
        if !expected_voters.contains(&target) {
            return Err("membership shrink target is not a voter".to_string());
        }
        if learner == target {
            return Err("membership shrink learner must differ from target".to_string());
        }
        let remaining_voters: Vec<NodeId> = expected_voters
            .iter()
            .copied()
            .filter(|voter| *voter != target)
            .collect();
        validate_voters(&remaining_voters)?;
        Ok(Self {
            schema_version: MEMBERSHIP_SHRINK_SCHEMA_VERSION,
            operation_id: operation_id(group_id, target, learner, expected_term, &expected_voters),
            group_id,
            target,
            learner,
            expected_term,
            expected_voters,
            remaining_voters,
            phase: MembershipShrinkPhase::Proposed,
            evidence: None,
            abort_reason: None,
        })
    }

    pub fn node_id(&self) -> String {
        format!("membership-shrink:{}", self.operation_id)
    }

    pub fn validate(&self) -> bool {
        journal_identity_is_consistent(self)
            && journal_voter_sets_are_valid(self)
            && journal_abort_state_is_valid(self)
            && journal_evidence_is_valid(self)
    }

    fn evidence_matches(
        &self,
        next: MembershipShrinkPhase,
        evidence: &MembershipShrinkEvidence,
    ) -> bool {
        let expected_voters = match next {
            MembershipShrinkPhase::RemovalCommitted | MembershipShrinkPhase::Completed
                if evidence.membership_change_committed =>
            {
                &self.remaining_voters
            }
            _ => &self.expected_voters,
        };
        evidence.validate().is_ok()
            && evidence.observed_term == self.expected_term
            && evidence.observed_learner == Some(self.learner)
            && evidence.observed_voters.as_slice() == expected_voters.as_slice()
    }

    /// Validate and persist the next phase. There are no skips: each safety
    /// gate is independently evidenced and a stale term/voter set is rejected.
    pub fn advance(
        &self,
        next: MembershipShrinkPhase,
        evidence: MembershipShrinkEvidence,
    ) -> Result<Self, TransitionRefused> {
        use MembershipShrinkPhase as P;
        let event = match next {
            P::DrainRequested => MembershipShrinkEvent::RequestDrain,
            P::Drained => MembershipShrinkEvent::ConfirmDrained,
            P::LearnerCaughtUp => MembershipShrinkEvent::ConfirmLearnerCaughtUp,
            P::LeadershipTransferred => MembershipShrinkEvent::ConfirmLeadershipTransferred,
            P::SafetyChecked => MembershipShrinkEvent::ConfirmSafety,
            P::RemovalCommitted => MembershipShrinkEvent::CommitRemoval,
            P::Completed => MembershipShrinkEvent::Complete,
            P::Proposed | P::Aborted => {
                let event = MembershipShrinkEvent::Abort;
                return Err(shrink_refusal(
                    self.phase,
                    event,
                    TransitionRefusalReason::IllegalTransition,
                ));
            }
        };
        // A stale or foreign evidence set (or an invalid journal) fails the edge guard.
        if !self.phase.terminal() && (!self.validate() || !self.evidence_matches(next, &evidence)) {
            return Err(shrink_refusal(
                self.phase,
                event,
                TransitionRefusalReason::GuardFailed,
            ));
        }
        let to = self.phase.apply(event, |_| {
            transition_is_valid(
                self.phase,
                next,
                &evidence,
                self.target,
                &self.remaining_voters,
            )
        })?;
        if to != next {
            return Err(shrink_refusal(
                self.phase,
                event,
                TransitionRefusalReason::IllegalTransition,
            ));
        }
        let mut updated = self.clone();
        updated.phase = to;
        updated.evidence = Some(evidence);
        Ok(updated)
    }

    /// Abort is itself durable recovery evidence and never deletes the journal.
    pub fn abort(&self, reason: &str) -> Result<Self, TransitionRefused> {
        let bounded = !reason.is_empty()
            && reason.len() <= MAX_EVIDENCE_REF
            && !reason.bytes().any(|byte| byte.is_ascii_control());
        let valid = self.validate();
        let to = self
            .phase
            .apply(MembershipShrinkEvent::Abort, |_| valid && bounded)?;
        let mut updated = self.clone();
        updated.phase = to;
        updated.abort_reason = Some(reason.to_string());
        Ok(updated)
    }

    /// Decide what a restarted controller may do from the retained journal and
    /// the currently observed committed voter set. Ambiguous state aborts.
    pub fn recovery_action(&self, observed_voters: &[NodeId]) -> ShrinkRecoveryAction {
        if !self.validate() || self.phase == MembershipShrinkPhase::Aborted {
            return ShrinkRecoveryAction::Abort;
        }
        let mut observed = observed_voters.to_vec();
        observed.sort_unstable();
        observed.dedup();
        match self.phase {
            MembershipShrinkPhase::SafetyChecked
            | MembershipShrinkPhase::RemovalCommitted
            | MembershipShrinkPhase::Completed
                if observed == self.remaining_voters =>
            {
                ShrinkRecoveryAction::Complete
            }
            phase if !phase.terminal() && observed == self.expected_voters => {
                ShrinkRecoveryAction::Resume
            }
            _ => ShrinkRecoveryAction::Abort,
        }
    }

    /// The removal actuator may run only after every drain/safety gate is
    /// represented by one durable `SafetyChecked` journal state.
    pub fn ready_for_removal(&self) -> bool {
        self.validate() && self.phase == MembershipShrinkPhase::SafetyChecked
    }

    pub fn permits_successor(&self, next: &Self) -> bool {
        if !journal_identities_match(self, next) {
            return false;
        }
        if self.phase == next.phase {
            return self.evidence == next.evidence;
        }
        if self.phase.terminal() {
            return false;
        }
        is_valid_phase_step(self.phase, next.phase)
    }

    pub fn expected_voter_set(&self) -> BTreeSet<NodeId> {
        self.expected_voters.iter().copied().collect()
    }

    pub fn remaining_voter_set(&self) -> BTreeSet<NodeId> {
        self.remaining_voters.iter().copied().collect()
    }
}

fn journal_identity_is_consistent(journal: &MembershipShrinkJournal) -> bool {
    journal.schema_version == MEMBERSHIP_SHRINK_SCHEMA_VERSION
        && journal.operation_id
            == operation_id(
                journal.group_id,
                journal.target,
                journal.learner,
                journal.expected_term,
                &journal.expected_voters,
            )
        && journal.expected_voters.contains(&journal.target)
        && journal.learner != journal.target
}

fn journal_voter_sets_are_valid(journal: &MembershipShrinkJournal) -> bool {
    let expected_remaining: Vec<NodeId> = journal
        .expected_voters
        .iter()
        .copied()
        .filter(|voter| *voter != journal.target)
        .collect();
    validate_voters(&journal.expected_voters).is_ok()
        && validate_voters(&journal.remaining_voters).is_ok()
        && journal.remaining_voters == expected_remaining
        && journal
            .remaining_voters
            .iter()
            .all(|voter| *voter != journal.target && journal.expected_voters.contains(voter))
}

fn journal_abort_state_is_valid(journal: &MembershipShrinkJournal) -> bool {
    let abort_reason_valid = journal.abort_reason.as_ref().is_none_or(|reason| {
        !reason.is_empty()
            && reason.len() <= MAX_EVIDENCE_REF
            && !reason.bytes().any(|byte| byte.is_ascii_control())
    });
    abort_reason_valid
        && (journal.phase == MembershipShrinkPhase::Aborted || journal.abort_reason.is_none())
}

fn journal_evidence_is_valid(journal: &MembershipShrinkJournal) -> bool {
    journal
        .evidence
        .as_ref()
        .is_none_or(|evidence| evidence.validate().is_ok())
}

/// Whether `evidence` proves the single-step transition `phase -> next` is
/// safe. Each phase pair names its own gate; anything else is not a valid
/// single step -- `advance` never skips a gate. The match over the phase
/// pair deliberately ends in a catch-all: most of the 9x9 phase-pair space is
/// not a defined transition at all, so there is no exhaustiveness guarantee
/// to preserve here.
/// The typed refusal of a shrink event that the guard or the journal refuses before the
/// edge table is consulted.
fn shrink_refusal(
    from: MembershipShrinkPhase,
    event: MembershipShrinkEvent,
    reason: TransitionRefusalReason,
) -> TransitionRefused {
    TransitionRefused {
        machine: MEMBERSHIP_SHRINK_MACHINE.name.to_string(),
        from: from.stored().to_string(),
        event: event.name().to_string(),
        reason,
    }
}

fn transition_is_valid(
    phase: MembershipShrinkPhase,
    next: MembershipShrinkPhase,
    evidence: &MembershipShrinkEvidence,
    target: NodeId,
    remaining_voters: &[NodeId],
) -> bool {
    use MembershipShrinkPhase::*;
    match (phase, next) {
        (Proposed, DrainRequested) => true,
        (DrainRequested, Drained) => gate_drained(evidence),
        (Drained, LearnerCaughtUp) => gate_learner_caught_up(evidence),
        (LearnerCaughtUp, LeadershipTransferred) => gate_leadership_transferred(evidence, target),
        (LeadershipTransferred, SafetyChecked) => gate_safety_checked(evidence),
        (SafetyChecked, RemovalCommitted) | (RemovalCommitted, Completed) => {
            gate_removal_committed(evidence, remaining_voters)
        }
        _ => false,
    }
}

fn gate_drained(evidence: &MembershipShrinkEvidence) -> bool {
    evidence.drained
}

fn gate_learner_caught_up(evidence: &MembershipShrinkEvidence) -> bool {
    evidence.drained && evidence.learner_caught_up
}

fn gate_leadership_transferred(evidence: &MembershipShrinkEvidence, target: NodeId) -> bool {
    evidence.learner_caught_up
        && evidence.leadership_transferred
        && matches!(evidence.observed_leader, Some(leader) if leader != target)
}

fn gate_safety_checked(evidence: &MembershipShrinkEvidence) -> bool {
    evidence.leadership_transferred
        && evidence.quorum_preserved
        && evidence.failure_domain_preserved
        && evidence.headroom_preserved
        && evidence.pdb_preserved
}

fn gate_removal_committed(
    evidence: &MembershipShrinkEvidence,
    remaining_voters: &[NodeId],
) -> bool {
    evidence.membership_change_committed
        && evidence.target_absent
        && evidence.observed_voters.as_slice() == remaining_voters
}

fn journal_identities_match(
    current: &MembershipShrinkJournal,
    next: &MembershipShrinkJournal,
) -> bool {
    current.validate()
        && next.validate()
        && current.operation_id == next.operation_id
        && current.group_id == next.group_id
        && current.target == next.target
        && current.learner == next.learner
        && current.expected_term == next.expected_term
        && current.expected_voters == next.expected_voters
        && current.remaining_voters == next.remaining_voters
}

/// Whether `next` is the single defined successor phase of `phase` (or an
/// abort from anywhere non-terminal). Mirrors the transition table
/// [`transition_is_valid`] gates -- this checks the shape only, not the
/// evidence.
fn is_valid_phase_step(phase: MembershipShrinkPhase, next: MembershipShrinkPhase) -> bool {
    use MembershipShrinkPhase::*;
    matches!(
        (phase, next),
        (Proposed, DrainRequested)
            | (DrainRequested, Drained)
            | (Drained, LearnerCaughtUp)
            | (LearnerCaughtUp, LeadershipTransferred)
            | (LeadershipTransferred, SafetyChecked)
            | (SafetyChecked, RemovalCommitted)
            | (RemovalCommitted, Completed)
            | (_, Aborted)
    )
}

#[cfg(test)]
mod tests {
    // MEDULLA-PLACEMENT-R783.2 — the membership shrink machine.
    use super::{MembershipShrinkEvent, MembershipShrinkPhase, MEMBERSHIP_SHRINK_MACHINE};
    use medulla_types::transition_refused::{MachineState, TransitionRefusalReason};

    fn shrink_edge_case(
        id: &str,
        from: MembershipShrinkPhase,
        event: MembershipShrinkEvent,
        to: MembershipShrinkPhase,
    ) {
        let edge = MEMBERSHIP_SHRINK_MACHINE
            .edges
            .iter()
            .find(|e| e.id == id)
            .unwrap();
        assert_eq!((edge.from, edge.event, edge.to), (from, event, to));
        assert_eq!(from.apply(event, |_| true), Ok(to));
        if edge.guard.is_some() {
            let refused = from.apply(event, |_| false).unwrap_err();
            assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
        }
    }

    /// Every (state, event) pair with no edge.
    fn shrink_illegal_pairs() -> Vec<(MembershipShrinkPhase, MembershipShrinkEvent)> {
        let machine = &MEMBERSHIP_SHRINK_MACHINE;
        let mut out = Vec::new();
        for &state in machine.states {
            for &event in machine.events {
                if !machine
                    .edges
                    .iter()
                    .any(|e| e.from == state && e.event == event)
                {
                    out.push((state, event));
                }
            }
        }
        out
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn edge_s01_proposed_request_drain() {
        shrink_edge_case(
            "S01",
            MembershipShrinkPhase::Proposed,
            MembershipShrinkEvent::RequestDrain,
            MembershipShrinkPhase::DrainRequested,
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn edge_s02_drain_requested_confirm_drained() {
        shrink_edge_case(
            "S02",
            MembershipShrinkPhase::DrainRequested,
            MembershipShrinkEvent::ConfirmDrained,
            MembershipShrinkPhase::Drained,
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn edge_s03_drained_confirm_learner_caught_up() {
        shrink_edge_case(
            "S03",
            MembershipShrinkPhase::Drained,
            MembershipShrinkEvent::ConfirmLearnerCaughtUp,
            MembershipShrinkPhase::LearnerCaughtUp,
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn edge_s04_learner_caught_up_confirm_leadership_transferred() {
        shrink_edge_case(
            "S04",
            MembershipShrinkPhase::LearnerCaughtUp,
            MembershipShrinkEvent::ConfirmLeadershipTransferred,
            MembershipShrinkPhase::LeadershipTransferred,
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn edge_s05_leadership_transferred_confirm_safety() {
        shrink_edge_case(
            "S05",
            MembershipShrinkPhase::LeadershipTransferred,
            MembershipShrinkEvent::ConfirmSafety,
            MembershipShrinkPhase::SafetyChecked,
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn edge_s06_safety_checked_commit_removal() {
        shrink_edge_case(
            "S06",
            MembershipShrinkPhase::SafetyChecked,
            MembershipShrinkEvent::CommitRemoval,
            MembershipShrinkPhase::RemovalCommitted,
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn edge_s07_removal_committed_complete() {
        shrink_edge_case(
            "S07",
            MembershipShrinkPhase::RemovalCommitted,
            MembershipShrinkEvent::Complete,
            MembershipShrinkPhase::Completed,
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn edge_s08_proposed_abort() {
        shrink_edge_case(
            "S08",
            MembershipShrinkPhase::Proposed,
            MembershipShrinkEvent::Abort,
            MembershipShrinkPhase::Aborted,
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn edge_s09_drain_requested_abort() {
        shrink_edge_case(
            "S09",
            MembershipShrinkPhase::DrainRequested,
            MembershipShrinkEvent::Abort,
            MembershipShrinkPhase::Aborted,
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn edge_s10_drained_abort() {
        shrink_edge_case(
            "S10",
            MembershipShrinkPhase::Drained,
            MembershipShrinkEvent::Abort,
            MembershipShrinkPhase::Aborted,
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn edge_s11_learner_caught_up_abort() {
        shrink_edge_case(
            "S11",
            MembershipShrinkPhase::LearnerCaughtUp,
            MembershipShrinkEvent::Abort,
            MembershipShrinkPhase::Aborted,
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn edge_s12_leadership_transferred_abort() {
        shrink_edge_case(
            "S12",
            MembershipShrinkPhase::LeadershipTransferred,
            MembershipShrinkEvent::Abort,
            MembershipShrinkPhase::Aborted,
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn edge_s13_safety_checked_abort() {
        shrink_edge_case(
            "S13",
            MembershipShrinkPhase::SafetyChecked,
            MembershipShrinkEvent::Abort,
            MembershipShrinkPhase::Aborted,
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn edge_s14_removal_committed_abort() {
        shrink_edge_case(
            "S14",
            MembershipShrinkPhase::RemovalCommitted,
            MembershipShrinkEvent::Abort,
            MembershipShrinkPhase::Aborted,
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn every_illegal_pair_is_refused() {
        let machine = &MEMBERSHIP_SHRINK_MACHINE;
        let pairs = shrink_illegal_pairs();
        assert!(!pairs.is_empty());
        for (state, event) in pairs {
            let refused = state.apply(event, |_| true).unwrap_err();
            let expected = if machine.is_terminal(state) {
                TransitionRefusalReason::TerminalState
            } else {
                TransitionRefusalReason::IllegalTransition
            };
            assert_eq!(refused.reason, expected, "{state:?} {event:?}");
        }
        for &terminal in machine.terminals {
            for &event in machine.events {
                assert!(terminal.apply(event, |_| true).is_err());
            }
        }
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn refusal_carries_machine_from_event() {
        let refused = MembershipShrinkPhase::Proposed
            .apply(MembershipShrinkEvent::ConfirmDrained, |_| true)
            .unwrap_err();
        assert_eq!(refused.machine, "MembershipShrinkMachine");
        assert_eq!(refused.from, MembershipShrinkPhase::Proposed.stored());
        assert_eq!(refused.event, "ConfirmDrained");
        assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
    }

    /// Lines that assign the state field directly (`.<field> = <Type>::`), outside the
    /// file's `#[cfg(test)] mod tests`.
    fn direct_state_writes<'a>(source: &'a str, field: &str, ty: &str) -> Vec<&'a str> {
        let needle = format!(".{field} = {ty}::");
        let mut out = Vec::new();
        let mut previous = "";
        for line in source.lines().map(str::trim) {
            if previous == "#[cfg(test)]" && line.starts_with("mod tests") {
                break;
            }
            previous = line;
            if !line.starts_with("//") && line.contains(&needle) {
                out.push(line);
            }
        }
        out
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn no_writer_sets_state_outside_apply() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/raft/membership_shrink.rs");
        let source = std::fs::read_to_string(path).expect("writer source");
        let writes = direct_state_writes(&source, "phase", "MembershipShrinkPhase");
        assert!(
            writes.is_empty(),
            "direct state writes in src/raft/membership_shrink.rs: {writes:?}"
        );
    }

    // spec: MEDULLA-PLACEMENT-R783.2

    #[test]
    fn every_transition_and_refusal_is_covered() {
        let machine = &MEMBERSHIP_SHRINK_MACHINE;
        assert_eq!(machine.edges.len(), 14);
        assert_eq!(machine.states.len(), 9);
        let mut ids: Vec<_> = machine.edges.iter().map(|e| e.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), machine.edges.len(), "edge ids are unique");
        for edge in machine.edges {
            assert!(
                !machine.is_terminal(edge.from),
                "{} leaves a terminal state",
                edge.id
            );
        }
        for &state in machine.states {
            assert_eq!(machine.state_from_stored(state.stored()), Some(state));
            for &event in machine.events {
                let legal = state.apply(event, |_| true).is_ok();
                let has_edge = machine.edges_from(state).any(|e| e.event == event);
                assert_eq!(legal, has_edge, "{state:?} {event:?}");
            }
        }
    }

    use super::*;

    fn evidence(voters: Vec<NodeId>) -> MembershipShrinkEvidence {
        MembershipShrinkEvidence {
            observed_term: 7,
            observed_voters: voters,
            observed_learner: Some(4),
            observed_leader: Some(1),
            learner_caught_up: false,
            drained: false,
            leadership_transferred: false,
            quorum_preserved: false,
            failure_domain_preserved: false,
            headroom_preserved: false,
            pdb_preserved: false,
            membership_change_committed: false,
            target_absent: false,
            evidence_ref: "evidence:shrink-test".into(),
        }
    }

    #[test]
    fn shrink_requires_each_gate_in_order_and_retains_recovery_state() {
        let journal = MembershipShrinkJournal::new(0, 3, 4, 7, vec![1, 2, 3]).unwrap();
        let mut current = journal
            .advance(
                MembershipShrinkPhase::DrainRequested,
                evidence(vec![1, 2, 3]),
            )
            .unwrap();
        let mut next = evidence(vec![1, 2, 3]);
        next.drained = true;
        current = current
            .advance(MembershipShrinkPhase::Drained, next)
            .unwrap();
        let mut next = evidence(vec![1, 2, 3]);
        next.drained = true;
        next.learner_caught_up = true;
        current = current
            .advance(MembershipShrinkPhase::LearnerCaughtUp, next)
            .unwrap();
        let mut next = evidence(vec![1, 2, 3]);
        next.drained = true;
        next.learner_caught_up = true;
        next.leadership_transferred = true;
        current = current
            .advance(MembershipShrinkPhase::LeadershipTransferred, next)
            .unwrap();
        let mut next = evidence(vec![1, 2, 3]);
        next.drained = true;
        next.learner_caught_up = true;
        next.leadership_transferred = true;
        next.quorum_preserved = true;
        next.failure_domain_preserved = true;
        next.headroom_preserved = true;
        next.pdb_preserved = true;
        current = current
            .advance(MembershipShrinkPhase::SafetyChecked, next)
            .unwrap();
        assert!(current.ready_for_removal());
        assert_eq!(current.node_id(), journal.node_id());
        assert_eq!(
            current.recovery_action(&[1, 2, 3]),
            ShrinkRecoveryAction::Resume
        );
    }

    #[test]
    fn stale_term_and_ambiguous_restart_abort() {
        let journal = MembershipShrinkJournal::new(0, 3, 4, 7, vec![1, 2, 3]).unwrap();
        let mut stale = evidence(vec![1, 2, 3]);
        stale.observed_term = 8;
        assert!(journal
            .advance(MembershipShrinkPhase::DrainRequested, stale)
            .is_err());
        assert_eq!(
            journal.recovery_action(&[1, 3]),
            ShrinkRecoveryAction::Abort
        );
    }

    #[test]
    fn advance_rejects_a_skipped_phase() {
        let journal = MembershipShrinkJournal::new(0, 3, 4, 7, vec![1, 2, 3]).unwrap();
        // Proposed -> LearnerCaughtUp skips DrainRequested/Drained: the transition
        // table's catch-all must reject it even though the evidence looks ready.
        let mut skip_evidence = evidence(vec![1, 2, 3]);
        skip_evidence.drained = true;
        skip_evidence.learner_caught_up = true;
        assert!(journal
            .advance(MembershipShrinkPhase::LearnerCaughtUp, skip_evidence)
            .is_err());
    }

    #[test]
    fn validate_rejects_inconsistent_identity_and_voter_sets() {
        let journal = MembershipShrinkJournal::new(0, 3, 4, 7, vec![1, 2, 3]).unwrap();
        assert!(journal.validate());

        let mut bad_schema = journal.clone();
        bad_schema.schema_version += 1;
        assert!(!bad_schema.validate());

        let mut bad_remaining = journal.clone();
        bad_remaining.remaining_voters = vec![1, 2, 3];
        assert!(!bad_remaining.validate());

        // Re-derive the operation id for the tampered identity, so ONLY the
        // learner-differs-from-target rule can reject it (a stale id would be
        // caught by the id check instead).
        let mut target_is_learner = journal.clone();
        target_is_learner.learner = target_is_learner.target;
        target_is_learner.operation_id = operation_id(
            target_is_learner.group_id,
            target_is_learner.target,
            target_is_learner.learner,
            target_is_learner.expected_term,
            &target_is_learner.expected_voters,
        );
        assert!(!target_is_learner.validate());
    }

    #[test]
    fn permits_successor_accepts_the_one_defined_step_and_rejects_others() {
        let journal = MembershipShrinkJournal::new(0, 3, 4, 7, vec![1, 2, 3]).unwrap();
        let mut next = evidence(vec![1, 2, 3]);
        let advanced = journal
            .advance(MembershipShrinkPhase::DrainRequested, next.clone())
            .unwrap();
        assert!(journal.permits_successor(&advanced));

        // A different operation entirely must never be a permitted successor.
        let other = MembershipShrinkJournal::new(0, 3, 4, 7, vec![1, 2, 3, 5]).unwrap();
        assert!(other.validate());
        assert!(!journal.permits_successor(&other));

        // Skipping straight to LearnerCaughtUp is not a single valid step.
        next.drained = true;
        next.learner_caught_up = true;
        let mut skipped = journal.clone();
        skipped.phase = MembershipShrinkPhase::LearnerCaughtUp;
        skipped.evidence = Some(next);
        assert!(!journal.permits_successor(&skipped));
    }

    /// Evidence that satisfies every gate up to and including `phase` for the
    /// voter set `voters`.
    fn evidence_through(
        phase: MembershipShrinkPhase,
        voters: Vec<NodeId>,
    ) -> MembershipShrinkEvidence {
        use MembershipShrinkPhase::*;
        let mut ready = evidence(voters);
        let reached = |gate: MembershipShrinkPhase| phase_rank(phase) >= phase_rank(gate);
        ready.drained = reached(Drained);
        ready.learner_caught_up = reached(LearnerCaughtUp);
        ready.leadership_transferred = reached(LeadershipTransferred);
        let safe = reached(SafetyChecked);
        ready.quorum_preserved = safe;
        ready.failure_domain_preserved = safe;
        ready.headroom_preserved = safe;
        ready.pdb_preserved = safe;
        ready.membership_change_committed = reached(RemovalCommitted);
        ready.target_absent = reached(RemovalCommitted);
        ready
    }

    fn phase_rank(phase: MembershipShrinkPhase) -> usize {
        use MembershipShrinkPhase::*;
        [
            Proposed,
            DrainRequested,
            Drained,
            LearnerCaughtUp,
            LeadershipTransferred,
            SafetyChecked,
            RemovalCommitted,
            Completed,
        ]
        .iter()
        .position(|candidate| *candidate == phase)
        .expect("a non-abort phase")
    }

    /// The voter set the evidence for a transition into `next` must name: the
    /// remaining voters once the removal is committed, otherwise the expected set.
    fn evidence_voters_for(next: MembershipShrinkPhase) -> Vec<NodeId> {
        if phase_rank(next) >= phase_rank(MembershipShrinkPhase::RemovalCommitted) {
            vec![1, 2]
        } else {
            vec![1, 2, 3]
        }
    }

    /// Advance a fresh journal through every phase up to `phase`, each with
    /// exactly-ready evidence.
    fn journal_at(phase: MembershipShrinkPhase) -> MembershipShrinkJournal {
        use MembershipShrinkPhase::*;
        let mut journal = MembershipShrinkJournal::new(0, 3, 4, 7, vec![1, 2, 3]).unwrap();
        for next in [
            DrainRequested,
            Drained,
            LearnerCaughtUp,
            LeadershipTransferred,
            SafetyChecked,
            RemovalCommitted,
            Completed,
        ] {
            if phase_rank(journal.phase) >= phase_rank(phase) {
                break;
            }
            journal = journal
                .advance(next, evidence_through(next, evidence_voters_for(next)))
                .unwrap();
        }
        journal
    }

    #[test]
    fn advance_rejects_skips_from_every_intermediate_phase() {
        use MembershipShrinkPhase::*;
        // From each non-terminal phase, jumping two or more steps ahead is refused
        // by `advance` (transition table) and by `permits_successor` (step shape),
        // even with evidence that satisfies the skipped-to phase's own gate.
        let phases = [
            Proposed,
            DrainRequested,
            Drained,
            LearnerCaughtUp,
            LeadershipTransferred,
            SafetyChecked,
        ];
        for (index, phase) in phases.iter().copied().enumerate() {
            let journal = journal_at(phase);
            assert_eq!(journal.phase, phase);
            for skipped_to in [
                Drained,
                LearnerCaughtUp,
                LeadershipTransferred,
                SafetyChecked,
                RemovalCommitted,
                Completed,
            ]
            .into_iter()
            .filter(|next| phase_rank(*next) >= index + 2)
            {
                let ready = evidence_through(skipped_to, evidence_voters_for(skipped_to));
                assert!(
                    journal.advance(skipped_to, ready.clone()).is_err(),
                    "{phase:?} -> {skipped_to:?} must not skip a gate"
                );
                let mut forged = journal.clone();
                forged.phase = skipped_to;
                forged.evidence = Some(ready);
                assert!(
                    !journal.permits_successor(&forged),
                    "{phase:?} -> {skipped_to:?} is not a single step"
                );
            }
        }
    }

    #[test]
    fn removal_commit_and_completion_require_committed_target_absent_evidence() {
        use MembershipShrinkPhase::*;
        let safety_checked = journal_at(SafetyChecked);
        assert!(safety_checked.ready_for_removal());

        let mut target_present = evidence_through(RemovalCommitted, vec![1, 2]);
        target_present.target_absent = false;
        assert!(safety_checked
            .advance(RemovalCommitted, target_present)
            .is_err());

        let removal_committed = safety_checked
            .advance(
                RemovalCommitted,
                evidence_through(RemovalCommitted, vec![1, 2]),
            )
            .unwrap();
        assert!(safety_checked.permits_successor(&removal_committed));
        assert_eq!(
            removal_committed.recovery_action(&[1, 2]),
            ShrinkRecoveryAction::Complete
        );

        let mut completion_target_present = evidence_through(Completed, vec![1, 2]);
        completion_target_present.target_absent = false;
        assert!(removal_committed
            .advance(Completed, completion_target_present)
            .is_err());

        let completed = removal_committed
            .advance(Completed, evidence_through(Completed, vec![1, 2]))
            .unwrap();
        assert!(completed.phase.terminal());
        assert!(removal_committed.permits_successor(&completed));
        assert!(completed
            .advance(Completed, evidence_through(Completed, vec![1, 2]))
            .is_err());
    }

    #[test]
    fn abort_is_a_permitted_successor_from_every_non_terminal_phase() {
        use MembershipShrinkPhase::*;
        for phase in [
            Proposed,
            DrainRequested,
            Drained,
            LearnerCaughtUp,
            LeadershipTransferred,
            SafetyChecked,
            RemovalCommitted,
        ] {
            let journal = journal_at(phase);
            let aborted = journal.abort("operator cancelled").unwrap();
            assert!(aborted.validate());
            assert!(
                journal.permits_successor(&aborted),
                "{phase:?} -> Aborted must be permitted"
            );
            assert!(!aborted.permits_successor(&journal), "an abort is terminal");
        }
    }

    #[test]
    fn validate_rejects_an_invalid_abort_state() {
        let journal = MembershipShrinkJournal::new(0, 3, 4, 7, vec![1, 2, 3]).unwrap();
        let aborted = journal.abort("operator cancelled").unwrap();
        assert!(aborted.validate());

        let mut reason_without_abort = journal.clone();
        reason_without_abort.abort_reason = Some("operator cancelled".to_string());
        assert!(!reason_without_abort.validate());

        for invalid_reason in [
            String::new(),
            "x".repeat(MAX_EVIDENCE_REF + 1),
            "bad\u{7}reason".to_string(),
        ] {
            let mut tampered = aborted.clone();
            tampered.abort_reason = Some(invalid_reason.clone());
            assert!(!tampered.validate(), "abort reason {invalid_reason:?}");
            assert!(journal.abort(&invalid_reason).is_err());
        }
    }

    #[test]
    fn validate_rejects_invalid_retained_evidence() {
        let journal = MembershipShrinkJournal::new(0, 3, 4, 7, vec![1, 2, 3]).unwrap();
        let mut with_evidence = journal.clone();
        with_evidence.evidence = Some(evidence(vec![1, 2, 3]));
        assert!(with_evidence.validate());

        let tampers: [fn(&mut MembershipShrinkEvidence); 4] = [
            |evidence| evidence.evidence_ref.clear(),
            |evidence| evidence.evidence_ref = "x".repeat(MAX_EVIDENCE_REF + 1),
            |evidence| evidence.evidence_ref = "bad\nref".to_string(),
            |evidence| evidence.observed_voters = vec![2, 1, 3],
        ];
        for tamper in tampers {
            let mut tampered = with_evidence.clone();
            tamper(tampered.evidence.as_mut().unwrap());
            assert!(!tampered.validate());
        }
    }
}
