//! Team, specialist, population and social records (MEDULLA-ORCH-VOCAB-R751.750).
//!
//! A stored team is a `Definition { Team }` revision and a proven specialist
//! is a `Definition { Agent }` revision (MEDULLA-DECISION-ENGINE-R805.10);
//! these records carry no second team store. Reads use immutable snapshots
//! and writes go through the single write authority.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A team: its member refs and the digest of that member set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TeamConfig {
    pub team_id: String,
    pub members: Vec<String>,
    /// `sha256:` over the sorted member refs; independent of member order.
    pub config_digest: String,
}

impl TeamConfig {
    /// SHA-256 over the sorted member refs, one per line.
    pub fn digest_of(members: &[String]) -> String {
        let mut sorted: Vec<&str> = members.iter().map(String::as_str).collect();
        sorted.sort_unstable();
        let mut hasher = Sha256::new();
        for member in sorted {
            hasher.update(member.as_bytes());
            hasher.update(b"\n");
        }
        format!("sha256:{}", hex::encode(hasher.finalize()))
    }

    /// A team whose digest is computed from its members.
    pub fn new(team_id: impl Into<String>, members: Vec<String>) -> Self {
        let config_digest = Self::digest_of(&members);
        Self {
            team_id: team_id.into(),
            members,
            config_digest,
        }
    }
}

/// A proposed specialist definition and the traces that justify it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SpecialistProposal {
    pub proposal_id: String,
    pub evidence_trace_ids: Vec<String>,
    /// Reference to the proposed `Definition { Agent }` revision.
    pub proposed_definition: String,
}

/// One population outcome of a team on one trace, in parts per million.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PopulationOutcome {
    pub team_id: String,
    pub trace_id: String,
    pub outcome_ppm: u32,
}

/// The closed set of social interaction kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SocialInteractionKind {
    Handoff,
    Critique,
    Vote,
}

/// One interaction among team participants within a trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SocialInteraction {
    pub interaction_id: String,
    pub participants: Vec<String>,
    pub trace_id: String,
    pub kind: SocialInteractionKind,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-ORCH-VOCAB-R751.750
    #[test]
    fn records_round_trip_and_digest_is_order_free() {
        let a = TeamConfig::new("team:a", vec!["agent:x".into(), "agent:y".into()]);
        let b = TeamConfig::new("team:a", vec!["agent:y".into(), "agent:x".into()]);
        assert_eq!(a.config_digest, b.config_digest);
        assert!(a.config_digest.starts_with("sha256:"));
        assert_ne!(
            a.config_digest,
            TeamConfig::new("team:a", vec!["agent:x".into()]).config_digest
        );
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<TeamConfig>(&json).unwrap(), a);

        let proposal = SpecialistProposal {
            proposal_id: "proposal:1".into(),
            evidence_trace_ids: vec!["trace:1".into()],
            proposed_definition: "definition:agent:z:2".into(),
        };
        let json = serde_json::to_string(&proposal).unwrap();
        assert_eq!(
            serde_json::from_str::<SpecialistProposal>(&json).unwrap(),
            proposal
        );

        let outcome = PopulationOutcome {
            team_id: "team:a".into(),
            trace_id: "trace:1".into(),
            outcome_ppm: 750_000,
        };
        let json = serde_json::to_string(&outcome).unwrap();
        assert_eq!(
            serde_json::from_str::<PopulationOutcome>(&json).unwrap(),
            outcome
        );

        for kind in [
            SocialInteractionKind::Handoff,
            SocialInteractionKind::Critique,
            SocialInteractionKind::Vote,
        ] {
            let interaction = SocialInteraction {
                interaction_id: "interaction:1".into(),
                participants: vec!["agent:x".into(), "agent:y".into()],
                trace_id: "trace:1".into(),
                kind,
            };
            let json = serde_json::to_string(&interaction).unwrap();
            assert_eq!(
                serde_json::from_str::<SocialInteraction>(&json).unwrap(),
                interaction
            );
        }
        assert!(serde_json::from_str::<SocialInteractionKind>("\"veto\"").is_err());
    }
}
