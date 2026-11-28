//! MEDULLA-DECISION-ENGINE-R813.6: the `decide/score_corpus` group times
//! `corpus_from_scores` over 10000 stored score rows; JSON on stdout. Budget:
//! the whole corpus builds in at most 1 second in the optimized profile. A
//! benchmark never gates a PR; the nightly budget run reads this line.
//! Run with `cargo bench -p medulla-program --bench score_corpus`.

use std::hint::black_box;
use std::time::Instant;

use medulla_decision_policy_types::decision::held_out::GraderKind;
use medulla_modality::{
    ArtifactId, Classification, DerivationId, EvidenceAddress, EvidenceLocus, EvidenceLocusId,
    OpaqueRef, PolicyEnvelope, PrivacyAttestation, ResourceId,
};
use medulla_program::corpus::{corpus_from_scores, CorpusPolicy, ScoreRow};
use medulla_program::{EvidenceBinding, ExampleSplit, ProgramModality};
use sha2::{Digest, Sha256};

const ROWS: usize = 10_000;
const SAMPLES: usize = 9;
const BUDGET_MS: u128 = 1_000;

fn token(label: &str) -> String {
    hex::encode(Sha256::digest(label.as_bytes()))
}

fn opaque(namespace: &str, label: &str) -> OpaqueRef {
    OpaqueRef::scoped(namespace, &token(label)).expect("opaque reference")
}

fn envelope() -> PolicyEnvelope {
    PolicyEnvelope {
        tenant_ref: opaque("tenant", "t"),
        access_policy_ref: opaque("policy", "a"),
        classification: Classification::Internal,
        retention_policy_ref: opaque("retention", "r"),
        deletion_policy_ref: opaque("deletion", "d"),
        legal_hold_ref: None,
        purpose_refs: vec![opaque("purpose", "p")],
    }
}

fn row(index: usize, envelope: &PolicyEnvelope) -> ScoreRow {
    let label = format!("score-{index}");
    ScoreRow {
        score_ref: opaque("score", &label),
        run_ref: (!index.is_multiple_of(50)).then(|| opaque("run", &label)),
        value: 1.0 + (index % 5) as f64,
        range_min: 1.0,
        range_max: 5.0,
        grader_kind: if index.is_multiple_of(7) {
            GraderKind::ModelJudge
        } else {
            GraderKind::Human
        },
        author: Some(opaque("author", &format!("author-{}", index % 20))),
        split: Some(if index.is_multiple_of(5) {
            ExampleSplit::Test
        } else {
            ExampleSplit::Train
        }),
        input_refs: vec![opaque("run", &label)],
        observed_output_ref: Some(opaque("output", &label)),
        evidence: vec![EvidenceBinding {
            modality: ProgramModality::Graph,
            locus: EvidenceLocus {
                id: EvidenceLocusId::from_token(&token(&format!("locus:{label}"))).expect("locus"),
                subject: ResourceId::Artifact(
                    ArtifactId::from_token(&token(&format!("artifact:{label}"))).expect("artifact"),
                ),
                address: EvidenceAddress::RowVersion {
                    row_ref: opaque("row", &label),
                    version: 1,
                },
                policy_ref: envelope.access_policy_ref.clone(),
                derivation_ref: DerivationId::from_token(&token(&format!("derivation:{label}")))
                    .expect("derivation"),
            },
            policy: envelope.clone(),
        }],
    }
}

fn main() {
    let envelope = envelope();
    let rows: Vec<ScoreRow> = (0..ROWS).map(|index| row(index, &envelope)).collect();
    let policy = CorpusPolicy {
        corpus_ref: opaque("corpus", "bench"),
        snapshot_version: 1,
        privacy: PrivacyAttestation {
            scanner_ref: opaque("scanner", "s"),
            policy_version_ref: opaque("privacy_policy", "pp"),
            raw_pii_persisted: false,
            local_identifiers_persisted: false,
        },
        envelope,
    };
    let mut millis = Vec::with_capacity(SAMPLES);
    let mut built = 0;
    for _ in 0..SAMPLES {
        let start = Instant::now();
        let corpus = black_box(
            corpus_from_scores(black_box(&rows), black_box(&policy)).expect("the rows build"),
        );
        millis.push(start.elapsed().as_millis());
        built = corpus.corpus.examples.len();
    }
    millis.sort_unstable();
    let (p50, worst) = (millis[SAMPLES / 2], millis[SAMPLES - 1]);
    println!(
        "{{\"bench\":\"decide/score_corpus\",\"rows\":{ROWS},\"examples\":{built},\
         \"p50_ms\":{p50},\"max_ms\":{worst},\"budget_ms\":{BUDGET_MS},\"optimized\":{}}}",
        !cfg!(debug_assertions)
    );
    assert_eq!(built, ROWS - ROWS / 50);
    if !cfg!(debug_assertions) {
        assert!(worst <= BUDGET_MS, "corpus_from_scores exceeded its budget");
    }
}
