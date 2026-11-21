//! The in-flight request registry and its guard.

use medulla_observe::machine::request::{RequestEvent, RequestState};
use medulla_observe::registry::requests::{self, EnterRequest, RequestRegistry};

fn enter<'a>(registry: &RequestRegistry, id: u64, principal: &'a str) -> requests::RequestGuard {
    registry.enter(EnterRequest {
        request_id: id,
        trace_id: Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        method: "Introspect",
        graph: "g",
        principal,
    })
}

// spec: MEDULLA-OBSERVE-R006.2
#[test]
fn a_request_is_listed_until_its_guard_drops() {
    let registry = RequestRegistry::new(8);
    let guard = enter(&registry, 7, "alice");
    let rows = registry.snapshot();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].request_id, 7);
    assert_eq!(rows[0].phase, RequestState::Received);
    assert_eq!(rows[0].method, "Introspect");
    assert_eq!(rows[0].graph, "g");
    assert!(rows[0].waiting_for.is_none());
    guard.apply(RequestEvent::Admit).unwrap();
    assert_eq!(registry.snapshot()[0].phase, RequestState::Admitted);
    drop(guard);
    assert!(registry.is_empty());
}

// spec: MEDULLA-OBSERVE-R006.2
#[test]
fn the_principal_is_a_keyed_hash_never_the_name() {
    let registry = RequestRegistry::new(8);
    let a = enter(&registry, 1, "alice");
    let b = enter(&registry, 2, "alice");
    let c = enter(&registry, 3, "bob");
    let rows = registry.snapshot();
    assert!(rows[0].principal_ref.starts_with("p:"));
    assert!(!rows[0].principal_ref.contains("alice"));
    assert_eq!(rows[0].principal_ref, rows[1].principal_ref);
    assert_ne!(rows[0].principal_ref, rows[2].principal_ref);
    drop((a, b, c));
}

// spec: MEDULLA-OBSERVE-R006.2
#[test]
fn a_refused_transition_leaves_the_phase_and_the_registry_bounded() {
    let registry = RequestRegistry::new(1);
    let guard = enter(&registry, 1, "alice");
    assert!(guard.apply(RequestEvent::Complete).is_err());
    assert_eq!(registry.snapshot()[0].phase, RequestState::Received);
    // A full registry does not list the next request, counts it, and still runs its machine.
    let extra = enter(&registry, 2, "bob");
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.dropped_total(), 1);
    assert_eq!(extra.apply(RequestEvent::Admit).unwrap(), RequestState::Admitted);
}

// spec: MEDULLA-OBSERVE-R006.2
#[tokio::test]
async fn a_waiting_request_names_its_lock_and_returns_to_handling() {
    let registry = RequestRegistry::new(8);
    let guard = enter(&registry, 1, "alice");
    for event in [RequestEvent::Admit, RequestEvent::Authorize, RequestEvent::Handle] {
        guard.apply(event).unwrap();
    }
    requests::scope(guard.handle(), async {
        let handle = requests::current().expect("inside the scope");
        handle.begin_wait("lock_graph:g");
        let row = &registry.snapshot()[0];
        assert_eq!(row.phase, RequestState::WaitingLock);
        assert_eq!(row.waiting_for.as_deref(), Some("lock_graph:g"));
        handle.end_wait();
        let row = &registry.snapshot()[0];
        assert_eq!(row.phase, RequestState::Handling);
        assert!(row.waiting_for.is_none());
    })
    .await;
    assert!(requests::current().is_none());
}
