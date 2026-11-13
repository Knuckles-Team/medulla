//! Per-write admission state machine: exactly one admitted batch, exactly one
//! owner-row window inside it, and a poisoned write on any unfinished drop.

use crate::admission_machine::AdmissionEvent;
use crate::admitted::{is_ledger_only, AdmittedMutation};
use medulla_protocol_types::{mutation_batch::MutationBatch, mutation_batch::MutationBatchRecord};
use medulla_storage::{encode_bounded, OwnerDomain, OwnerLayout};

pub(crate) enum AdmissionState {
    Idle,
    Applying {
        batch: Vec<u8>,
        owner: Option<OwnerLayout>,
    },
    Finished {
        batch: Vec<u8>,
    },
    /// Admission resolved to a terminal receipt that is ALREADY durable, so
    /// this write applies nothing for it.
    ///
    /// A replayed member is a first-class terminal state: it may be aborted
    /// without consuming its fresh nonce, or committed to consume only that
    /// nonce. It writes no owner, receipt, version, fence or outbox rows.
    /// `open_owner_admission` and `finish_batch_admission` both refuse it,
    /// which is what "writes nothing" means mechanically.
    Replayed {
        batch: Vec<u8>,
        /// Boxed only to keep this state machine's own footprint small next to
        /// the `Idle`/`Applying`/`Finished` steady-state variants; this enum
        /// is process-local (never encoded), so the box has no wire effect.
        record: Box<MutationBatchRecord>,
        /// The last batch this write actually FINISHED before the replay, if
        /// any.
        ///
        /// A write may carry a SEQUENCE of batches (the change-envelope page
        /// commits a whole page through one member: `Idle | Finished ->
        /// Applying` is the transition that allows it), and a replayed member
        /// writes nothing. So a replay must not displace the terminal
        /// reference the sequence already earned: `commit_change_envelopes`
        /// deliberately keeps the LAST FRESH batch as the name it hands
        /// `commit_group`, and without this a trailing replay would make that
        /// name fail `validate_commit_admission` with "mutation commit does not
        /// match a finished batch".
        finished: Option<Vec<u8>>,
    },
    Poisoned,
}

