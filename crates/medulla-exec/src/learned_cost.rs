//! KAN-learned cardinality-estimate correction (CONCEPT:MEDULLA-KG.query.adaptive-reoptimization,
//! W4.12 phase 2 — "planner feedback + learned costs").
//!
//! Phase 1 (`optimizer::reoptimize_remaining`, auto-wired into
//! [`crate::exec::run_with_adaptive_reopt`]) is purely REACTIVE and purely PER-QUERY:
//! it re-costs a plan's not-yet-run tail the moment an earlier op's ACTUAL cardinality
//! blows the plan-time estimate, then forgets everything. If op X in this graph
//! systematically under/over-estimates (a hub whose fan-out the degree-AVERAGE
//! estimator can never see coming), the NEXT query pays the identical miss — and the
//! identical re-plan churn — all over again.
//!
//! This module closes that loop. [`LearnedCostStore`] holds one small KAN edge
//! function per op-kind — `medulla_graph_algos::graphlearn::edge_fn::KanEdgeFn`, the SAME
//! polynomial-basis primitive W4.2's link-predictor trains (reused, not
//! reinvented) — trained ONLINE from the identical `(estimated, actual)` pairs
//! [`crate::exec::run_with_adaptive_reopt`] already computes every op: no extra
//! instrumentation, no offline job, no new data path. [`crate::cost::ModalityCardinality::rows_out`]
//! applies the learned correction to its static estimate before returning it, so a
//! bias learned on query N nudges the ESTIMATE — not just the reactive re-plan — on
//! query N+1. That is "the learned correction adjusts future estimates" from the task
//! spec.
//!
//! ## Why a curve per op-kind, not one global correction
//!
//! A `Traverse`'s degree-average miss and a `Filter`'s selectivity miss are different
//! error DISTRIBUTIONS; lumping them into one correction would have each contaminate
//! the other's fit. [`op_kind`] buckets by the SAME per-modality split
//! [`crate::cost::ModalityCardinality::rows_out`] already uses.
//!
//! ## Why log-space, residual-around-identity
//!
//! Each curve fits `ln(actual) ≈ ln(estimated) + f(ln(estimated))` rather than
//! `actual ≈ g(estimated)` directly: log-space keeps a multiplicative bias (a
//! systematic 10× under-count) a SMOOTH, small-magnitude target regardless of scale,
//! and the RESIDUAL-around-identity form means a freshly-zeroed curve (`f ≡ 0`, every
//! coefficient `0.0`) is an EXACT no-op — flipping [`enabled`] on for the first time
//! never perturbs an as-yet-untrained op-kind's estimate ([`KanEdgeFn::zeros`]).
//!
//! ## Opt-in, two gates
//!
//! 1. **Compile-time** — the `learned-cost` cargo feature (pulls `medulla-compute`'s
//!    dependency-light `graphlearn` domain). A default/Pi build links none of it.
//! 2. **Runtime** — the `MEDULLA_LEARNED_COST=1` env flag ([`enabled`], read
//!    fresh on every call, mirroring [`crate::optimizer::enabled`]'s convention). A
//!    build that compiles the capability in still ships it OFF by default: every
//!    existing planner/exec test, and every deployment that doesn't opt in, sees
//!    BYTE-FOR-BYTE the pre-existing static-estimate behavior.
//!
//! ## Queryable artifact
//!
//! [`LearnedCostStore::snapshot`] (and the free function [`artifacts`]) returns one
//! [`CostCorrectionArtifact`] per trained op-kind — the KAN basis/degree/coefficients
//! (the interpretable curve itself, exactly as `KanEdgeFn`'s own docs describe) plus a
//! sample count and the running static-vs-learned error — a plain
//! `Serialize`/`Deserialize` struct (proven by `artifact_json_roundtrip` below) ready
//! for a caller to expose however it likes. A server-side KG writeback mirroring the
//! graphlearn `:EdgeFunction` node convention (`src/server/handlers/graphlearn.rs`) is
//! the natural next step, but that write path needs a mutable `GraphCore` + protocol/
//! dispatch plumbing that lives outside medulla-plan's boundary — logged as a follow-up in
//! `reports/issue-register.md` rather than built here.

use medulla_graph_algos::{graphlearn::edge_fn::Basis, graphlearn::edge_fn::KanEdgeFn};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use medulla_query_types::wire::Op;

/// Runtime opt-in switch (CONCEPT:MEDULLA-KG.query.adaptive-reoptimization). Checked fresh on every
/// call — no caching — exactly like [`crate::optimizer::enabled`], so a test (or an
/// operator) can flip the env var and see the effect on the very next call. Unset, or
/// anything other than the literal `"1"`, means disabled: the capability is compiled
/// in but inert.
pub fn enabled() -> bool {
    matches!(
        std::env::var("MEDULLA_LEARNED_COST").ok().as_deref(),
        Some("1")
    )
}

/// Learning rate for the single-sample online gradient step [`Correction::observe`]
/// takes per `(estimated, actual)` pair. Small enough that one wild outlier cannot
/// swing the curve; large enough that a genuinely systematic miss visibly corrects
/// within the handful of observations one hot op-kind produces in real traffic.
const LEARNING_RATE: f64 = 0.05;

