//! MEDULLA-PLACEMENT-R751.761: the leader lease record and its state machine.

use medulla_jobs::leader_lease::{
    contend, LeaseCell, LeaseEvent, LeaseRecord, LeaseState, MemoryLeaseCell, LEASE_MACHINE,
};
use medulla_jobs::schedule::LEADER_FENCE_EXPIRED;
use medulla_types::transition_refused::TransitionRefusalReason;

// spec: MEDULLA-PLACEMENT-R751.761
#[test]
fn cas_on_revision_admits_one_holder_and_every_illegal_transition_is_refused() {
    // Two engines read the same revision; exactly one compare-and-set wins.
    let cell = MemoryLeaseCell::default();
    let seen = cell.read();
    let a = seen.apply(LeaseEvent::Acquire, "host-a", 0, 100).unwrap();
    let b = seen.apply(LeaseEvent::Acquire, "host-b", 0, 100).unwrap();
    assert!(cell.compare_and_set(seen.revision, &a));
    assert!(
        !cell.compare_and_set(seen.revision, &b),
        "stale revision loses"
    );
    assert_eq!(cell.read().holder.as_deref(), Some("host-a"));

    // Contention through the full path: the live holder keeps it, the rival gets none.
    let fence_a = contend(&cell, "host-a", 10, 100).unwrap().unwrap();
    assert_eq!(contend(&cell, "host-b", 20, 100).unwrap(), None);
    let leadership = cell.read().leadership();
    assert!(leadership.authorize("host-a", fence_a, 30).is_ok());
    assert_eq!(
        leadership
            .authorize("host-b", fence_a, 30)
            .unwrap_err()
            .code,
        LEADER_FENCE_EXPIRED
    );
    // After the lease lapses the rival takes over with a higher fence and the old one is fenced.
    let fence_b = contend(&cell, "host-b", 500, 100).unwrap().unwrap();
    assert!(fence_b > fence_a);
    assert_eq!(cell.read().state, LeaseState::Held);
    assert!(cell
        .read()
        .leadership()
        .authorize("host-a", fence_a, 510)
        .is_err());

    // Every legal edge fires; every other (state, event) pair is refused with the typed error.
    let free = LeaseRecord::default();
    let held = free.apply(LeaseEvent::Acquire, "h", 0, 100).unwrap();
    assert_eq!(
        held.apply(LeaseEvent::Renew, "h", 50, 100)
            .unwrap()
            .expires_ms,
        150
    );
    let expired = held.apply(LeaseEvent::Expire, "h", 100, 100).unwrap();
    assert_eq!(expired.state, LeaseState::Expired);
    assert_eq!(
        held.apply(LeaseEvent::Release, "h", 10, 100).unwrap().state,
        LeaseState::Free
    );
    assert_eq!(
        expired
            .apply(LeaseEvent::Acquire, "i", 200, 100)
            .unwrap()
            .holder
            .as_deref(),
        Some("i")
    );
    let refused = |record: &LeaseRecord, event, host: &str, now| {
        record.apply(event, host, now, 100).unwrap_err()
    };
    for (record, event) in [
        (&free, LeaseEvent::Renew),
        (&free, LeaseEvent::Expire),
        (&free, LeaseEvent::Release),
        (&held, LeaseEvent::Acquire),
        (&expired, LeaseEvent::Renew),
        (&expired, LeaseEvent::Expire),
        (&expired, LeaseEvent::Release),
    ] {
        let error = refused(record, event, "h", 10);
        assert_eq!(error.machine, LEASE_MACHINE.name);
        assert_eq!(error.reason, TransitionRefusalReason::IllegalTransition);
        assert_eq!(
            error.from,
            medulla_types::transition_refused::MachineState::stored(record.state)
        );
    }
    // Guards: another host cannot renew or release; a live lease cannot expire; a lapsed
    // lease cannot renew.
    for (event, host, now) in [
        (LeaseEvent::Renew, "other", 10),
        (LeaseEvent::Release, "other", 10),
        (LeaseEvent::Expire, "h", 10),
        (LeaseEvent::Renew, "h", 100),
    ] {
        assert_eq!(
            refused(&held, event, host, now).reason,
            TransitionRefusalReason::GuardFailed
        );
    }
}
