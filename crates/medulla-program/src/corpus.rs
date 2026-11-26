//! MEDULLA-DECISION-ENGINE-R813.2: a training corpus built from stored score
//! rows. Writes nothing; the optimizer job stages the result.

use medulla_decision_types::decision::program::{
    SCORE_CORPUS_TOO_LARGE, SCORE_GRADER_UNKNOWN, SCORE_INVALID, SCORE_OUT_OF_RANGE,
};
use medulla_modality::{OpaqueRef, PolicyEnvelope, PrivacyAttestation};
use medulla_types::refusal_code::RefusalCode;

use crate::{
    EvidenceBinding, ExampleOutcome, ExampleSplit, GraderKind, TrainingCorpus, TrainingExample,
};

/// Why a stored score cannot become a training example
/// (MEDULLA-DECISION-ENGINE-R813.4). Each variant has one registered code; a
/// refusal at submit stores no job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScoreRefusal {
    /// The score node does not decode, or names an unknown split.
    Invalid { node: String },
    /// The score's `grader_kind` is outside the five kinds.
    GraderUnknown { node: String },
    /// The score has no value or range, a value outside
    /// `[range_min, range_max]`, or a `range_max` not above `range_min`.
    OutOfRange { node: String },
    /// The graph holds more scores than one corpus carries.
    CorpusTooLarge,
}

impl ScoreRefusal {
    /// The refusal code of this variant.
    pub fn code(&self) -> RefusalCode {
        match self {
            Self::Invalid { .. } => SCORE_INVALID,
            Self::GraderUnknown { .. } => SCORE_GRADER_UNKNOWN,
            Self::OutOfRange { .. } => SCORE_OUT_OF_RANGE,
            Self::CorpusTooLarge => SCORE_CORPUS_TOO_LARGE,
        }
    }
}

impl std::fmt::Display for ScoreRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = self.code();
        match self {
            Self::Invalid { node } => {
                write!(f, "{}: score '{node}' does not decode or names an unknown split", code.as_str())
            }
            Self::GraderUnknown { node } => {
                write!(f, "{}: score '{node}' has no known grader kind", code.as_str())
            }
            Self::OutOfRange { node } => write!(
                f,
                "{}: score '{node}' has no value, or a value outside a range whose maximum is above its minimum",
                code.as_str()
            ),
            Self::CorpusTooLarge => write!(
                f,
                "{}: the graph holds more scores than a corpus carries",
                code.as_str()
            ),
        }
    }
}

impl std::error::Error for ScoreRefusal {}

/// One stored score row: the value, its range, the run it scores and who
/// graded it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreRow {
    pub score_ref: OpaqueRef,
    /// The `:scoresRun` link. A row without one is skipped.
    pub run_ref: Option<OpaqueRef>,
    pub value: f64,
    pub range_min: f64,
    pub range_max: f64,
    pub grader_kind: GraderKind,
    pub author: Option<OpaqueRef>,
    /// The score row's own split; `None` trains.
    pub split: Option<ExampleSplit>,
    pub input_refs: Vec<OpaqueRef>,
    pub observed_output_ref: Option<OpaqueRef>,
    pub evidence: Vec<EvidenceBinding>,
}

/// One `:Score` node as the engine stored it, before refs are minted: ids are
/// the stored node ids. The server reads these through the caller's own
/// projection (MEDULLA-DECISION-ENGINE-R813.3); a client never supplies them.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredScore {
    pub node_id: String,
    /// The node linked by `:scoresRun`; `None` leaves the score out.
    pub run_id: Option<String>,
    pub value: f64,
    pub range_min: f64,
    pub range_max: f64,
    pub grader_kind: GraderKind,
    pub author: Option<String>,
    pub split: Option<ExampleSplit>,
    pub input_ids: Vec<String>,
    pub observed_output_id: Option<String>,
}

/// The `:Score` nodes a caller can see in one graph, with the graph version
/// they were read at (the corpus snapshot version).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredScoreSet {
    pub snapshot_version: u64,
    pub scores: Vec<StoredScore>,
}

