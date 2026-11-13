//! Rerank model (MEDULLA-FEDERATED-QUERY-R652.1).
//!
//! The scorer port `rerank_top` calls from served `ContextCompile`, the
//! calibration curve that maps raw scores onto a shared scale, and the receipt
//! that records what the rerank did. Plain serde types; [`CalibrationCurve`]
//! and [`ScorerId`] are validated on construction.

use serde::{Deserialize, Serialize};

/// Most knots a calibration curve may carry.
pub const MAX_KNOTS: usize = 64;

/// Names a scorer and its version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScorerId {
    pub name: String,
    pub version: String,
}

impl ScorerId {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Result<Self, RerankError> {
        let id = Self {
            name: name.into(),
            version: version.into(),
        };
        id.validate()?;
        Ok(id)
    }

    pub fn validate(&self) -> Result<(), RerankError> {
        if self.name.trim().is_empty() || self.version.trim().is_empty() {
            return Err(RerankError::InvalidScorerId);
        }
        Ok(())
    }
}

/// A scorer the rerank stage can call.
pub trait RerankScorer {
    fn id(&self) -> ScorerId;
    fn score(&self, query: &str, docs: &[&str]) -> Result<Vec<f32>, RerankError>;
}

/// A monotone non-decreasing piecewise-linear map `(raw, calibrated)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationCurve {
    pub scorer: ScorerId,
    pub knots: Vec<(f32, f32)>,
    /// SHA-256 hex of the curve.
    pub digest: String,
}

impl CalibrationCurve {
    pub fn new(
        scorer: ScorerId,
        knots: Vec<(f32, f32)>,
        digest: impl Into<String>,
    ) -> Result<Self, RerankError> {
        let curve = Self {
            scorer,
            knots,
            digest: digest.into(),
        };
        curve.validate()?;
        Ok(curve)
    }

    /// Refuse an empty, oversized, non-finite or non-monotone knot list and a
    /// digest that is not SHA-256 hex.
    pub fn validate(&self) -> Result<(), RerankError> {
        self.scorer.validate()?;
        if self.knots.is_empty() || self.knots.len() > MAX_KNOTS {
            return Err(RerankError::KnotCount {
                count: self.knots.len(),
            });
        }
        if self
            .knots
            .iter()
            .any(|(x, y)| !x.is_finite() || !y.is_finite())
        {
            return Err(RerankError::NonMonotoneKnots);
        }
        if self
            .knots
            .windows(2)
            .any(|w| w[1].0 < w[0].0 || w[1].1 < w[0].1)
        {
            return Err(RerankError::NonMonotoneKnots);
        }
        if !is_sha256_hex(&self.digest) {
            return Err(RerankError::InvalidDigest);
        }
        Ok(())
    }
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Default `knob retrieval.rerank_k`: how many candidates are scored.
pub const DEFAULT_RERANK_K: usize = 64;

const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

/// One candidate for lexical scoring: its node id and its three text fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexicalDoc<'a> {
    pub node_id: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub text: &'a str,
}

/// BM25 over name, description and text with k1 = 1.2 and b = 0.75. Document
/// statistics (length, average length, document frequency) come from the
/// candidate set only.
#[derive(Debug, Clone, Copy, Default)]
pub struct LexicalScorer;

impl LexicalScorer {
    pub fn id() -> ScorerId {
        ScorerId {
            name: "lexical_bm25".to_string(),
            version: "1".to_string(),
        }
    }

    /// Raw BM25 score per document, in input order.
    pub fn score(&self, query: &str, docs: &[LexicalDoc<'_>]) -> Vec<f32> {
        use std::collections::BTreeMap;
        let terms: std::collections::BTreeSet<String> =
            medulla_nlq::nl::context::question_terms(query)
                .into_iter()
                .collect();
        let tokenised: Vec<Vec<String>> = docs
            .iter()
            .map(|d| {
                let mut all = medulla_nlq::nl::context::question_terms(d.name);
                all.extend(medulla_nlq::nl::context::question_terms(d.description));
                all.extend(medulla_nlq::nl::context::question_terms(d.text));
                all
            })
            .collect();
        let n = docs.len() as f64;
        let avg_len = if docs.is_empty() {
            1.0
        } else {
            (tokenised.iter().map(Vec::len).sum::<usize>() as f64 / n).max(1.0)
        };
        let mut doc_freq: BTreeMap<&str, f64> = BTreeMap::new();
        for term in &terms {
            let df = tokenised.iter().filter(|d| d.contains(term)).count();
            doc_freq.insert(term.as_str(), df as f64);
        }
        tokenised
            .iter()
            .map(|doc| {
                let len = doc.len() as f64;
                let mut score = 0.0_f64;
                for term in &terms {
                    let tf = doc.iter().filter(|t| *t == term).count() as f64;
                    if tf == 0.0 {
                        continue;
                    }
                    let df = doc_freq[term.as_str()];
                    let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
                    score += idf * tf * (BM25_K1 + 1.0)
                        / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * len / avg_len));
                }
                score as f32
            })
            .collect()
    }
}