/// Degree of the per-op-kind Chebyshev correction curve. Deliberately small: a
/// cardinality miss is a smooth, roughly log-linear systematic bias (an under-counted
/// hub, a stale histogram), not a function needing many degrees of freedom — and a
/// low-degree curve stays well-behaved under one-sample-at-a-time SGD.
const CORRECTION_DEGREE: usize = 3;

/// Numeric floor so `ln(0)` never happens on a genuinely empty estimate/actual.
const EPS: f64 = 1e-6;

/// EWMA smoothing for the running error numbers [`CostCorrectionArtifact`] reports —
/// recent samples dominate (so the artifact reflects the curve's CURRENT fit) without
/// a single outlier swamping it.
const EWMA_ALPHA: f64 = 0.2;

/// One op-kind's learned correction curve plus the bookkeeping
/// [`CostCorrectionArtifact`] reports (CONCEPT:MEDULLA-KG.query.adaptive-reoptimization). See the
/// module doc for why log-space + residual-around-identity.
#[derive(Clone, Debug)]
struct Correction {
    kan: KanEdgeFn,
    samples: u64,
    /// Running EWMA of `|ln(actual) - ln(estimated)|` — the STATIC model's error,
    /// unaffected by learning (the "A" side of the A/B comparison).
    ewma_static_abs_err: f64,
    /// Running EWMA of `|ln(actual) - (ln(estimated) + kan.eval(ln(estimated)))|`,
    /// evaluated with the curve AS IT STOOD just before that sample corrected it —
    /// the "B" side.
    ewma_learned_abs_err: f64,
}

impl Correction {
    fn new() -> Self {
        Self {
            kan: KanEdgeFn::zeros(Basis::Chebyshev, CORRECTION_DEGREE),
            samples: 0,
            ewma_static_abs_err: 0.0,
            ewma_learned_abs_err: 0.0,
        }
    }

    /// Apply the learned residual to a raw estimate: `exp(x + f(x))` with
    /// `x = ln(estimated)`. Clamped to a bounded multiplicative window around the
    /// input (`[estimate * 1e-4, estimate * 1e4]`) so a still-training (or
    /// pathologically fit) curve can never hand a downstream permutation search a
    /// wildly-off-scale number — the correction can move an estimate, never blow it
    /// up or collapse it to zero.
    fn correct(&self, estimated: f64) -> f64 {
        let est = estimated.max(EPS);
        let x = est.ln();
        let y = x + self.kan.eval(x);
        y.exp().clamp(est * 1e-4, est * 1e4)
    }

    /// One online SGD step from a fresh `(estimated, actual)` observation — squared
    /// log-error loss `L = (y - (x + f(x)))^2`, so `dL/dcoeffs[k] = 2·residual·B_k(u)`
    /// where `residual = (x + f(x)) - y` and `B_k(u)` is exactly
    /// [`KanEdgeFn::grad_coeffs`] (its own doc: `∂f/∂coeffs[k] = B_k(u)`). Returns the
    /// `(static_abs_err, learned_abs_err)` pair for THIS sample (using the curve as it
    /// stood before this step), the A/B harness's per-sample numbers.
    fn observe(&mut self, estimated: f64, actual: f64) -> (f64, f64) {
        let est = estimated.max(EPS);
        let act = actual.max(EPS);
        let x = est.ln();
        let y = act.ln();

        let static_err = (y - x).abs();
        let learned_pred = x + self.kan.eval(x);
        let learned_err = (y - learned_pred).abs();

        let residual = learned_pred - y;
        let grad = self.kan.grad_coeffs(x);
        for (c, g) in self.kan.coeffs.iter_mut().zip(grad.iter()) {
            *c -= LEARNING_RATE * residual * g;
        }

        self.samples += 1;
        let a = if self.samples == 1 { 1.0 } else { EWMA_ALPHA };
        self.ewma_static_abs_err = a * static_err + (1.0 - a) * self.ewma_static_abs_err;
        self.ewma_learned_abs_err = a * learned_err + (1.0 - a) * self.ewma_learned_abs_err;

        (static_err, learned_err)
    }
}

/// The op-kind tag [`LearnedCostStore`] buckets corrections by
/// (CONCEPT:MEDULLA-KG.query.adaptive-reoptimization) — mirrors EXACTLY the per-modality split
/// [`crate::cost::ModalityCardinality::rows_out`] already draws (same ops, same feature
/// gates), so a curve only ever trains on the modality it names. `None` for an op this
/// tier has no real cardinality MODEL for (`Limit`'s output is exact arithmetic —
/// `in_card.min(k)` — nothing to learn; every other passthrough op in `rows_out`'s
/// wildcard arm) — the caller skips learning for those rather than fitting noise.
pub fn op_kind(op: &Op) -> Option<&'static str> {
    match op {
        Op::Scan { .. } => Some("Scan"),
        Op::Filter { .. } => Some("Filter"),
        Op::Traverse { .. } => Some("Traverse"),
        Op::Rank { .. } | Op::RankEmbed { .. } => Some("Rank"),
        Op::AsOf { .. } => Some("AsOf"),
        #[cfg(feature = "owl")]
        Op::Reason { .. } => Some("Reason"),
        #[cfg(feature = "text")]
        Op::FuseRrf { .. } => Some("FuseRrf"),
        #[cfg(feature = "text")]
        Op::RankText { .. } => Some("RankText"),
        _ => None,
    }
}

