use super::*;

fn issue() -> IssueControlLeaseRequest {
    let mut grant = Map::new();
    grant.insert("tool_ids".into(), serde_json::json!(["click"]));
    IssueControlLeaseRequest {
        pending_call_registration: None,
        tenant: "tenant-a".into(),
        lease_id: "browserlease_1".into(),
        kind: "browser.control".into(),
        grant,
        issued_at_ms: 1_000,
        expires_at_ms: 301_000,
        hard_expires_at_ms: 901_000,
        idempotency_key: "issue-1".into(),
    }
}

#[test]
fn every_status_round_trips_through_its_stored_text() {
    for (text, status) in STORED_STATUS {
        assert_eq!(status.as_stored(), text);
        assert_eq!(ControlLeaseStatus::from_stored(text), Some(status));
    }
    assert_eq!(
        ControlLeaseTarget::Revoked.status(),
        ControlLeaseStatus::Revoked
    );
    assert_eq!(
        ControlLeaseTarget::Expired.status(),
        ControlLeaseStatus::Expired
    );
    assert_eq!(ControlLeaseStatus::from_stored("renewed"), None);
}

#[test]
fn an_issued_row_projects_back_to_the_request_as_revision_one() {
    let request = issue();
    request.validate().unwrap();
    let row = request.row();
    assert!(is_tenant_control_lease(&row, "tenant-a"));
    assert!(!is_tenant_control_lease(&row, "tenant-b"));
    let view = ControlLeaseView::from_row("browserlease_1", &row).unwrap();
    assert_eq!(view.status, ControlLeaseStatus::Active);
    assert_eq!(view.revision, 1);
    assert_eq!(view.grant, request.grant);
    assert_eq!(
        (
            view.issued_at_ms,
            view.expires_at_ms,
            view.hard_expires_at_ms
        ),
        (1_000, 301_000, 901_000)
    );
}

#[test]
fn issue_refuses_disordered_or_unbounded_timing_and_oversized_grants() {
    let mut late = issue();
    late.expires_at_ms = late.hard_expires_at_ms + 1;
    assert!(late.validate().is_err());
    let mut unissued = issue();
    unissued.issued_at_ms = 0;
    assert!(unissued.validate().is_err());
    let mut forever = issue();
    forever.hard_expires_at_ms = forever.issued_at_ms + MAX_CONTROL_LEASE_SPAN_MS + 1;
    assert!(forever.validate().is_err());
    let mut heavy = issue();
    heavy.grant.insert(
        "blob".into(),
        "x".repeat(MAX_CONTROL_LEASE_GRANT_BYTES).into(),
    );
    assert!(heavy.validate().is_err());
    let mut anonymous = issue();
    anonymous.tenant = " ".into();
    assert!(anonymous.validate().is_err());
}

#[test]
fn transition_and_get_requests_are_bounded() {
    let transition = TransitionControlLeaseRequest {
        pending_call_consumption: None,
        tenant: "tenant-a".into(),
        lease_id: "browserlease_1".into(),
        expected_revision: 1,
        to: ControlLeaseTarget::Revoked,
        idempotency_key: String::new(),
    };
    assert!(transition.validate().is_err());
    assert!(validate_control_lease_get("tenant-a", "").is_err());
    validate_control_lease_get("tenant-a", "browserlease_1").unwrap();
}

#[test]
fn only_the_declared_edges_are_legal() {
    use ControlLeaseStatus::{Active, Consumed, Expired, Revoked};
    use ControlLeaseTarget as To;
    for from in [Active, Consumed, Revoked, Expired] {
        for to in [To::Consumed, To::Revoked, To::Expired] {
            let legal = matches!(
                (from, to),
                (Active, _) | (Consumed, To::Revoked | To::Expired)
            );
            let moved = from.apply(to.event(), |_| true);
            assert_eq!(moved.is_ok(), legal, "{from:?} -> {to:?}");
            if legal {
                assert_eq!(moved, Ok(to.status()));
            }
        }
    }
}

