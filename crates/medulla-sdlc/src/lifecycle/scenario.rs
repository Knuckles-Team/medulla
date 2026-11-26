//! Scenario runs gate phases and release bundles (MEDULLA-SDLC-R013).
//!
//! A release bundle is complete when every scoped row is delivered (it meets the delivered
//! qualification, which VERIFIED alone does not) and the latest `ScenarioRun` of its phase on
//! `main` passed. [`bundle_completeness`] is that rule as a pure function: the served read
//! lists the stored runs, the caller passes the rows it found delivered, and the function
//! answers which rows are missing and what the phase's latest run said.

use std::collections::BTreeSet;

use medulla_source_types::lifecycle::{
    ScenarioResult, StoredScenarioRun, SCENARIO_RUN_DEFAULT_BRANCH,
};

/// The completeness of one release bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleCompleteness {
    /// Every scoped row is delivered and the phase's latest run on `main` passed.
    pub complete: bool,
    /// The scoped rows that are not delivered, sorted.
    pub undelivered_rows: Vec<String>,
    /// The latest run of the phase on `main`; `None` when the phase has none.
    pub latest_run: Option<StoredScenarioRun>,
}

/// The latest run of `phase` on `main`: the greatest engine instant, with the run id breaking
/// a tie so the answer never depends on the order of `runs`.
pub fn latest_main_run<'a>(
    phase: &str,
    runs: &'a [StoredScenarioRun],
) -> Option<&'a StoredScenarioRun> {
    runs.iter()
        .filter(|stored| {
            stored.run.phase == phase && stored.run.branch == SCENARIO_RUN_DEFAULT_BRANCH
        })
        .max_by(|a, b| a.recorded_ms.cmp(&b.recorded_ms).then(a.id.cmp(&b.id)))
}

/// Decide whether a bundle of `scoped_rows` is complete for `phase`. `delivered` holds the
/// rows that meet the delivered qualification; a scoped row outside it is undelivered.
pub fn bundle_completeness(
    scoped_rows: &[String],
    delivered: &BTreeSet<String>,
    phase: &str,
    runs: &[StoredScenarioRun],
) -> BundleCompleteness {
    let mut undelivered_rows: Vec<String> = scoped_rows
        .iter()
        .filter(|row| !delivered.contains(*row))
        .cloned()
        .collect();
    undelivered_rows.sort();
    undelivered_rows.dedup();
    let latest_run = latest_main_run(phase, runs).cloned();
    let passed = latest_run
        .as_ref()
        .is_some_and(|stored| stored.run.result == ScenarioResult::Passed);
    BundleCompleteness {
        complete: undelivered_rows.is_empty() && passed,
        undelivered_rows,
        latest_run,
    }
}
