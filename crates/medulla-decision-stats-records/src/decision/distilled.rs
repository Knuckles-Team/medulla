//! Stored-score ranking (MEDULLA-DECISION-ENGINE-R147.2).
//!
//! A distilled scorer's judgments are computed offline and stored, so ranking
//! reads stored integers only. [`rank_stored`] takes the stored values and
//! nothing else: it has no model handle, so no query-time model call is
//! possible.

use std::collections::BTreeMap;

use medulla_decision_types::decision::statistical::distilled::TeacherJudgment;

/// A candidate with no stored score: the first such document id, in the
/// caller's candidate order (`DISTILLED_SCORE_MISSING`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingScore(pub String);

/// Rank `candidates` by stored `score_q16` descending, ties by `document_id`
/// ascending, and return the first `top_k` judgments. Lookup is one map read
/// per candidate. A candidate without a stored score refuses before any
/// ranking happens.
pub fn rank_stored(
    candidates: &[String],
    scores: &BTreeMap<String, TeacherJudgment>,
    top_k: usize,
) -> Result<Vec<TeacherJudgment>, MissingScore> {
    let mut found: Vec<&TeacherJudgment> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        match scores.get(candidate) {
            Some(judgment) => found.push(judgment),
            None => return Err(MissingScore(candidate.clone())),
        }
    }
    found.sort_by(|a, b| {
        b.score_q16
            .cmp(&a.score_q16)
            .then_with(|| a.document_id.cmp(&b.document_id))
    });
    found.dedup_by(|a, b| a.document_id == b.document_id);
    Ok(found.into_iter().take(top_k).cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn judgment(id: &str, score: u32) -> TeacherJudgment {
        TeacherJudgment {
            document_id: id.into(),
            score_q16: score,
            fidelity_lo_q16: 0,
            fidelity_hi_q16: 65_536,
        }
    }

    fn store() -> BTreeMap<String, TeacherJudgment> {
        [
            ("d1", 100),
            ("d2", 900),
            ("d3", 900),
            ("d4", 500),
            ("d5", 50),
        ]
        .into_iter()
        .map(|(id, score)| (id.to_string(), judgment(id, score)))
        .collect()
    }

    fn ids(ranked: &[TeacherJudgment]) -> Vec<&str> {
        ranked.iter().map(|j| j.document_id.as_str()).collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R147.2
    #[test]
    fn no_query_time_calls() {
        let candidates: Vec<String> = ["d5", "d3", "d1", "d4", "d2"]
            .into_iter()
            .map(String::from)
            .collect();
        let scores = store();
        // Order: score descending, the d2/d3 tie by document id.
        let ranked = rank_stored(&candidates, &scores, 5).unwrap();
        assert_eq!(ids(&ranked), vec!["d2", "d3", "d4", "d1", "d5"]);
        // top_k keeps the head, in order.
        assert_eq!(
            ids(&rank_stored(&candidates, &scores, 2).unwrap()),
            vec!["d2", "d3"]
        );
        assert_eq!(
            ids(&rank_stored(&candidates, &scores, 99).unwrap()).len(),
            5
        );
        assert!(rank_stored(&candidates, &scores, 0).unwrap().is_empty());
        // The result depends on the stored values and the candidate set only.
        let mut reversed = candidates.clone();
        reversed.reverse();
        assert_eq!(
            rank_stored(&reversed, &scores, 5).unwrap(),
            rank_stored(&candidates, &scores, 5).unwrap()
        );
        // A candidate with no stored score refuses; there is no scoring fallback.
        let mut with_unknown = candidates.clone();
        with_unknown.insert(1, "ghost".into());
        with_unknown.push("ghost2".into());
        assert_eq!(
            rank_stored(&with_unknown, &scores, 5).unwrap_err(),
            MissingScore("ghost".into())
        );
        assert!(rank_stored(&[], &scores, 3).unwrap().is_empty());
    }
}
