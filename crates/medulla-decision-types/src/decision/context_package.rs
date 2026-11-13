//! Context package records (MEDULLA-DECISION-ENGINE-R137.7).
//!
//! The typed request and selection of one context-package decision: which
//! measured evidence spans fit a model window under token, money and latency
//! ceilings. Pure data plus its validation; the solver lives above this crate.
//! A `None` measurement is an unknown fact, never zero.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use medulla_types::closed_error_codes;
use medulla_types::contract::BoundedVec;

/// The most candidates one context-package request carries.
pub const MAX_CONTEXT_PACKAGE_CANDIDATES: usize = 64;

closed_error_codes! {
    /// Every refusal the context-package records report (R137.7).
    pub enum ContextPackageErrorCode {
        /// A duplicate id, repeated resolution, malformed range, malformed
        /// digest or overflowing sum.
        InvalidMeasurement => "CONTEXT_PACKAGE_INVALID_MEASUREMENT",
        /// A ceiling was supplied without the rate that converts it to tokens.
        MissingFact => "CONTEXT_PACKAGE_MISSING_FACT",
    }
}

/// One measured candidate span at one resolution of one evidence unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContextPackageCandidate {
    /// The candidate id, unique in the request.
    pub candidate_id: String,
    /// The evidence unit this candidate is one resolution of.
    pub evidence_unit: String,
    /// The resolution level, unique within its evidence unit.
    pub resolution: u8,
    /// The cited source.
    pub source_id: String,
    /// The first cited offset.
    pub cited_start: u64,
    /// The end of the cited range, exclusive; greater than `cited_start`.
    pub cited_end: u64,
    /// The cited source revision.
    pub source_revision: u64,
    /// SHA-256 of the candidate content, 64 lowercase hex characters.
    pub content_digest: String,
    /// The tokenizer that measured `token_weight`.
    pub tokenizer_id: String,
    /// That tokenizer's version.
    pub tokenizer_version: String,
    /// The measured token weight.
    pub token_weight: u64,
    /// The measured value in micro units.
    pub value_micro: u64,
    /// Where the measurement came from.
    pub measurement_provenance: String,
}

/// The ceilings one package must fit. A `None` is an unknown fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContextPackageBudget {
    /// The model window in tokens.
    pub window_tokens: u64,
    /// Tokens reserved for the output.
    pub reserved_output_tokens: u64,
    /// The caller's own token ceiling.
    pub caller_ceiling_tokens: Option<u64>,
    /// The money ceiling in micro units.
    pub money_ceiling_micro: Option<u64>,
    /// The price per token in micro units.
    pub price_micro_per_token: Option<u64>,
    /// The latency ceiling in milliseconds.
    pub latency_ceiling_ms: Option<u64>,
    /// The measured throughput.
    pub tokens_per_second: Option<u64>,
}

/// One context-package question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContextPackageRequest {
    /// The ceilings.
    pub budget: ContextPackageBudget,
    /// The measured candidates.
    pub candidates: BoundedVec<ContextPackageCandidate, MAX_CONTEXT_PACKAGE_CANDIDATES>,
    /// The least value per token a chosen candidate must carry, micro units.
    pub value_floor_micro_per_token: u64,
    /// The solver's node budget.
    pub node_budget: u32,
}

/// The chosen package and its certificate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContextPackageSelection {
    /// The chosen candidate ids.
    pub chosen_ids: BoundedVec<String, MAX_CONTEXT_PACKAGE_CANDIDATES>,
    /// The token capacity left after every ceiling.
    pub effective_capacity_tokens: u64,
    /// The summed token weight of the chosen ids.
    pub total_tokens: u64,
    /// The total cost in micro units, when the price is known.
    pub total_cost_micro: Option<u64>,
    /// The total latency in milliseconds, when the throughput is known.
    pub total_latency_ms: Option<u64>,
    /// The objective value in micro units.
    pub objective_micro: i64,
    /// SHA-256 of the request.
    pub input_digest: String,
    /// SHA-256 of the solved model.
    pub model_digest: String,
    /// SHA-256 of the optimality certificate.
    pub certificate_digest: String,
    /// Whether the certificate verified.
    pub verified: bool,
}

/// `true` for a SHA-256 digest written as 64 lowercase hex characters.
pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn invalid<T>(detail: impl Into<String>) -> Result<T, (ContextPackageErrorCode, String)> {
    Err((ContextPackageErrorCode::InvalidMeasurement, detail.into()))
}

impl ContextPackageRequest {
    /// Refuse duplicate ids, a repeated resolution in one evidence unit,
    /// `cited_end <= cited_start`, a malformed digest and any sum that
    /// overflows `u64`.
    pub fn validate(&self) -> Result<(), (ContextPackageErrorCode, String)> {
        let mut ids = BTreeSet::new();
        let mut resolutions = BTreeSet::new();
        let (mut tokens, mut value) = (0_u64, 0_u64);
        for candidate in &self.candidates {
            if !ids.insert(candidate.candidate_id.as_str()) {
                return invalid(format!("duplicate candidate id {}", candidate.candidate_id));
            }
            if !resolutions.insert((candidate.evidence_unit.as_str(), candidate.resolution)) {
                return invalid(format!(
                    "resolution {} repeats in evidence unit {}",
                    candidate.resolution, candidate.evidence_unit
                ));
            }
            if candidate.cited_end <= candidate.cited_start {
                return invalid(format!(
                    "cited range of {} is empty",
                    candidate.candidate_id
                ));
            }
            if !is_sha256_hex(&candidate.content_digest) {
                return invalid(format!(
                    "content digest of {} is malformed",
                    candidate.candidate_id
                ));
            }
            tokens = match tokens.checked_add(candidate.token_weight) {
                Some(sum) => sum,
                None => return invalid("token weight sum overflows u64"),
            };
            value = match value.checked_add(candidate.value_micro) {
                Some(sum) => sum,
                None => return invalid("value sum overflows u64"),
            };
        }
        if self
            .budget
            .window_tokens
            .checked_add(self.budget.reserved_output_tokens)
            .is_none()
        {
            return invalid("budget token sum overflows u64");
        }
        Ok(())
    }
}