/// The caller-bound corpus identity and policy every example is built under.
#[derive(Clone, Debug, PartialEq)]
pub struct CorpusPolicy {
    pub corpus_ref: OpaqueRef,
    pub snapshot_version: u64,
    pub privacy: PrivacyAttestation,
    pub envelope: PolicyEnvelope,
}

/// A corpus built from score rows and the count of rows left out.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreCorpus {
    pub corpus: TrainingCorpus,
    pub skipped_without_run: usize,
    /// Examples whose grader decides promotion (R804.4): the only ones that
    /// count toward held-out evidence.
    pub promotion_deciding: usize,
}

/// The example score: `(value - range_min) / (range_max - range_min)`.
fn normalised(row: &ScoreRow) -> Result<f64, ScoreRefusal> {
    let span = row.range_max - row.range_min;
    let finite = row.value.is_finite() && row.range_min.is_finite() && row.range_max.is_finite();
    if !finite || span <= 0.0 || row.value < row.range_min || row.value > row.range_max {
        return Err(ScoreRefusal::OutOfRange {
            node: row.score_ref.as_str().to_string(),
        });
    }
    Ok((row.value - row.range_min) / span)
}

/// Build a corpus from score rows. A row without a run link is skipped and
/// counted; every grader kind may train, and only `decides_promotion()`
/// kinds are counted as held-out evidence.
pub fn corpus_from_scores(
    rows: &[ScoreRow],
    policy: &CorpusPolicy,
) -> Result<ScoreCorpus, ScoreRefusal> {
    let mut examples = Vec::with_capacity(rows.len());
    let mut skipped_without_run = 0;
    let mut promotion_deciding = 0;
    for row in rows {
        if row.run_ref.is_none() {
            skipped_without_run += 1;
            continue;
        }
        let score = normalised(row)?;
        let split = row.split.unwrap_or_default();
        if row.grader_kind.decides_promotion() && !matches!(split, ExampleSplit::Train) {
            promotion_deciding += 1;
        }
        examples.push(TrainingExample {
            example_ref: row.score_ref.clone(),
            input_refs: row.input_refs.clone(),
            expected_output_ref: None,
            observed_output_ref: row.observed_output_ref.clone(),
            feedback_ref: None,
            trace_ref: row.run_ref.clone(),
            split,
            outcome: if row.observed_output_ref.is_some() {
                ExampleOutcome::Success
            } else {
                ExampleOutcome::Failure
            },
            grader_kind: row.grader_kind,
            score_ref: Some(row.score_ref.clone()),
            author: row.author.clone(),
            score,
            weight: 1.0,
            evidence: row.evidence.clone(),
        });
    }
    Ok(ScoreCorpus {
        corpus: TrainingCorpus {
            corpus_ref: policy.corpus_ref.clone(),
            snapshot_version: policy.snapshot_version,
            privacy: policy.privacy.clone(),
            examples,
        },
        skipped_without_run,
        promotion_deciding,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_modality::{
        ArtifactId, Classification, DerivationId, EvidenceAddress, EvidenceLocus, EvidenceLocusId,
        ResourceId,
    };
    use sha2::{Digest, Sha256};

    use crate::ProgramModality;

    fn token(label: &str) -> String {
        hex::encode(Sha256::digest(label.as_bytes()))
    }

    fn opaque(ns: &str, label: &str) -> OpaqueRef {
        OpaqueRef::scoped(ns, &token(label)).expect("opaque ref")
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

    fn policy() -> CorpusPolicy {
        CorpusPolicy {
            corpus_ref: opaque("corpus", "c"),
            snapshot_version: 1,
            privacy: PrivacyAttestation {
                scanner_ref: opaque("scanner", "s"),
                policy_version_ref: opaque("privacy_policy", "pp"),
                raw_pii_persisted: false,
                local_identifiers_persisted: false,
            },
            envelope: envelope(),
        }
    }

    fn binding(label: &str) -> EvidenceBinding {
        let env = envelope();
        EvidenceBinding {
            modality: ProgramModality::Graph,
            locus: EvidenceLocus {
                id: EvidenceLocusId::from_token(&token(&format!("locus:{label}"))).expect("id"),
                subject: ResourceId::Artifact(
                    ArtifactId::from_token(&token(&format!("artifact:{label}"))).expect("id"),
                ),
                address: EvidenceAddress::RowVersion {
                    row_ref: opaque("row", label),
                    version: 1,
                },
                policy_ref: env.access_policy_ref.clone(),
                derivation_ref: DerivationId::from_token(&token(&format!("derivation:{label}")))
                    .expect("id"),
            },
            policy: env,
        }
    }

    fn row(label: &str, kind: GraderKind, run: bool, value: f64) -> ScoreRow {
        ScoreRow {
            score_ref: opaque("score", label),
            run_ref: run.then(|| opaque("run", label)),
            value,
            range_min: 1.0,
            range_max: 5.0,
            grader_kind: kind,
            author: None,
            split: None,
            input_refs: vec![opaque("input", label)],
            observed_output_ref: Some(opaque("output", label)),
            evidence: vec![binding(label)],
        }
    }

    /// Each built example names the score row it came from, the kind that
    /// graded it, its author and the run it scores; a row with no run link is
    /// counted, not built (the L2 of MEDULLA-DECISION-ENGINE-R813.5).
    #[test]
    fn built_example_carries_its_row_provenance() {
        let mut authored = row("authored", GraderKind::Human, true, 4.0);
        authored.author = Some(opaque("author", "annotator"));
        let unlinked = row("unlinked", GraderKind::Api, false, 2.0);
        let built = corpus_from_scores(&[authored.clone(), unlinked], &policy()).expect("corpus");
        assert_eq!(built.skipped_without_run, 1);
        assert_eq!(built.corpus.examples.len(), 1);
        let example = &built.corpus.examples[0];
        assert_eq!(example.score_ref.as_ref(), Some(&authored.score_ref));
        assert_eq!(example.example_ref, authored.score_ref);
        assert_eq!(example.grader_kind, GraderKind::Human);
        assert_eq!(example.author, authored.author);
        assert_eq!(example.trace_ref, authored.run_ref);
    }

    // spec: MEDULLA-DECISION-ENGINE-R813.2
    #[test]
    fn each_grader_kind_and_the_skip() {
        let kinds = [
            GraderKind::Annotation,
            GraderKind::Human,
            GraderKind::ModelJudge,
            GraderKind::TraceMetric,
            GraderKind::Api,
        ];
        let mut rows: Vec<ScoreRow> = kinds
            .iter()
            .enumerate()
            .map(|(i, kind)| {
                let mut r = row(&format!("k{i}"), *kind, true, 3.0);
                r.split = Some(ExampleSplit::Test);
                r
            })
            .collect();
        rows.push(row("norun", GraderKind::Human, false, 4.0));
        let built = corpus_from_scores(&rows, &policy()).expect("corpus");
        assert_eq!(built.skipped_without_run, 1);
        assert_eq!(built.corpus.examples.len(), kinds.len());
        // Every grader kind trains; only non-judge kinds decide promotion.
        assert_eq!(built.promotion_deciding, kinds.len() - 1);
        for example in &built.corpus.examples {
            assert!((example.score - 0.5).abs() < 1e-12);
        }
        assert_eq!(built.corpus.examples[2].grader_kind, GraderKind::ModelJudge);
        // The split defaults to train.
        let mut plain = row("plain", GraderKind::Api, true, 5.0);
        plain.split = None;
        let built = corpus_from_scores(&[plain], &policy()).expect("corpus");
        assert_eq!(built.corpus.examples[0].split, ExampleSplit::Train);
        assert!((built.corpus.examples[0].score - 1.0).abs() < 1e-12);
        assert_eq!(built.promotion_deciding, 0);
        // A value outside its range or a degenerate range is refused.
        let hi = row("hi", GraderKind::Api, true, 6.0);
        assert_eq!(
            corpus_from_scores(&[hi.clone()], &policy()),
            Err(ScoreRefusal::OutOfRange {
                node: hi.score_ref.as_str().to_string()
            })
        );
        let mut flat = row("flat", GraderKind::Api, true, 1.0);
        flat.range_max = 1.0;
        let refused = corpus_from_scores(&[flat], &policy()).unwrap_err();
        assert_eq!(refused.code(), SCORE_OUT_OF_RANGE);
        assert!(refused.to_string().starts_with("SCORE_OUT_OF_RANGE: "));
    }
}
