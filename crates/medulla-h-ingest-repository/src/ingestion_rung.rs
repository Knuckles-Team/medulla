//! Producing-rung recording and overwrite resistance (MEDULLA-DECISION-ENGINE-R080.2).
//!
//! Every served ingest producer stamps the facts it derives with the rung of
//! the ingestion ladder that produced them (property [`PRODUCING_RUNG`]) and,
//! before writing, resolves its candidate against the fact already stored on
//! the same subject through [`resolve_rung`]. A candidate from a higher rung
//! never replaces a stored fact from a lower rung.

use medulla_decision_outcome_types::decision::ingestion_ladder::{
    resolve_rung, ProvenanceRung, RungFact,
};
use serde_json::Value;

/// The property a stored fact carries its producing rung in.
pub const PRODUCING_RUNG: &str = "producing_rung";

/// The stored label of `rung` (the ladder's own snake_case wire name).
pub fn rung_label(rung: ProvenanceRung) -> String {
    match serde_json::to_value(rung) {
        Ok(Value::String(label)) => label,
        _ => unreachable!("a rung always serializes to its snake_case name"),
    }
}

/// The rung a stored property map carries, if it carries a known one.
pub fn stored_rung(properties: &Value) -> Option<ProvenanceRung> {
    serde_json::from_value(properties.get(PRODUCING_RUNG)?.clone()).ok()
}

/// Whether `candidate` is refused because the fact already stored on the
/// same subject came from a strictly lower rung.
pub fn outranked(stored: Option<ProvenanceRung>, candidate: ProvenanceRung) -> bool {
    let fact = |rung| RungFact { value: (), rung };
    stored.is_some_and(|stored| {
        stored < candidate && resolve_rung(Some(fact(stored)), fact(candidate)).rung == stored
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // spec: MEDULLA-DECISION-ENGINE-R080.2
    #[test]
    fn a_higher_rung_is_refused_against_a_stored_lower_rung() {
        assert!(outranked(
            Some(ProvenanceRung::AstExtraction),
            ProvenanceRung::Embeddings
        ));
        assert!(!outranked(
            Some(ProvenanceRung::Embeddings),
            ProvenanceRung::Embeddings
        ));
        assert!(!outranked(
            Some(ProvenanceRung::LanguageModel),
            ProvenanceRung::Embeddings
        ));
        assert!(!outranked(None, ProvenanceRung::Embeddings));
    }

    #[test]
    fn the_label_round_trips_through_the_stored_property() {
        let label = rung_label(ProvenanceRung::Embeddings);
        assert_eq!(label, "embeddings");
        assert_eq!(
            stored_rung(&json!({ (PRODUCING_RUNG): label })),
            Some(ProvenanceRung::Embeddings)
        );
        assert_eq!(stored_rung(&json!({})), None);
    }
}
