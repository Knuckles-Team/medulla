// CONCEPT:MEDULLA-KG.mining.process-mining — Directly-follows graph + alpha-miner-lite.
//
// Pure-Rust, dependency-light: given a set of ORDERED event traces (each a
// time-ordered list of interned activity ids — an activity may repeat within a
// trace), build the **directly-follows graph** (DFG): a directed edge `a -> b`
// weighted by how many times `a` is immediately followed by `b` across all
// traces. `alpha_lite` derives the classic alpha-algorithm LOG ABSTRACTION (the
// "footprint matrix") from the DFG:
//
//   * **causal** (`a > b`): `a` directly-follows `b` somewhere but `b` never
//     directly-follows `a` — a one-way dependency (sequential flow `a -> b`).
//   * **parallel** (`a || b`): BOTH `a > b` and `b > a` occur — the two
//     activities interleave (concurrent flow, no fixed order).
//   * **choice** (`a # b`): neither directly-follows the other anywhere — the
//     two activities never appear adjacent (an exclusive branch, or unrelated).
//
// plus the trace-level **start** / **end** activity sets. This is the standard
// alpha-algorithm footprint (Van der Aalst), stopping short of the full
// place/transition Petri-net construction — a documented "lite" scope: the
// footprint alone is already the queryable process MODEL this family writes
// back (a `:ProcessModel` claim), and the Petri-net synthesis step adds no
// further epistemic content over the relations it's built from.

use std::collections::HashMap;

pub type ActivityId = u32;

/// The directly-follows graph: dense activity ids `0..activities.len()` and the
/// `(from, to, count)` edges observed across all traces.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectlyFollowsGraph {
    pub n_activities: usize,
    /// `(from, to, count)`, sorted by `(from, to)`.
    pub edges: Vec<(ActivityId, ActivityId, usize)>,
}

/// The alpha-miner-lite footprint + start/end sets over a [`DirectlyFollowsGraph`].
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessModel {
    pub n_activities: usize,
    pub dfg_edges: Vec<(ActivityId, ActivityId, usize)>,
    /// One-way directly-follows pairs `(a, b)`: `a > b` and NOT `b > a`.
    pub causal: Vec<(ActivityId, ActivityId)>,
    /// Two-way directly-follows pairs `(a, b)` with `a < b` (each pair listed once).
    pub parallel: Vec<(ActivityId, ActivityId)>,
    /// Activities that open at least one trace.
    pub start_activities: Vec<ActivityId>,
    /// Activities that close at least one trace.
    pub end_activities: Vec<ActivityId>,
}

/// Build the directly-follows graph over interned traces (CONCEPT:MEDULLA-KG.mining.process-mining).
pub fn mine_dfg(traces: &[Vec<ActivityId>], n_activities: usize) -> DirectlyFollowsGraph {
    let mut counts: HashMap<(ActivityId, ActivityId), usize> = HashMap::new();
    for trace in traces {
        for w in trace.windows(2) {
            *counts.entry((w[0], w[1])).or_insert(0) += 1;
        }
    }
    let mut edges: Vec<(ActivityId, ActivityId, usize)> =
        counts.into_iter().map(|((a, b), c)| (a, b, c)).collect();
    edges.sort_unstable();
    DirectlyFollowsGraph {
        n_activities,
        edges,
    }
}

/// Derive the alpha-miner-lite footprint + start/end sets (CONCEPT:MEDULLA-KG.mining.process-mining).
pub fn alpha_lite(traces: &[Vec<ActivityId>], n_activities: usize) -> ProcessModel {
    let dfg = mine_dfg(traces, n_activities);
    let mut follows: std::collections::HashSet<(ActivityId, ActivityId)> =
        std::collections::HashSet::new();
    for &(a, b, _) in &dfg.edges {
        follows.insert((a, b));
    }

    let mut causal = Vec::new();
    let mut parallel = Vec::new();
    // Only compare pairs that appear in EITHER direction (a footprint over
    // OBSERVED activities, not the full n×n cross product of all possible ids).
    let mut seen_pairs: std::collections::BTreeSet<(ActivityId, ActivityId)> =
        std::collections::BTreeSet::new();
    for &(a, b) in &follows {
        seen_pairs.insert((a.min(b), a.max(b)));
    }
    for (lo, hi) in seen_pairs {
        let fwd = follows.contains(&(lo, hi));
        let bwd = follows.contains(&(hi, lo));
        match (fwd, bwd) {
            (true, true) => parallel.push((lo, hi)),
            (true, false) => causal.push((lo, hi)),
            (false, true) => causal.push((hi, lo)),
            (false, false) => {} // unreachable given seen_pairs construction
        }
    }
    causal.sort_unstable();
    parallel.sort_unstable();

    let mut starts: std::collections::BTreeSet<ActivityId> = std::collections::BTreeSet::new();
    let mut ends: std::collections::BTreeSet<ActivityId> = std::collections::BTreeSet::new();
    for trace in traces {
        if let Some(&first) = trace.first() {
            starts.insert(first);
        }
        if let Some(&last) = trace.last() {
            ends.insert(last);
        }
    }

    ProcessModel {
        n_activities,
        dfg_edges: dfg.edges,
        causal,
        parallel,
        start_activities: starts.into_iter().collect(),
        end_activities: ends.into_iter().collect(),
    }
}