/// A registry of per-op-kind [`Correction`] curves (CONCEPT:MEDULLA-KG.query.adaptive-reoptimization).
/// Production code reaches the single process-wide instance via [`Self::global`];
/// tests and standalone tools construct their OWN isolated instance via [`Self::new`]
/// so concurrent `cargo test` runs never share (and therefore never contaminate) each
/// other's learned state.
pub struct LearnedCostStore {
    corrections: RwLock<HashMap<String, Correction>>,
}

/// Take the corrections lock even after a holder panicked, and say so.
///
/// The curves are advisory and re-learnable: an untrained curve is the exact
/// identity, and every later observation keeps fitting from whatever state is
/// there, so a torn gradient step costs a few mis-corrected estimates, never a
/// wrong answer. Serving continues. The report is what keeps the recovery from
/// being silent. `medulla-plan` sits below the root crate's `lock_recovery`, so it
/// reports locally, like the derived-tensor CAS in `exec.rs`.
fn recover_corrections<G>(locked: std::sync::LockResult<G>) -> G {
    // Invariant `reporting-recovery`: docs/architecture/liveness_invariants.md.
    #[allow(clippy::disallowed_methods)]
    locked.unwrap_or_else(|poisoned| {
        eprintln!(
            "medulla-plan: recovered the poisoned learned-cost corrections lock; a previous \
             holder panicked mid-update, so one curve may carry a partial gradient step. \
             The curves are advisory and re-learnable, so serving continues."
        );
        poisoned.into_inner()
    })
}

impl Default for LearnedCostStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LearnedCostStore {
    /// A fresh, empty, independent store — use this in tests/tools; production code
    /// wants [`Self::global`] so learning persists across queries.
    pub fn new() -> Self {
        Self {
            corrections: RwLock::new(HashMap::new()),
        }
    }

    /// The single process-wide instance [`crate::cost::ModalityCardinality::rows_out`]
    /// and [`crate::exec::run_with_adaptive_reopt`] read/write when [`enabled`] is
    /// true. Deliberately NOT scoped to a [`crate::exec::PlanCtx`]/[`crate::cost::PlanStats`]
    /// snapshot (unlike `PlanStats`'s per-view memo): the whole point of this module is
    /// that a miss learned on query N adjusts query N+1's estimate, and a fresh
    /// `PlanCtx`/`GraphView` is built per query / per committed write — tying the store
    /// to either would silently reset learning on every one.
    pub fn global() -> &'static LearnedCostStore {
        static STORE: OnceLock<LearnedCostStore> = OnceLock::new();
        STORE.get_or_init(LearnedCostStore::new)
    }

    /// Apply `op_kind`'s learned correction to a raw estimate. An op-kind with no
    /// observations yet returns `estimated` unchanged (an untrained curve IS the
    /// identity per [`Correction::new`]'s zero-init, but skipping the lookup entirely
    /// for a never-seen key avoids allocating an entry just to compute a no-op).
    pub fn correct(&self, op_kind: &str, estimated: f64) -> f64 {
        let guard = recover_corrections(self.corrections.read());
        match guard.get(op_kind) {
            Some(c) => c.correct(estimated),
            None => estimated,
        }
    }

    /// Record one `(estimated, actual)` observation for `op_kind` and take one online
    /// gradient step, creating the curve on first sight. Returns the
    /// `(static_abs_log_error, learned_abs_log_error)` pair for this sample — see
    /// [`Correction::observe`].
    pub fn observe(&self, op_kind: &str, estimated: f64, actual: f64) -> (f64, f64) {
        let mut guard = recover_corrections(self.corrections.write());
        let entry = guard
            .entry(op_kind.to_string())
            .or_insert_with(Correction::new);
        entry.observe(estimated, actual)
    }

    /// The queryable-artifact surface (CONCEPT:MEDULLA-KG.query.adaptive-reoptimization) — one
    /// [`CostCorrectionArtifact`] per op-kind observed so far, in no particular order.
    /// See the module doc's "Queryable artifact" section.
    pub fn snapshot(&self) -> Vec<CostCorrectionArtifact> {
        let guard = recover_corrections(self.corrections.read());
        guard
            .iter()
            .map(|(kind, c)| CostCorrectionArtifact {
                op_kind: kind.clone(),
                basis: c.kan.basis,
                degree: c.kan.degree,
                coeffs: c.kan.coeffs.clone(),
                samples: c.samples,
                mean_abs_log_error_static: c.ewma_static_abs_err,
                mean_abs_log_error_learned: c.ewma_learned_abs_err,
            })
            .collect()
    }
}

