//! Daily paper target measured over ingest stage receipts
//! (MEDULLA-REPO-INGEST-R006.1).
//!
//! The ingest worker's ack path records one `:StageReceipt` node per stage
//! outcome for one paper revision. [`measure_window`] counts a paper once
//! when all four stages completed inside the half-open window
//! `[window_end_ms - 86_400_000, window_end_ms)`; retries add nothing. The
//! state is [`ThroughputState::Unknown`] for a partial window (no telemetry
//! at all) or stale telemetry (the newest receipt older than the
//! `ingest.window_fresh_for` knob). Below target, `bottleneck_stage` names
//! the stage with the lowest distinct completions.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// One stage of a paper's ingest pipeline, in pipeline order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum IngestStage {
    Fetch,
    Extract,
    Embed,
    Index,
}

impl IngestStage {
    /// All four stages, in pipeline order.
    pub const ALL: [Self; 4] = [Self::Fetch, Self::Extract, Self::Embed, Self::Index];
}

/// The `:StageReceipt` node payload: one stage's outcome for one paper
/// revision, acknowledged by the ingest worker's ack path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StageReceipt {
    pub stage: IngestStage,
    pub paper_id: String,
    pub revision: u64,
    pub attempted: bool,
    pub completed: bool,
    pub duplicate: bool,
    pub elapsed_ms: u64,
    pub completed_at_ms: u64,
    pub source_commit: String,
}

/// Whether a measured window can be trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ThroughputState {
    /// The window is complete: fresh telemetry covers its tail.
    Complete,
    /// A partial window (no telemetry) or stale telemetry: the count is not
    /// trustworthy and the window reports no measurements.
    Unknown,
}

/// One measured daily window for a corpus.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ThroughputWindow {
    pub completed_distinct: u64,
    pub target: u64,
    pub shortfall: u64,
    pub bottleneck_stage: Option<IngestStage>,
    pub state: ThroughputState,
}

/// `knob ingest.daily_paper_target`: papers per 24 h window.
pub const INGEST_DAILY_PAPER_TARGET_KNOB: &str = "ingest.daily_paper_target";
/// Default of `knob ingest.daily_paper_target` (papers per window).
pub const DAILY_PAPER_TARGET_DEFAULT: u64 = 2_000;
/// Inclusive bounds of `knob ingest.daily_paper_target`.
pub const DAILY_PAPER_TARGET_RANGE: (u64, u64) = (1, 1_000_000);
/// Environment override of `knob ingest.daily_paper_target`.
pub const ENV_DAILY_PAPER_TARGET: &str = "MEDULLA_INGEST_DAILY_PAPER_TARGET";

/// `knob ingest.window_fresh_for`: newest-receipt freshness in seconds.
pub const INGEST_WINDOW_FRESH_FOR_KNOB: &str = "ingest.window_fresh_for";
/// Default of `knob ingest.window_fresh_for` (seconds).
pub const WINDOW_FRESH_FOR_DEFAULT_S: u64 = 300;
/// Inclusive bounds of `knob ingest.window_fresh_for` (seconds).
pub const WINDOW_FRESH_FOR_RANGE_S: (u64, u64) = (10, 3_600);
/// Environment override of `knob ingest.window_fresh_for` (seconds).
pub const ENV_WINDOW_FRESH_FOR_S: &str = "MEDULLA_INGEST_WINDOW_FRESH_FOR_S";

/// The measured half-open window, in milliseconds: 24 h.
pub const WINDOW_MS: u64 = 24 * 60 * 60 * 1_000;

/// True when `value` is a valid `knob ingest.daily_paper_target`.
pub fn valid_daily_paper_target(value: u64) -> bool {
    (DAILY_PAPER_TARGET_RANGE.0..=DAILY_PAPER_TARGET_RANGE.1).contains(&value)
}

/// True when `value_s` is a valid `knob ingest.window_fresh_for`.
pub fn valid_window_fresh_for_s(value_s: u64) -> bool {
    (WINDOW_FRESH_FOR_RANGE_S.0..=WINDOW_FRESH_FOR_RANGE_S.1).contains(&value_s)
}

fn stage_index(stage: IngestStage) -> usize {
    match stage {
        IngestStage::Fetch => 0,
        IngestStage::Extract => 1,
        IngestStage::Embed => 2,
        IngestStage::Index => 3,
    }
}