/// Token counts of one footprint replay (MEDULLA-REPO-INGEST-R002.651.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReplayCounts {
    /// Non-empty traces replayed.
    pub traces: u64,
    /// Tokens produced: the start token plus one per fired activity, per trace.
    pub produced: u64,
    /// Tokens consumed: one per fired activity plus the final token, per trace.
    pub consumed: u64,
    /// Tokens that had to be created: an unlisted start, or an unlisted step.
    pub missing: u64,
    /// Tokens left behind: a trace that does not end on an end activity.
    pub remaining: u64,
}

impl ReplayCounts {
    /// `round(1000 * ((1 - missing/consumed) + (1 - remaining/produced)) / 2)`; 1000 when
    /// nothing was replayed.
    pub fn fitness_milli(&self) -> u32 {
        if self.consumed == 0 || self.produced == 0 {
            return 1000;
        }
        let missing = 1.0 - self.missing as f64 / self.consumed as f64;
        let remaining = 1.0 - self.remaining as f64 / self.produced as f64;
        (1000.0 * (missing + remaining) / 2.0).round().clamp(0.0, 1000.0) as u32
    }
}

/// Replay `traces` on the footprint of `model` read as an implicit net with one place per
/// directly-follows step. Per non-empty trace `a1..an`: `produced` and `consumed` grow by
/// `n + 1`; `missing` grows by one when `a1` is not a start activity and by one for each step
/// `(ai, ai+1)` in neither `causal` (in that order) nor `parallel` (either order), which
/// includes any step touching an activity the model does not list; `remaining` grows by one
/// when `an` is not an end activity. Empty traces are skipped and not counted.
pub fn footprint_replay(model: &ProcessModel, traces: &[Vec<ActivityId>]) -> ReplayCounts {
    let mut counts = ReplayCounts::default();
    for trace in traces {
        let (Some(&first), Some(&last)) = (trace.first(), trace.last()) else {
            continue;
        };
        let steps = trace.len() as u64;
        counts.traces += 1;
        counts.produced += steps + 1;
        counts.consumed += steps + 1;
        if !model.start_activities.contains(&first) {
            counts.missing += 1;
        }
        for pair in trace.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let allowed = model.causal.contains(&(a, b))
                || model.parallel.contains(&(a, b))
                || model.parallel.contains(&(b, a));
            if !allowed {
                counts.missing += 1;
            }
        }
        if !model.end_activities.contains(&last) {
            counts.remaining += 1;
        }
    }
    counts
}

