//! MEDULLA-DECISION-ENGINE-R100.100.1: the typed request and result of a
//! pinned retrieval ranking evaluation. The kernel lives in
//! `medulla-decision-stats` (`decision::ranking_eval::hits_at_k`).
//!
//! Every input is pinned by id, revision and a SHA-256 digest (64 lowercase
//! hex characters) of its content, so an evaluation names exactly what it
//! scored. Local fusion scores are never relevance truth: only the
//! independently pinned label pairs are.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Version of the ranking-evaluation request and result shapes.
pub const RANKING_EVAL_SCHEMA_VERSION: u16 = 1;

/// One pinned input: its id, revision and the SHA-256 of its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PinnedSet {
    pub id: String,
    pub revision: u32,
    pub digest: String,
}

/// One independent relevance judgement: `doc_id` is relevant to `query_id`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LabelPair {
    pub query_id: String,
    pub doc_id: String,
}

/// A pinned set of relevance pairs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LabelSet {
    pub pinned: PinnedSet,
    pub pairs: Vec<LabelPair>,
}

/// A pinned list of query ids. The output of `SynthesizeSearchTask` is an
/// accepted source of these items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QuerySet {
    pub pinned: PinnedSet,
    pub query_ids: Vec<String>,
}

/// One algorithm's ranked document ids for one query, best first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QueryRanking {
    pub query_id: String,
    pub ranked_ids: Vec<String>,
}

/// One algorithm's run over the pinned queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AlgorithmRun {
    pub algorithm_id: String,
    pub version: String,
    pub settings_digest: String,
    /// The digest of the corpus the run searched; it must be the request's.
    pub corpus_digest: String,
    /// The digest of the request inputs the run was given
    /// ([`RetrievalRankingRequest::input_digest`]).
    pub input_digest: String,
    pub rankings: Vec<QueryRanking>,
}

/// A ranking evaluation: which pinned corpus, labels and queries, at which
/// cutoff, over which algorithm runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RetrievalRankingRequest {
    pub schema_version: u16,
    pub tenant_id: String,
    pub idempotency_key: String,
    pub corpus: PinnedSet,
    pub labels: LabelSet,
    pub queries: QuerySet,
    pub k: u32,
    pub evaluator_revision: u32,
    pub algorithms: Vec<AlgorithmRun>,
}

/// `hits@k` of one run: `numerator` of `denominator` queries have a relevant
/// id among their first `k` ranked ids; `value_ppm` is the ratio in parts per
/// million, rounded half to even.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HitsAtK {
    pub numerator: u64,
    pub denominator: u64,
    pub value_ppm: u32,
}

