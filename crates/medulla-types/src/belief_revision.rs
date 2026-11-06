//! Belief revision wire types (MEDULLA-DECISION-ENGINE-R700.1).
//!
//! A pass revises the confidence of claims whose evidence changed. Confidence is
//! log-odds in micro-nats (`i64`). A pass keys its output by claim id in a
//! `BTreeMap`, so output order is deterministic. Every type refuses unknown fields.

use serde::{Deserialize, Serialize};

use crate::claims::EvidenceChannel;
use crate::evidence_class::EvidenceClass;

/// One piece of evidence behind a revision.
///
/// `weight_ppm` is the effective weight: the raw weight capped by
/// `belief.weight_cap_micro` and scaled by the channel prior, so a stored revision
/// replays from its recorded inputs alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EvidenceRef {
    pub evidence_id: String,
    pub channel: EvidenceChannel,
    pub weight_ppm: u32,
    pub class: EvidenceClass,
}

/// The revised confidence of one claim with its explanation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BeliefRevision {
    pub claim_id: String,
    pub prior_micro: i64,
    pub posterior_micro: i64,
    pub support: Vec<EvidenceRef>,
    pub oppose: Vec<EvidenceRef>,
    pub reason: String,
    /// The class of the weakest premise (`claim` is weaker than `observation`,
    /// which is weaker than `proof`).
    pub weakest_class: EvidenceClass,
    pub pass_id: String,
}

/// Two claims whose text is similar enough to compare and which a negation or
/// antonym confirms as opposed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FrictionFinding {
    pub left: String,
    pub right: String,
    pub similarity_ppm: u32,
    pub opposes: bool,
}

/// A proposal to retract a claim. The pass never mutates the claim itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RetractionProposal {
    pub claim_id: String,
    pub posterior_micro: i64,
    pub evidence_class: EvidenceClass,
}

/// The `Method::BeliefRevise` request body: the claims whose evidence changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BeliefReviseRequest {
    pub graph: String,
    pub claim_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies MEDULLA-DECISION-ENGINE-R700.1: Model
    // spec: MEDULLA-DECISION-ENGINE-R700.1
    #[test]
    fn belief_types_round_trip_with_golden_vector() {
        let revision = BeliefRevision {
            claim_id: "c1".into(),
            prior_micro: 0,
            posterior_micro: 1_200_000,
            support: vec![EvidenceRef {
                evidence_id: "e1".into(),
                channel: EvidenceChannel::Outcome,
                weight_ppm: 1_200_000,
                class: EvidenceClass::Observation,
            }],
            oppose: vec![],
            reason: "support".into(),
            weakest_class: EvidenceClass::Observation,
            pass_id: "p1".into(),
        };
        let golden = r#"{"claim_id":"c1","prior_micro":0,"posterior_micro":1200000,"support":[{"evidence_id":"e1","channel":"outcome","weight_ppm":1200000,"class":"observation"}],"oppose":[],"reason":"support","weakest_class":"observation","pass_id":"p1"}"#;
        assert_eq!(serde_json::to_string(&revision).unwrap(), golden);
        assert_eq!(
            serde_json::from_str::<BeliefRevision>(golden).unwrap(),
            revision
        );
        assert!(serde_json::from_str::<FrictionFinding>(
            r#"{"left":"a","right":"b","similarity_ppm":1,"opposes":true,"x":1}"#
        )
        .is_err());
        let proposal = RetractionProposal {
            claim_id: "c1".into(),
            posterior_micro: -2,
            evidence_class: EvidenceClass::Claim,
        };
        let text = serde_json::to_string(&proposal).unwrap();
        assert_eq!(
            serde_json::from_str::<RetractionProposal>(&text).unwrap(),
            proposal
        );
    }
}
