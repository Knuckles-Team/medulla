//! Questions, answers, cards and derivation status of participant memory
//! (MEDULLA-DECISION-ENGINE-R655.1).
//!
//! Serde types only: nothing here reads a store or a clock. Every type refuses unknown fields on
//! the wire. The Rust round-trip vectors in the tests below are the fixtures the hand-written
//! Python mirror (`medulla/generated/models.py`) is tested against.

use serde::{Deserialize, Serialize};

use medulla_work_types::memory_scope::ScopeRef;
use medulla_work_types::participant::{Participant, ParticipantId};

use crate::claims::ObserverRef;
use crate::decision::record::outcome::AbstainReason;

/// A question about one participant, asked as one observer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ParticipantQuestion {
    /// The participant the question is about.
    pub subject: ParticipantId,
    /// The view the question is asked in; the caller must be allowed to act as it.
    pub observer: ObserverRef,
    /// The question text.
    pub question: String,
    /// A memory scope that narrows the sessions the answer may draw on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_scope: Option<ScopeRef>,
    /// Read the claims as of this instant; absent reads the current ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of_ms: Option<i64>,
}

/// The claim and the turns one sentence of an answer rests on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Citation {
    /// The id of the claim.
    pub claim_id: String,
    /// The ids of the turns the claim rests on.
    pub turn_ids: Vec<String>,
}

/// The answer to a [`ParticipantQuestion`]: cited text, or an abstention that names its reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ParticipantAnswer {
    /// The text of the answer.
    pub text: String,
    /// The claims the sentences of the answer rest on.
    pub citations: Vec<Citation>,
    /// Why the answer abstained; absent when it answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abstained: Option<AbstainReason>,
}

/// One claim that made it onto a card, with the two factors of its rank.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CardClaim {
    /// The id of the claim.
    pub claim_id: String,
    /// The text of the claim.
    pub text: String,
    /// How confident the claim is, in parts per million.
    pub confidence_ppm: u32,
    /// How recent the claim is, in parts per million.
    pub recency_ppm: u32,
}

/// The token-bounded summary the engine keeps for one subject as seen by one observer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ParticipantCard {
    /// The participant the card describes.
    pub subject: ParticipantId,
    /// Whose view the card belongs to.
    pub observer: ObserverRef,
    /// The claims on the card, ranked.
    pub claims: Vec<CardClaim>,
    /// The card size in tokens, estimated.
    pub token_estimate: u32,
    /// When the card was computed, in milliseconds since the Unix epoch.
    pub computed_at_ms: i64,
    /// SHA-256 over the card's claims, as `sha256:<hex>`.
    pub digest: String,
}

/// How far derivation has got for one subject in one session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DerivationStatus {
    /// The memory scope the derivation runs under; empty when it runs over the whole view.
    pub scope_id: String,
    /// The session the derivation runs in.
    pub session_id: String,
    /// The participant the conclusions are about.
    pub subject: ParticipantId,
    /// How many turns await derivation.
    pub pending: u32,
    /// The age of the oldest pending turn, in milliseconds.
    pub oldest_pending_age_ms: u64,
}

/// One turn the derive worker reads (MEDULLA-DECISION-ENGINE-R651.12).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DeriveTurn {
    /// The memory record that holds the turn; the evidence of a conclusion cites it.
    pub turn_id: String,
    /// The participant who spoke the turn.
    pub speaker: ParticipantId,
    /// The text of the turn.
    pub text: String,
    /// When the turn was said, in milliseconds since the Unix epoch.
    pub timestamp_ms: u64,
}

/// What a derive worker reads for one claimed task: the perspective to conclude for and the turns
/// of the task's range that the perspective may see (MEDULLA-DECISION-ENGINE-R651.12).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DeriveInput {
    /// The memory scope the derivation runs under; empty over the whole view.
    pub scope_id: String,
    /// The session the derivation runs in.
    pub session_id: String,
    /// The participant the conclusions are about.
    pub subject: ParticipantId,
    /// Whose view the conclusions belong to.
    pub observer: ObserverRef,
    /// The first turn of the range.
    pub first_turn_id: String,
    /// The last turn of the range.
    pub last_turn_id: String,
    /// The turns of the range the perspective may see.
    pub turns: Vec<DeriveTurn>,
    /// At most this many conclusions are kept (`memory.derive_max_conclusions`).
    pub max_conclusions: u32,
    /// Every claim id the worker proposes starts with this prefix, followed by the digest of the
    /// conclusion's whitespace-normalised, case-folded text; `CompleteDerivation` counts the
    /// claims that carry it (MEDULLA-DECISION-ENGINE-R651.12).
    pub claim_prefix: String,
}

/// One served read of the participant surface (`Method::ParticipantRead`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ParticipantReadOp {
    /// One participant the caller may read.
    Get {
        /// The participant to read.
        participant_id: ParticipantId,
    },
    /// Every participant the caller may read.
    List,
    /// Ask a question about a participant.
    Ask {
        /// The question to ask.
        question: ParticipantQuestion,
    },
    /// The stored card of a subject as one observer sees it.
    Card {
        /// The participant the card describes.
        subject: ParticipantId,
        /// Whose view the card is read as.
        observer: ObserverRef,
    },
    /// The turns and perspective of one claimed derive task (MEDULLA-DECISION-ENGINE-R651.12);
    /// `payload_ref` is the payload the queue task names. A range that starts after the stored
    /// checkpoint answers `DERIVE_ORDER_HELD`.
    DeriveInput {
        /// The payload the queue task names.
        payload_ref: String,
    },
    /// The derivation backlog the caller may see, optionally narrowed to one scope.
    Status {
        /// The scope to narrow the backlog to; absent reads every scope.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        memory_scope: Option<ScopeRef>,
    },
}