#[test]
fn a_lease_listing_selects_the_tenants_leases_by_kind_status_and_grant() {
    use medulla_types::{keyset_page::KeysetListing, keyset_page::KeysetScan};
    let mut approval = issue();
    approval.kind = "action.approval".into();
    approval
        .grant
        .insert("request_digest".into(), serde_json::json!("d-1"));
    let rows = [
        ("a-1", approval.row()),
        ("b-1", issue().row()),
        (
            "c-1",
            IssueControlLeaseRequest {
                pending_call_registration: None,
                tenant: "tenant-b".into(),
                ..approval.clone()
            }
            .row(),
        ),
    ];
    let request = ListControlLeasesRequest {
        tenant: "tenant-a".into(),
        kind: Some("action.approval".into()),
        status: Some(ControlLeaseStatus::Active),
        grant_match: serde_json::json!({"request_digest": "d-1"})
            .as_object()
            .cloned(),
        cursor: None,
        limit: 10,
    };
    request.validate().unwrap();
    let mut scan = KeysetScan::new(&request);
    for (id, row) in &rows {
        assert!(scan.admits_another_row());
        scan.consume(id, 10, row).unwrap();
    }
    let page = scan.finish();
    let ids: Vec<&str> = page
        .items
        .iter()
        .map(|lease| lease.lease_id.as_str())
        .collect();
    assert_eq!(ids, ["a-1"]);
    assert_eq!(page.next_cursor, None);
    assert_eq!(request.limit(), 10);
    assert!(ListControlLeasesRequest {
        limit: 0,
        ..request
    }
    .validate()
    .is_err());
}

#[test]
fn the_elevation_kind_cannot_be_issued_as_a_generic_control_lease() {
    let mut request = issue();
    request.kind = RBAC_ELEVATION_KIND.to_string();
    let error = request.validate().unwrap_err();
    assert!(error.contains("reserved"), "{error}");
}

/// EH-560: a governed kind is issued only through `GovernedChange`; the
/// generic lease path refuses the whole `governed.` namespace, while an
/// ordinary kind next to it is still accepted.
// spec: MEDULLA-DURABLE-KERNEL-R027.1
#[test]
fn a_governed_kind_cannot_be_issued_as_a_generic_control_lease() {
    for kind in [
        medulla_mutation_types::governed_change::SCHEMA_REPAIR_KIND,
        "governed.anything-new",
    ] {
        let mut request = issue();
        request.kind = kind.to_string();
        let error = request.validate().unwrap_err();
        assert!(error.contains("reserved"), "{kind}: {error}");
    }
    let mut ordinary = issue();
    ordinary.kind = "action.approval".to_string();
    assert!(ordinary.validate().is_ok());
}

// spec: MEDULLA-DURABLE-KERNEL-R039.3
#[test]
fn pending_registration_requires_reserved_kind_and_bounded_preimage() {
    let mut request = issue();
    let registration = PendingCallRegistration {
        plan: None,
        work_item: PendingCallFence {
            work_item_id: "work-a".into(),
            expected_revision: 1,
            expected_metadata: Map::new(),
        },
        expected_lease:
            medulla_result_types::epistemic_operations_ext::CasWorkItemMetadataLeaseFence {
                worker_ref: "worker-a".into(),
                lease_epoch: 1,
                fencing_token: 1,
            },
    };
    request.kind = PENDING_CALL_KIND.into();
    assert!(request.validate().is_err());
    request.pending_call_registration = Some(registration.clone());
    assert!(request.validate().is_ok());
    request.kind = "browser.control".into();
    assert!(request.validate().is_err());
    let mut invalid = registration.work_item;
    invalid.expected_revision = 0;
    assert!(invalid.validate().is_err());
    invalid.expected_revision = 1;
    invalid.expected_metadata.insert(
        "large".into(),
        "x".repeat(MAX_CONTROL_LEASE_GRANT_BYTES).into(),
    );
    assert!(invalid.validate().is_err());
}

