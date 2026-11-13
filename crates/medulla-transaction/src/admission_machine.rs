//! MEDULLA-TYPED-PACKS-R810.1: the per-write admission lifecycle as one explicit state
//! machine (organization constitution Article XXXI).
//!
//! `AdmissionState` (in `admission.rs`) is the process-local holder of one physical write's
//! admission: it carries the admitted batch, the open owner window and the replay record.
//! Its phase is the closed set [`AdmissionPhase`]; the closed event set is
//! [`AdmissionTrigger`]; the edge table is [`ADMISSION_MACHINE`]. [`AdmissionState::apply`]
//! is the only function that changes the state: it checks the edge table through the generic
//! `transition` function, and an illegal pair is refused with the typed `TRANSITION_REFUSED`
//! naming the machine, the current phase and the event, changing nothing. The data a variant
//! carries (batch bytes, owner layout, replay record, last finished batch) is kept by the
//! transition that writes it. The ontology description is `AdmissionMachine` in
//! `state-machines-v1.ttl`.

use medulla_protocol_types::mutation_batch::MutationBatchRecord;
use medulla_storage::OwnerLayout;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefusalReason,
    TransitionRefused,
};

use crate::admission::AdmissionState;

/// The closed phase set of `AdmissionMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdmissionPhase {
    /// No batch is admitted yet.
    Idle,
    /// One batch is admitted; its owner-row window opens and closes inside it.
    Applying,
    /// The admitted batch finished and awaits its commit.
    Finished,
    /// Admission resolved to an already-durable receipt: this write applies nothing for it.
    Replayed,
    /// An unfinished drop poisoned the write; it accepts no further event.
    Poisoned,
}

/// The closed event set of `AdmissionMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdmissionTrigger {
    /// A batch is admitted for apply.
    Admit,
    /// The batch's owner-row window opens.
    OpenOwner,
    /// The batch's owner work finished.
    Finish,
    /// The batch resolved to an already-durable replay receipt.
    Replay,
    /// The write is poisoned.
    Poison,
}

/// The named guards of `AdmissionMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdmissionGuard {
    /// The event names the admitted batch and its owner window is still closed.
    BatchMatchesOwnerClosed,
    /// The event names the admitted batch and its owner work is done (or the layout is
    /// ledger-only).
    BatchMatchesOwnerReady,
}

impl MachineState for AdmissionPhase {
    fn stored(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Applying => "applying",
            Self::Finished => "finished",
            Self::Replayed => "replayed",
            Self::Poisoned => "poisoned",
        }
    }
}

impl MachineEvent for AdmissionTrigger {
    fn name(self) -> &'static str {
        match self {
            Self::Admit => "Admit",
            Self::OpenOwner => "OpenOwner",
            Self::Finish => "Finish",
            Self::Replay => "Replay",
            Self::Poison => "Poison",
        }
    }
}

