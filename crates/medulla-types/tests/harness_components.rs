//! Versioned harness components as graph facts (MEDULLA-ORCH-VOCAB-R801): the
//! serde shape of one version, the write-time behavior (no supersession; a
//! version becomes current only by a champion pointer move, R801.7), the typed
//! refusals and the champion-chain evidence.

use medulla_work_types::{
    orchestration::register_version, orchestration::version_history,
    orchestration::HarnessComponentKind, orchestration::HarnessComponentRefusal,
    orchestration::HarnessComponentVersion,
};

/// One valid version of the given component, with the content reference and
/// its digest kept in agreement and an evidence run on file.
fn version(
    component_id: &str,
    kind: HarnessComponentKind,
    content_ref: &str,
    valid_from: u64,
) -> HarnessComponentVersion {
    HarnessComponentVersion {
        component_id: component_id.to_string(),
        kind,
        content_ref: content_ref.to_string(),
        content_digest: HarnessComponentVersion::content_digest(content_ref),
        valid_from,
        confidence: 0.8,
        evidence_run_id: format!("run-{component_id}-{valid_from}"),
        applied_to_id: Some("agent:demo".to_string()),
    }
}

// spec: MEDULLA-ORCH-VOCAB-R801.1
#[test]
fn component_version_round_trips() {
    let version = version(
        "comp:prompt",
        HarnessComponentKind::SystemPrompt,
        "blob:abc",
        10,
    );
    let json = serde_json::to_string(&version).unwrap();
    let back: HarnessComponentVersion = serde_json::from_str(&json).unwrap();
    assert_eq!(back, version);

    // Every field survives the trip by name.
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    for field in [
        "component_id",
        "kind",
        "content_ref",
        "content_digest",
        "valid_from",
        "confidence",
        "evidence_run_id",
        "applied_to_id",
    ] {
        assert!(value.get(field).is_some(), "field {field} missing");
    }
    assert_eq!(value["kind"], "system_prompt");

    // The kind is the closed set: a name outside it does not decode.
    let unknown_kind = json.replace("\"system_prompt\"", "\"widget\"");
    assert!(serde_json::from_str::<HarnessComponentVersion>(&unknown_kind).is_err());

    // The shape is strict: an unknown field is refused, not ignored.
    let unknown_field = json.replace('}', ",\"unknown_field\":1}");
    assert!(serde_json::from_str::<HarnessComponentVersion>(&unknown_field).is_err());
}

// spec: MEDULLA-ORCH-VOCAB-R801.6
#[test]
fn two_versions_are_admitted_and_stay_readable_with_no_window_close() {
    let mut versions = Vec::new();
    let v1 = version(
        "comp:prompt",
        HarnessComponentKind::SystemPrompt,
        "blob:v1",
        10,
    );
    register_version(&mut versions, v1.clone()).unwrap();
    let v2 = version(
        "comp:prompt",
        HarnessComponentKind::SystemPrompt,
        "blob:v2",
        20,
    );
    register_version(&mut versions, v2.clone()).unwrap();

    // Admission stamps nothing on the older version: both are stored exactly as
    // written, and neither names the other.
    assert_eq!(versions, vec![v1.clone(), v2.clone()]);
    let json = serde_json::to_value(&versions[0]).unwrap();
    assert!(json.get("valid_to").is_none());
    assert!(json.get("supersedes_id").is_none());

    // The retired fields do not decode (hard cutover, no strip-on-open).
    let mut stale = serde_json::to_value(&v1).unwrap();
    stale["valid_to"] = serde_json::json!(20);
    assert!(serde_json::from_value::<HarnessComponentVersion>(stale).is_err());
    let mut stale = serde_json::to_value(&v1).unwrap();
    stale["supersedes_id"] = serde_json::json!("harness-component:x:y");
    assert!(serde_json::from_value::<HarnessComponentVersion>(stale).is_err());

    // Replaying the same write is an idempotent no-op.
    register_version(&mut versions, v2.clone()).unwrap();
    assert_eq!(versions.len(), 2);
}

