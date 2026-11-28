// MEDULLA-MEDIA-VOCAB-R654: transcript segments carry storage, event and
// narrative time, each with a source and provenance companion.

use medulla_source_types::source_ingestion::SourceRecordProvenance;
use medulla_source_types::source_transcript::segment_clock_properties;
use medulla_types::contract::{Digest256, ResourceId};
use serde_json::json;

fn provenance() -> SourceRecordProvenance {
    SourceRecordProvenance {
        connector: ResourceId::new("audio").unwrap(),
        adapter_kind: ResourceId::new("mcp").unwrap(),
        server: "audio-server".into(),
        tool: "transcribe_audio".into(),
        tool_schema_sha256: Digest256::from_bytes([2; 32]),
        source_uri: "audio://rec1/seg3".into(),
    }
}

// spec: MEDULLA-MEDIA-VOCAB-R654
#[test]
fn segments_carry_three_typed_clocks() {
    let payload = json!({
        "storage_time_ms": 1_700_000_005_000_i64,
        "event_time_ms": 1_700_000_000_000_i64,
        "narrative_time_ms": 42_000_i64,
        "text": "hello",
    });
    let props = segment_clock_properties(payload.as_object().unwrap(), &provenance());
    assert_eq!(props["storage_time"], json!(1_700_000_005_000_i64));
    assert_eq!(props["event_time"], json!(1_700_000_000_000_i64));
    assert_eq!(props["narrative_time"], json!(42_000));
    for clock in ["storage_time", "event_time", "narrative_time"] {
        assert_eq!(
            props[&format!("{clock}_source")],
            json!("audio-server/transcribe_audio")
        );
        assert_eq!(
            props[&format!("{clock}_provenance")],
            json!("audio://rec1/seg3")
        );
    }
}

// spec: MEDULLA-MEDIA-VOCAB-R654
#[test]
fn missing_clock_stays_absent() {
    let payload = json!({"event_time_ms": 7, "storage_time_ms": "not-a-number"});
    let props = segment_clock_properties(payload.as_object().unwrap(), &provenance());
    assert_eq!(props["event_time"], json!(7));
    assert!(!props.contains_key("narrative_time"));
    assert!(!props.contains_key("narrative_time_source"));
    assert!(!props.contains_key("storage_time"));
}