/// String-labeled convenience: intern → mine → relabel, returning `(labels,
/// model)` so a caller can round-trip `ActivityId`s back to their original
/// strings. Reuses [`super::association::intern`] (same first-seen interning
/// scheme every other string-labeled mining family uses).
pub fn alpha_lite_labeled(traces: &[Vec<String>]) -> (Vec<String>, ProcessModel) {
    let (interned, labels) = super::association::intern(traces);
    let model = alpha_lite(&interned, labels.len());
    (labels, model)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Classic loan-process fixture: `register -> check -> decide -> {accept|reject}`.
    fn traces() -> Vec<Vec<ActivityId>> {
        vec![
            vec![0, 1, 2, 3], // register, check, decide, accept
            vec![0, 1, 2, 4], // register, check, decide, reject
            vec![0, 1, 2, 3],
        ]
    }

    // spec: MEDULLA-REPO-INGEST-R002.651.4
    #[test]
    fn footprint_replay_counts_missing_and_remaining_tokens() {
        // register(0) -> check(1) -> decide(2) -> accept(3) | reject(4)
        let log: Vec<Vec<ActivityId>> = vec![vec![0, 1, 2, 3], vec![0, 1, 2, 4]];
        let model = alpha_lite(&log, 5);
        // A conforming trace consumes and produces every token.
        let conforming = footprint_replay(&model, &[vec![0, 1, 2, 3]]);
        assert_eq!(
            conforming,
            ReplayCounts {
                traces: 1,
                produced: 5,
                consumed: 5,
                missing: 0,
                remaining: 0
            }
        );
        assert_eq!(conforming.fitness_milli(), 1000);
        // A reversed step (2 -> 1) is missing, and ending on 1 leaves a token behind.
        let deviating = footprint_replay(&model, &[vec![0, 2, 1], vec![]]);
        assert_eq!(deviating.traces, 1, "empty traces are skipped");
        assert_eq!((deviating.produced, deviating.consumed), (4, 4));
        // 0 -> 2 is not a footprint step and 2 -> 1 is reversed; 1 is no end activity.
        assert_eq!((deviating.missing, deviating.remaining), (2, 1));
        assert!(deviating.fitness_milli() < 1000);
        // An unknown activity is missing as a start and as a step, and is no end.
        let unknown = footprint_replay(&model, &[vec![9, 0]]);
        assert_eq!((unknown.missing, unknown.remaining), (2, 1));
        // A parallel pair is allowed in either order.
        let parallel_model = alpha_lite(&[vec![0, 1, 2], vec![0, 2, 1]], 3);
        let either = footprint_replay(&parallel_model, &[vec![0, 2, 1]]);
        assert_eq!(either.missing, 0);
    }

    #[test]
    fn dfg_counts_directly_follows_pairs() {
        let dfg = mine_dfg(&traces(), 5);
        let get = |a, b| {
            dfg.edges
                .iter()
                .find(|&&(x, y, _)| x == a && y == b)
                .map(|&(_, _, c)| c)
        };
        assert_eq!(get(0, 1), Some(3));
        assert_eq!(get(1, 2), Some(3));
        assert_eq!(get(2, 3), Some(2));
        assert_eq!(get(2, 4), Some(1));
        assert_eq!(get(3, 4), None); // never directly follow each other
    }

    #[test]
    fn alpha_lite_footprint_classifies_causal_and_choice() {
        let model = alpha_lite(&traces(), 5);
        assert!(model.causal.contains(&(0, 1)));
        assert!(model.causal.contains(&(1, 2)));
        assert!(model.causal.contains(&(2, 3)));
        assert!(model.causal.contains(&(2, 4)));
        // accept(3) and reject(4) never appear adjacent in any trace ⇒ choice
        // (neither direction in `follows`), so NOT in causal or parallel.
        assert!(!model
            .causal
            .iter()
            .any(|&(a, b)| (a, b) == (3, 4) || (a, b) == (4, 3)));
        assert!(!model
            .parallel
            .iter()
            .any(|&(a, b)| (a, b) == (3, 4) || (a, b) == (4, 3)));
        assert_eq!(model.start_activities, vec![0]);
        assert_eq!(model.end_activities, vec![3, 4]);
    }

    #[test]
    fn alpha_lite_detects_parallel_activities() {
        // b and c interleave in either order after a, both leading to d.
        let traces = vec![vec![0, 1, 2, 3], vec![0, 2, 1, 3]];
        let model = alpha_lite(&traces, 4);
        assert!(model.parallel.contains(&(1, 2)));
        assert!(!model
            .causal
            .iter()
            .any(|&(a, b)| (a, b) == (1, 2) || (a, b) == (2, 1)));
    }

    #[test]
    fn labeled_roundtrip_recovers_activity_names() {
        let traces = vec![
            vec![
                "register".to_string(),
                "check".to_string(),
                "accept".to_string(),
            ],
            vec![
                "register".to_string(),
                "check".to_string(),
                "accept".to_string(),
            ],
        ];
        let (labels, model) = alpha_lite_labeled(&traces);
        assert_eq!(labels.len(), 3);
        assert!(labels.contains(&"register".to_string()));
        assert_eq!(model.start_activities.len(), 1);
        assert_eq!(model.end_activities.len(), 1);
    }
}
