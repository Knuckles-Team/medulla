//! Ingest capability types (MEDULLA-REPO-INGEST-R002.650).

use medulla_types::contract::Digest256;
use medulla_types::ingest_capabilities::{
    ConformanceReport, DeadLetterItem, FragmentAddress, Supersession,
    INGEST_CAPABILITIES_SCHEMA_VERSION,
};
use serde::de::DeserializeOwned;
use serde::Serialize;

fn contract_digest<T: Serialize>(value: &T) -> Digest256 {
    let bytes = serde_json::to_vec(value).expect("encode");
    Digest256::framed(b"ingest-capability", &[&bytes]).expect("frame")
}

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) {
    let json = serde_json::to_value(value).expect("encode");
    assert_eq!(json["schema_version"], INGEST_CAPABILITIES_SCHEMA_VERSION);
    let decoded: T = serde_json::from_value(json.clone()).expect("decode");
    assert_eq!(&decoded, value);
    let mut unknown = json;
    unknown["extra"] = serde_json::json!(1);
    assert!(serde_json::from_value::<T>(unknown).is_err());
}

// spec: MEDULLA-REPO-INGEST-R002.650
#[test]
fn types_round_trip_and_contract_digest_changes_on_field_change() {
    let fragment = FragmentAddress {
        schema_version: 1,
        content_digest: Digest256::sha256(b"doc"),
        span_start: 4,
        span_end: 9,
    };
    let supersession = Supersession {
        schema_version: 1,
        old_fact: "fact:old".into(),
        new_fact: "fact:new".into(),
        envelope_id: "envelope:1".into(),
    };
    let dead = DeadLetterItem {
        schema_version: 1,
        item_id: "item:1".into(),
        queue: "ingest".into(),
        reason: "attempts_exhausted".into(),
        attempts: 5,
        last_error: "timeout".into(),
        parked_at_ms: 1_000,
        payload_digest: Digest256::sha256(b"payload"),
    };
    let report = ConformanceReport {
        schema_version: 1,
        process_id: "process:1".into(),
        traces: 10,
        fitness_milli: 950,
        missing_tokens: 1,
        remaining_tokens: 0,
    };
    round_trip(&fragment);
    round_trip(&supersession);
    round_trip(&dead);
    round_trip(&report);

    let mut changed_fragment = fragment.clone();
    changed_fragment.span_end = 10;
    assert_ne!(
        contract_digest(&fragment),
        contract_digest(&changed_fragment)
    );
    let mut changed_supersession = supersession.clone();
    changed_supersession.new_fact = "fact:newer".into();
    assert_ne!(
        contract_digest(&supersession),
        contract_digest(&changed_supersession)
    );
    let mut changed_dead = dead.clone();
    changed_dead.attempts = 6;
    assert_ne!(contract_digest(&dead), contract_digest(&changed_dead));
    let mut changed_report = report.clone();
    changed_report.fitness_milli = 951;
    assert_ne!(contract_digest(&report), contract_digest(&changed_report));
}
