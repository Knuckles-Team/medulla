//! EH-560: every governed-change rule, both ways.

use super::*;

const NOW: u64 = 1_800_000_000_000;
const PROPOSE: &str = "governance:propose";
const APPROVE: &str = "governance:approve-schema-repair";

fn actor(who: &str, scopes: &[&str], delegated: bool) -> GovernedActor {
    GovernedActor::from_identities(
        who,
        [who],
        delegated,
        scopes.iter().map(|scope| scope.to_string()).collect(),
    )
}

fn proposal() -> GovernedProposal {
    GovernedProposal {
        change_id: "c1".to_string(),
        kind: SCHEMA_REPAIR_KIND.to_string(),
        target: "approved:orders".to_string(),
        digest: "d1".to_string(),
        justification: "drift in orders".to_string(),
        proposal_ttl_ms: 60_000,
        window_ms: 60_000,
    }
}

fn approval(digest: &str) -> GovernedApproval {
    GovernedApproval {
        change_id: "c1".to_string(),
        digest: digest.to_string(),
    }
}

fn consumption(digest: &str) -> GovernedConsumption {
    GovernedConsumption {
        change_id: "c1".to_string(),
        kind: SCHEMA_REPAIR_KIND.to_string(),
        target: "approved:orders".to_string(),
        digest: digest.to_string(),
    }
}

fn proposed() -> GovernedLedger {
    let mut ledger = GovernedLedger::default();
    ledger
        .propose(&actor("svc:au", &[PROPOSE], false), &proposal(), NOW)
        .unwrap();
    ledger
}

