//! Consolidation and selection of run-derived insights
//! (MEDULLA-DECISION-ENGINE-R135.11).
//!
//! Pure and deterministic: no clock, no store, no float. Two insights whose
//! text is equal after lower-casing and whitespace collapse are one insight;
//! selection keeps the insights whose context matches the caller's context
//! signature, by Jaccard similarity of the token sets, in a total order.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::program_store::insight::{
    Insight, InsightKnobs, MAX_INSIGHT_SOURCE_RUNS,
};
use medulla_types::contract::BoundedVec;

/// An insight chosen for a context, with its match quantised to Q32.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedInsight {
    pub insight: Insight,
    /// Jaccard similarity of the context signatures, as a Q32 integer.
    pub match_q32: i64,
}

/// Lower-case and collapse whitespace: the consolidation key of an insight.
fn text_key(text: &str) -> String {
    text.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Merge insights with equal text. The lowest `insight_id` survives; source
/// runs are unioned (sorted, at most [`MAX_INSIGHT_SOURCE_RUNS`], lowest ids
/// kept) and `success_count` is summed. The output is ordered by `insight_id`.
pub fn consolidate_insights(insights: &[Insight]) -> Vec<Insight> {
    let mut groups: BTreeMap<String, Vec<&Insight>> = BTreeMap::new();
    for insight in insights {
        groups
            .entry(text_key(&insight.text))
            .or_default()
            .push(insight);
    }
    let mut merged: Vec<Insight> = groups
        .into_values()
        .filter_map(|mut group| {
            group.sort_by(|a, b| a.insight_id.cmp(&b.insight_id));
            let survivor = group.first().copied()?;
            let runs: BTreeSet<&String> = group
                .iter()
                .flat_map(|insight| insight.source_run_ids.iter())
                .collect();
            let runs: Vec<String> = runs
                .into_iter()
                .take(MAX_INSIGHT_SOURCE_RUNS)
                .cloned()
                .collect();
            let success_count = group.iter().fold(0_u32, |sum, insight| {
                sum.saturating_add(insight.success_count)
            });
            let selection_revision = group
                .iter()
                .map(|insight| insight.selection_revision)
                .max()
                .unwrap_or(survivor.selection_revision);
            let source_run_ids = BoundedVec::new(runs).ok()?;
            Some(Insight {
                source_run_ids,
                success_count,
                selection_revision,
                ..survivor.clone()
            })
        })
        .collect();
    merged.sort_by(|a, b| a.insight_id.cmp(&b.insight_id));
    merged
}

fn token_set(signature: &str) -> BTreeSet<&str> {
    signature.split_whitespace().collect()
}

/// Jaccard similarity as an exact `(intersection, union)` pair; two empty
/// signatures are not a match.
fn jaccard(left: &str, right: &str) -> (u64, u64) {
    let (left, right) = (token_set(left), token_set(right));
    let intersection = left.intersection(&right).count() as u64;
    let union = left.union(&right).count() as u64;
    (intersection, union)
}

/// Q32 of `numerator / denominator`, rounded down.
fn q32_ratio(numerator: u64, denominator: u64) -> i64 {
    if denominator == 0 {
        return 0;
    }
    let value = (u128::from(numerator) << 32) / u128::from(denominator);
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Consolidate `insights`, then pick those whose context matches
/// `context_signature` by at least `knobs.min_match_micros`, at most
/// `knobs.max_selected`. Order: match descending, `success_count` descending,
/// `insight_id` ascending. The threshold test is exact integer arithmetic.
pub fn select_insights(
    insights: &[Insight],
    context_signature: &str,
    knobs: &InsightKnobs,
) -> Vec<SelectedInsight> {
    let mut chosen: Vec<(u64, u64, SelectedInsight)> = consolidate_insights(insights)
        .into_iter()
        .filter_map(|insight| {
            let (intersection, union) = jaccard(&insight.context_signature, context_signature);
            let min_match = u64::try_from(knobs.min_match_micros).unwrap_or(0);
            let meets = union > 0
                && u128::from(intersection) * 1_000_000
                    >= u128::from(min_match) * u128::from(union);
            meets.then(|| {
                let match_q32 = q32_ratio(intersection, union);
                (intersection, union, SelectedInsight { insight, match_q32 })
            })
        })
        .collect();
    chosen.sort_by(|(ai, au, a), (bi, bu, b)| {
        // Compare the exact fractions, then the recorded order keys.
        (u128::from(*bi) * u128::from(*au))
            .cmp(&(u128::from(*ai) * u128::from(*bu)))
            .then(b.insight.success_count.cmp(&a.insight.success_count))
            .then(a.insight.insight_id.cmp(&b.insight.insight_id))
    });
    chosen
        .into_iter()
        .take(knobs.max_selected)
        .map(|(_, _, selected)| selected)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::program_store::insight::INSIGHT_SCHEMA_VERSION;

    fn insight(id: &str, text: &str, runs: &[&str], context: &str, successes: u32) -> Insight {
        Insight {
            schema_version: INSIGHT_SCHEMA_VERSION,
            insight_id: id.into(),
            text: text.into(),
            source_run_ids: BoundedVec::new(runs.iter().map(|run| run.to_string()).collect())
                .unwrap(),
            context_signature: context.into(),
            success_count: successes,
            selection_revision: 1,
        }
    }

    fn knobs(min_match_micros: i64, max_selected: usize) -> InsightKnobs {
        InsightKnobs {
            min_match_micros,
            max_selected,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R135.11
    #[test]
    fn consolidation_and_selection_order() {
        let fixture = vec![
            insight(
                "b",
                "Retry the  export once",
                &["r2", "r3"],
                "export report",
                2,
            ),
            insight(
                "a",
                "retry the export ONCE",
                &["r1", "r2"],
                "export report",
                3,
            ),
            insight("c", "check the quota", &["r9"], "export report monthly", 7),
            insight("d", "pin the schema", &["r5"], "export report", 3),
            insight("e", "unrelated hint", &["r6"], "billing run", 9),
            insight("f", "warm the cache", &["r7"], "report", 3),
        ];

        // Consolidation: equal text after lowercase + whitespace collapse is one
        // insight; the lowest id survives, runs are unioned, counts are summed.
        let merged = consolidate_insights(&fixture);
        assert_eq!(merged.len(), 5);
        let retry = merged.iter().find(|i| i.insight_id == "a").unwrap();
        assert_eq!(
            retry.source_run_ids.iter().collect::<Vec<_>>(),
            ["r1", "r2", "r3"]
        );
        assert_eq!(retry.success_count, 5);
        assert!(merged.iter().all(|i| i.insight_id != "b"));

        // Selection: context "export report". Jaccard: a=1, d=1, c=2/3, f=1/2,
        // e=0. Order: match desc, then success_count desc, then id asc.
        let selected = select_insights(&fixture, "export report", &knobs(300_000, 8));
        let ids: Vec<&str> = selected
            .iter()
            .map(|s| s.insight.insight_id.as_str())
            .collect();
        assert_eq!(ids, ["a", "d", "c", "f"]);
        assert_eq!(selected[0].match_q32, 1_i64 << 32);
        assert_eq!(selected[2].match_q32, (2_i64 << 32) / 3);
        assert_eq!(selected[3].match_q32, 1_i64 << 31);

        // Equal match and equal success_count order by insight_id.
        let tie = vec![
            insight("y", "second", &["r1"], "export report", 4),
            insight("x", "first", &["r1"], "export report", 4),
        ];
        let tied = select_insights(&tie, "export report", &knobs(0, 8));
        assert_eq!(
            tied.iter()
                .map(|s| s.insight.insight_id.as_str())
                .collect::<Vec<_>>(),
            ["x", "y"]
        );

        // The threshold is inclusive and exact: 1/2 passes at 0.5, fails above.
        let half = select_insights(&fixture, "export report", &knobs(500_000, 8));
        assert!(half.iter().any(|s| s.insight.insight_id == "f"));
        let above = select_insights(&fixture, "export report", &knobs(500_001, 8));
        assert!(above.iter().all(|s| s.insight.insight_id != "f"));

        // max_selected truncates after ordering.
        let top = select_insights(&fixture, "export report", &knobs(300_000, 2));
        assert_eq!(
            top.iter()
                .map(|s| s.insight.insight_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "d"]
        );

        // Nothing matches an empty context; nothing is selected from no insights.
        assert!(select_insights(&fixture, "", &knobs(1, 8)).is_empty());
        assert!(select_insights(&[], "export report", &knobs(0, 8)).is_empty());
    }
}
