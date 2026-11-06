//! Strategy research record kernels (MEDULLA-FINANCE-PRIMITIVES-R751.751).
//!
//! The pure part of the research record write handler: the weighted fusion of
//! component scores. Evidence resolution and debate round order read the graph
//! and stay in the handler, so this crate holds no graph dependency.

pub use medulla_types::finance_research::ScoreComponent as Component;

/// Why a research record was refused. Each variant is one stable wire code
/// (a registered `ServerErrorCode`), rendered `<code>: detail`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResearchRecordError {
    /// A component without a resolvable evidence node.
    ScoreEvidenceMissing(String),
    /// A component score outside the record range.
    ScoreRangeInvalid(String),
    /// A weight at or below zero, or not finite.
    ScoreWeightsInvalid(String),
    /// A round index that is not the prior index plus one.
    DebateRoundOutOfOrder(String),
    /// A strategy card without a licence class.
    StrategyCardLicenceMissing(String),
    /// An evidence node of another tenant.
    RecordTenantMismatch(String),
    /// A rewrite of a stored record.
    RecordImmutable(String),
}

impl ResearchRecordError {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ScoreEvidenceMissing(_) => "SCORE_EVIDENCE_MISSING",
            Self::ScoreRangeInvalid(_) => "SCORE_RANGE_INVALID",
            Self::ScoreWeightsInvalid(_) => "SCORE_WEIGHTS_INVALID",
            Self::DebateRoundOutOfOrder(_) => "DEBATE_ROUND_OUT_OF_ORDER",
            Self::StrategyCardLicenceMissing(_) => "STRATEGY_CARD_LICENCE_MISSING",
            Self::RecordTenantMismatch(_) => "RECORD_TENANT_MISMATCH",
            Self::RecordImmutable(_) => "RECORD_IMMUTABLE",
        }
    }

    /// The human detail that follows the code.
    pub fn detail(&self) -> &str {
        match self {
            Self::ScoreEvidenceMissing(detail)
            | Self::ScoreRangeInvalid(detail)
            | Self::ScoreWeightsInvalid(detail)
            | Self::DebateRoundOutOfOrder(detail)
            | Self::StrategyCardLicenceMissing(detail)
            | Self::RecordTenantMismatch(detail)
            | Self::RecordImmutable(detail) => detail,
        }
    }
}

impl std::fmt::Display for ResearchRecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.detail())
    }
}

/// Fuse component scores: `sum(weight * score) / sum(weight)`, both sums taken in
/// input order. Every weight must be positive and finite and every score inside
/// `[min, max]`; an empty component list has no weight and is refused.
pub fn fuse_scores(
    components: &[Component],
    min: f64,
    max: f64,
) -> Result<f64, ResearchRecordError> {
    if components.is_empty() {
        return Err(ResearchRecordError::ScoreWeightsInvalid(
            "at least one component is required".to_string(),
        ));
    }
    let mut weighted = 0.0_f64;
    let mut total = 0.0_f64;
    for (index, component) in components.iter().enumerate() {
        if !component.weight.is_finite() || component.weight <= 0.0 {
            return Err(ResearchRecordError::ScoreWeightsInvalid(format!(
                "component {index} weight must be positive and finite"
            )));
        }
        if !component.score.is_finite() || component.score < min || component.score > max {
            return Err(ResearchRecordError::ScoreRangeInvalid(format!(
                "component {index} score must be within [{min}, {max}]"
            )));
        }
        weighted += component.weight * component.score;
        total += component.weight;
    }
    Ok(weighted / total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(score: f64, weight: f64) -> Component {
        Component {
            source: "s".to_string(),
            score,
            weight,
            evidence_ref: "e".to_string(),
        }
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R751.751
    #[test]
    fn fusion_matches_hand_computed_vectors() {
        // (1*0.5 + 3*(-0.5)) / 4 = -0.25
        let two = [component(0.5, 1.0), component(-0.5, 3.0)];
        assert_eq!(fuse_scores(&two, -1.0, 1.0).unwrap(), -0.25);
        // (2*0.25 + 2*0.75 + 4*1.0) / 8 = 0.75
        let three = [
            component(0.25, 2.0),
            component(0.75, 2.0),
            component(1.0, 4.0),
        ];
        assert_eq!(fuse_scores(&three, 0.0, 1.0).unwrap(), 0.75);
        // A single component fuses to its own score.
        let one = [component(-0.8, 0.3)];
        assert_eq!(fuse_scores(&one, -1.0, 1.0).unwrap(), -0.8);
    }

    #[test]
    fn invalid_weights_and_ranges_carry_their_codes() {
        let zero = [component(0.1, 0.0)];
        assert_eq!(
            fuse_scores(&zero, -1.0, 1.0).unwrap_err().code(),
            "SCORE_WEIGHTS_INVALID"
        );
        let nan = [component(0.1, f64::NAN)];
        assert_eq!(
            fuse_scores(&nan, -1.0, 1.0).unwrap_err().code(),
            "SCORE_WEIGHTS_INVALID"
        );
        let high = [component(1.5, 1.0)];
        assert_eq!(
            fuse_scores(&high, -1.0, 1.0).unwrap_err().code(),
            "SCORE_RANGE_INVALID"
        );
    }
}