/// Measure the daily paper target over the corpus's stage receipts.
///
/// A paper counts once when all four stages completed inside the half-open
/// window `[window_end_ms - 86_400_000, window_end_ms)`; retries (repeated
/// receipts for the same paper and stage) add nothing. `newest_receipt_at_ms`
/// is the newest `completed_at_ms` observed across the corpus's stage
/// receipts (completed or attempted); pass `None` when the corpus has no
/// receipts at all. The state is [`ThroughputState::Unknown`] when there is
/// no telemetry (partial window) or when that newest receipt is older than
/// `window_end_ms - window_fresh_for_ms` (stale telemetry); an unknown window
/// reports `completed_distinct` 0, `shortfall` equal to the target and no
/// bottleneck. Below target, `bottleneck_stage` is the stage with the lowest
/// distinct completions, ties broken in pipeline order.
pub fn measure_window(
    receipts: &[StageReceipt],
    target: u64,
    window_fresh_for_ms: u64,
    newest_receipt_at_ms: Option<u64>,
    window_end_ms: u64,
) -> ThroughputWindow {
    let window_start_ms = window_end_ms.saturating_sub(WINDOW_MS);
    let fresh_floor_ms = window_end_ms.saturating_sub(window_fresh_for_ms);

    // Partial window (no telemetry) or stale telemetry: report nothing.
    let known = match newest_receipt_at_ms {
        Some(newest) => newest >= fresh_floor_ms,
        None => false,
    };
    if !known {
        return ThroughputWindow {
            completed_distinct: 0,
            target,
            shortfall: target,
            bottleneck_stage: None,
            state: ThroughputState::Unknown,
        };
    }

    let mut papers: BTreeMap<String, BTreeSet<IngestStage>> = BTreeMap::new();
    let mut stage_papers: [BTreeSet<String>; 4] = Default::default();
    for receipt in receipts.iter().filter(|receipt| {
        receipt.completed
            && receipt.completed_at_ms >= window_start_ms
            && receipt.completed_at_ms < window_end_ms
    }) {
        papers
            .entry(receipt.paper_id.clone())
            .or_default()
            .insert(receipt.stage);
        stage_papers[stage_index(receipt.stage)].insert(receipt.paper_id.clone());
    }
    let completed_distinct = papers
        .values()
        .filter(|stages| stages.len() == IngestStage::ALL.len())
        .count() as u64;
    let bottleneck_stage = if completed_distinct < target {
        let mut best = IngestStage::Fetch;
        let mut best_count = stage_papers[0].len();
        for stage in &IngestStage::ALL[1..] {
            let count = stage_papers[stage_index(*stage)].len();
            if count < best_count {
                best = *stage;
                best_count = count;
            }
        }
        Some(best)
    } else {
        None
    };
    ThroughputWindow {
        completed_distinct,
        target,
        shortfall: target.saturating_sub(completed_distinct),
        bottleneck_stage,
        state: ThroughputState::Complete,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW_END: u64 = WINDOW_MS; // window [0, 86_400_000)
    const FRESH_MS: u64 = WINDOW_FRESH_FOR_DEFAULT_S * 1_000;

    fn receipt(stage: IngestStage, paper_id: &str, completed_at_ms: u64) -> StageReceipt {
        StageReceipt {
            stage,
            paper_id: paper_id.to_string(),
            revision: 1,
            attempted: true,
            completed: true,
            duplicate: false,
            elapsed_ms: 100,
            completed_at_ms,
            source_commit: "c1".to_string(),
        }
    }

    fn all_stages(paper_id: &str, completed_at_ms: u64) -> Vec<StageReceipt> {
        IngestStage::ALL
            .iter()
            .map(|stage| receipt(*stage, paper_id, completed_at_ms))
            .collect()
    }

    // spec: MEDULLA-REPO-INGEST-R006.1
    #[test]
    fn window_counts_distinct_papers_and_reports_unknown() {
        let mut receipts: Vec<StageReceipt> = Vec::new();
        for i in 0..1_999u64 {
            receipts.extend(all_stages(&format!("paper-{i:04}"), 1_000 + i));
        }
        let window = measure_window(
            &receipts,
            2_000,
            FRESH_MS,
            Some(WINDOW_END - 1_000),
            WINDOW_END,
        );
        assert_eq!(window.state, ThroughputState::Complete);
        assert_eq!(window.completed_distinct, 1_999);
        assert_eq!(window.target, 2_000);
        assert_eq!(window.shortfall, 1);
        // All four stages tie at 1999 distinct completions: pipeline order wins.
        assert_eq!(window.bottleneck_stage, Some(IngestStage::Fetch));

        // The 2000th paper completes all four stages inside the window.
        receipts.extend(all_stages("paper-2000", WINDOW_END - 500));
        let window = measure_window(
            &receipts,
            2_000,
            FRESH_MS,
            Some(WINDOW_END - 1_000),
            WINDOW_END,
        );
        assert_eq!(window.state, ThroughputState::Complete);
        assert_eq!(window.completed_distinct, 2_000);
        assert_eq!(window.shortfall, 0);
        assert_eq!(window.bottleneck_stage, None);
    }

    // spec: MEDULLA-REPO-INGEST-R006.1
    #[test]
    fn retries_and_duplicates_add_nothing() {
        let mut receipts = all_stages("paper-1", 1_000);
        // Retries of the same paper and stage, plus a flagged duplicate.
        for at in [2_000u64, 3_000, 4_000] {
            let mut retried = receipt(IngestStage::Embed, "paper-1", at);
            retried.revision = 2;
            receipts.push(retried);
        }
        let mut duplicate = receipt(IngestStage::Index, "paper-1", 5_000);
        duplicate.duplicate = true;
        receipts.push(duplicate);
        // A second paper that completes only three stages never counts.
        let mut partial = all_stages("paper-2", 1_000);
        partial.pop(); // drop Index
        receipts.extend(partial);

        let window = measure_window(
            &receipts,
            2_000,
            FRESH_MS,
            Some(WINDOW_END - 1_000),
            WINDOW_END,
        );
        assert_eq!(window.completed_distinct, 1);
        assert_eq!(window.shortfall, 1_999);
        // Index completed for one paper only.
        assert_eq!(window.bottleneck_stage, Some(IngestStage::Index));
    }

    // spec: MEDULLA-REPO-INGEST-R006.1
    #[test]
    fn the_window_is_half_open_at_both_edges() {
        // Completed exactly at the start counts; exactly at the end does not.
        let inside = all_stages("paper-1", 0);
        let window = measure_window(&inside, 1, FRESH_MS, Some(WINDOW_END - 1_000), WINDOW_END);
        assert_eq!(window.completed_distinct, 1);

        let outside = all_stages("paper-1", WINDOW_END);
        let window = measure_window(&outside, 1, FRESH_MS, Some(WINDOW_END - 1_000), WINDOW_END);
        assert_eq!(window.completed_distinct, 0);
    }

    // spec: MEDULLA-REPO-INGEST-R006.1
    #[test]
    fn a_partial_window_reports_unknown() {
        let window = measure_window(&[], 2_000, FRESH_MS, None, WINDOW_END);
        assert_eq!(window.state, ThroughputState::Unknown);
        assert_eq!(window.completed_distinct, 0);
        assert_eq!(window.shortfall, 2_000);
        assert_eq!(window.bottleneck_stage, None);
    }

    // spec: MEDULLA-REPO-INGEST-R006.1
    #[test]
    fn stale_telemetry_reports_unknown_even_with_in_window_completions() {
        // All four stages of one paper completed inside the window, but the
        // newest receipt is two hours old — far beyond the 300 s knob.
        let at = WINDOW_END - 2 * 60 * 60 * 1_000;
        let receipts = all_stages("paper-1", at);
        let window = measure_window(&receipts, 2_000, FRESH_MS, Some(at), WINDOW_END);
        assert_eq!(window.state, ThroughputState::Unknown);
        assert_eq!(window.completed_distinct, 0);
        assert_eq!(window.shortfall, 2_000);
        assert_eq!(window.bottleneck_stage, None);
    }

    // spec: MEDULLA-REPO-INGEST-R006.1
    #[test]
    fn telemetry_exactly_at_the_fresh_floor_is_not_stale() {
        let at = WINDOW_END - FRESH_MS;
        let receipts = all_stages("paper-1", at);
        let window = measure_window(&receipts, 2_000, FRESH_MS, Some(at), WINDOW_END);
        assert_eq!(window.state, ThroughputState::Complete);
        assert_eq!(window.completed_distinct, 1);
    }

    #[test]
    fn the_knobs_carry_their_defaults_and_ranges() {
        assert_eq!(INGEST_DAILY_PAPER_TARGET_KNOB, "ingest.daily_paper_target");
        assert_eq!(DAILY_PAPER_TARGET_DEFAULT, 2_000);
        assert!(valid_daily_paper_target(1));
        assert!(valid_daily_paper_target(1_000_000));
        assert!(!valid_daily_paper_target(0));
        assert!(!valid_daily_paper_target(1_000_001));

        assert_eq!(INGEST_WINDOW_FRESH_FOR_KNOB, "ingest.window_fresh_for");
        assert_eq!(WINDOW_FRESH_FOR_DEFAULT_S, 300);
        assert!(valid_window_fresh_for_s(10));
        assert!(valid_window_fresh_for_s(3_600));
        assert!(!valid_window_fresh_for_s(9));
        assert!(!valid_window_fresh_for_s(3_601));
    }

    #[test]
    fn a_failed_attempt_never_counts_as_a_completion() {
        let mut failed = all_stages("paper-1", 1_000);
        let index = failed
            .iter_mut()
            .find(|r| r.stage == IngestStage::Index)
            .unwrap();
        index.completed = false;
        let window = measure_window(
            &failed,
            2_000,
            FRESH_MS,
            Some(WINDOW_END - 1_000),
            WINDOW_END,
        );
        assert_eq!(window.state, ThroughputState::Complete);
        assert_eq!(window.completed_distinct, 0);
        assert_eq!(window.bottleneck_stage, Some(IngestStage::Index));
    }
}
