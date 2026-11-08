//! Persisted origin chain of a temporal rule-derived fact
//! (MEDULLA-FEDERATED-QUERY-R006.1).
//!
//! A derived fact is identified by the SHA-256 digest of its rule digest, its
//! premise ids and its validity interval, so the same derivation always names
//! the same fact. The record is pure data; the derivation lives in
//! `medulla-plan` and the write goes through the served `ApplyMutation`.

use medulla_types::contract::Digest256;
use serde::{Deserialize, Serialize};

/// The only rule-form version this build understands.
pub const TEMPORAL_RULES_VERSION: &str = "temporal-rules-v1";

/// Where a premise's authority comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginKind {
    /// A ledger sentence.
    Sentence,
    /// An ontology axiom.
    Axiom,
}

impl OriginKind {
    /// The stable token used inside digests and records.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sentence => "sentence",
            Self::Axiom => "axiom",
        }
    }
}

/// The origin record of one derived fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedFact {
    /// SHA-256 of rule digest, premise ids and interval (64 lowercase hex).
    pub fact_id: String,
    /// Digest of the rule that produced the fact.
    pub rule_digest: Digest256,
    /// Ids of the premises the rule joined, in body order.
    pub premise_ids: Vec<String>,
    /// Whether the origin is a sentence or an axiom.
    pub origin_kind: OriginKind,
    /// Reference of the origin sentence or axiom.
    pub origin_ref: String,
    /// Graph generation the derivation read.
    pub graph_generation: u64,
}

/// One revision of a derived fact as persisted: the origin record, its closed-open
/// event-time interval `[valid_from, valid_to)` and the revision chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedFactRecord {
    /// The origin record.
    pub fact: DerivedFact,
    /// Inclusive interval start (event time).
    pub valid_from: i64,
    /// Exclusive interval end (event time).
    pub valid_to: i64,
    /// Revision, starting at 1; an update or retraction increments it.
    pub revision: u64,
    /// A retraction writes a tombstone revision.
    pub tombstone: bool,
}

/// Identity of a derived fact: SHA-256 over the rule digest, the premise ids and
/// the interval, framed so no concatenation is ambiguous.
pub fn derived_fact_id(
    rule_digest: &Digest256,
    premise_ids: &[String],
    valid_from: i64,
    valid_to: i64,
) -> Result<String, String> {
    let mut fields: Vec<&[u8]> = vec![rule_digest.as_bytes()];
    fields.extend(premise_ids.iter().map(|id| id.as_bytes()));
    let from = valid_from.to_be_bytes();
    let to = valid_to.to_be_bytes();
    fields.push(&from);
    fields.push(&to);
    Ok(Digest256::framed(b"medulla/derived-fact/v1", &fields)?.to_hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fact_id_is_stable_hex_and_binds_every_input() {
        let rule = Digest256::sha256(b"rule");
        let ids = vec!["a".to_string(), "b".to_string()];
        let base = derived_fact_id(&rule, &ids, 1, 5).unwrap();
        assert_eq!(base.len(), 64);
        assert!(base
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_eq!(base, derived_fact_id(&rule, &ids, 1, 5).unwrap());
        assert_ne!(base, derived_fact_id(&rule, &ids, 1, 6).unwrap());
        assert_ne!(base, derived_fact_id(&rule, &ids, 2, 5).unwrap());
        assert_ne!(
            base,
            derived_fact_id(&Digest256::sha256(b"other"), &ids, 1, 5).unwrap()
        );
        assert_ne!(
            base,
            derived_fact_id(&rule, &["ab".to_string()], 1, 5).unwrap()
        );
    }
}