pub fn hex_digest(domain: &str, lines: impl IntoIterator<Item = String>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0u8]);
    for line in lines {
        hasher.update(line.as_bytes());
        hasher.update([b'\n']);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Whether `digest` is 64 lowercase hex characters.
pub fn is_sha256_hex(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl LabelSet {
    /// The digest of the pairs, order-independent: `sha256` over the sorted
    /// `query_id TAB doc_id` lines under the `eg/ranking-labels/v1` domain.
    pub fn content_digest(&self) -> String {
        let mut pairs: Vec<&LabelPair> = self.pairs.iter().collect();
        pairs.sort();
        hex_digest(
            "eg/ranking-labels/v1",
            pairs
                .into_iter()
                .map(|pair| format!("{}\t{}", pair.query_id, pair.doc_id)),
        )
    }
}

impl QuerySet {
    /// The digest of the query ids, order-independent: `sha256` over the
    /// sorted ids under the `eg/ranking-queries/v1` domain.
    pub fn content_digest(&self) -> String {
        let mut ids: Vec<&String> = self.query_ids.iter().collect();
        ids.sort();
        hex_digest("eg/ranking-queries/v1", ids.into_iter().cloned())
    }
}

impl RetrievalRankingRequest {
    /// The digest every run must echo: it covers the corpus, label and query
    /// digests, the cutoff and the evaluator revision, so a run given other
    /// inputs or settings than the request names is detectable.
    pub fn input_digest(&self) -> String {
        hex_digest(
            "eg/ranking-input/v1",
            [
                format!("corpus {}", self.corpus.digest),
                format!("labels {}", self.labels.pinned.digest),
                format!("queries {}", self.queries.pinned.digest),
                format!("k {}", self.k),
                format!("evaluator {}", self.evaluator_revision),
            ],
        )
    }
}

/// One algorithm's sealed score: its pinned identity, its `hits@k` and the
/// digest of the whole entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AlgorithmScore {
    pub algorithm_id: String,
    pub version: String,
    pub settings_digest: String,
    pub numerator: u64,
    pub denominator: u64,
    pub value_ppm: u32,
    pub result_digest: String,
}

impl AlgorithmScore {
    /// The score of `run` with its `result_digest` computed over every other
    /// field.
    pub fn sealed(run: &AlgorithmRun, hits: HitsAtK) -> Self {
        let result_digest = hex_digest(
            "eg/ranking-result/v1",
            [
                format!("algorithm {}@{}", run.algorithm_id, run.version),
                format!("settings {}", run.settings_digest),
                format!("hits {}/{}", hits.numerator, hits.denominator),
                format!("ppm {}", hits.value_ppm),
            ],
        );
        Self {
            algorithm_id: run.algorithm_id.clone(),
            version: run.version.clone(),
            settings_digest: run.settings_digest.clone(),
            numerator: hits.numerator,
            denominator: hits.denominator,
            value_ppm: hits.value_ppm,
            result_digest,
        }
    }
}

/// The sealed receipt of one ranking evaluation: the pinned inputs, one score
/// per algorithm in request order, the evidence premises and the receipt
/// digest. A refused run never reaches a receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RetrievalRankingReceipt {
    pub schema_version: u16,
    pub input_digest: String,
    pub evaluator_revision: u32,
    pub corpus_digest: String,
    pub labels_digest: String,
    pub queries_digest: String,
    pub k: u32,
    pub scores: Vec<AlgorithmScore>,
    /// `corpus:`, `labels:`, `queries:`, `algorithm:`, `hits_at_k:` and
    /// `knob:` premises, in that order.
    pub premises: Vec<String>,
    pub receipt_digest: String,
}

impl RetrievalRankingReceipt {
    /// The digest of the receipt with an empty `receipt_digest`.
    pub fn digest(&self) -> String {
        let mut lines = vec![
            format!("schema {}", self.schema_version),
            format!("input {}", self.input_digest),
            format!("evaluator {}", self.evaluator_revision),
            format!("corpus {}", self.corpus_digest),
            format!("labels {}", self.labels_digest),
            format!("queries {}", self.queries_digest),
            format!("k {}", self.k),
        ];
        lines.extend(
            self.scores
                .iter()
                .map(|score| format!("score {}", score.result_digest)),
        );
        lines.extend(self.premises.iter().map(|p| format!("premise {p}")));
        hex_digest("eg/ranking-receipt/v1", lines)
    }

    /// The receipt of `request` scored as `scores`, sealed. `knob_premises`
    /// are the resolved `knob:<name>=<value>` premises.
    pub fn seal(
        request: &RetrievalRankingRequest,
        scores: Vec<AlgorithmScore>,
        knob_premises: Vec<String>,
    ) -> Self {
        let mut premises = vec![
            format!("corpus:{}", request.corpus.digest),
            format!("labels:{}", request.labels.pinned.digest),
            format!("queries:{}", request.queries.pinned.digest),
        ];
        premises.extend(scores.iter().map(|score| {
            format!(
                "algorithm:{}@{}:{}",
                score.algorithm_id, score.version, score.settings_digest
            )
        }));
        premises.extend(scores.iter().map(|score| {
            format!(
                "hits_at_k:{}={}/{}",
                score.algorithm_id, score.numerator, score.denominator
            )
        }));
        premises.extend(knob_premises);
        let mut receipt = Self {
            schema_version: RANKING_EVAL_SCHEMA_VERSION,
            input_digest: request.input_digest(),
            evaluator_revision: request.evaluator_revision,
            corpus_digest: request.corpus.digest.clone(),
            labels_digest: request.labels.pinned.digest.clone(),
            queries_digest: request.queries.pinned.digest.clone(),
            k: request.k,
            scores,
            premises,
            receipt_digest: String::new(),
        };
        receipt.receipt_digest = receipt.digest();
        receipt
    }
}
