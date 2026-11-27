//! Per-source trust posterior over judged retrieval runs
//! (MEDULLA-DECISION-ENGINE-R821). Derived, never stored: each source's
//! Beta(alpha, beta) is the prior (1, 1) plus counts over the judged-success
//! outcomes the log already holds.

use std::collections::BTreeMap;

use medulla_decision_types::decision::statistical::retrieval::RetrievalOutcome;

use medulla_decision_stats_records::decision::retrieval::hard_negatives;

/// One source's Beta posterior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceTrust {
    /// One plus the cited, judged-correct units of the source.
    pub alpha: u64,
    /// One plus the source's hard negatives.
    pub beta: u64,
}

impl Default for SourceTrust {
    fn default() -> Self {
        Self { alpha: 1, beta: 1 }
    }
}

/// The units of one judged-success outcome that carry a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgedUnits {
    /// `(source, cited, hard_negative)` per returned unit with a source.
    pub units: Vec<(String, bool, bool)>,
}

impl JudgedUnits {
    /// The sourced units of `outcome`, which the caller has judged a success.
    pub fn of(outcome: &RetrievalOutcome) -> Self {
        let negatives = hard_negatives(outcome);
        let units = outcome
            .returned
            .iter()
            .filter_map(|unit| {
                let source = unit.source.clone()?;
                let cited = outcome.cited.iter().any(|id| *id == unit.evidence_id);
                let negative = negatives.iter().any(|n| n.evidence_id == unit.evidence_id);
                Some((source, cited, negative))
            })
            .collect();
        Self { units }
    }
}

/// The posterior of every source seen in `judged`, by source name. A cited
/// unit adds one to `alpha`; a hard negative adds one to `beta`.
pub fn posterior(judged: &[JudgedUnits]) -> BTreeMap<String, SourceTrust> {
    let mut out: BTreeMap<String, SourceTrust> = BTreeMap::new();
    for run in judged {
        for (source, cited, negative) in &run.units {
            let slot = out.entry(source.clone()).or_default();
            slot.alpha += u64::from(*cited);
            slot.beta += u64::from(*negative);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::statistical::retrieval::ReturnedEvidence;
    use medulla_types::contract::BoundedVec;

    fn unit(id: &str, source: Option<&str>) -> ReturnedEvidence {
        ReturnedEvidence {
            evidence_id: id.to_string(),
            content_class: None,
            raw_score_ppm: None,
            source: source.map(str::to_string),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R821
    #[test]
    fn cited_adds_alpha_hard_negative_adds_beta_and_unsourced_is_skipped() {
        let outcome = RetrievalOutcome {
            record_id: "r".to_string(),
            returned: BoundedVec::new(vec![
                unit("a", Some("s1")),
                unit("b", Some("s2")),
                unit("c", None),
                unit("d", Some("s1")),
            ])
            .unwrap(),
            cited: BoundedVec::new(vec!["d".to_string()]).unwrap(),
            query: None,
            path: None,
            scorer_version: None,
        };
        let map = posterior(&[JudgedUnits::of(&outcome)]);
        assert_eq!(map["s1"], SourceTrust { alpha: 2, beta: 2 });
        assert_eq!(map["s2"], SourceTrust { alpha: 1, beta: 2 });
        assert_eq!(map.len(), 2);
    }
}