/// One op-kind's learned correction curve, serialized for external inspection
/// (CONCEPT:MEDULLA-KG.query.adaptive-reoptimization) — "stored as queryable artifacts" per the task
/// spec. Plain data (`Serialize`/`Deserialize`), so any caller — a test, a future MCP
/// tool, a server-side KG writeback mirroring the graphlearn `:EdgeFunction`
/// convention — can inspect *why* a cardinality estimate for this op-kind now differs
/// from the static model: `coeffs` (with `basis`/`degree`) IS the interpretable curve,
/// exactly as with `KanEdgeFn` itself, and `mean_abs_log_error_{static,learned}` is
/// the running evidence that it helps.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CostCorrectionArtifact {
    /// The [`Op`] discriminant this curve corrects (`"Filter"`, `"Traverse"`, …) — see
    /// [`op_kind`].
    pub op_kind: String,
    pub basis: Basis,
    pub degree: usize,
    /// The learned basis coefficients — the interpretable curve itself.
    pub coeffs: Vec<f64>,
    /// Observations folded into this curve so far.
    pub samples: u64,
    /// Running EWMA of the STATIC model's `|ln(actual) - ln(estimated)|` — unaffected
    /// by learning, the "A" side of the A/B comparison.
    pub mean_abs_log_error_static: f64,
    /// Running EWMA of the LEARNED (KAN-corrected) estimate's absolute log-error,
    /// each sample scored against the curve as it stood just before that sample
    /// updated it — the "B" side.
    pub mean_abs_log_error_learned: f64,
}

/// The process-wide store's queryable-artifact surface (CONCEPT:MEDULLA-KG.query.adaptive-reoptimization)
/// — `LearnedCostStore::global().snapshot()`. See the module doc's "Queryable
/// artifact" section.
pub fn artifacts() -> Vec<CostCorrectionArtifact> {
    LearnedCostStore::global().snapshot()
}

/// The estimated and measured cost of one executed operator
/// (MEDULLA-FEDERATED-QUERY-R732.1). Only an operator with an [`op_kind`] is costed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OpCost {
    /// The learned-cost bucket of the operator ([`op_kind`]).
    pub op_kind: &'static str,
    /// The planner's estimate of the operator's output rows.
    pub estimated: f64,
    /// The operator's measured wall time in microseconds.
    pub measured_us: u64,
}

/// One served plan run, always the champion's (MEDULLA-FEDERATED-QUERY-R732.1).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlanExecutionRecord {
    /// The tenant whose read ran the plan.
    pub tenant: String,
    /// Digest of the plan with every literal value replaced by `null`.
    pub shape_digest: String,
    /// Digest of the executed plan.
    pub plan_digest: String,
    /// The per-operator costs, in execution order.
    pub op_costs: Vec<OpCost>,
    /// The sum of the operators' estimates.
    pub estimated_cost: f64,
    /// The measured wall time of the run in microseconds.
    pub measured_cost_us: u64,
    /// Rows the run returned.
    pub result_rows: u64,
    /// `RowSet::result_digest` of the returned rows, hex.
    pub result_digest: String,
    /// Digest of the planner knob values in force.
    pub knob_revision: String,
    /// Digest of the cost head (the correction table) in force.
    pub cost_head_revision: String,
    /// Epoch milliseconds the run was recorded.
    pub recorded_at_ms: i64,
}

/// Collects the per-operator costs of one run; bound on the [`crate::exec::PlanCtx`] so
/// both drivers report through the one adaptive loop.
#[derive(Debug, Default)]
pub struct OpCostRecorder {
    costs: Mutex<Vec<OpCost>>,
}

impl OpCostRecorder {
    /// Record one executed operator; an operator with no [`op_kind`] is not costed.
    pub fn record(&self, op: &Op, estimated: f64, measured_us: u64) {
        let Some(kind) = op_kind(op) else { return };
        recover_corrections(self.costs.lock()).push(OpCost {
            op_kind: kind,
            estimated,
            measured_us,
        });
    }

    /// The costs recorded so far, in order.
    pub fn costs(&self) -> Vec<OpCost> {
        recover_corrections(self.costs.lock()).clone()
    }
}

/// Digest of an executed plan (MEDULLA-FEDERATED-QUERY-R732.1): `eg/plan-record/v1` over
/// the MessagePack of the ops.
pub fn plan_digest(ops: &[Op]) -> Result<String, String> {
    let bytes = rmp_serde::to_vec_named(ops).map_err(|error| format!("plan digest: {error}"))?;
    medulla_types::contract::Digest256::framed(b"eg/plan-record/v1", &[&bytes])
        .map(|digest| digest.to_hex())
}

