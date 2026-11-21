//! Gate verdict records and their `judges` edge (MEDULLA-SCM-R008, MEDULLA-SDLC-R012.7).

use medulla_h_ingest::gate_verdict::record;
use medulla_source_types::gate_verdict::{
    landing_gate_check, tree_listing_digest, GateVerdictRecord, LandingPolicy,
};
use medulla_work_types::step_verdict::VerificationVerdict;

fn verdict(gate: &str, outcome: VerificationVerdict, input: &str) -> GateVerdictRecord {
    GateVerdictRecord {
        changeset: "a".repeat(64),
        gate_id: gate.into(),
        input_digest: input.into(),
        verdict: outcome,
        evidence_digest: "b".repeat(64),
        duration_ms: 12,
    }
}

// spec: MEDULLA-SDLC-R012.7
#[test]
fn verdict_writes_one_judges_edge_and_refuses_unknown_changeset() {
    let tree = tree_listing_digest([("src/a.rs", "sha256:x", 0o100644)]);
    let write = record(verdict("fmt", VerificationVerdict::Pass, &tree), true, Some(&tree), None)
        .expect("a verdict of a known changeset records");
    let (from, to) = write.judges();
    assert_eq!(from, format!("gate_verdict:{}:fmt", "a".repeat(64)));
    assert_eq!(to, format!("commit:{}", "a".repeat(64)));

    let refusal = record(verdict("fmt", VerificationVerdict::Pass, &tree), false, Some(&tree), None)
        .expect_err("no changeset node");
    assert_eq!(refusal.code(), "LIFECYCLE_ENDPOINT_UNKNOWN");
}

// spec: MEDULLA-SCM-R008
#[test]
fn a_verdict_is_sealed_create_only_and_its_digest_must_match_the_tree() {
    let tree = tree_listing_digest([("src/a.rs", "sha256:x", 0o100644)]);
    let pass = verdict("fmt", VerificationVerdict::Pass, &tree);
    let first = record(pass.clone(), true, Some(&tree), None).expect("records");
    assert!(!first.replayed);

    // An identical record replays; a different one for the same gate is refused.
    let again = record(pass.clone(), true, Some(&tree), Some(&pass)).expect("replays");
    assert!(again.replayed);
    assert_eq!(again.seal, first.seal);
    let fail = verdict("fmt", VerificationVerdict::Fail, &tree);
    let refusal = record(fail, true, Some(&tree), Some(&pass)).expect_err("sealed");
    assert_eq!(refusal.code(), "GATE_VERDICT_SEALED");

    // A digest that is not the changeset tree is refused.
    let other = verdict("lint", VerificationVerdict::Pass, &"c".repeat(64));
    let refusal = record(other, true, Some(&tree), None).expect_err("mismatch");
    assert_eq!(refusal.code(), "GATE_INPUT_DIGEST_MISMATCH");
}

// spec: MEDULLA-SCM-R008
#[test]
fn a_failed_or_missing_required_verdict_blocks_the_landing() {
    let policy = LandingPolicy {
        repo: "demo".into(),
        branch: "main".into(),
        required_gates: vec!["fmt".into(), "test".into()],
    };
    let both = [
        ("fmt".to_string(), VerificationVerdict::Pass),
        ("test".to_string(), VerificationVerdict::Pass),
    ];
    assert!(landing_gate_check(&policy, &both).is_ok());
    let failed = [
        ("fmt".to_string(), VerificationVerdict::Pass),
        ("test".to_string(), VerificationVerdict::Fail),
    ];
    assert_eq!(landing_gate_check(&policy, &failed).unwrap_err().code(), "LANDING_GATE_BLOCKED");
    assert_eq!(landing_gate_check(&policy, &both[..1]).unwrap_err().code(), "LANDING_GATE_BLOCKED");
}
