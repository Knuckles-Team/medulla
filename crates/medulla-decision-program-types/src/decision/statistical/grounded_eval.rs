//! MEDULLA-DECISION-ENGINE-R100.101.1: the typed inputs and result of grounded
//! topic coverage. The kernel lives in `medulla-decision-stats`
//! (`decision::grounded_eval::topic_coverage`).

use serde::{Deserialize, Serialize};

use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_decision_statistical_types::decision::numeric::UnitRationalWire;
use {
    medulla_decision_policy_types::decision::statistical::ranking_eval::hex_digest,
    medulla_decision_policy_types::decision::statistical::ranking_eval::PinnedSet,
};

/// The reach of the corpus a grounded answer may draw on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GroundedScope {
    SingleDocument,
    LocalMultiDocument,
    GlobalCorpus,
}

/// The topics one query's answer is expected to cover: non-empty, unique.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ExpectedTopicSet {
    pub query_id: String,
    pub topic_ids: Vec<String>,
}

/// Valid independent support votes for one citation: `supporting` of `total`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CitationSupport {
    pub citation_id: String,
    pub supporting: u32,
    pub total: u32,
}

/// One answer claim and the citations it binds to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClaimSupport {
    pub claim_id: String,
    pub citations: Vec<CitationSupport>,
}

/// One topic of one query bound to its answer claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TopicSupport {
    pub query_id: String,
    pub topic_id: String,
    pub claims: Vec<ClaimSupport>,
}

/// `knob grounded.min_topic_support` (default 1/2, [1/2, 1], ratio) and
/// `knob grounded.min_valid_votes` (default 3, [1, 25], votes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GroundedKnobs {
    pub min_topic_support: UnitRationalWire,
    pub min_valid_votes: u32,
}

impl Default for GroundedKnobs {
    fn default() -> Self {
        Self {
            min_topic_support: UnitRationalWire::new(1, 2).expect("one half is a unit rational"),
            min_valid_votes: 3,
        }
    }
}

/// The expected and covered topics of one query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QueryTopicCoverage {
    pub query_id: String,
    pub expected_topic_ids: Vec<String>,
    pub covered_topic_ids: Vec<String>,
}

/// Pooled coverage of one scope: summed covered over summed expected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScopeTopicCoverage {
    pub scope: GroundedScope,
    pub queries: Vec<QueryTopicCoverage>,
    pub covered: u64,
    pub expected: u64,
}

/// A detector's predicted unsupported probability for one answer claim
/// (MEDULLA-DECISION-ENGINE-R100.101.2). The scorer identity pins which
/// scorer, at which revision and artifact, produced it for which input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GroundedClaimPrediction {
    pub claim_id: String,
    pub answer_digest: String,
    pub input_digest: String,
    pub scorer_id: String,
    pub scorer_revision: u32,
    pub scorer_artifact_digest: String,
    /// Predicted unsupported probability in parts per million, [0, 1000000].
    pub p_ppm: u32,
}

/// The annotator votes on one claim. `annotators` lists each annotator once;
/// the valid vote count is `unsupported_votes + supported_votes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClaimVotes {
    pub claim_id: String,
    pub annotators: Vec<String>,
    pub unsupported_votes: u32,
    pub supported_votes: u32,
}

impl ClaimVotes {
    /// The valid vote count `v`.
    pub fn valid_votes(&self) -> u64 {
        u64::from(self.unsupported_votes) + u64::from(self.supported_votes)
    }
}

/// The soft-label Brier score of one answer's unsupported-claim predictions:
/// the exact weighted numerator and denominator, and the score rounded half to
/// even to `digits` decimal digits as the integer `score_scaled`
/// (`score = score_scaled / 10^digits`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WeightedBrier {
    /// Sum of `v * (p - q)^2`, in parts per million squared. Carried on the
    /// wire as a decimal string: the binary wire encoding has no 128-bit
    /// integer, and narrowing would lose exactness.
    #[serde(with = "u128_decimal")]
    #[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
    pub numerator: u128,
    /// Sum of `v`.
    pub denominator: u64,
    pub digits: u32,
    pub score_scaled: u64,
}

/// Serde codec for an exact `u128` as its decimal string.
mod u128_decimal {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse::<u128>().map_err(D::Error::custom)
    }
}

/// Version of the grounded-evaluation request and receipt shapes.
pub const GROUNDED_EVAL_SCHEMA_VERSION: u16 = 1;

/// The scorer that produced the predictions, and the digest of every input it
/// read (MEDULLA-DECISION-ENGINE-R100.101.3). The label set's digest must not
/// be among them: a scorer that saw the labels cannot be evaluated by them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GroundedScorerRecord {
    pub scorer_id: String,
    pub scorer_revision: u32,
    pub scorer_artifact_digest: String,
    pub inputs_read: Vec<String>,
}

/// A grounded evaluation: the pinned corpus, query, answer, claim, citation
/// and label-set revisions, the scope, the evaluator and policy revisions, and
/// the content those pins name. Retrieved evidence comes from
/// `UnifiedQuery`; `DecisionEval` only scores and seals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GroundedAnswerRequest {
    pub schema_version: u16,
    pub tenant_id: String,
    pub idempotency_key: String,
    pub corpus: PinnedSet,
    pub queries: PinnedSet,
    pub answers: PinnedSet,
    pub claims: PinnedSet,
    pub citations: PinnedSet,
    pub labels: PinnedSet,
    pub scope: GroundedScope,
    pub evaluator_revision: u32,
    pub policy: DecisionPolicyRef,
    /// The expected topics of every query of `queries`.
    pub expected: Vec<ExpectedTopicSet>,
    /// The citation support votes of every topic.
    pub support: Vec<TopicSupport>,
    /// The required claim set `C` of the Brier score.
    pub claim_ids: Vec<String>,
    pub predictions: Vec<GroundedClaimPrediction>,
    pub votes: Vec<ClaimVotes>,
    pub scorer: GroundedScorerRecord,
}

