//! The context-source partition kernel (MEDULLA-DECISION-ENGINE-R029.2.1).
//!
//! Sources are ranked by relevance descending, then source id ascending, and
//! each is placed exactly once. In rank order a source is:
//!
//! 1. `Irrelevant` when its relevance is below `relevance_min`;
//! 2. `AuthorityDisplaced` when a source already selected speaks about the
//!    same subject at a strictly higher authority rank;
//! 3. `OverBudget` when its tokens do not fit in what the budget has left
//!    (the scan continues: a later, smaller source may still fit);
//! 4. otherwise selected.
//!
//! The kernel is integer-only and deterministic. `Unauthorized` is never
//! produced here: the caller reports it for sources it may not read.

use medulla_decision_outcome_types::decision::context_budget::{
    AvailableContextSource, ContextPartition, OmissionReason, OmittedContextSource,
};

/// Partition `sources` under `budget_tokens` and `relevance_min` (Q32).
pub fn partition(
    sources: &[AvailableContextSource],
    budget_tokens: u64,
    relevance_min_q32: i64,
) -> ContextPartition {
    let mut order: Vec<&AvailableContextSource> = sources.iter().collect();
    order.sort_by(|a, b| {
        b.relevance_q32
            .cmp(&a.relevance_q32)
            .then_with(|| a.source_id.cmp(&b.source_id))
    });
    let mut selected: Vec<&AvailableContextSource> = Vec::new();
    let mut omitted = Vec::new();
    let mut total: u64 = 0;
    for source in order {
        let reason = if source.relevance_q32 < relevance_min_q32 {
            Some(OmissionReason::Irrelevant)
        } else if selected.iter().any(|kept| {
            kept.subject == source.subject && kept.authority_rank > source.authority_rank
        }) {
            Some(OmissionReason::AuthorityDisplaced)
        } else if total.saturating_add(source.tokens) > budget_tokens {
            Some(OmissionReason::OverBudget)
        } else {
            None
        };
        match reason {
            Some(reason) => omitted.push(OmittedContextSource {
                source_id: source.source_id.clone(),
                reason,
            }),
            None => {
                total += source.tokens;
                selected.push(source);
            }
        }
    }
    ContextPartition {
        selected: selected.iter().map(|s| s.source_id.clone()).collect(),
        omitted,
        total_tokens: total,
        budget_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn source(
        id: &str,
        relevance: i64,
        tokens: u64,
        subject: &str,
        rank: u8,
    ) -> AvailableContextSource {
        AvailableContextSource {
            source_id: id.to_string(),
            relevance_q32: relevance,
            tokens,
            subject: subject.to_string(),
            authority_rank: rank,
        }
    }

    /// A small deterministic generator, so the property needs no extra crate.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, bound: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % bound
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R029.2.1
    #[test]
    fn partition_is_exhaustive_and_disjoint() {
        let mut rng = Lcg(0x29_2_1);
        for case in 0..500 {
            let count = rng.next(12) as usize;
            let sources: Vec<AvailableContextSource> = (0..count)
                .map(|i| {
                    source(
                        &format!("s{case}-{i}"),
                        rng.next(1 << 32) as i64,
                        rng.next(400),
                        &format!("subject-{}", rng.next(4)),
                        rng.next(4) as u8,
                    )
                })
                .collect();
            let budget = rng.next(1500);
            let min = rng.next(1 << 32) as i64;
            let result = partition(&sources, budget, min);

            let selected: BTreeSet<&str> = result.selected.iter().map(String::as_str).collect();
            let omitted: BTreeSet<&str> = result
                .omitted
                .iter()
                .map(|o| o.source_id.as_str())
                .collect();
            assert_eq!(
                selected.len(),
                result.selected.len(),
                "a source is selected twice"
            );
            assert_eq!(
                omitted.len(),
                result.omitted.len(),
                "a source is omitted twice"
            );
            assert!(
                selected.is_disjoint(&omitted),
                "a source is both selected and omitted"
            );
            let all: BTreeSet<&str> = sources.iter().map(|s| s.source_id.as_str()).collect();
            assert_eq!(
                &selected | &omitted,
                all,
                "a source is neither selected nor omitted"
            );
            assert!(result.total_tokens <= budget);
            let spent: u64 = sources
                .iter()
                .filter(|s| selected.contains(s.source_id.as_str()))
                .map(|s| s.tokens)
                .sum();
            assert_eq!(spent, result.total_tokens);
            assert!(result
                .omitted
                .iter()
                .all(|o| o.reason != OmissionReason::Unauthorized));
        }
    }

    #[test]
    fn rank_order_reasons_and_continued_scan() {
        let q = 1i64 << 31;
        let sources = vec![
            source("low", q / 4, 10, "a", 1),
            source("proof", q * 2 - 2, 60, "a", 3),
            source("claim", q * 2 - 3, 10, "a", 1),
            source("big", q * 2 - 4, 90, "b", 1),
            source("small", q * 2 - 5, 30, "c", 1),
            source("twin", q * 2 - 5, 30, "d", 1),
        ];
        let result = partition(&sources, 100, q);
        assert_eq!(result.selected, ["proof", "small"]);
        let reasons: Vec<(&str, OmissionReason)> = result
            .omitted
            .iter()
            .map(|o| (o.source_id.as_str(), o.reason))
            .collect();
        assert_eq!(
            reasons,
            [
                ("claim", OmissionReason::AuthorityDisplaced),
                ("big", OmissionReason::OverBudget),
                ("twin", OmissionReason::OverBudget),
                ("low", OmissionReason::Irrelevant),
            ]
        );
        assert_eq!(result.total_tokens, 90);
    }
}