/// Digest of a plan's shape: [`plan_digest`] with every literal value replaced by `null`.
pub fn shape_digest(ops: &[Op]) -> Result<String, String> {
    fn blank(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                let literal = map.len() == 1
                    && ["Str", "Num", "Bool"]
                        .iter()
                        .any(|tag| map.contains_key(*tag));
                if literal {
                    *value = serde_json::Value::Null;
                } else {
                    map.values_mut().for_each(blank);
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(blank),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(ops).map_err(|error| format!("shape digest: {error}"))?;
    blank(&mut value);
    let bytes = rmp_serde::to_vec(&value).map_err(|error| format!("shape digest: {error}"))?;
    medulla_types::contract::Digest256::framed(b"eg/plan-record/v1", &[&bytes])
        .map(|digest| digest.to_hex())
}

/// One revision of the cost head: the correction curves plus the planner knob values
/// (MEDULLA-FEDERATED-QUERY-R732.2). Its content digest is the `cost_head_revision` a
/// [`PlanExecutionRecord`] names.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CostHeadRevision {
    /// The content digest of `corrections` and `knobs`.
    pub digest: String,
    /// The correction curves, one per op kind.
    pub corrections: Vec<CostCorrectionArtifact>,
    /// The planner knob values, name to value.
    pub knobs: Vec<(String, String)>,
}

/// The digest of a cost head over its curves and knobs (MEDULLA-FEDERATED-QUERY-R732.2),
/// defined once in the decision types so the component store verifies the same bytes.
fn revision_digest(corrections: &[CostCorrectionArtifact], knobs: &[(String, String)]) -> String {
    let curves: Vec<(&str, &[f64])> = corrections
        .iter()
        .map(|c| (c.op_kind.as_str(), c.coeffs.as_slice()))
        .collect();
    let knobs: Vec<(&str, &str)> = knobs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    medulla_decision_statistical_types::decision::planner_cost_head::cost_head_digest(
        &curves, &knobs,
    )
}

impl CostHeadRevision {
    /// A revision over `corrections` and `knobs`; the digest is derived from them.
    pub fn new(corrections: Vec<CostCorrectionArtifact>, knobs: Vec<(String, String)>) -> Self {
        Self {
            digest: revision_digest(&corrections, &knobs),
            corrections,
            knobs,
        }
    }

    /// The correction table in force when no revision is published: the process
    /// [`LearnedCostStore`] as it stands.
    pub fn builtin() -> Self {
        Self::new(LearnedCostStore::global().snapshot(), Vec::new())
    }

    /// The revision's corrected estimate for `op_kind`; an op kind with no curve in the
    /// revision keeps its raw estimate.
    pub fn correct(&self, op_kind: &str, estimated: f64) -> f64 {
        let Some(curve) = self.corrections.iter().find(|c| c.op_kind == op_kind) else {
            return estimated;
        };
        let mut kan = KanEdgeFn::zeros(curve.basis, curve.degree);
        kan.coeffs = curve.coeffs.clone();
        let est = estimated.max(EPS);
        let x = est.ln();
        (x + kan.eval(x)).exp().clamp(est * 1e-4, est * 1e4)
    }
}

/// One shadow run of a challenger cost head beside the champion's served run
/// (MEDULLA-FEDERATED-QUERY-R732.3). The challenger's rows are digested and dropped.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ShadowComparison {
    /// The tenant whose read was shadowed.
    pub tenant: String,
    /// The shape digest of the plan.
    pub shape_digest: String,
    /// The plan the champion ran.
    pub champion_plan_digest: String,
    /// The plan the challenger ran.
    pub challenger_plan_digest: String,
    /// The champion run's estimated cost.
    pub champion_estimated_cost: f64,
    /// The challenger run's estimated cost.
    pub challenger_estimated_cost: f64,
    /// The champion run's measured cost in microseconds.
    pub champion_cost_us: u64,
    /// The challenger run's measured cost in microseconds.
    pub challenger_cost_us: u64,
    /// The challenger's `RowSet::result_digest`, hex; `None` when it failed.
    pub challenger_result_digest: Option<String>,
    /// Whether the challenger's rows equal the champion's.
    pub result_match: bool,
    /// Whether the challenger errored or panicked.
    pub challenger_failed: bool,
    /// Epoch milliseconds the comparison was recorded.
    pub recorded_at_ms: i64,
}

/// The default of the `query.planner_shadow_fraction` knob.
pub const DEFAULT_SHADOW_FRACTION: f64 = 0.05;

/// The bounded in-memory store of served plan runs (MEDULLA-FEDERATED-QUERY-R732.1): a
/// planner cache like [`LearnedCostStore`], rebuilt after a restart and never copied to
/// another store. Each tenant keeps at most the `query.plan_records` knob's records; the
/// oldest is dropped first, and one tenant never reads another's records.
pub struct PlanRecordStore {
    per_tenant_cap: AtomicUsize,
    records: Mutex<HashMap<String, VecDeque<PlanExecutionRecord>>>,
    champion: RwLock<Option<Arc<CostHeadRevision>>>,
    challenger: RwLock<Option<Arc<CostHeadRevision>>>,
    shadows: Mutex<HashMap<String, VecDeque<ShadowComparison>>>,
    shadow_fraction_bits: AtomicU64,
}

/// The default of the `query.plan_records` knob.
pub const DEFAULT_PLAN_RECORDS: usize = 4_096;

impl PlanRecordStore {
    /// An empty store keeping `per_tenant_cap` records per tenant.
    pub fn new(per_tenant_cap: usize) -> Self {
        Self {
            per_tenant_cap: AtomicUsize::new(per_tenant_cap.max(1)),
            records: Mutex::new(HashMap::new()),
            champion: RwLock::new(None),
            challenger: RwLock::new(None),
            shadows: Mutex::new(HashMap::new()),
            shadow_fraction_bits: AtomicU64::new(DEFAULT_SHADOW_FRACTION.to_bits()),
        }
    }

