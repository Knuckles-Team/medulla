//! The operation-audit machine `AuditMachine` (MEDULLA-DURABLE-KERNEL-R874.12,
//! organization constitution Article XXXI).
//!
//! [`AuditStatus`] is the closed state set, [`AuditEvent`] the closed event set,
//! [`AUDIT_MACHINE`] the edge table and [`AuditStatus::apply`] the one transition function.
//! A record starts `Reserved`; each outcome event settles it into one of the three terminal
//! states, which accept no further event.

use super::audit_status::AuditStatus;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, TransitionRefused,
};

/// The closed event set of the audit machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditEvent {
    /// The effect succeeded.
    Succeed,
    /// The effect failed.
    Fail,
    /// The effect was refused.
    Deny,
}

impl MachineEvent for AuditEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Succeed => "Succeed",
            Self::Fail => "Fail",
            Self::Deny => "Deny",
        }
    }
}

/// The audit machine has no guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditGuard {}

/// The audit machine: `Reserved` start; `Ok`, `Error` and `Denied` are terminal.
pub const AUDIT_MACHINE: MachineDef<AuditStatus, AuditEvent, AuditGuard> = MachineDef {
    name: "AuditMachine",
    states: &[
        AuditStatus::Reserved,
        AuditStatus::Ok,
        AuditStatus::Error,
        AuditStatus::Denied,
    ],
    initial: AuditStatus::Reserved,
    terminals: &[AuditStatus::Ok, AuditStatus::Error, AuditStatus::Denied],
    events: &[AuditEvent::Succeed, AuditEvent::Fail, AuditEvent::Deny],
    guards: &[],
    edges: &[
        Edge {
            id: "A01",
            from: AuditStatus::Reserved,
            event: AuditEvent::Succeed,
            guard: None,
            to: AuditStatus::Ok,
        },
        Edge {
            id: "A02",
            from: AuditStatus::Reserved,
            event: AuditEvent::Fail,
            guard: None,
            to: AuditStatus::Error,
        },
        Edge {
            id: "A03",
            from: AuditStatus::Reserved,
            event: AuditEvent::Deny,
            guard: None,
            to: AuditStatus::Denied,
        },
    ],
};

impl AuditStatus {
    /// The one transition function: the next status, or the typed refusal.
    pub fn apply(self, event: AuditEvent) -> Result<Self, TransitionRefused> {
        transition(&AUDIT_MACHINE, self, event, |guard| match guard {}).map(|edge| edge.to)
    }

    /// The status a request settles into: the reservation's initial state advanced by the
    /// outcome event. The reservation accepts every event, so the refusal arm is
    /// unreachable; it fails closed to `Denied`.
    pub fn settled(event: AuditEvent) -> Self {
        AUDIT_MACHINE
            .initial
            .apply(event)
            .unwrap_or(AuditStatus::Denied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::{MachineState, TransitionRefusalReason};

    // spec: MEDULLA-DURABLE-KERNEL-R874.12
    #[test]
    fn every_legal_transition_applies() {
        for edge in AUDIT_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        assert_eq!(AuditStatus::settled(AuditEvent::Succeed), AuditStatus::Ok);
        assert_eq!(AuditStatus::settled(AuditEvent::Fail), AuditStatus::Error);
        assert_eq!(AuditStatus::settled(AuditEvent::Deny), AuditStatus::Denied);
    }

    // spec: MEDULLA-DURABLE-KERNEL-R874.12
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let machine = &AUDIT_MACHINE;
        let mut illegal = 0;
        for &state in machine.states {
            for &event in machine.events {
                if machine
                    .edges
                    .iter()
                    .any(|e| e.from == state && e.event == event)
                {
                    continue;
                }
                illegal += 1;
                let refused = state.apply(event).unwrap_err();
                assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
                assert_eq!(refused.machine, "AuditMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
            }
        }
        assert_eq!(
            illegal,
            machine.states.len() * machine.events.len() - machine.edges.len()
        );
    }

    // spec: MEDULLA-DURABLE-KERNEL-R874.12
    #[test]
    fn no_writer_sets_status_outside_apply() {
        let source = include_str!("operation_audit.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let writes: Vec<&str> = production
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//") && l.contains(".status = "))
            .collect();
        assert!(writes.is_empty(), "{writes:?}");
        assert!(AuditStatus::parse("closed").is_err());
        assert_eq!(AuditStatus::parse("denied"), Ok(AuditStatus::Denied));
    }
}