/// `AdmissionMachine`: the closed edge table. Every other pair is refused.
pub static ADMISSION_MACHINE: MachineDef<AdmissionPhase, AdmissionTrigger, AdmissionGuard> =
    MachineDef {
        name: "AdmissionMachine",
        states: &[
            AdmissionPhase::Idle,
            AdmissionPhase::Applying,
            AdmissionPhase::Finished,
            AdmissionPhase::Replayed,
            AdmissionPhase::Poisoned,
        ],
        initial: AdmissionPhase::Idle,
        terminals: &[AdmissionPhase::Poisoned],
        events: &[
            AdmissionTrigger::Admit,
            AdmissionTrigger::OpenOwner,
            AdmissionTrigger::Finish,
            AdmissionTrigger::Replay,
            AdmissionTrigger::Poison,
        ],
        guards: &[
            AdmissionGuard::BatchMatchesOwnerClosed,
            AdmissionGuard::BatchMatchesOwnerReady,
        ],
        edges: &[
            Edge {
                id: "A01",
                from: AdmissionPhase::Idle,
                event: AdmissionTrigger::Admit,
                guard: None,
                to: AdmissionPhase::Applying,
            },
            Edge {
                id: "A02",
                from: AdmissionPhase::Finished,
                event: AdmissionTrigger::Admit,
                guard: None,
                to: AdmissionPhase::Applying,
            },
            Edge {
                id: "A03",
                from: AdmissionPhase::Replayed,
                event: AdmissionTrigger::Admit,
                guard: None,
                to: AdmissionPhase::Applying,
            },
            Edge {
                id: "A04",
                from: AdmissionPhase::Applying,
                event: AdmissionTrigger::OpenOwner,
                guard: Some(AdmissionGuard::BatchMatchesOwnerClosed),
                to: AdmissionPhase::Applying,
            },
            Edge {
                id: "A05",
                from: AdmissionPhase::Applying,
                event: AdmissionTrigger::Finish,
                guard: Some(AdmissionGuard::BatchMatchesOwnerReady),
                to: AdmissionPhase::Finished,
            },
            Edge {
                id: "A06",
                from: AdmissionPhase::Idle,
                event: AdmissionTrigger::Replay,
                guard: None,
                to: AdmissionPhase::Replayed,
            },
            Edge {
                id: "A07",
                from: AdmissionPhase::Finished,
                event: AdmissionTrigger::Replay,
                guard: None,
                to: AdmissionPhase::Replayed,
            },
            Edge {
                id: "A08",
                from: AdmissionPhase::Replayed,
                event: AdmissionTrigger::Replay,
                guard: None,
                to: AdmissionPhase::Replayed,
            },
            Edge {
                id: "A09",
                from: AdmissionPhase::Idle,
                event: AdmissionTrigger::Poison,
                guard: None,
                to: AdmissionPhase::Poisoned,
            },
            Edge {
                id: "A10",
                from: AdmissionPhase::Applying,
                event: AdmissionTrigger::Poison,
                guard: None,
                to: AdmissionPhase::Poisoned,
            },
            Edge {
                id: "A11",
                from: AdmissionPhase::Finished,
                event: AdmissionTrigger::Poison,
                guard: None,
                to: AdmissionPhase::Poisoned,
            },
            Edge {
                id: "A12",
                from: AdmissionPhase::Replayed,
                event: AdmissionTrigger::Poison,
                guard: None,
                to: AdmissionPhase::Poisoned,
            },
        ],
    };

/// One event with its data. The data is what the entered state keeps.
pub(crate) enum AdmissionEvent {
    Admit {
        batch: Vec<u8>,
    },
    OpenOwner {
        batch: Vec<u8>,
        layout: OwnerLayout,
    },
    Finish {
        batch: Vec<u8>,
        ledger_only: bool,
    },
    Replay {
        batch: Vec<u8>,
        record: Box<MutationBatchRecord>,
    },
    Poison,
}

impl AdmissionEvent {
    pub(crate) fn trigger(&self) -> AdmissionTrigger {
        match self {
            Self::Admit { .. } => AdmissionTrigger::Admit,
            Self::OpenOwner { .. } => AdmissionTrigger::OpenOwner,
            Self::Finish { .. } => AdmissionTrigger::Finish,
            Self::Replay { .. } => AdmissionTrigger::Replay,
            Self::Poison => AdmissionTrigger::Poison,
        }
    }
}

impl AdmissionState {
    /// The phase of this state: the machine's state.
    pub(crate) fn phase(&self) -> AdmissionPhase {
        match self {
            Self::Idle => AdmissionPhase::Idle,
            Self::Applying { .. } => AdmissionPhase::Applying,
            Self::Finished { .. } => AdmissionPhase::Finished,
            Self::Replayed { .. } => AdmissionPhase::Replayed,
            Self::Poisoned => AdmissionPhase::Poisoned,
        }
    }

    fn guard_holds(&self, guard: AdmissionGuard, event: &AdmissionEvent) -> bool {
        match (guard, self, event) {
            (
                AdmissionGuard::BatchMatchesOwnerClosed,
                Self::Applying {
                    batch: admitted,
                    owner: None,
                },
                AdmissionEvent::OpenOwner { batch, .. },
            ) => admitted == batch,
            (
                AdmissionGuard::BatchMatchesOwnerReady,
                Self::Applying {
                    batch: admitted,
                    owner,
                },
                AdmissionEvent::Finish {
                    batch, ledger_only, ..
                },
            ) => admitted == batch && (*ledger_only || owner.is_some()),
            _ => false,
        }
    }

