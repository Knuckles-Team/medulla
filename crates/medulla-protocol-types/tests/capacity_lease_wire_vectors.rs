//! MEDULLA-PLACEMENT-R786.7: the wire vectors of the capacity-lease methods, one per method and
//! per refusal code. Each request round-trips through `Method` and each reply through its
//! result body, so a field the Rust types lack or add fails here. The consumer
//! (`ARBORA-CAPACITY-R754`) replays the same file in its own tests.

use medulla_protocol_types::protocol::Method;
use medulla_work_types::native_control::{
    CapacityAcquireResult, CapacityMutationResult, CapacityStatusResult,
};
use serde_json::Value;

const VECTORS: &str = include_str!("vectors/capacity_lease_wire.json");

fn vectors() -> Value {
    serde_json::from_str(VECTORS).expect("the vector file is JSON")
}

fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(value: &Value, label: &str) -> T {
    let typed: T = serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{label}: {e}"));
    assert_eq!(&serde_json::to_value(&typed).unwrap(), value, "{label}");
    typed
}

// spec: MEDULLA-PLACEMENT-R786.7
#[test]
fn every_method_vector_round_trips_through_its_types() {
    let vectors = vectors();
    let methods = vectors["methods"].as_array().expect("methods");
    let mut seen = std::collections::BTreeSet::new();
    for vector in methods {
        let name = vector["name"].as_str().expect("name");
        seen.insert(name);
        let method: Method = round_trip(&vector["request"], name);
        let reply = &vector["reply"];
        match (&method, name) {
            (Method::AcquireCapacity { .. }, "AcquireCapacity") => {
                round_trip::<CapacityAcquireResult>(reply, name);
            }
            (
                Method::StartCapacity { .. }
                | Method::RenewCapacity { .. }
                | Method::ReleaseCapacity { .. }
                | Method::RevokeCapacity { .. },
                _,
            ) => {
                round_trip::<CapacityMutationResult>(reply, name);
            }
            (Method::CapacityStatus { .. }, "CapacityStatus") => {
                round_trip::<CapacityStatusResult>(reply, name);
            }
            other => panic!("vector {name} decoded as {other:?}"),
        }
    }
    let expected = [
        "AcquireCapacity",
        "CapacityStatus",
        "ReleaseCapacity",
        "RenewCapacity",
        "RevokeCapacity",
        "StartCapacity",
    ];
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), expected);
}

// spec: MEDULLA-PLACEMENT-R786.7
#[test]
fn a_queued_acquire_is_accepted_with_a_queue_position() {
    let vectors = vectors();
    let queued = vectors["methods"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|vector| vector["name"] == "AcquireCapacity")
        .map(|vector| round_trip::<CapacityAcquireResult>(&vector["reply"], "acquire"))
        .find(|reply| reply.queue_position.is_some())
        .expect("a queued acquire vector");
    assert_eq!(queued.queue_position, Some(1));
    assert_eq!(
        queued.leases[0].state,
        medulla_mutation_types::capacity_lease::CapacityLeaseState::Queued
    );
}

// spec: MEDULLA-PLACEMENT-R786.7
#[test]
fn every_refusal_code_has_a_vector_and_the_transition_record_names_its_fields() {
    let vectors = vectors();
    let refusals = vectors["refusals"].as_array().expect("refusals");
    let codes: std::collections::BTreeSet<&str> = refusals
        .iter()
        .map(|vector| vector["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes.into_iter().collect::<Vec<_>>(),
        [
            "ACCESS_DENIED",
            "CAPACITY_UNAVAILABLE",
            "PRIORITY_NOT_ALLOWED_FOR_RESOURCE",
            "TRANSITION_REFUSED"
        ]
    );
    for vector in refusals {
        if vector["code"] != "TRANSITION_REFUSED" {
            continue;
        }
        // The text is the `Display` of the typed refusal, and its fields are the wire record.
        let error = vector["error"].as_str().unwrap();
        let detail = error
            .strip_prefix("TRANSITION_REFUSED: ")
            .expect("prefixed");
        let parsed = medulla_types::transition_refused::TransitionRefused::from_detail(detail)
            .expect("the text is the refusal's own detail");
        assert_eq!(parsed.machine, vector["fields"]["machine"]);
        assert_eq!(parsed.from, vector["fields"]["from"]);
        assert_eq!(parsed.event, vector["fields"]["event"]);
        assert_eq!(parsed.reason.as_str(), vector["fields"]["reason"]);
    }
}