impl ContextPackageSelection {
    /// Refuse a duplicate chosen id or a malformed digest.
    pub fn validate(&self) -> Result<(), (ContextPackageErrorCode, String)> {
        let mut ids = BTreeSet::new();
        if !self.chosen_ids.iter().all(|id| ids.insert(id.as_str())) {
            return invalid("duplicate chosen id");
        }
        for digest in [
            &self.input_digest,
            &self.model_digest,
            &self.certificate_digest,
        ] {
            if !is_sha256_hex(digest) {
                return invalid("selection digest is malformed");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::statistical::{QuestionKind, TypedValue};

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn candidate(id: &str, unit: &str, resolution: u8) -> ContextPackageCandidate {
        ContextPackageCandidate {
            candidate_id: id.into(),
            evidence_unit: unit.into(),
            resolution,
            source_id: "source:a".into(),
            cited_start: 10,
            cited_end: 20,
            source_revision: 3,
            content_digest: DIGEST.into(),
            tokenizer_id: "tok".into(),
            tokenizer_version: "1".into(),
            token_weight: 100,
            value_micro: 5_000,
            measurement_provenance: "measured".into(),
        }
    }

    fn request(candidates: Vec<ContextPackageCandidate>) -> ContextPackageRequest {
        ContextPackageRequest {
            budget: ContextPackageBudget {
                window_tokens: 8_000,
                reserved_output_tokens: 1_000,
                caller_ceiling_tokens: None,
                money_ceiling_micro: Some(10),
                price_micro_per_token: None,
                latency_ceiling_ms: Some(500),
                tokens_per_second: None,
            },
            candidates: BoundedVec::new(candidates).unwrap(),
            value_floor_micro_per_token: 1,
            node_budget: 1_000,
        }
    }

    fn refused(request: &ContextPackageRequest) -> bool {
        matches!(
            request.validate(),
            Err((ContextPackageErrorCode::InvalidMeasurement, _))
        )
    }

    fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
        value: &T,
    ) {
        let wire = serde_json::to_string(value).unwrap();
        assert_eq!(&serde_json::from_str::<T>(&wire).unwrap(), value);
    }

    // spec: MEDULLA-DECISION-ENGINE-R137.7
    #[test]
    fn package_records_round_trip() {
        let good = request(vec![candidate("a", "u1", 0), candidate("b", "u1", 1)]);
        assert_eq!(good.validate(), Ok(()));
        round_trip(&good);
        round_trip(&good.candidates.as_slice()[0]);
        round_trip(&good.budget);
        let selection = ContextPackageSelection {
            chosen_ids: BoundedVec::new(vec!["a".to_string()]).unwrap(),
            effective_capacity_tokens: 7_000,
            total_tokens: 100,
            total_cost_micro: None,
            total_latency_ms: Some(40),
            objective_micro: -3,
            input_digest: DIGEST.into(),
            model_digest: DIGEST.into(),
            certificate_digest: DIGEST.into(),
            verified: true,
        };
        assert_eq!(selection.validate(), Ok(()));
        round_trip(&selection);
        let typed = TypedValue::ContextPackages(Box::new(good.clone()));
        round_trip(&typed);
        assert_eq!(
            serde_json::to_string(&QuestionKind::ContextPackage).unwrap(),
            "\"context_package\""
        );
        round_trip(&QuestionKind::ContextPackage);

        assert!(refused(&request(vec![
            candidate("a", "u1", 0),
            candidate("a", "u2", 0)
        ])));
        assert!(refused(&request(vec![
            candidate("a", "u1", 0),
            candidate("b", "u1", 0)
        ])));
        let mut empty_range = candidate("a", "u1", 0);
        empty_range.cited_end = empty_range.cited_start;
        assert!(refused(&request(vec![empty_range])));
        let mut bad_digest = candidate("a", "u1", 0);
        bad_digest.content_digest = DIGEST.to_uppercase();
        assert!(refused(&request(vec![bad_digest])));
        let mut heavy = candidate("b", "u2", 0);
        heavy.token_weight = u64::MAX;
        assert!(refused(&request(vec![candidate("a", "u1", 0), heavy])));
        let mut rich = candidate("b", "u2", 0);
        rich.value_micro = u64::MAX;
        assert!(refused(&request(vec![candidate("a", "u1", 0), rich])));
        let mut window = request(vec![]);
        window.budget.window_tokens = u64::MAX;
        assert!(refused(&window));
        let mut bad_selection = selection;
        bad_selection.model_digest = "sha256:x".into();
        assert!(bad_selection.validate().is_err());
        assert_eq!(
            ContextPackageErrorCode::InvalidMeasurement.as_str(),
            "CONTEXT_PACKAGE_INVALID_MEASUREMENT"
        );
    }
}