// spec: MEDULLA-SRC-AUTH-R003.1
#[test]
fn source_authority_kind_has_exact_scopes() {
    assert_eq!(
        kind_scopes(SOURCE_AUTHORITY_KIND),
        Some((PROPOSE, "governance:approve-source-authority"))
    );
    assert_eq!(
        kind_scopes(SCHEMA_REPAIR_KIND),
        Some((PROPOSE, "governance:approve-schema-repair"))
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R027.1
#[test]
fn proposing_needs_the_proposer_scope_and_a_registered_kind() {
    let mut ledger = GovernedLedger::default();
    assert_eq!(
        ledger.propose(&actor("svc:au", &[], false), &proposal(), NOW),
        Err(GovernedRefusal::NotAuthorized)
    );
    let mut unknown = proposal();
    unknown.kind = "governed.unknown".to_string();
    assert_eq!(
        ledger.propose(&actor("svc:au", &[PROPOSE], false), &unknown, NOW),
        Err(GovernedRefusal::UnknownKind)
    );
    assert!(ledger
        .propose(&actor("svc:au", &[PROPOSE], false), &proposal(), NOW)
        .is_ok());
    assert_eq!(
        ledger.propose(&actor("svc:au", &[PROPOSE], false), &proposal(), NOW),
        Err(GovernedRefusal::Collision)
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R027.1
#[test]
fn the_approver_must_be_a_second_direct_person_with_the_exact_scope() {
    let mut ledger = proposed();
    let same = actor("svc:au", &[APPROVE], false);
    assert_eq!(
        ledger.approve(&same, &approval("d1"), NOW),
        Err(GovernedRefusal::SelfApproval)
    );
    let delegated = actor("usr:bob", &[APPROVE], true);
    assert_eq!(
        ledger.approve(&delegated, &approval("d1"), NOW),
        Err(GovernedRefusal::DelegatedApprover)
    );
    let unscoped = actor("usr:bob", &[PROPOSE, "kg:admin", "*"], false);
    assert_eq!(
        ledger.approve(&unscoped, &approval("d1"), NOW),
        Err(GovernedRefusal::NotAuthorized)
    );
    let bob = actor("usr:bob", &[APPROVE], false);
    assert_eq!(
        ledger.approve(&bob, &approval("d2"), NOW),
        Err(GovernedRefusal::DigestMismatch)
    );
    let approved = ledger.approve(&bob, &approval("d1"), NOW).unwrap();
    assert_eq!(approved.status, GovernedStatus::Approved);
    assert_eq!(
        ledger.approve(&bob, &approval("d1"), NOW),
        Err(GovernedRefusal::Conflict),
        "a replayed approval is refused"
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R027.1
#[test]
fn an_approval_is_consumed_once_for_the_exact_candidate_inside_its_window() {
    let mut ledger = proposed();
    assert_eq!(
        ledger.consume(&consumption("d1"), "engine", NOW),
        Err(GovernedRefusal::Conflict),
        "an unapproved proposal cannot be consumed"
    );
    ledger
        .approve(&actor("usr:bob", &[APPROVE], false), &approval("d1"), NOW)
        .unwrap();
    assert_eq!(
        ledger.consume(&consumption("other"), "engine", NOW),
        Err(GovernedRefusal::DigestMismatch)
    );
    let mut late = ledger.clone();
    assert_eq!(
        late.consume(&consumption("d1"), "engine", NOW + 60_000),
        Err(GovernedRefusal::Conflict),
        "the window is closed"
    );
    assert_eq!(
        ledger
            .consume(&consumption("d1"), "engine", NOW + 1)
            .unwrap()
            .status,
        GovernedStatus::Consumed
    );
    assert_eq!(
        ledger.consume(&consumption("d1"), "engine", NOW + 2),
        Err(GovernedRefusal::Conflict),
        "single use"
    );
    assert!(ledger.audit().verify().is_ok());
    assert_eq!(ledger.audit().entries().count(), 3);
}

// spec: MEDULLA-DURABLE-KERNEL-R027.1
#[test]
fn a_stranger_cannot_revoke_but_the_proposer_or_an_approver_can() {
    let mut ledger = proposed();
    assert_eq!(
        ledger.revoke(&actor("usr:eve", &[PROPOSE], false), "c1", NOW),
        Err(GovernedRefusal::NotAuthorized)
    );
    let mut by_approver = ledger.clone();
    assert!(by_approver
        .revoke(&actor("usr:bob", &[APPROVE], false), "c1", NOW)
        .is_ok());
    let revoked = ledger
        .revoke(&actor("svc:au", &[], false), "c1", NOW)
        .unwrap();
    assert_eq!(revoked.status, GovernedStatus::Revoked);
    assert_eq!(
        ledger.approve(&actor("usr:bob", &[APPROVE], false), &approval("d1"), NOW),
        Err(GovernedRefusal::Conflict)
    );
}

#[test]
fn an_unapproved_proposal_expires() {
    let ledger = proposed();
    assert_eq!(
        ledger.get("c1", NOW + 60_000).unwrap().status,
        GovernedStatus::Expired
    );
    assert_eq!(
        ledger.get("c1", NOW).unwrap().status,
        GovernedStatus::Proposed
    );
}

// MEDULLA-IDENTITY-R720.1 — the governed change machine.
use super::{GovernedEvent, GovernedStatus, GOVERNED_CHANGE_MACHINE};
use medulla_types::transition_refused::{MachineState, TransitionRefusalReason};

fn edge_case(id: &str, from: GovernedStatus, event: GovernedEvent, to: GovernedStatus) {
    let edge = GOVERNED_CHANGE_MACHINE
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
fn illegal_pairs() -> Vec<(GovernedStatus, GovernedEvent)> {
    let machine = &GOVERNED_CHANGE_MACHINE;
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

// spec: MEDULLA-IDENTITY-R720.1

#[test]
fn edge_g01_proposed_approve() {
    edge_case(
        "G01",
        GovernedStatus::Proposed,
        GovernedEvent::Approve,
        GovernedStatus::Approved,
    );
}

// spec: MEDULLA-IDENTITY-R720.1

#[test]
fn edge_g02_proposed_revoke() {
    edge_case(
        "G02",
        GovernedStatus::Proposed,
        GovernedEvent::Revoke,
        GovernedStatus::Revoked,
    );
}

// spec: MEDULLA-IDENTITY-R720.1

#[test]
fn edge_g03_proposed_lapse() {
    edge_case(
        "G03",
        GovernedStatus::Proposed,
        GovernedEvent::Lapse,
        GovernedStatus::Expired,
    );
}

// spec: MEDULLA-IDENTITY-R720.1

#[test]
fn edge_g04_approved_consume() {
    edge_case(
        "G04",
        GovernedStatus::Approved,
        GovernedEvent::Consume,
        GovernedStatus::Consumed,
    );
}

// spec: MEDULLA-IDENTITY-R720.1

#[test]
fn edge_g05_approved_revoke() {
    edge_case(
        "G05",
        GovernedStatus::Approved,
        GovernedEvent::Revoke,
        GovernedStatus::Revoked,
    );
}

// spec: MEDULLA-IDENTITY-R720.1

#[test]
fn edge_g06_approved_lapse() {
    edge_case(
        "G06",
        GovernedStatus::Approved,
        GovernedEvent::Lapse,
        GovernedStatus::Expired,
    );
}

// spec: MEDULLA-IDENTITY-R720.1

#[test]
fn every_illegal_pair_is_refused() {
    let machine = &GOVERNED_CHANGE_MACHINE;
    let pairs = illegal_pairs();
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

// spec: MEDULLA-IDENTITY-R720.1

#[test]
fn refusal_carries_machine_from_event() {
    let refused = GovernedStatus::Proposed
        .apply(GovernedEvent::Consume, |_| true)
        .unwrap_err();
    assert_eq!(refused.machine, "GovernedChangeMachine");
    assert_eq!(refused.from, GovernedStatus::Proposed.stored());
    assert_eq!(refused.event, "Consume");
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

// spec: MEDULLA-IDENTITY-R720.1

#[test]
fn no_writer_sets_state_outside_apply() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/governed_change.rs");
    let source = std::fs::read_to_string(path).expect("writer source");
    let writes = direct_state_writes(&source, "status", "GovernedStatus");
    assert!(
        writes.is_empty(),
        "direct state writes in src/governed_change.rs: {writes:?}"
    );
}

// spec: MEDULLA-IDENTITY-R720.1

#[test]
fn every_transition_and_refusal_is_covered() {
    let machine = &GOVERNED_CHANGE_MACHINE;
    assert_eq!(machine.edges.len(), 6);
    assert_eq!(machine.states.len(), 5);
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
