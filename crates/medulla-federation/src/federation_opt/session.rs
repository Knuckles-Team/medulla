//! The per-query federation state: budget meter, fragment trace, and the LIMIT hints the
//! lookahead pass derived from the plan about to run.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use medulla_exec::algebra::Op;

use super::budget::BudgetMeter;
use super::cache::FragmentCacheScope;
use super::snapshot::{Consistency, SnapshotBound, SnapshotHandle};
use super::stats::Fingerprint;
use super::trace::FragmentTrace;
use medulla_exec::budget::FederationBudget;
use medulla_types::wire::ForeignSourceSpec;

/// One foreign source op of the plan and the limit that immediately follows EVERY
/// occurrence of it (`None` when an occurrence has no `Limit` or the limits disagree —
/// the op is then fetched unlimited, which is always correct).
#[derive(Debug)]
struct Hint {
    op: Op,
    limit: Option<usize>,
}

#[derive(Debug)]
struct SessionState {
    meter: BudgetMeter,
    trace: Vec<FragmentTrace>,
    hints: Vec<Hint>,
    /// The text of the `RankText` branch each search-source `ForeignScan` is fused with.
    search_texts: Vec<(Op, String)>,
    cache_scope: Option<Arc<FragmentCacheScope>>,
    /// How many foreign fragments of each source spec the prepared plan holds.
    source_counts: Vec<(ForeignSourceSpec, usize)>,
    /// One snapshot per source, opened by the first fragment and dropped with the session.
    snapshots: HashMap<Fingerprint, SnapshotHandle>,
    bound: SnapshotBound,
    consistency: Consistency,
}

/// One federated query's optimizer state, bound on the `PlanCtx` with
/// [`medulla_exec::exec::PlanCtx::with_federation`]. A served query creates one per request;
/// without one, each foreign op runs under a fresh default budget and records no trace.
#[derive(Debug)]
pub struct FederationSession {
    state: Mutex<SessionState>,
}

impl FederationSession {
    /// A session spending at most `budget`.
    pub fn new(budget: FederationBudget) -> Self {
        Self {
            state: Mutex::new(SessionState {
                meter: BudgetMeter::new(budget),
                trace: Vec::new(),
                hints: Vec::new(),
                search_texts: Vec::new(),
                cache_scope: None,
                source_counts: Vec::new(),
                snapshots: HashMap::new(),
                bound: SnapshotBound::default(),
                consistency: Consistency::default(),
            }),
        }
    }

    /// A session under the server budget ([`FederationBudget::from_env`]).
    pub fn from_env() -> Self {
        Self::new(FederationBudget::from_env())
    }

    /// The session is per query: a fragment that panicked mid-update already failed the
    /// query, so a poisoned lock is not recovered.
    fn lock(&self) -> MutexGuard<'_, SessionState> {
        self.state
            .lock()
            .expect("federation session lock poisoned: a fragment of this query panicked")
    }

    /// The budget this session enforces.
    pub fn budget(&self) -> FederationBudget {
        self.lock().meter.budget()
    }

    /// Attach the verified caller's fresh source checkpoints. Standalone plans
    /// retain no cache authority, even when they register a source by name.
    pub fn set_cache_scope(&self, scope: Option<Arc<FragmentCacheScope>>) {
        self.lock().cache_scope = scope;
    }

    pub(super) fn cache_scope(&self) -> Option<Arc<FragmentCacheScope>> {
        self.lock().cache_scope.clone()
    }

    /// Apply an untrusted request hint only as a field-wise reduction of the server cap.
    /// UQL calls this before its first fragment; no hint can grant extra network work.
    pub fn narrow_budget(&self, requested: FederationBudget) {
        self.lock().meter.narrow(requested);
    }

    /// The bounds snapshots of this query open under (the server knobs).
    pub fn set_snapshot_bound(&self, bound: SnapshotBound) {
        self.lock().bound = bound;
    }

    pub(super) fn snapshot_bound(&self) -> SnapshotBound {
        self.lock().bound
    }

    /// Accept per-fragment reads for this query (the request field `consistency`).
    pub fn set_consistency(&self, consistency: Consistency) {
        self.lock().consistency = consistency;
    }

    /// How this query's fragments of one source relate in time.
    pub fn consistency(&self) -> Consistency {
        self.lock().consistency
    }

    /// How many foreign fragments of `source` the prepared plan holds.
    pub(super) fn fragments_of(&self, source: &ForeignSourceSpec) -> usize {
        self.lock()
            .source_counts
            .iter()
            .find(|(spec, _)| spec == source)
            .map_or(0, |(_, n)| *n)
    }

    /// The snapshot already open for a source, if an earlier fragment opened it.
    pub(super) fn snapshot_of(&self, fingerprint: &Fingerprint) -> Option<SnapshotHandle> {
        self.lock().snapshots.get(fingerprint).cloned()
    }

    pub(super) fn hold_snapshot(&self, fingerprint: Fingerprint, handle: SnapshotHandle) {
        self.lock().snapshots.insert(fingerprint, handle);
    }

    /// Every fragment executed so far, in execution order.
    pub fn trace(&self) -> Vec<FragmentTrace> {
        self.lock().trace.clone()
    }

    /// Derive the LIMIT hints of `ops` (and of every nested `FuseRrf` branch): a source op
    /// (`ForeignScan { join: false }` / `Foreign`) immediately followed by `Limit k`.
    pub fn prepare(&self, ops: &[Op]) {
        let mut hints = Vec::new();
        collect_hints(ops, &mut hints);
        let mut counts = Vec::new();
        count_sources(ops, &mut counts);
        let mut texts = Vec::new();
        collect_search_texts(ops, &mut texts);
        let mut state = self.lock();
        state.search_texts = texts;
        state.hints = hints;
        state.source_counts = counts;
    }

    /// The limit that may be pushed into `op`'s source.
    pub(crate) fn limit_hint(&self, op: &Op) -> Option<usize> {
        self.lock()
            .hints
            .iter()
            .find(|h| &h.op == op)
            .and_then(|h| h.limit)
    }

    /// The full-text query pushed into the search source `op`: the `RankText` query of the
    /// `FuseRrf` that holds `op` as a branch (MEDULLA-UNIFIED-DATA-PLANE-R038.5.3).
    pub(crate) fn search_text(&self, op: &Op) -> Option<String> {
        self.lock()
            .search_texts
            .iter()
            .find(|(known, _)| known == op)
            .map(|(_, text)| text.clone())
    }

    /// Run `f` against the budget meter.
    pub(crate) fn with_meter<R>(&self, f: impl FnOnce(&mut BudgetMeter) -> R) -> R {
        f(&mut self.lock().meter)
    }

    /// Append one fragment's trace.
    pub fn record(&self, fragment: FragmentTrace) {
        tracing::debug!(fragment = ?fragment, "federation fragment (EH-563)");
        self.lock().trace.push(fragment);
    }
}

