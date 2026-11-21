//! EH-560 governed changes over the RBAC image: each transition is applied
//! to the ledger, written through with the whole authorization image, and
//! rolled back if the write fails -- the elevation discipline.
#![cfg(feature = "security")]

use medulla_mutation_types::{
    governed_change::GovernedActor, governed_change::GovernedApproval,
    governed_change::GovernedChange, governed_change::GovernedConsumption,
    governed_change::GovernedLedger, governed_change::GovernedProposal,
    governed_change::GovernedRefusal,
};

use super::IsolationLayer;

/// A refused governed op, or a persistence failure after an accepted one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GovernedError {
    Refused(GovernedRefusal),
    Persist(String),
}

impl std::fmt::Display for GovernedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => refusal.fmt(f),
            Self::Persist(error) => f.write_str(error),
        }
    }
}

impl IsolationLayer {
    /// The clock of every governed-change decision: `base_ms` (the authoritative
    /// time of the request) plus the controlled skew, which is zero in service.
    pub fn governed_now_ms(&self, base_ms: u64) -> u64 {
        base_ms.saturating_add(
            self.governed_clock_skew_ms
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// Move the governed clock forward by `ms`: the controlled clock a test
    /// drives to close an approval window.
    pub fn advance_governed_clock_ms(&self, ms: u64) {
        self.governed_clock_skew_ms
            .fetch_add(ms, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn try_propose_change(
        &mut self,
        actor: &GovernedActor,
        request: &GovernedProposal,
        now_ms: u64,
    ) -> Result<GovernedChange, GovernedError> {
        self.apply_governed(|ledger| ledger.propose(actor, request, now_ms))
    }

    pub fn try_approve_change(
        &mut self,
        actor: &GovernedActor,
        request: &GovernedApproval,
        now_ms: u64,
    ) -> Result<GovernedChange, GovernedError> {
        self.apply_governed(|ledger| ledger.approve(actor, request, now_ms))
    }

    pub fn try_revoke_change(
        &mut self,
        actor: &GovernedActor,
        change_id: &str,
        now_ms: u64,
    ) -> Result<GovernedChange, GovernedError> {
        self.apply_governed(|ledger| ledger.revoke(actor, change_id, now_ms))
    }

    /// Consume an approved change: the ONE entry point for the engine path
    /// that applies it (e.g. the approved schema attach). Consume BEFORE
    /// applying: a crash in between loses the approval (fail closed), never
    /// spends it twice.
    pub fn try_consume_governed_change(
        &mut self,
        consumption: &GovernedConsumption,
        consumer: &str,
        now_ms: u64,
    ) -> Result<GovernedChange, GovernedError> {
        self.apply_governed(|ledger| ledger.consume(consumption, consumer, now_ms))
    }

    fn apply_governed(
        &mut self,
        transition: impl FnOnce(&mut GovernedLedger) -> Result<GovernedChange, GovernedRefusal>,
    ) -> Result<GovernedChange, GovernedError> {
        use super::layer_store::PolicyWriteError;
        self.transact_policy(|policy| transition(policy.governed_mut()))
            .map_err(|error| match error {
                PolicyWriteError::Refused(refusal) => GovernedError::Refused(refusal),
                PolicyWriteError::Persist(error) => GovernedError::Persist(error),
            })
    }
}