    /// The one transition function of `AdmissionMachine`: the only writer of this state.
    /// A refused event changes nothing.
    pub(crate) fn apply(&mut self, event: AdmissionEvent) -> Result<(), TransitionRefused> {
        let edge = transition(&ADMISSION_MACHINE, self.phase(), event.trigger(), |guard| {
            self.guard_holds(guard, &event)
        })?;
        let from = self.phase();
        let current = std::mem::replace(self, Self::Poisoned);
        let next = match (current, event) {
            (_, AdmissionEvent::Admit { batch }) => Self::Applying { batch, owner: None },
            (Self::Applying { batch, .. }, AdmissionEvent::OpenOwner { layout, .. }) => {
                Self::Applying {
                    batch,
                    owner: Some(layout),
                }
            }
            (Self::Applying { batch, .. }, AdmissionEvent::Finish { .. }) => {
                Self::Finished { batch }
            }
            (Self::Idle, AdmissionEvent::Replay { batch, record }) => Self::Replayed {
                batch,
                record,
                finished: None,
            },
            (Self::Finished { batch: done }, AdmissionEvent::Replay { batch, record }) => {
                Self::Replayed {
                    batch,
                    record,
                    finished: Some(done),
                }
            }
            (Self::Replayed { finished, .. }, AdmissionEvent::Replay { batch, record }) => {
                Self::Replayed {
                    batch,
                    record,
                    finished,
                }
            }
            (_, AdmissionEvent::Poison) => Self::Poisoned,
            // The edge table admitted a pair this function does not build: the write stays
            // poisoned (closed failure) and the refusal is typed.
            _ => {
                return Err(TransitionRefused {
                    machine: ADMISSION_MACHINE.name.to_string(),
                    from: from.stored().to_string(),
                    event: edge.event.name().to_string(),
                    reason: TransitionRefusalReason::IllegalTransition,
                });
            }
        };
        debug_assert_eq!(next.phase(), edge.to);
        *self = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(tag: u8) -> Vec<u8> {
        vec![tag; 4]
    }

    fn record() -> Box<MutationBatchRecord> {
        Box::new(crate::tests::committed_record_fixture())
    }

    /// A state in `phase`, built only through `apply` from `Idle`.
    fn state_in(phase: AdmissionPhase) -> AdmissionState {
        let mut state = AdmissionState::Idle;
        match phase {
            AdmissionPhase::Idle => {}
            AdmissionPhase::Applying => {
                state
                    .apply(AdmissionEvent::Admit { batch: bytes(1) })
                    .unwrap();
            }
            AdmissionPhase::Finished => {
                state
                    .apply(AdmissionEvent::Admit { batch: bytes(1) })
                    .unwrap();
                state
                    .apply(AdmissionEvent::Finish {
                        batch: bytes(1),
                        ledger_only: true,
                    })
                    .unwrap();
            }
            AdmissionPhase::Replayed => {
                state
                    .apply(AdmissionEvent::Replay {
                        batch: bytes(1),
                        record: record(),
                    })
                    .unwrap();
            }
            AdmissionPhase::Poisoned => {
                state.apply(AdmissionEvent::Poison).unwrap();
            }
        }
        assert_eq!(state.phase(), phase);
        state
    }

    fn event_of(trigger: AdmissionTrigger, batch: u8) -> AdmissionEvent {
        match trigger {
            AdmissionTrigger::Admit => AdmissionEvent::Admit {
                batch: bytes(batch),
            },
            AdmissionTrigger::OpenOwner => AdmissionEvent::OpenOwner {
                batch: bytes(batch),
                layout: OwnerLayout::AgentLibrary,
            },
            AdmissionTrigger::Finish => AdmissionEvent::Finish {
                batch: bytes(batch),
                ledger_only: true,
            },
            AdmissionTrigger::Replay => AdmissionEvent::Replay {
                batch: bytes(batch),
                record: record(),
            },
            AdmissionTrigger::Poison => AdmissionEvent::Poison,
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R810.1
    // spec: MEDULLA-TYPED-PACKS-R810
    #[test]
    fn every_legal_transition_applies() {
        for edge in ADMISSION_MACHINE.edges {
            let mut state = state_in(edge.from);
            // The batch the event names is the admitted one (tag 1).
            state.apply(event_of(edge.event, 1)).unwrap_or_else(|e| {
                panic!("edge {} must apply: {e}", edge.id);
            });
            assert_eq!(state.phase(), edge.to, "edge {}", edge.id);
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R810.1
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        for from in ADMISSION_MACHINE.states {
            for trigger in ADMISSION_MACHINE.events {
                let legal = ADMISSION_MACHINE
                    .edges
                    .iter()
                    .any(|e| e.from == *from && e.event == *trigger);
                if legal {
                    continue;
                }
                let mut state = state_in(*from);
                let refused = state.apply(event_of(*trigger, 1)).unwrap_err();
                assert_eq!(refused.machine, "AdmissionMachine");
                assert_eq!(refused.from, from.stored());
                assert_eq!(refused.event, trigger.name());
                let expected = if ADMISSION_MACHINE.is_terminal(*from) {
                    TransitionRefusalReason::TerminalState
                } else {
                    TransitionRefusalReason::IllegalTransition
                };
                assert_eq!(refused.reason, expected);
                assert_eq!(state.phase(), *from, "a refusal changes nothing");
            }
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R810.1
    #[test]
    fn a_failed_guard_is_refused_and_changes_nothing() {
        // A different batch cannot open the owner window or finish.
        let mut applying = state_in(AdmissionPhase::Applying);
        for event in [
            AdmissionEvent::OpenOwner {
                batch: bytes(9),
                layout: OwnerLayout::AgentLibrary,
            },
            AdmissionEvent::Finish {
                batch: bytes(9),
                ledger_only: true,
            },
        ] {
            let refused = applying.apply(event).unwrap_err();
            assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
            assert_eq!(applying.phase(), AdmissionPhase::Applying);
        }
        // A non-ledger layout cannot finish before its owner window opened.
        let refused = applying
            .apply(AdmissionEvent::Finish {
                batch: bytes(1),
                ledger_only: false,
            })
            .unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
        // The window opens once.
        applying
            .apply(event_of(AdmissionTrigger::OpenOwner, 1))
            .unwrap();
        let refused = applying
            .apply(event_of(AdmissionTrigger::OpenOwner, 1))
            .unwrap_err();
        assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
        applying
            .apply(AdmissionEvent::Finish {
                batch: bytes(1),
                ledger_only: false,
            })
            .unwrap();
        assert_eq!(applying.phase(), AdmissionPhase::Finished);
    }

    // spec: MEDULLA-TYPED-PACKS-R810.1
    #[test]
    fn a_replay_after_a_finished_batch_keeps_that_batch() {
        let mut state = state_in(AdmissionPhase::Finished);
        state
            .apply(AdmissionEvent::Replay {
                batch: bytes(2),
                record: record(),
            })
            .unwrap();
        state
            .apply(AdmissionEvent::Replay {
                batch: bytes(3),
                record: record(),
            })
            .unwrap();
        match state {
            AdmissionState::Replayed {
                batch, finished, ..
            } => {
                assert_eq!(batch, bytes(3));
                assert_eq!(finished, Some(bytes(1)));
            }
            _ => panic!("expected Replayed"),
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R810.1
    #[test]
    fn no_writer_sets_state_outside_apply() {
        // Every assignment to an admission cell goes through `apply`: the only direct
        // construction of a non-Idle value outside `admission_machine.rs` is forbidden.
        for (name, source) in [
            ("admission.rs", include_str!("admission.rs")),
            ("admitted.rs", include_str!("admitted.rs")),
        ] {
            for (n, line) in source.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                assert!(
                    !code.contains("*state = AdmissionState::")
                        && !code.contains("*self.admission.borrow_mut() = AdmissionState::"),
                    "{name}:{}: assigns the admission state outside apply",
                    n + 1
                );
            }
        }
    }
}