// spec: MEDULLA-ORCH-VOCAB-R801.4
#[test]
fn evidence_kind_confidence_and_immutability_refusals() {
    let mut versions = Vec::new();

    // A version without an evidence run refuses.
    let mut no_evidence = version(
        "comp:prompt",
        HarnessComponentKind::SystemPrompt,
        "blob:v1",
        10,
    );
    no_evidence.evidence_run_id = String::new();
    assert_eq!(
        register_version(&mut versions, no_evidence),
        Err(HarnessComponentRefusal::VersionWithoutEvidence)
    );
    assert!(versions.is_empty());

    // An unknown kind refuses by name (the closed set is the model).
    let unknown_kind = HarnessComponentRefusal::KindUnknown;
    assert_eq!(HarnessComponentKind::from_wire("widget"), Err(unknown_kind));

    // A confidence outside 0 to 1 refuses.
    let mut low_confidence = version(
        "comp:prompt",
        HarnessComponentKind::SystemPrompt,
        "blob:v1",
        10,
    );
    low_confidence.confidence = 1.01;
    assert_eq!(
        register_version(&mut versions, low_confidence),
        Err(HarnessComponentRefusal::ConfidenceInvalid)
    );
    let mut high_confidence = version(
        "comp:prompt",
        HarnessComponentKind::SystemPrompt,
        "blob:v1",
        10,
    );
    high_confidence.confidence = -0.1;
    assert_eq!(
        register_version(&mut versions, high_confidence),
        Err(HarnessComponentRefusal::ConfidenceInvalid)
    );

    // A published version never mutates: same identity, other metadata
    // refuses; the byte-identical write is a no-op.
    let base = version(
        "comp:prompt",
        HarnessComponentKind::SystemPrompt,
        "blob:v1",
        10,
    );
    register_version(&mut versions, base.clone()).unwrap();
    let mut mutated = base.clone();
    mutated.confidence = 0.9;
    assert_eq!(
        register_version(&mut versions, mutated),
        Err(HarnessComponentRefusal::VersionImmutable)
    );
    register_version(&mut versions, base.clone()).unwrap();
    assert_eq!(versions.len(), 1);
}

// spec: MEDULLA-ORCH-VOCAB-R801.5
#[test]
fn version_history_cites_evidence_and_digests() {
    let mut versions = Vec::new();

    let v1 = version("comp:memory", HarnessComponentKind::Memory, "blob:m1", 10);
    let mut v2 = version("comp:memory", HarnessComponentKind::Memory, "blob:m2", 20);
    v2.confidence = 0.95;
    let mut v3 = version("comp:memory", HarnessComponentKind::Memory, "blob:m3", 30);
    v3.confidence = 0.99;
    register_version(&mut versions, v1.clone()).unwrap();
    register_version(&mut versions, v2.clone()).unwrap();
    register_version(&mut versions, v3.clone()).unwrap();

    // The champions are the `Activated` targets of the subject pointer, oldest
    // first. v2 was never promoted, so it is not in the chain.
    let champions = vec![v1.node_id(), v3.node_id()];
    let history = version_history("comp:memory", &versions, &champions);
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].version.node_id(), v3.node_id());
    assert_eq!(history[1].version.node_id(), v1.node_id());

    // Every entry carries its evidence run id, its confidence and its valid
    // time, and the digest recomputes from the content reference.
    for (entry, expected) in history.iter().zip([&v3, &v1]) {
        assert_eq!(entry.version.evidence_run_id, expected.evidence_run_id);
        assert_eq!(entry.version.confidence, expected.confidence);
        assert_eq!(entry.version.valid_from, expected.valid_from);
        assert_eq!(entry.content_digest_recomputed, expected.content_digest);
    }

    // `supersedes_id` is the previous champion, not the previous write.
    assert_eq!(history[0].supersedes_id, Some(v1.node_id()));
    assert_eq!(history[1].supersedes_id, None);

    // A component with no champions has no history, and another component's
    // champion is not claimed.
    assert!(version_history("comp:empty", &versions, &champions).is_empty());
    assert!(version_history("comp:memory", &versions, &[]).is_empty());
}
