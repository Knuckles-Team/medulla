//! Grounding a question in the claims a reader may see (MEDULLA-DECISION-ENGINE-R655.2).
//!
//! Pure text matching: a question words are compared with the words of each visible claim's
//! statement, and the answer is built only from claims that share one. Every sentence of an answer
//! is one claim's statement and carries that claim's id and the turn ids of its evidence. No model
//! is asked.

use medulla_decision_types::participant_memory::Citation;

/// The shortest word that counts when matching.
const MIN_WORD_CHARS: usize = 3;
/// Two words match when they share this many leading characters (or all of the shorter word).
const STEM_CHARS: usize = 4;

/// One claim the reader may see, with what an answer quotes from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskClaim {
    pub claim_id: String,
    pub statement: String,
    pub confidence_ppm: u32,
    /// The turns the claim's mined evidence cites.
    pub turn_ids: Vec<String>,
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|word| word.chars().count() >= MIN_WORD_CHARS)
        .collect()
}

fn same_stem(a: &str, b: &str) -> bool {
    let n = a.chars().count().min(b.chars().count()).min(STEM_CHARS);
    a.chars().take(n).eq(b.chars().take(n))
}

/// How many words of the question a statement shares.
pub fn overlap(question: &str, statement: &str) -> usize {
    let statement_words = words(statement);
    words(question)
        .iter()
        .filter(|word| statement_words.iter().any(|held| same_stem(word, held)))
        .count()
}

/// The grounded claims, best first, at most `max_claims`: those that share a word with the
/// question, ordered by overlap, then confidence, then lowest claim id.
pub fn ground(question: &str, claims: &[AskClaim], max_claims: usize) -> Vec<AskClaim> {
    let mut scored: Vec<(usize, &AskClaim)> = claims
        .iter()
        .map(|claim| (overlap(question, &claim.statement), claim))
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.confidence_ppm.cmp(&a.1.confidence_ppm))
            .then_with(|| a.1.claim_id.cmp(&b.1.claim_id))
    });
    scored
        .into_iter()
        .take(max_claims)
        .map(|(_, claim)| claim.clone())
        .collect()
}

/// The answer text and one citation per sentence, in the order of `grounded`.
pub fn cite(grounded: &[AskClaim]) -> (String, Vec<Citation>) {
    let text = grounded
        .iter()
        .map(|claim| {
            let statement = claim.statement.trim();
            if statement.ends_with(['.', '!', '?']) {
                statement.to_string()
            } else {
                format!("{statement}.")
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let citations = grounded
        .iter()
        .map(|claim| Citation {
            claim_id: claim.claim_id.clone(),
            turn_ids: claim.turn_ids.clone(),
        })
        .collect();
    (text, citations)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(id: &str, statement: &str, confidence: u32, turns: &[&str]) -> AskClaim {
        AskClaim {
            claim_id: id.into(),
            statement: statement.into(),
            confidence_ppm: confidence,
            turn_ids: turns.iter().map(|t| t.to_string()).collect(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R655.2
    #[test]
    fn an_answer_quotes_only_grounded_claims_with_their_turns() {
        let claims = [
            claim("c1", "Prefers green tea", 700_000, &["m1", "m4"]),
            claim("c2", "Lives in Lisbon", 900_000, &["m2"]),
            claim(
                "c3",
                "Prefers tea over coffee in the morning",
                800_000,
                &["m3"],
            ),
        ];
        let grounded = ground("what tea does she prefer", &claims, 20);
        assert_eq!(
            grounded
                .iter()
                .map(|c| c.claim_id.as_str())
                .collect::<Vec<_>>(),
            ["c3", "c1"]
        );
        let (text, citations) = cite(&grounded);
        assert_eq!(
            text,
            "Prefers tea over coffee in the morning. Prefers green tea."
        );
        assert_eq!(citations[0].claim_id, "c3");
        assert_eq!(citations[1].turn_ids, ["m1", "m4"]);
        // The cap keeps the best.
        assert_eq!(ground("tea", &claims, 1).len(), 1);
        // No shared word grounds nothing: the caller abstains.
        assert!(ground("where was she born", &claims, 20).is_empty());
        assert!(ground("tea", &[], 20).is_empty());
        // A stem matches a longer form.
        assert_eq!(overlap("prefer", "she prefers tea"), 1);
    }
}
