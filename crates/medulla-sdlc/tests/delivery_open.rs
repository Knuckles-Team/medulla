//! MEDULLA-SDLC-R019.3 and R019.4: one delivery per dispatched slice, and the shadow of an
//! unprompted slice in `development` mode.

use medulla_sdlc::lifecycle::delivery::open::{
    delivery_id, open_delivery, shadow_or_dispatch, DispatchOutcome, OpenDelivery,
};
use medulla_source_types::delivery::{AssignmentState, MandateStrength};
use medulla_source_types::lifecycle::{ProjectMode, ProposalOrigin};

fn input<'a>(
    rows: &'a [String],
    origin: ProposalOrigin,
    mode: ProjectMode,
) -> OpenDelivery<'a> {
    OpenDelivery {
        tenant: "t1",
        repo: "medulla",
        slice_root: "MEDULLA-SDLC-R019",
        scope_rows: rows,
        dispatch_decision_id: "dec-1",
        policy_version: "p1",
        actor: "person-1",
        origin,
        mode,
        run_trace_id: "run-1",
        now_ms: 42,
    }
}

// spec: MEDULLA-SDLC-R019.3
#[test]
fn one_delivery_per_slice_and_idempotent_reopen() {
    let rows = vec!["B-R2".to_string(), "A-R1".to_string(), "A-R1".to_string()];
    let requested = open_delivery(
        &input(&rows, ProposalOrigin::Requested, ProjectMode::Development),
        None,
    )
    .expect("opens");
    assert_eq!(requested.id, delivery_id("t1", "dec-1", "MEDULLA-SDLC-R019"));
    assert!(requested.id.starts_with("delivery:") && requested.id.len() == 9 + 32);
    assert_eq!(requested.scope_rows, ["A-R1", "B-R2"]);
    assert_eq!(requested.mandate.strength, MandateStrength::Explicit);
    assert_eq!(requested.mandate.basis, "requested by person-1");
    assert_eq!(requested.assignments.len(), 1);
    assert_eq!(requested.assignments[0].state, AssignmentState::Pending);
    assert!(requested.assignments[0].id.starts_with("assignment:"));
    assert_eq!((requested.seq, requested.created_at), (0, 42));
    let inferred = open_delivery(
        &input(&rows, ProposalOrigin::Unprompted, ProjectMode::Evolving),
        None,
    )
    .expect("opens");
    assert_eq!(inferred.mandate.strength, MandateStrength::Inferred);
    assert_eq!(inferred.mandate.basis, "inferred from dispatch decision dec-1");
    // A second call with the same key returns the first delivery unchanged.
    let wider = vec!["A-R1".to_string(), "B-R2".to_string(), "C-R3".to_string()];
    let again = open_delivery(
        &input(&wider, ProposalOrigin::Requested, ProjectMode::Development),
        Some(requested.clone()),
    )
    .expect("reopens");
    assert_eq!(again, requested);
    let empty: Vec<String> = Vec::new();
    let refused = open_delivery(
        &input(&empty, ProposalOrigin::Requested, ProjectMode::Development),
        None,
    )
    .unwrap_err();
    assert_eq!(refused.code(), "DELIVERY_SCOPE_EMPTY");
    let reference = open_delivery(
        &input(&rows, ProposalOrigin::Requested, ProjectMode::Reference),
        None,
    )
    .unwrap_err();
    assert_eq!(reference.code(), "MODE_REFERENCE_NO_PROPOSALS");
}

// spec: MEDULLA-SDLC-R019.4
#[test]
fn unprompted_slice_is_recorded_not_run_in_development_mode() {
    let rows = vec!["A-R1".to_string()];
    let digests = vec!["sha256:b".to_string(), "sha256:a".to_string()];
    let shadow = shadow_or_dispatch(
        &input(&rows, ProposalOrigin::Unprompted, ProjectMode::Development),
        None,
        &digests,
    )
    .expect("decides");
    match shadow {
        DispatchOutcome::WouldDispatch(record) => {
            assert_eq!(record.rows, ["A-R1"]);
            assert_eq!(record.evidence_digests, ["sha256:a", "sha256:b"]);
        }
        other => panic!("expected a shadow, got {other:?}"),
    }
    for (origin, mode) in [
        (ProposalOrigin::Unprompted, ProjectMode::Evolving),
        (ProposalOrigin::Requested, ProjectMode::Development),
        (ProposalOrigin::Requested, ProjectMode::Evolving),
    ] {
        let outcome = shadow_or_dispatch(&input(&rows, origin, mode), None, &digests)
            .expect("decides");
        assert!(matches!(outcome, DispatchOutcome::Dispatched(_)));
    }
    let reference = shadow_or_dispatch(
        &input(&rows, ProposalOrigin::Unprompted, ProjectMode::Reference),
        None,
        &digests,
    )
    .unwrap_err();
    assert_eq!(reference.code(), "MODE_REFERENCE_NO_PROPOSALS");
}
