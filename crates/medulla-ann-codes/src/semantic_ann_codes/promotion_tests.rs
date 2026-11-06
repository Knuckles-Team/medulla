//! The S6 promotion gate on a real semantic code store
//! (MEDULLA-UNIFIED-DATA-PLANE-R040.3-.5). Kept in its own `*_tests.rs` file so
//! the write-door scan reads only production code.

use super::*;
use crate::semantic_ann_codes::tests::{seeded_s6_publication, S6Fixture};
use medulla_mutation_types::mutation_scope::MutationOutboxLease;
use medulla_transaction::OutboxClaimBudget;
use medulla_types::embedding_promotion::ProbeSet;

fn claim(fixture: &S6Fixture) -> MutationOutboxLease {
    let mut budget = OutboxClaimBudget::new(1, 5_000, 11).unwrap();
    fixture
        .codes
        .claim_stage_leases(fixture.consumer, &mut budget)
        .unwrap()
        .claims
        .into_iter()
        .next()
        .expect("the seeded S6 intent is claimable")
}

fn finalize(
    fixture: &S6Fixture,
    lease: &MutationOutboxLease,
) -> Result<super::super::SemanticMutationReceipt, SemanticCodeError> {
    fixture.codes.finalize_generation(
        lease,
        &fixture.s6_transition,
        &fixture.s6_checkpoint,
        &fixture.activation_artifact,
        &fixture.image,
        13,
    )
}

/// A text model at `model_version` whose probe set has one query with
/// ground truth `[1, 2]` at k = 2 and a 0.5 recall floor.
fn config(fixture: &S6Fixture, model_version: u64) -> SemanticModelConfig {
    let probe = ProbeSet {
        k: 2,
        queries: vec![vec![1.0, 0.0]],
        ground_truth: vec![vec![1, 2]],
        recall_floor: 0.5,
    };
    let digest = probe.digest();
    SemanticModelConfig {
        model_id: fixture.generation_two.model_id.clone(),
        model_version,
        modalities: vec![PromotionModality::Text],
        probe_set: BTreeMap::from([(PromotionModality::Text, probe)]),
        probe_set_digests: BTreeMap::from([(PromotionModality::Text, digest)]),
    }
}

/// Generation one is live under model version 1, measured at full recall.
fn seed_live_version_one(fixture: &S6Fixture) {
    let codes = &fixture.codes;
    codes.install_model_config(config(fixture, 1)).unwrap();
    let baseline = codes
        .record_probe_measurement(
            PromotionStage::AnnIndex,
            1,
            PromotionModality::Text,
            &[vec![1, 2]],
            12,
        )
        .unwrap();
    assert_eq!(baseline.recall, 1.0);
    let prior = PromotionVerdict {
        binding_id: codes.binding.clone(),
        generation: 1,
        model: fixture.generation_two.model_id.clone(),
        model_version: 1,
        tolerance: DEFAULT_PROMOTION_TOLERANCE,
        entries: Vec::new(),
        promoted: true,
    };
    let bytes = encode_promotion_record(&prior).unwrap();
    let digest = semantic_digest(&bytes);
    codes
        .commit_maintenance(
            MetadataMutation {
                batch_id: "semantic-index:promotion-seed:1",
                event_type: "semantic_promotion_seed",
                subject: "generation:1",
                mutation_digest: digest,
            },
            Vec::new(),
            12,
            None,
            |_, rows| codes.write_verdict(rows, &prior, &bytes),
        )
        .unwrap();
    codes.install_model_config(config(fixture, 2)).unwrap();
}

/// Verifies MEDULLA-UNIFIED-DATA-PLANE-R040.3: Wiring: the gate runs in the activation path before a binding with a new model version goes `Live`
// spec: MEDULLA-UNIFIED-DATA-PLANE-R040.3
#[test]
fn activation_runs_gate() {
    let fixture = seeded_s6_publication("promotion-gate-pass");
    seed_live_version_one(&fixture);
    let codes = &fixture.codes;
    // S5 of generation two measures recall 0.5 over the probe set; the
    // baseline is generation one's S5 receipt.
    let receipt = codes
        .record_probe_measurement(
            PromotionStage::AnnIndex,
            2,
            PromotionModality::Text,
            &[vec![1, 7]],
            12,
        )
        .unwrap();
    assert_eq!(receipt.recall, recall_at_k(&[1, 7], &[1, 2], 2));
    let read = codes.door.serving_read().unwrap();
    let check = codes
        .read_check_in(
            &read,
            2,
            &fixture.generation_two.model_id,
            PromotionModality::Text,
        )
        .unwrap()
        .unwrap();
    drop(read);
    assert_eq!(
        check.check.baseline, 1.0,
        "baseline is the prior Live receipt"
    );
    assert_eq!(check.model_version, 2);

    // Recall 0.5 against baseline 1.0 is a regression: the gate blocks.
    let lease = claim(&fixture);
    let refused = finalize(&fixture, &lease).unwrap_err().to_string();
    assert!(refused.contains(PROMOTION_BLOCKED), "{refused}");
    assert!(
        refused.contains("text: baseline 1 measured 0.5"),
        "{refused}"
    );
    assert_eq!(codes.live_generation().unwrap(), Some(1));
    let stored = codes
        .promotion_verdict(2)
        .unwrap()
        .expect("blocked verdict stored");
    assert!(!stored.promoted);
    assert_eq!(codes.embedding_version_of(0).unwrap(), None);

    // A re-measured S5 at full recall passes: generation two goes Live in
    // the commit that stores its verdict and tags its vectors.
    codes
        .record_probe_measurement(
            PromotionStage::AnnIndex,
            2,
            PromotionModality::Text,
            &[vec![1, 2]],
            12,
        )
        .unwrap();
    finalize(&fixture, &lease).unwrap();
    assert_eq!(codes.live_generation().unwrap(), Some(2));
    let verdict = codes.promotion_verdict(2).unwrap().unwrap();
    assert!(verdict.promoted);
    assert_eq!((verdict.model_version, verdict.entries.len()), (2, 1));
    assert_eq!(
        codes.embedding_version_of(0).unwrap(),
        Some(EmbeddingVersion::new(
            fixture.generation_two.model_id.clone(),
            2
        ))
    );
    let _ = std::fs::remove_dir_all(&fixture.dir);
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R040.4
#[test]
fn missing_check_blocks_and_keeps_prior_live() {
    let fixture = seeded_s6_publication("promotion-gate-missing");
    seed_live_version_one(&fixture);
    let lease = claim(&fixture);
    let refused = finalize(&fixture, &lease).unwrap_err().to_string();
    // Version 2 has no stored check: the probe-set side is what is missing.
    assert!(refused.contains("text: probe set missing"), "{refused}");
    assert_eq!(fixture.codes.live_generation().unwrap(), Some(1));
    let _ = std::fs::remove_dir_all(&fixture.dir);
}