impl<D: OwnerDomain> AdmittedMutation<'_, D> {
    pub(crate) fn admit_apply_batch(&self, batch: &MutationBatch) -> Result<(), String> {
        let encoded = encode_bounded(batch, "admitted mutation batch")?;
        self.admission
            .borrow_mut()
            .apply(AdmissionEvent::Admit { batch: encoded })
            .map_err(|refused| refused.to_string())
    }

    /// Record that this member's batch resolved to an already-durable receipt.
    pub(crate) fn remember_replayed_batch(
        &self,
        batch: &MutationBatch,
        record: &MutationBatchRecord,
    ) -> Result<(), String> {
        let encoded = encode_bounded(batch, "replayed mutation batch")?;
        let mut state = self.admission.borrow_mut();
        // Remembering the same replay again is idempotent when its record agrees.
        if let AdmissionState::Replayed {
            batch: existing,
            record: existing_record,
            ..
        } = &*state
        {
            if existing.as_slice() == encoded.as_slice() {
                let existing_batch =
                    encode_bounded(&existing_record.batch, "replayed mutation batch")?;
                let record_batch = encode_bounded(&record.batch, "replayed mutation batch")?;
                return if existing_batch == record_batch {
                    Ok(())
                } else {
                    Err("a replayed batch record does not match its admitted batch".to_string())
                };
            }
        }
        // A replay may follow a COMPLETED batch in the same write: the sequence's terminal
        // reference is carried forward by the transition (see
        // `AdmissionState::Replayed::finished`). A replay over an OPEN (`Applying`) batch is
        // refused by the machine: that really would be two live batches at once.
        state
            .apply(AdmissionEvent::Replay {
                batch: encoded,
                record: Box::new(record.clone()),
            })
            .map_err(|refused| refused.to_string())
    }

    /// Mark the already-remembered replay as terminal for a group member.
    /// `commit::begin` stores the receipt before this compatibility call, so a
    /// group and a sole writer share one replay state machine.
    pub(crate) fn admit_replayed_batch(&self, batch: &MutationBatch) -> Result<(), String> {
        let encoded = encode_bounded(batch, "replayed mutation batch")?;
        match &*self.admission.borrow() {
            AdmissionState::Replayed {
                batch: existing, ..
            } if existing.as_slice() == encoded.as_slice() => Ok(()),
            AdmissionState::Poisoned => Err("mutation write is poisoned".to_string()),
            _ => Err("a replayed batch cannot follow an admitted one".to_string()),
        }
    }

    pub(crate) fn replayed_record(
        &self,
        batch: &MutationBatch,
    ) -> Result<Option<MutationBatchRecord>, String> {
        let encoded = encode_bounded(batch, "replayed mutation batch")?;
        match &*self.admission.borrow() {
            AdmissionState::Replayed {
                batch: existing,
                record,
                ..
            } if existing.as_slice() == encoded.as_slice() => Ok(Some((**record).clone())),
            // The named batch is the sequence's last FINISHED one, which a
            // later replay did not displace: it is not a replay, so there is no
            // replay record to return.
            AdmissionState::Replayed {
                finished: Some(finished),
                ..
            } if finished.as_slice() == encoded.as_slice() => Ok(None),
            AdmissionState::Replayed { .. } => {
                Err("mutation replay commit does not match its admitted batch".to_string())
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn admit_prepared_batch(&self, batch: &MutationBatch) -> Result<(), String> {
        self.admit_apply_batch(batch)
    }

    pub(crate) fn open_owner_admission(
        &self,
        batch: &MutationBatch,
        layout: OwnerLayout,
    ) -> Result<(), String> {
        let encoded = encode_bounded(batch, "owner mutation batch")?;
        self.admission
            .borrow_mut()
            .apply(AdmissionEvent::OpenOwner {
                batch: encoded,
                layout,
            })
            .map_err(|refused| refused.to_string())
    }

    pub(crate) fn finish_owner_admission(&self, layout: OwnerLayout) -> Result<(), String> {
        let state = self.admission.borrow();
        match &*state {
            AdmissionState::Applying {
                owner: Some(open), ..
            } if *open == layout => Ok(()),
            AdmissionState::Poisoned => Err("mutation write is poisoned".to_string()),
            _ => Err("owner write completion does not match its admission".to_string()),
        }
    }

    pub(crate) fn poison_owner_admission(&self) {
        let mut state = self.admission.borrow_mut();
        if state.phase() == crate::admission_machine::AdmissionPhase::Poisoned {
            return;
        }
        if let Err(refused) = state.apply(AdmissionEvent::Poison) {
            tracing::error!(%refused, "poisoning a mutation write was refused");
        }
    }

    pub(crate) fn finish_batch_admission(&self, batch: &MutationBatch) -> Result<(), String> {
        let encoded = encode_bounded(batch, "finished mutation batch")?;
        self.admission
            .borrow_mut()
            .apply(AdmissionEvent::Finish {
                batch: encoded,
                ledger_only: is_ledger_only(D::LAYOUT),
            })
            .map_err(|refused| refused.to_string())
    }

    /// Check the finish preconditions before any terminal metadata is written.
    ///
    /// `finish` persists the receipt, key row, version, fence and outbox before
    /// it transitions `Applying` to `Finished`. A replayed group member must
    /// therefore be rejected before those writes, or its failed finish call
    /// would overwrite the durable replay receipt inside the shared group
    /// transaction.
    pub(crate) fn validate_finish_admission(&self, batch: &MutationBatch) -> Result<(), String> {
        let encoded = encode_bounded(batch, "finished mutation batch")?;
        match &*self.admission.borrow() {
            AdmissionState::Applying {
                batch: admitted,
                owner,
            } if admitted.as_slice() == encoded.as_slice()
                && (is_ledger_only(D::LAYOUT) || owner.is_some()) =>
            {
                Ok(())
            }
            AdmissionState::Replayed { .. } => {
                Err("a replayed member cannot be finished".to_string())
            }
            AdmissionState::Poisoned => Err("mutation write is poisoned".to_string()),
            _ => Err("mutation batch owner work is unfinished or mismatched".to_string()),
        }
    }

    pub(crate) fn validate_commit_admission(&self, batch: &MutationBatch) -> Result<(), String> {
        let encoded = encode_bounded(batch, "committed mutation batch")?;
        match &*self.admission.borrow() {
            AdmissionState::Finished {
                batch: finished, ..
            } if finished.as_slice() == encoded.as_slice() => Ok(()),
            // A replayed member wrote nothing and needs nothing written; its
            // receipt was already durable before this transaction opened.
            AdmissionState::Replayed {
                batch: replayed, ..
            } if replayed.as_slice() == encoded.as_slice() => Ok(()),
            // ... and a replay that trailed a completed batch left that batch
            // as the sequence's terminal reference.
            AdmissionState::Replayed {
                finished: Some(finished),
                ..
            } if finished.as_slice() == encoded.as_slice() => Ok(()),
            AdmissionState::Poisoned => Err("mutation write is poisoned".to_string()),
            _ => Err("mutation commit does not match a finished batch".to_string()),
        }
    }
}