    /// The process store.
    pub fn global() -> &'static PlanRecordStore {
        static STORE: OnceLock<PlanRecordStore> = OnceLock::new();
        STORE.get_or_init(|| PlanRecordStore::new(DEFAULT_PLAN_RECORDS))
    }

    /// Bind the per-tenant bound (the server sets it from `query.plan_records` at startup).
    pub fn set_per_tenant_cap(&self, cap: usize) {
        self.per_tenant_cap.store(cap.max(1), Ordering::Relaxed);
    }

    /// Keep one record under its tenant, dropping that tenant's oldest past the bound.
    pub fn record(&self, record: PlanExecutionRecord) {
        let cap = self.per_tenant_cap.load(Ordering::Relaxed);
        let mut guard = recover_corrections(self.records.lock());
        let queue = guard.entry(record.tenant.clone()).or_default();
        queue.push_back(record);
        while queue.len() > cap {
            queue.pop_front();
        }
    }

    /// The tenant's records, oldest first.
    pub fn records(&self, tenant: &str) -> Vec<PlanExecutionRecord> {
        recover_corrections(self.records.lock())
            .get(tenant)
            .map(|queue| queue.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The champion cost head the planner reads (MEDULLA-FEDERATED-QUERY-R732.2): the
    /// published revision, else the built-in correction table.
    pub fn champion(&self) -> Arc<CostHeadRevision> {
        match self.published_champion() {
            Some(revision) => revision,
            None => Arc::new(CostHeadRevision::builtin()),
        }
    }

    /// The published champion revision; `None` while the built-in table is in force.
    pub fn published_champion(&self) -> Option<Arc<CostHeadRevision>> {
        recover_corrections(self.champion.read()).clone()
    }

    /// The challenger revision, which only a shadow run reads.
    pub fn challenger(&self) -> Option<Arc<CostHeadRevision>> {
        recover_corrections(self.challenger.read()).clone()
    }

    /// Set (or clear) the challenger. A challenger never answers a query. Shadow comparisons
    /// measure one challenger against one champion, so a different challenger starts with
    /// none.
    pub fn set_challenger(&self, revision: Option<CostHeadRevision>) {
        let mut slot = recover_corrections(self.challenger.write());
        let held = slot.as_ref().map(|held| held.digest.as_str());
        if held != revision.as_ref().map(|next| next.digest.as_str()) {
            recover_corrections(self.shadows.lock()).clear();
        }
        *slot = revision.map(Arc::new);
    }

    /// Move the champion pointer to `revision` (MEDULLA-FEDERATED-QUERY-R732.5); the
    /// challenger slot is cleared when it held that revision.
    pub fn promote(&self, revision: CostHeadRevision) {
        let mut challenger = recover_corrections(self.challenger.write());
        if challenger
            .as_ref()
            .is_some_and(|held| held.digest == revision.digest)
        {
            *challenger = None;
        }
        *recover_corrections(self.champion.write()) = Some(Arc::new(revision));
        // The comparisons measured the previous champion.
        recover_corrections(self.shadows.lock()).clear();
    }

    /// Bind the fraction of interactive plans that also run under the challenger (the server
    /// sets it from `query.planner_shadow_fraction` at startup).
    pub fn set_shadow_fraction(&self, fraction: f64) {
        self.shadow_fraction_bits
            .store(fraction.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Whether a plan with this digest is in the shadow sample: its first eight bytes, as
    /// a fraction of the digest space, fall below the bound fraction.
    pub fn in_shadow_sample(&self, plan_digest: &str) -> bool {
        let fraction = f64::from_bits(self.shadow_fraction_bits.load(Ordering::Relaxed));
        let Some(head) = plan_digest
            .get(..16)
            .and_then(|h| u64::from_str_radix(h, 16).ok())
        else {
            return false;
        };
        (head as f64 / u64::MAX as f64) < fraction
    }

    /// Keep one shadow comparison under its tenant, with the same bound as the records.
    pub fn record_shadow(&self, comparison: ShadowComparison) {
        let cap = self.per_tenant_cap.load(Ordering::Relaxed);
        let mut guard = recover_corrections(self.shadows.lock());
        let queue = guard.entry(comparison.tenant.clone()).or_default();
        queue.push_back(comparison);
        while queue.len() > cap {
            queue.pop_front();
        }
    }

    /// The tenant's shadow comparisons, oldest first.
    pub fn shadows(&self, tenant: &str) -> Vec<ShadowComparison> {
        recover_corrections(self.shadows.lock())
            .get(tenant)
            .map(|queue| queue.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Every tenant's shadow comparisons, oldest first (ties by tenant): the pool the planner
    /// promotion is judged on (MEDULLA-FEDERATED-QUERY-R732.5).
    pub fn all_shadows(&self) -> Vec<ShadowComparison> {
        let mut all: Vec<ShadowComparison> = recover_corrections(self.shadows.lock())
            .values()
            .flat_map(|queue| queue.iter().cloned())
            .collect();
        all.sort_by(|a, b| (a.recorded_at_ms, &a.tenant).cmp(&(b.recorded_at_ms, &b.tenant)));
        all
    }

    /// Every tenant that has a record.
    pub fn tenants(&self) -> Vec<String> {
        let mut tenants: Vec<String> = recover_corrections(self.records.lock())
            .keys()
            .cloned()
            .collect();
        tenants.sort();
        tenants
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(tenant: &str, serial: u64) -> PlanExecutionRecord {
        PlanExecutionRecord {
            tenant: tenant.into(),
            shape_digest: "shape".into(),
            plan_digest: format!("plan-{serial}"),
            op_costs: Vec::new(),
            estimated_cost: 1.0,
            measured_cost_us: serial,
            result_rows: 0,
            result_digest: String::new(),
            knob_revision: String::new(),
            cost_head_revision: String::new(),
            recorded_at_ms: serial as i64,
        }
    }

    fn artifact(kind: &str, offset: f64) -> CostCorrectionArtifact {
        CostCorrectionArtifact {
            op_kind: kind.into(),
            basis: Basis::Chebyshev,
            degree: CORRECTION_DEGREE,
            coeffs: vec![offset, 0.0, 0.0, 0.0],
            samples: 1,
            mean_abs_log_error_static: 0.0,
            mean_abs_log_error_learned: 0.0,
        }
    }

    #[test]
    // spec: MEDULLA-FEDERATED-QUERY-R732.2
    fn champion_revision_digest_names_the_corrections_in_use() {
        let store = PlanRecordStore::new(8);
        // With nothing published the champion is the built-in table.
        assert!(store.published_champion().is_none());
        assert_eq!(store.champion().digest, CostHeadRevision::builtin().digest);

        let doubling = CostHeadRevision::new(vec![artifact("Scan", 2f64.ln())], Vec::new());
        let tripling = CostHeadRevision::new(vec![artifact("Scan", 3f64.ln())], Vec::new());
        assert_ne!(doubling.digest, tripling.digest);
        assert_eq!(
            doubling.digest,
            revision_digest(&doubling.corrections, &doubling.knobs)
        );
        let tuned = CostHeadRevision::new(
            doubling.corrections.clone(),
            vec![("join_cost".into(), "2".into())],
        );
        assert_ne!(
            doubling.digest, tuned.digest,
            "knob values are part of the digest"
        );

        // A challenger never changes what the planner reads.
        store.promote(doubling.clone());
        store.set_challenger(Some(tripling.clone()));
        assert_eq!(store.champion().digest, doubling.digest);
        assert_eq!(
            store.challenger().map(|c| c.digest.clone()),
            Some(tripling.digest.clone())
        );
        assert!((store.champion().correct("Scan", 100.0) - 200.0).abs() < 1e-6);
        assert!((tripling.correct("Scan", 100.0) - 300.0).abs() < 1e-6);
        assert_eq!(store.champion().correct("Filter", 7.0), 7.0);

        // Promotion moves the pointer and empties the slot that held the revision.
        store.promote(tripling.clone());
        assert_eq!(store.champion().digest, tripling.digest);
        assert!(store.challenger().is_none());
    }

    #[test]
    // spec: MEDULLA-FEDERATED-QUERY-R732.1
    fn plan_record_store_bounds_and_shadow_error() {
        let store = PlanRecordStore::new(3);
        for serial in 0..5 {
            store.record(record("t1", serial));
        }
        store.record(record("t2", 100));
        let kept: Vec<u64> = store
            .records("t1")
            .iter()
            .map(|r| r.measured_cost_us)
            .collect();
        assert_eq!(kept, vec![2, 3, 4], "the oldest are evicted first");
        let other = store.records("t2");
        assert_eq!(other.len(), 1);
        assert!(other.iter().all(|r| r.tenant == "t2"));
        assert!(store.records("t3").is_empty());
        assert_eq!(store.tenants(), vec!["t1".to_string(), "t2".to_string()]);
        store.set_per_tenant_cap(1);
        store.record(record("t1", 9));
        assert_eq!(store.records("t1").len(), 1);
    }

    /// A freshly-observed op-kind's FIRST correction (before any training) is the
    /// identity: `KanEdgeFn::zeros` means `f ≡ 0`, so `correct` returns (within
    /// floating-point round-trip through `ln`/`exp`) exactly the input estimate. This
    /// is the property that makes flipping [`enabled`] on for the first time safe —
    /// no existing plan's estimates jump on the very first query.
    #[test]
    fn untrained_curve_is_identity() {
        let store = LearnedCostStore::new();
        // No `observe` yet: `op_kind` isn't even in the map, so `correct` short-circuits
        // to the raw estimate.
        assert_eq!(store.correct("Traverse", 250.0), 250.0);

        // After ONE `observe` (which creates the zero-init curve and takes one gradient
        // step), a FRESH estimate this curve has never seen is still very close to
        // identity — the single step nudges the curve toward the ONE sample it just
        // saw, not toward a generic input.
        store.observe("Traverse", 250.0, 250.0); // no miss at all: gradient is ~0.
        let corrected = store.correct("Traverse", 999.0);
        assert!(
            (corrected - 999.0).abs() < 1.0,
            "an on-target observation must not perturb the curve: got {corrected}"
        );
    }

    /// The core claim: online training on a SYSTEMATIC miss (here, actual is always
    /// ~8x the estimate — a stand-in for a hub whose real fan-out the degree-average
    /// estimator can't see) measurably reduces the learned arm's error relative to the
    /// static arm, and the learned CORRECTION converges toward actually predicting the
    /// bias on held-out estimates the curve was never directly trained on.
    #[test]
    fn training_reduces_error_on_a_systematic_bias() {
        let store = LearnedCostStore::new();
        const SKEW: f64 = 8.0;

        // Deterministic LCG jitter (no `rand` dep) so estimates vary run to run within
        // the training loop without becoming nondeterministic across test runs.
        let mut state = 0x2545F4914F6CDD1Du64;
        let mut next = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((state >> 33) as f64 / u32::MAX as f64) * 0.4 + 0.8 // in [0.8, 1.2)
        };

        let mut static_errs = Vec::new();
        let mut learned_errs = Vec::new();
        for i in 0..300 {
            let estimated = 50.0 * next() * (1.0 + (i % 7) as f64 * 0.1);
            let actual = estimated * SKEW * next();
            let (s, l) = store.observe("Traverse", estimated, actual);
            static_errs.push(s);
            learned_errs.push(l);
        }

        // Compare the LAST THIRD (curve warmed up) against the static baseline over
        // the SAME window — an honest, non-cherry-picked comparison window.
        let last_third = static_errs.len() * 2 / 3;
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let static_mean = mean(&static_errs[last_third..]);
        let learned_mean = mean(&learned_errs[last_third..]);
        println!(
            "training_reduces_error_on_a_systematic_bias: static_mean_abs_log_err={static_mean:.4} \
             learned_mean_abs_log_err={learned_mean:.4} (n={})",
            static_errs.len() - last_third
        );
        assert!(
            learned_mean < static_mean * 0.5,
            "learned correction must at least halve the static model's log-error on a \
             systematic bias: static={static_mean:.4} learned={learned_mean:.4}"
        );

        // Held-out check: a fresh estimate the curve was never directly `observe`d on
        // must land close to the TRUE skewed value, not the raw (unskewed) estimate.
        let holdout_estimate = 500.0;
        let corrected = store.correct("Traverse", holdout_estimate);
        let true_actual = holdout_estimate * SKEW;
        let rel_err_learned = (corrected - true_actual).abs() / true_actual;
        let rel_err_static = (holdout_estimate - true_actual).abs() / true_actual;
        assert!(
            rel_err_learned < rel_err_static * 0.5,
            "held-out corrected estimate ({corrected:.1}) must be much closer to the \
             true skewed value ({true_actual:.1}) than the raw estimate \
             ({holdout_estimate:.1}) was: rel_err_learned={rel_err_learned:.4} \
             rel_err_static={rel_err_static:.4}"
        );
    }

    /// An op-kind this tier assigns no cardinality model to (`Limit` — exact
    /// arithmetic) is never bucketed, so it can never absorb noise from an unrelated
    /// modality's miss.
    #[test]
    fn limit_has_no_learned_op_kind() {
        assert_eq!(op_kind(&Op::Limit { k: 5 }), None);
        assert_eq!(
            op_kind(&Op::Traverse {
                rel: "R".into(),
                min: 1,
                max: 1
            }),
            Some("Traverse")
        );
        assert_eq!(op_kind(&Op::Scan { label: "L".into() }), Some("Scan"));
    }

    /// The queryable-artifact surface is genuinely inspectable: it round-trips through
    /// JSON byte-for-byte (mirrors `KanEdgeFn`'s own `edge_fn_serde_roundtrip`), and
    /// carries the interpretable coefficients directly.
    #[test]
    fn artifact_json_roundtrip() {
        let store = LearnedCostStore::new();
        store.observe("Filter", 100.0, 40.0);
        store.observe("Filter", 120.0, 45.0);
        store.observe("Traverse", 2.0, 500.0);

        let snap = store.snapshot();
        assert_eq!(snap.len(), 2, "one artifact per distinct op-kind observed");

        let json = serde_json::to_string(&snap).expect("artifact must serialize");
        let back: Vec<CostCorrectionArtifact> =
            serde_json::from_str(&json).expect("artifact must deserialize");
        assert_eq!(back.len(), snap.len());
        for (a, b) in snap.iter().zip(back.iter()) {
            assert_eq!(a.op_kind, b.op_kind);
            assert_eq!(a.coeffs, b.coeffs);
            assert_eq!(a.samples, b.samples);
            assert_eq!(a.degree, b.degree);
        }

        let filter_artifact = snap.iter().find(|a| a.op_kind == "Filter").unwrap();
        assert_eq!(filter_artifact.samples, 2);
        assert_eq!(filter_artifact.degree, CORRECTION_DEGREE);
    }

    /// [`enabled`] defaults OFF (relies on the var being unset in the test env — the
    /// CI default, the same convention `runtime::tests::config_defaults_to_serial`
    /// documents for its own env-gated default).
    #[test]
    fn learned_cost_defaults_disabled() {
        if std::env::var_os("MEDULLA_LEARNED_COST").is_none() {
            assert!(!enabled());
        }
    }

    /// The process-wide singleton is a real `'static` singleton (two calls to
    /// `global()` observe each other's writes) — proves [`LearnedCostStore::global`]
    /// is not accidentally handing out a fresh store each time.
    #[test]
    fn global_store_is_shared() {
        // Use an op-kind unlikely to collide with another test's use of the SAME
        // process-wide singleton (tests run in threads within one process).
        let key = "AsOf";
        let before = LearnedCostStore::global().snapshot();
        let before_samples = before
            .iter()
            .find(|a| a.op_kind == key)
            .map(|a| a.samples)
            .unwrap_or(0);
        LearnedCostStore::global().observe(key, 10.0, 10.0);
        let after_samples = LearnedCostStore::global()
            .snapshot()
            .into_iter()
            .find(|a| a.op_kind == key)
            .map(|a| a.samples)
            .unwrap_or(0);
        assert!(
            after_samples > before_samples,
            "global() must return the SAME store across calls: before={before_samples} after={after_samples}"
        );
    }
}