/// Map `raw` through the pinned curve by linear interpolation between knots;
/// clamp outside the first and last knot.
pub fn interpolate(curve: &CalibrationCurve, raw: f32) -> f32 {
    let knots = &curve.knots;
    if raw <= knots[0].0 {
        return knots[0].1;
    }
    for pair in knots.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        if raw <= x1 {
            if x1 <= x0 {
                return y1;
            }
            return y0 + (y1 - y0) * (raw - x0) / (x1 - x0);
        }
    }
    knots[knots.len() - 1].1
}

/// Calibrate raw scores to [0, 1]. With a pinned curve the map is linear
/// interpolation between knots; with none it is min-max scaling and the
/// returned reason is [`FallbackReason::NoCalibration`].
pub fn calibrate(
    raw: &[f32],
    curve: Option<&CalibrationCurve>,
) -> (Vec<f32>, Option<FallbackReason>) {
    match curve {
        Some(curve) => (raw.iter().map(|r| interpolate(curve, *r)).collect(), None),
        None => {
            let lo = raw.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = raw.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let scaled = raw
                .iter()
                .map(|r| if hi > lo { (r - lo) / (hi - lo) } else { 0.0 })
                .collect();
            (scaled, Some(FallbackReason::NoCalibration))
        }
    }
}

/// One reranked candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct RerankedHit {
    pub node_id: String,
    pub raw: f32,
    pub calibrated: f32,
}

/// Score only the first `rerank_k` candidates (the knob `retrieval.rerank_k`,
/// default 64, [8, 256]), calibrate, and order by calibrated score descending
/// with ties broken by node id. Candidates past `rerank_k` are not returned.
pub fn rerank_top(
    query: &str,
    docs: &[LexicalDoc<'_>],
    rerank_k: usize,
    curve: Option<&CalibrationCurve>,
) -> (Vec<RerankedHit>, Option<FallbackReason>) {
    let scored = &docs[..docs.len().min(rerank_k)];
    let raw = LexicalScorer.score(query, scored);
    let (calibrated, fallback) = calibrate(&raw, curve);
    let mut hits: Vec<RerankedHit> = scored
        .iter()
        .zip(raw.iter().zip(&calibrated))
        .map(|(d, (r, c))| RerankedHit {
            node_id: d.node_id.to_string(),
            raw: *r,
            calibrated: *c,
        })
        .collect();
    hits.sort_by(|a, b| {
        b.calibrated
            .partial_cmp(&a.calibrated)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
    (hits, fallback)
}

pub use medulla_decision_types::decision::context_bundle::FallbackReason;

/// What one rerank did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RerankReceipt {
    pub scorer: ScorerId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_digest: Option<String>,
    pub input_ranks: Vec<u32>,
    pub output_ranks: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<FallbackReason>,
}

/// A refused scorer, curve or score call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum RerankError {
    InvalidScorerId,
    KnotCount { count: usize },
    NonMonotoneKnots,
    InvalidDigest,
    ScorerUnavailable { detail: String },
}

impl std::fmt::Display for RerankError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidScorerId => write!(f, "scorer name and version must be non-empty"),
            Self::KnotCount { count } => {
                write!(f, "calibration has {count} knots; 1 to {MAX_KNOTS} allowed")
            }
            Self::NonMonotoneKnots => {
                write!(f, "calibration knots must be finite and non-decreasing")
            }
            Self::InvalidDigest => write!(f, "calibration digest must be SHA-256 hex"),
            Self::ScorerUnavailable { detail } => write!(f, "scorer unavailable: {detail}"),
        }
    }
}