impl GroundedAnswerRequest {
    /// The digest `queries` must bind: the sorted expected topic sets.
    pub fn queries_content_digest(&self) -> String {
        let mut lines: Vec<String> = self
            .expected
            .iter()
            .map(|set| {
                let mut topics = set.topic_ids.clone();
                topics.sort();
                format!("{}\t{}", set.query_id, topics.join(","))
            })
            .collect();
        lines.sort();
        hex_digest("eg/grounded-queries/v1", lines)
    }

    /// The digest `claims` must bind: the sorted required claim ids.
    pub fn claims_content_digest(&self) -> String {
        let mut ids = self.claim_ids.clone();
        ids.sort();
        hex_digest("eg/grounded-claims/v1", ids)
    }

    /// The digest `citations` must bind: every citation's support votes.
    pub fn citations_content_digest(&self) -> String {
        let mut lines: Vec<String> = self
            .support
            .iter()
            .flat_map(|topic| {
                topic.claims.iter().flat_map(move |claim| {
                    claim.citations.iter().map(move |cite| {
                        format!(
                            "{}\t{}\t{}\t{}\t{}/{}",
                            topic.query_id,
                            topic.topic_id,
                            claim.claim_id,
                            cite.citation_id,
                            cite.supporting,
                            cite.total
                        )
                    })
                })
            })
            .collect();
        lines.sort();
        hex_digest("eg/grounded-citations/v1", lines)
    }

    /// The digest `answers` must bind: each claim's answer digest.
    pub fn answers_content_digest(&self) -> String {
        let mut lines: Vec<String> = self
            .predictions
            .iter()
            .map(|p| format!("{}\t{}", p.claim_id, p.answer_digest))
            .collect();
        lines.sort();
        hex_digest("eg/grounded-answers/v1", lines)
    }

    /// The digest `labels` must bind: every claim's annotators and votes.
    pub fn labels_content_digest(&self) -> String {
        let mut lines: Vec<String> = self
            .votes
            .iter()
            .map(|v| {
                let mut annotators = v.annotators.clone();
                annotators.sort();
                format!(
                    "{}\t{}\t{}\t{}",
                    v.claim_id,
                    annotators.join(","),
                    v.unsupported_votes,
                    v.supported_votes
                )
            })
            .collect();
        lines.sort();
        hex_digest("eg/grounded-labels/v1", lines)
    }

    /// The six pinned sets in a fixed order: corpus, queries, answers, claims,
    /// citations, labels, each with its role name.
    pub fn bindings(&self) -> [(&'static str, &PinnedSet); 6] {
        [
            ("corpus", &self.corpus),
            ("queries", &self.queries),
            ("answers", &self.answers),
            ("claims", &self.claims),
            ("citations", &self.citations),
            ("labels", &self.labels),
        ]
    }
}

/// The sealed receipt of one grounded evaluation: pooled topic coverage, the
/// Brier score, every bound digest and the scorer identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GroundedEvalReceipt {
    pub schema_version: u16,
    pub scope: GroundedScope,
    pub coverage: ScopeTopicCoverage,
    pub brier: WeightedBrier,
    pub evaluator_revision: u32,
    pub policy_digest: String,
    /// `(role, pinned set)` for corpus, queries, answers, claims, citations
    /// and labels.
    pub bindings: Vec<GroundedBinding>,
    pub scorer_id: String,
    pub scorer_revision: u32,
    pub scorer_artifact_digest: String,
    pub scorer_inputs: Vec<String>,
    pub premises: Vec<String>,
    pub receipt_digest: String,
}

/// One bound pinned set of a receipt, with the role it plays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GroundedBinding {
    pub role: String,
    pub pinned: PinnedSet,
}

impl GroundedEvalReceipt {
    /// The digest of the receipt with an empty `receipt_digest`.
    pub fn digest(&self) -> String {
        let mut lines = vec![
            format!("schema {}", self.schema_version),
            format!("scope {:?}", self.scope),
            format!(
                "coverage {}/{}",
                self.coverage.covered, self.coverage.expected
            ),
            format!(
                "brier {}/{} digits {} scaled {}",
                self.brier.numerator,
                self.brier.denominator,
                self.brier.digits,
                self.brier.score_scaled
            ),
            format!("evaluator {}", self.evaluator_revision),
            format!("policy {}", self.policy_digest),
            format!(
                "scorer {}@{} {}",
                self.scorer_id, self.scorer_revision, self.scorer_artifact_digest
            ),
        ];
        for query in &self.coverage.queries {
            lines.push(format!(
                "query {} expected {} covered {}",
                query.query_id,
                query.expected_topic_ids.join(","),
                query.covered_topic_ids.join(",")
            ));
        }
        lines.extend(self.bindings.iter().map(|b| {
            format!(
                "bind {} {}@{} {}",
                b.role, b.pinned.id, b.pinned.revision, b.pinned.digest
            )
        }));
        lines.extend(self.scorer_inputs.iter().map(|i| format!("input {i}")));
        lines.extend(self.premises.iter().map(|p| format!("premise {p}")));
        hex_digest("eg/grounded-receipt/v1", lines)
    }

    /// Fill `receipt_digest` from the other fields.
    pub fn sealed(mut self) -> Self {
        self.receipt_digest = self.digest();
        self
    }
}