/// Is `op` a foreign SOURCE op (one whose rows replace the input)?
fn is_foreign_source(op: &Op) -> bool {
    matches!(op, Op::ForeignScan { join: false, .. } | Op::Foreign { .. })
}

/// The nested op sequences of `op` (the `FuseRrf` branches), each its own plan.
fn nested_branches(op: &Op) -> &[Vec<Op>] {
    #[cfg(feature = "text")]
    if let Op::FuseRrf { branches, .. } = op {
        return branches;
    }
    let _ = op;
    &[]
}

/// Count every foreign fragment of `ops` (and of every nested branch) by source spec.
fn count_sources(ops: &[Op], counts: &mut Vec<(ForeignSourceSpec, usize)>) {
    for op in ops {
        for branch in nested_branches(op) {
            count_sources(branch, counts);
        }
        let spec = match op {
            Op::ForeignScan { source, .. } => (**source).clone(),
            Op::Foreign { name } => ForeignSourceSpec::Named { name: name.clone() },
            _ => continue,
        };
        match counts.iter_mut().find(|(known, _)| *known == spec) {
            Some((_, n)) => *n += 1,
            None => counts.push((spec, 1)),
        }
    }
}

fn collect_hints(ops: &[Op], hints: &mut Vec<Hint>) {
    for (i, op) in ops.iter().enumerate() {
        for branch in nested_branches(op) {
            collect_hints(branch, hints);
        }
        if matches!(op, Op::ForeignScan { .. } | Op::Foreign { .. }) {
            let limit = match ops.get(i + 1) {
                Some(Op::Limit { k }) if is_foreign_source(op) => Some(*k),
                _ => None,
            };
            record_occurrence(hints, op, limit);
        }
    }
}

/// Merge one occurrence: every occurrence of an op must agree on its limit.
fn record_occurrence(hints: &mut Vec<Hint>, op: &Op, limit: Option<usize>) {
    match hints.iter_mut().find(|h| &h.op == op) {
        Some(existing) if existing.limit != limit => existing.limit = None,
        Some(_) => {}
        None => hints.push(Hint {
            op: op.clone(),
            limit,
        }),
    }
}

/// Pair every `ForeignScan` branch of a `FuseRrf` with the `RankText` query of a sibling branch.
fn collect_search_texts(ops: &[Op], out: &mut Vec<(Op, String)>) {
    for op in ops {
        let branches = nested_branches(op);
        for branch in branches {
            collect_search_texts(branch, out);
        }
        let text = branches.iter().flatten().find_map(|inner| match inner {
            #[cfg(feature = "text")]
            Op::RankText { query } => Some(query.clone()),
            _ => None,
        });
        let Some(text) = text else { continue };
        for scan in branches.iter().flatten() {
            if matches!(scan, Op::ForeignScan { .. }) && !out.iter().any(|(known, _)| known == scan)
            {
                out.push((scan.clone(), text.clone()));
            }
        }
    }
}