/// The answer of a [`ParticipantReadOp`] that returns participants.
pub type ParticipantList = Vec<Participant>;

#[cfg(test)]
mod tests {
    use super::*;

    fn ada() -> ParticipantId {
        ParticipantId::new("ada-1").unwrap()
    }

    fn round_trip<T>(value: &T, vector: &str)
    where
        T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let wire = serde_json::to_string(value).unwrap();
        assert_eq!(
            wire, vector,
            "the Python mirror is tested against this vector"
        );
        assert_eq!(&serde_json::from_str::<T>(vector).unwrap(), value);
        let packed = rmp_serde::to_vec_named(value).unwrap();
        assert_eq!(&rmp_serde::from_slice::<T>(&packed).unwrap(), value);
    }

    // spec: MEDULLA-DECISION-ENGINE-R655.1
    #[test]
    fn types_round_trip_and_write_vectors() {
        round_trip(
            &ParticipantQuestion {
                subject: ada(),
                observer: ObserverRef::Participant(ParticipantId::new("bo-2").unwrap()),
                question: "what does she prefer".into(),
                memory_scope: Some(ScopeRef {
                    scope_id: "work".into(),
                }),
                as_of_ms: Some(1_700),
            },
            r#"{"subject":"ada-1","observer":{"participant":"bo-2"},"question":"what does she prefer","memory_scope":{"scope_id":"work"},"as_of_ms":1700}"#,
        );
        round_trip(
            &ParticipantQuestion {
                subject: ada(),
                observer: ObserverRef::System,
                question: "q".into(),
                memory_scope: None,
                as_of_ms: None,
            },
            r#"{"subject":"ada-1","observer":"system","question":"q"}"#,
        );
        round_trip(
            &ParticipantAnswer {
                text: "She prefers tea.".into(),
                citations: vec![Citation {
                    claim_id: "c1".into(),
                    turn_ids: vec!["m1".into(), "m2".into()],
                }],
                abstained: None,
            },
            r#"{"text":"She prefers tea.","citations":[{"claim_id":"c1","turn_ids":["m1","m2"]}]}"#,
        );
        let card = ParticipantCard {
            subject: ada(),
            observer: ObserverRef::System,
            claims: vec![CardClaim {
                claim_id: "c1".into(),
                text: "prefers tea".into(),
                confidence_ppm: 800_000,
                recency_ppm: 500_000,
            }],
            token_estimate: 3,
            computed_at_ms: 9,
            digest: "sha256:ab".into(),
        };
        round_trip(
            &card,
            r#"{"subject":"ada-1","observer":"system","claims":[{"claim_id":"c1","text":"prefers tea","confidence_ppm":800000,"recency_ppm":500000}],"token_estimate":3,"computed_at_ms":9,"digest":"sha256:ab"}"#,
        );
        round_trip(
            &DerivationStatus {
                scope_id: String::new(),
                session_id: "s1".into(),
                subject: ada(),
                pending: 2,
                oldest_pending_age_ms: 40,
            },
            r#"{"scope_id":"","session_id":"s1","subject":"ada-1","pending":2,"oldest_pending_age_ms":40}"#,
        );
        round_trip(&ParticipantReadOp::List, r#"{"op":"list"}"#);
        round_trip(
            &ParticipantReadOp::Card {
                subject: ada(),
                observer: ObserverRef::System,
            },
            r#"{"op":"card","subject":"ada-1","observer":"system"}"#,
        );
        round_trip(
            &ParticipantReadOp::Status { memory_scope: None },
            r#"{"op":"status"}"#,
        );
        round_trip(
            &ParticipantReadOp::DeriveInput {
                payload_ref: "derive_payload:t:ab".into(),
            },
            r#"{"op":"derive_input","payload_ref":"derive_payload:t:ab"}"#,
        );
        round_trip(
            &DeriveInput {
                scope_id: String::new(),
                session_id: "s1".into(),
                subject: ada(),
                observer: ObserverRef::System,
                first_turn_id: "m1".into(),
                last_turn_id: "m2".into(),
                turns: vec![DeriveTurn {
                    turn_id: "m1".into(),
                    speaker: ada(),
                    text: "hi".into(),
                    timestamp_ms: 4,
                }],
                max_conclusions: 8,
                claim_prefix: "derived:ab:".into(),
            },
            r#"{"scope_id":"","session_id":"s1","subject":"ada-1","observer":"system","first_turn_id":"m1","last_turn_id":"m2","turns":[{"turn_id":"m1","speaker":"ada-1","text":"hi","timestamp_ms":4}],"max_conclusions":8,"claim_prefix":"derived:ab:"}"#,
        );
        // An unknown field and an invalid slug are refused.
        assert!(serde_json::from_str::<ParticipantQuestion>(
            r#"{"subject":"ada-1","observer":"system","question":"q","extra":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<ParticipantReadOp>(
            r#"{"op":"get","participant_id":"Bad Slug"}"#
        )
        .is_err());
    }
}