impl std::error::Error for RerankError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest() -> String {
        "ab".repeat(32)
    }

    #[test]
    fn types_round_trip_and_reject_non_monotone_knots() {
        let scorer = ScorerId::new("cross", "1").unwrap();
        let curve = CalibrationCurve::new(
            scorer.clone(),
            vec![(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)],
            digest(),
        )
        .unwrap();
        let json = serde_json::to_string(&curve).unwrap();
        assert_eq!(
            serde_json::from_str::<CalibrationCurve>(&json).unwrap(),
            curve
        );

        let receipt = RerankReceipt {
            scorer: scorer.clone(),
            calibration_digest: Some(digest()),
            input_ranks: vec![0, 1, 2],
            output_ranks: vec![2, 0, 1],
            fallback: Some(FallbackReason::NoCalibration),
        };
        let json = serde_json::to_string(&receipt).unwrap();
        assert!(json.contains("\"no_calibration\""));
        assert_eq!(
            serde_json::from_str::<RerankReceipt>(&json).unwrap(),
            receipt
        );
        assert!(serde_json::from_str::<FallbackReason>("\"timeout\"").is_err());

        assert_eq!(
            CalibrationCurve::new(scorer.clone(), vec![(0.0, 0.5), (1.0, 0.4)], digest()),
            Err(RerankError::NonMonotoneKnots)
        );
        assert_eq!(
            CalibrationCurve::new(scorer.clone(), vec![(0.5, 0.0), (0.4, 1.0)], digest()),
            Err(RerankError::NonMonotoneKnots)
        );
        let too_many: Vec<(f32, f32)> = (0..=MAX_KNOTS).map(|i| (i as f32, i as f32)).collect();
        assert_eq!(
            CalibrationCurve::new(scorer.clone(), too_many, digest()),
            Err(RerankError::KnotCount {
                count: MAX_KNOTS + 1
            })
        );
        let max: Vec<(f32, f32)> = (0..MAX_KNOTS).map(|i| (i as f32, 1.0)).collect();
        assert!(CalibrationCurve::new(scorer.clone(), max, digest()).is_ok());
        assert_eq!(
            CalibrationCurve::new(scorer, vec![(0.0, 0.0)], "XYZ"),
            Err(RerankError::InvalidDigest)
        );
        assert_eq!(ScorerId::new("", "1"), Err(RerankError::InvalidScorerId));

        let bad: CalibrationCurve =
            serde_json::from_str(&json_curve_with(r#"[[0.0,1.0],[1.0,0.0]]"#)).unwrap();
        assert_eq!(bad.validate(), Err(RerankError::NonMonotoneKnots));
    }

    fn json_curve_with(knots: &str) -> String {
        format!(
            r#"{{"scorer":{{"name":"s","version":"1"}},"knots":{knots},"digest":"{}"}}"#,
            digest()
        )
    }

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn doc<'a>(id: &'a str, name: &'a str, description: &'a str, text: &'a str) -> LexicalDoc<'a> {
        LexicalDoc {
            node_id: id,
            name,
            description,
            text,
        }
    }

    fn curve() -> CalibrationCurve {
        CalibrationCurve::new(
            LexicalScorer::id(),
            vec![(0.0, 0.0), (2.0, 0.5), (4.0, 1.0)],
            DIGEST,
        )
        .unwrap()
    }

    #[test]
    fn bm25_order_and_interpolation() {
        let docs = [
            doc(
                "n1",
                "invoice",
                "monthly invoice total",
                "invoice invoice paid",
            ),
            doc("n2", "ledger", "accounts", "an invoice appears once"),
            doc("n3", "gardening", "plants", "soil and water"),
        ];
        let raw = LexicalScorer.score("invoice total", &docs);
        assert!(raw[0] > raw[1] && raw[1] > raw[2]);
        assert_eq!(raw[2], 0.0);
        let curve = curve();
        assert_eq!(interpolate(&curve, -1.0), 0.0);
        assert_eq!(interpolate(&curve, 1.0), 0.25);
        assert_eq!(interpolate(&curve, 3.0), 0.75);
        assert_eq!(interpolate(&curve, 9.0), 1.0);
        let (hits, fallback) = rerank_top("invoice total", &docs, 64, Some(&curve));
        assert_eq!(fallback, None);
        assert_eq!(hits[0].node_id, "n1");
        let (hits, fallback) = rerank_top("invoice total", &docs, 64, None);
        assert_eq!(fallback, Some(FallbackReason::NoCalibration));
        assert_eq!((hits[0].calibrated, hits[2].calibrated), (1.0, 0.0));
        // Only the first rerank_k candidates are scored.
        let (hits, _) = rerank_top("invoice total", &docs, 2, None);
        assert_eq!(hits.len(), 2);
        // Equal scores break by node id.
        let tie = [
            doc("b", "x", "x", "same words"),
            doc("a", "x", "x", "same words"),
        ];
        let (hits, _) = rerank_top("same words", &tie, 64, None);
        assert_eq!(hits[0].node_id, "a");
    }

    #[test]
    fn calibration_is_monotone() {
        let curve = curve();
        let mut previous = f32::NEG_INFINITY;
        for step in -20..=100 {
            let value = interpolate(&curve, step as f32 / 20.0);
            assert!(value >= previous, "not monotone at {step}");
            previous = value;
        }
        let raw: Vec<f32> = (0..50).map(|i| (i * 37 % 11) as f32 / 3.0).collect();
        let (scaled, _) = calibrate(&raw, None);
        for i in 0..raw.len() {
            for j in 0..raw.len() {
                if raw[i] <= raw[j] {
                    assert!(scaled[i] <= scaled[j]);
                }
            }
        }
        assert!(scaled.iter().all(|v| (0.0..=1.0).contains(v)));
    }
}
