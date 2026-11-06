//! Strategy research records (MEDULLA-FINANCE-PRIMITIVES-R751.750).
//!
//! The typed shapes of the five immutable research record kinds served through
//! `FinanceRecordPut`: a debate round, a trade journal entry (with its bias
//! audits), a sentiment fusion, a geopolitical risk and a strategy card. A
//! record travels as `FinanceRecord.fields`, each value the JSON text of the
//! field named here; the engine parses it into these types, so an unknown field
//! (a caller-supplied `sentimentScore` for instance) is refused rather than
//! ignored. The fused scores are computed by the write handler, never supplied.

use serde::{Deserialize, Serialize};

/// The kernel version stored on every research record (R751.755).
pub const RESEARCH_KERNEL_VERSION: &str = "research-1";

/// Which side of a debate a round argues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebateSide {
    /// The case for the trade.
    Bull,
    /// The case against the trade.
    Bear,
}

/// The cognitive bias a journal audit claims to have found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BiasKind {
    /// Over-weighting recent events.
    Recency,
    /// Seeking evidence that agrees with a held view.
    Confirmation,
    /// Weighting losses above equal gains.
    LossAversion,
    /// Confidence above the evidence.
    Overconfidence,
}

/// How a strategy card may be reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LicenceClass {
    /// Freely reusable.
    Open,
    /// Reusable under stated limits.
    Restricted,
    /// Not reusable outside its owner.
    Proprietary,
}

/// One bias finding stored as a claim by the submitter; medulla computes none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BiasAudit {
    /// The bias found.
    #[serde(rename = "biasKind")]
    pub bias_kind: BiasKind,
    /// The submitter's finding text.
    pub finding: String,
    /// Evidence node ids the finding rests on.
    #[serde(rename = "evidenceRefs", default)]
    pub evidence_refs: Vec<String>,
}

/// One weighted input of a fused score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoreComponent {
    /// Where the component score came from.
    pub source: String,
    /// The component score, inside the record's range.
    pub score: f64,
    /// The weight; positive and finite.
    pub weight: f64,
    /// The evidence node the component rests on.
    #[serde(rename = "evidenceRef")]
    pub evidence_ref: String,
}

/// One round of a bull/bear debate; extends `TradingDebate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebateRound {
    /// The debate the round belongs to.
    #[serde(rename = "debateId")]
    pub debate_id: String,
    /// One-based position in the debate.
    #[serde(rename = "roundIndex")]
    pub round_index: u32,
    /// The side the round argues.
    #[serde(rename = "debateSide")]
    pub side: DebateSide,
    /// The argument text.
    #[serde(rename = "debateArgument")]
    pub argument: String,
    /// Evidence node ids the argument cites.
    #[serde(rename = "evidenceRefs", default)]
    pub evidence_refs: Vec<String>,
}

/// A trader's journal entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TradeJournalEntry {
    /// The entry id.
    #[serde(rename = "journalId")]
    pub journal_id: String,
    /// When the entry was written (RFC 3339).
    #[serde(rename = "entryAt")]
    pub entry_at: String,
    /// Why the trade was taken.
    #[serde(rename = "journalRationale")]
    pub rationale: String,
    /// The strategy run the entry reviews, if any.
    #[serde(
        rename = "strategyRunRef",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub strategy_run_ref: Option<String>,
    /// Bias audits claimed by the submitter.
    #[serde(rename = "biasAudits", default)]
    pub bias_audits: Vec<BiasAudit>,
}

/// The caller's part of a sentiment fusion; the score is computed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SentimentFusionInput {
    /// The listing the sentiment is about.
    #[serde(rename = "subjectListing")]
    pub subject_listing: String,
    /// The weighted components, scores in [-1, 1].
    pub components: Vec<ScoreComponent>,
}

/// A stored sentiment fusion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SentimentFusion {
    /// The listing the sentiment is about.
    #[serde(rename = "subjectListing")]
    pub subject_listing: String,
    /// The fused score in [-1, 1], computed by the write handler.
    #[serde(rename = "sentimentScore")]
    pub score: f64,
    /// The components the score was fused from.
    pub components: Vec<ScoreComponent>,
}

/// The caller's part of a geopolitical risk; the score is computed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeopoliticalRiskInput {
    /// The region the risk is about.
    #[serde(rename = "geoRegion")]
    pub region: String,
    /// The weighted components, scores in [0, 1].
    pub components: Vec<ScoreComponent>,
}

/// A stored geopolitical risk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeopoliticalRisk {
    /// The region the risk is about.
    #[serde(rename = "geoRegion")]
    pub region: String,
    /// The fused score in [0, 1], computed by the write handler.
    #[serde(rename = "geoRiskScore")]
    pub score: f64,
    /// The components the score was fused from.
    pub components: Vec<ScoreComponent>,
}

/// A strategy card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyCard {
    /// The card id.
    #[serde(rename = "cardId")]
    pub card_id: String,
    /// The strategy specification the card describes.
    #[serde(rename = "strategySpecRef")]
    pub strategy_spec_ref: String,
    /// How the card may be reused.
    #[serde(rename = "licenceClass")]
    pub licence_class: LicenceClass,
    /// A short summary.
    #[serde(rename = "cardSummary")]
    pub summary: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_supplied_score_is_an_unknown_field() {
        let supplied = r#"{"subjectListing":"l","components":[],"sentimentScore":0.5}"#;
        assert!(serde_json::from_str::<SentimentFusionInput>(supplied).is_err());
    }

    #[test]
    fn a_card_round_trips() {
        let text =
            r#"{"cardId":"c","strategySpecRef":"s","licenceClass":"open","cardSummary":"x"}"#;
        let card: StrategyCard = serde_json::from_str(text).unwrap();
        assert_eq!(card.licence_class, LicenceClass::Open);
    }
}