// spec: MEDULLA-DURABLE-KERNEL-R039.3
#[test]
fn ordinary_lease_wire_shape_remains_compatible_without_pending_fields() {
    let request = issue();
    let value = serde_json::to_value(&request).unwrap();
    assert!(value.get("pending_call_registration").is_none());
    assert_eq!(
        serde_json::from_value::<IssueControlLeaseRequest>(value).unwrap(),
        request
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R039.4, MEDULLA-DURABLE-KERNEL-R039.4.1
#[test]
fn paired_plan_preimage_is_bounded_and_cannot_name_another_pending_lease() {
    let mut plan = PendingPlanPreimage {
        lease_id: "configured-plan".into(),
        expected_revision: 1,
        expected_kind: "configured.kind".into(),
        expected_grant: Map::new(),
    };
    assert!(plan.validate().is_ok());
    plan.expected_kind = PENDING_CALL_KIND.into();
    assert!(plan.validate().is_err());
    plan.expected_kind = "configured.kind".into();
    plan.expected_revision = 0;
    assert!(plan.validate().is_err());
    plan.expected_revision = 1;
    plan.expected_grant.insert(
        "large".into(),
        "x".repeat(MAX_CONTROL_LEASE_GRANT_BYTES).into(),
    );
    assert!(plan.validate().is_err());
}

// MEDULLA-PLACEMENT-R782.3 — the control lease machine.
use super::{ControlLeaseEvent, ControlLeaseStatus, CONTROL_LEASE_MACHINE};
use medulla_types::transition_refused::{MachineState, TransitionRefusalReason};

fn edge_case(id: &str, from: ControlLeaseStatus, event: ControlLeaseEvent, to: ControlLeaseStatus) {
    let edge = CONTROL_LEASE_MACHINE
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
fn illegal_pairs() -> Vec<(ControlLeaseStatus, ControlLeaseEvent)> {
    let machine = &CONTROL_LEASE_MACHINE;
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

// spec: MEDULLA-PLACEMENT-R782.3

#[test]
fn edge_l01_active_consume() {
    edge_case(
        "L01",
        ControlLeaseStatus::Active,
        ControlLeaseEvent::Consume,
        ControlLeaseStatus::Consumed,
    );
}

// spec: MEDULLA-PLACEMENT-R782.3

#[test]
fn edge_l02_active_revoke() {
    edge_case(
        "L02",
        ControlLeaseStatus::Active,
        ControlLeaseEvent::Revoke,
        ControlLeaseStatus::Revoked,
    );
}

// spec: MEDULLA-PLACEMENT-R782.3

#[test]
fn edge_l03_active_reclaim() {
    edge_case(
        "L03",
        ControlLeaseStatus::Active,
        ControlLeaseEvent::Reclaim,
        ControlLeaseStatus::Expired,
    );
}

// spec: MEDULLA-PLACEMENT-R782.3

#[test]
fn edge_l04_consumed_revoke() {
    edge_case(
        "L04",
        ControlLeaseStatus::Consumed,
        ControlLeaseEvent::Revoke,
        ControlLeaseStatus::Revoked,
    );
}

// spec: MEDULLA-PLACEMENT-R782.3

#[test]
fn edge_l05_consumed_reclaim() {
    edge_case(
        "L05",
        ControlLeaseStatus::Consumed,
        ControlLeaseEvent::Reclaim,
        ControlLeaseStatus::Expired,
    );
}

// spec: MEDULLA-PLACEMENT-R782.3

#[test]
fn every_illegal_pair_is_refused() {
    let machine = &CONTROL_LEASE_MACHINE;
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

// spec: MEDULLA-PLACEMENT-R782.3

#[test]
fn refusal_carries_machine_from_event() {
    let refused = ControlLeaseStatus::Consumed
        .apply(ControlLeaseEvent::Consume, |_| true)
        .unwrap_err();
    assert_eq!(refused.machine, "ControlLeaseMachine");
    assert_eq!(refused.from, ControlLeaseStatus::Consumed.stored());
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

// spec: MEDULLA-PLACEMENT-R782.3

#[test]
fn no_writer_sets_state_outside_apply() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../medulla-work-store/src/redb_store/work_item/control_lease.rs"
    );
    let source = std::fs::read_to_string(path).expect("writer source");
    let writes = direct_state_writes(&source, "status", "ControlLeaseStatus");
    assert!(writes.is_empty(), "direct state writes in ../medulla-work-store/src/redb_store/work_item/control_lease.rs: {writes:?}");
}

// spec: MEDULLA-PLACEMENT-R782.3

#[test]
fn every_transition_and_refusal_is_covered() {
    let machine = &CONTROL_LEASE_MACHINE;
    assert_eq!(machine.edges.len(), 5);
    assert_eq!(machine.states.len(), 4);
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
