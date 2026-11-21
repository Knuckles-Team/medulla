//! The plan-evolution model (MEDULLA-FEDERATED-QUERY-R091.1): what federated query planning
//! observes about each query shape, and the candidate plans it may keep.
//!
//! A [`PlanShape`] is a query with its literals removed. Every execution of a shape records
//! one [`PlanObservation`]; a [`PlanCandidate`] is an alternative plan measured against the
//! baseline. The [`ObservationStore`] is in memory only and bounded twice: each shape keeps
//! at most `observations_per_shape` observations (the oldest is dropped first) and each
//! tenant keeps at most `shapes_per_tenant` shapes (the least recently seen shape is evicted,
//! as `stats::observe` evicts the stalest source). Kept plans persist as `medulla-jobs` job
//! results, never here.
//!
//! The two limits are the registry knobs `federation.observations_per_shape` and
//! `federation.shapes_per_tenant`. This leaf crate cannot see the registry, so the composition
//! root reads the resolved knob values and passes them in as [`StoreLimits`].
//!
//! Every entry is keyed by the verified tenant and the shape digest, never by the digest alone,
//! so one tenant's activity is never visible through another tenant's lookups.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use medulla_catalog_types::metadata_acceleration::AccelerationPolicy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use medulla_query_types::wire::ForeignSourceSpec;

use super::cache::FragmentCacheScope;
use super::trace::{FetchStrategy, FragmentTrace};

pub use super::run::replay_fragment;

use super::remote::RemoteFetch;
use crate::federation::ForeignSourceRegistry;

/// A query with its literals removed, identified by the digest of its template.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanShape {
    pub shape_digest: String,
    pub query_template: String,
    pub language: String,
    /// The read authority captured when the shape was recorded.
    pub authority_ref: String,
}

/// What one execution of a shape did.
///
/// `strategy`, `limit_pushed`, `keys_pushed` and `replay` are what a replay needs (MEDULLA-FEDERATED-QUERY-R091.3).
/// They stay in memory: the replay input may name a connection, so none of the four is ever
/// serialized.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanObservation {
    pub shape_digest: String,
    pub plan_digest: String,
    pub sources: Vec<String>,
    pub pushdowns: Vec<String>,
    pub latency_us: u64,
    pub rows_fetched: u64,
    pub bytes_fetched: u64,
    pub cache_hit: bool,
    pub graph_revision: u64,
    /// How the fragment was fetched.
    #[serde(skip)]
    pub strategy: FetchStrategy,
    /// The limit the fragment pushed into the source.
    #[serde(skip)]
    pub limit_pushed: Option<usize>,
    /// The distinct local ids the fragment shipped as keys.
    #[serde(skip)]
    pub keys_pushed: usize,
    /// What re-running the fragment needs.
    #[serde(skip)]
    pub replay: ReplayInput,
}

/// What a replay of one fragment reads: the source, the pushed limit hint and, for a join, the
/// local ids the join ran over (truncated to `max_bind_keys`; empty for a source read).
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayInput {
    pub source: ForeignSourceSpec,
    pub hint: Option<usize>,
    pub keys: Vec<String>,
}

impl Default for ReplayInput {
    fn default() -> Self {
        Self {
            source: ForeignSourceSpec::Named {
                name: String::new(),
            },
            hint: None,
            keys: Vec::new(),
        }
    }
}

/// A plan a plan-evolution run kept for one shape: the strategy a later query of the shape
/// reads before it chooses (MEDULLA-FEDERATED-QUERY-R091.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeptPlan {
    pub plan_digest: String,
    pub strategy: FetchStrategy,
    /// Whether the kept plan pushed the hinted limit into the source.
    pub limit_pushed: bool,
    /// The downtime job whose receipt kept the plan.
    pub receipt_job_id: String,
    /// The measurements that kept the plan (MEDULLA-FEDERATED-QUERY-R091.5).
    pub evidence: KeptEvidence,
}

/// What a plan-evolution run measured when it kept a plan: the digest of the plan it
/// replaced, the candidate's replay count, and the p50 latency and bytes fetched of the
/// baseline (`before`) and of the kept plan (`after`). All zero for a forced replay plan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeptEvidence {
    pub baseline_digest: String,
    pub replays: u32,
    pub p50_before_us: u64,
    pub p50_after_us: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

/// The evidence of one run: the p50 of `latency_us` and of `bytes_fetched` over the baseline
/// and the candidate replays (MEDULLA-FEDERATED-QUERY-R091.5).
pub fn evidence_of(
    baseline_digest: &str,
    baseline: &[ReplayMeasure],
    candidate: &[ReplayMeasure],
) -> KeptEvidence {
    let latency = |runs: &[ReplayMeasure]| p50(runs.iter().map(|r| r.latency_us).collect());
    let bytes = |runs: &[ReplayMeasure]| p50(runs.iter().map(|r| r.bytes_fetched).collect());
    KeptEvidence {
        baseline_digest: baseline_digest.to_string(),
        replays: u32::try_from(candidate.len()).unwrap_or(u32::MAX),
        p50_before_us: latency(baseline),
        p50_after_us: latency(candidate),
        bytes_before: bytes(baseline),
        bytes_after: bytes(candidate),
    }
}

/// The closed set of candidate kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    Rewrite,
    PushdownChoice,
    CacheSubgraph,
    MaterializeSubgraph,
    IndexSuggestion,
}

/// An alternative plan for a shape, named against the baseline it must beat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanCandidate {
    pub shape_digest: String,
    pub kind: CandidateKind,
    pub plan_digest: String,
    pub baseline_digest: String,
}

/// The bounds of an [`ObservationStore`], resolved from the registry knobs by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreLimits {
    /// `knob federation.observations_per_shape`, items.
    pub observations_per_shape: usize,
    /// `knob federation.shapes_per_tenant`, shapes.
    pub shapes_per_tenant: usize,
}

struct ShapeEntry {
    shape: PlanShape,
    observations: VecDeque<PlanObservation>,
    /// Position of the latest observation on the tenant's logical clock.
    last_seen: u64,
}

#[derive(Default)]
struct TenantShapes {
    shapes: HashMap<String, ShapeEntry>,
    clock: u64,
}

/// The bounded in-memory observations of every tenant's query shapes.
pub struct ObservationStore {
    limits: StoreLimits,
    tenants: HashMap<String, TenantShapes>,
}

impl ObservationStore {
    pub fn new(limits: StoreLimits) -> Self {
        Self {
            limits,
            tenants: HashMap::new(),
        }
    }

    /// Record one execution of `shape` for `tenant`. A new shape in a full tenant evicts the
    /// least recently seen shape first; a shape at its observation bound drops its oldest.
    pub fn observe(&mut self, tenant: &str, shape: PlanShape, observation: PlanObservation) {
        let limits = self.limits;
        let state = self.tenants.entry(tenant.to_string()).or_default();
        state.clock += 1;
        let now = state.clock;
        if !state.shapes.contains_key(&shape.shape_digest)
            && state.shapes.len() >= limits.shapes_per_tenant
        {
            let stalest = state
                .shapes
                .iter()
                .min_by_key(|(_, entry)| entry.last_seen)
                .map(|(digest, _)| digest.clone());
            if let Some(digest) = stalest {
                state.shapes.remove(&digest);
            }
        }
        let entry = state
            .shapes
            .entry(shape.shape_digest.clone())
            .or_insert_with(|| ShapeEntry {
                shape,
                observations: VecDeque::new(),
                last_seen: now,
            });
        entry.last_seen = now;
        while entry.observations.len() >= limits.observations_per_shape {
            entry.observations.pop_front();
        }
        entry.observations.push_back(observation);
    }

    /// The shapes `tenant` has recorded.
    pub fn shape_count(&self, tenant: &str) -> usize {
        self.tenants.get(tenant).map_or(0, |t| t.shapes.len())
    }

    /// The shape `tenant` recorded under `shape_digest`.
    pub fn shape(&self, tenant: &str, shape_digest: &str) -> Option<&PlanShape> {
        self.tenants
            .get(tenant)?
            .shapes
            .get(shape_digest)
            .map(|entry| &entry.shape)
    }

    /// The latest observation `tenant` holds of `plan_digest` under `shape_digest`.
    pub fn observation_of_plan(
        &self,
        tenant: &str,
        shape_digest: &str,
        plan_digest: &str,
    ) -> Option<&PlanObservation> {
        self.observations(tenant, shape_digest)
            .filter(|o| o.plan_digest == plan_digest)
            .last()
    }

    /// The tenants that hold observations.
    pub fn tenants(&self) -> Vec<String> {
        let mut tenants: Vec<String> = self.tenants.keys().cloned().collect();
        tenants.sort();
        tenants
    }

    /// The observations `tenant` holds for `shape_digest`, oldest first.
    pub fn observations(
        &self,
        tenant: &str,
        shape_digest: &str,
    ) -> impl Iterator<Item = &PlanObservation> {
        self.tenants
            .get(tenant)
            .and_then(|t| t.shapes.get(shape_digest))
            .into_iter()
            .flat_map(|entry| entry.observations.iter())
    }
}


impl ObservationStore {
    /// The `limit` most frequently observed shapes of `tenant` with their observation
    /// counts, most frequent first, ties by digest (MEDULLA-FEDERATED-QUERY-R091.2).
    pub fn top_shapes(&self, tenant: &str, limit: usize) -> Vec<(String, usize)> {
        let Some(state) = self.tenants.get(tenant) else {
            return Vec::new();
        };
        let mut ranked: Vec<(String, usize)> = state
            .shapes
            .iter()
            .map(|(digest, entry)| (digest.clone(), entry.observations.len()))
            .collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(limit);
        ranked
    }

    /// The plan `tenant` ran most often for `shape_digest`, the baseline a candidate must
    /// beat; ties go to the smaller digest.
    pub fn baseline_plan(&self, tenant: &str, shape_digest: &str) -> Option<String> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for observation in self.observations(tenant, shape_digest) {
            *counts.entry(observation.plan_digest.as_str()).or_default() += 1;
        }
        counts
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(digest, _)| digest.to_string())
    }
}

/// The resolved evolution knobs. This leaf crate cannot see the registry, so the composition
/// root passes `federation.evolution_top_shapes`, `federation.min_gain`,
/// `federation.min_replays` and `federation.drift_revalidate` in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvolutionPolicy {
    pub top_shapes: usize,
    /// Percent.
    pub min_gain_percent: u64,
    /// Runs.
    pub min_replays: usize,
    /// Percent.
    pub drift_revalidate_percent: u64,
}

/// One replay of a plan under the captured `authority_ref`: what it cost and the rows it
/// returned (row ids, in any order; compared as a multiset).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayMeasure {
    pub latency_us: u64,
    pub bytes_fetched: u64,
    pub results: Vec<String>,
}

/// Why a candidate was reverted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevertReason {
    /// Fewer than `min_replays` replays of the baseline or the candidate.
    TooFewReplays,
    /// A replay returned a different result multiset than the baseline.
    ResultsDiffer,
    /// Neither p50 latency nor bytes fetched improved by `min_gain`.
    NoMeasuredGain,
}

impl RevertReason {
    /// The typed refusal this revert reports on the job result, when it is one: a result
    /// that differs from the baseline's is `PLAN_RESULT_MISMATCH`. Too few replays and no
    /// measured gain are ordinary reverts, not refusals.
    pub fn refusal(self) -> Option<PlanEvolutionRefusal> {
        match self {
            Self::ResultsDiffer => Some(PlanEvolutionRefusal::ResultMismatch),
            Self::TooFewReplays | Self::NoMeasuredGain => None,
        }
    }
}

/// MEDULLA-FEDERATED-QUERY-R091.4: why the evolution of one shape was refused. Each refusal
/// leaves the baseline plan in force.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanEvolutionRefusal {
    /// A candidate returned a different result than the baseline (`PLAN_RESULT_MISMATCH`).
    ResultMismatch,
    /// The `authority_ref` the shape was captured under no longer resolves
    /// (`PLAN_AUTHORITY_EXPIRED`); the shape is skipped.
    AuthorityExpired,
}

impl PlanEvolutionRefusal {
    /// The registered refusal code.
    pub fn code(self) -> medulla_types::contract::ServerErrorCode {
        match self {
            Self::ResultMismatch => medulla_types::contract::ServerErrorCode::PlanResultMismatch,
            Self::AuthorityExpired => medulla_types::contract::ServerErrorCode::PlanAuthorityExpired,
        }
    }
}

/// The outcome of evaluating one candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Keep,
    Revert(RevertReason),
}

fn p50(mut values: Vec<u64>) -> u64 {
    values.sort_unstable();
    values.get(values.len() / 2).copied().unwrap_or(0)
}

fn improves(baseline: u64, candidate: u64, min_gain_percent: u64) -> bool {
    baseline > 0
        && u128::from(candidate) * 100 <= u128::from(baseline) * u128::from(100 - min_gain_percent.min(100))
}

fn sorted(results: &[String]) -> Vec<&str> {
    let mut rows: Vec<&str> = results.iter().map(String::as_str).collect();
    rows.sort_unstable();
    rows
}

/// The named default policy `keep_on_measured_gain`: keep a candidate when it was replayed
/// at least `min_replays` times, every replay returned the baseline's result multiset
/// (this check is fixed), and p50 latency or p50 bytes fetched improved by `min_gain`.
pub fn keep_on_measured_gain(
    policy: &EvolutionPolicy,
    baseline: &[ReplayMeasure],
    candidate: &[ReplayMeasure],
) -> Decision {
    if baseline.len() < policy.min_replays || candidate.len() < policy.min_replays {
        return Decision::Revert(RevertReason::TooFewReplays);
    }
    let equal = baseline
        .iter()
        .zip(candidate)
        .all(|(base, cand)| sorted(&base.results) == sorted(&cand.results));
    if !equal {
        return Decision::Revert(RevertReason::ResultsDiffer);
    }
    let latency = |runs: &[ReplayMeasure]| p50(runs.iter().map(|r| r.latency_us).collect());
    let bytes = |runs: &[ReplayMeasure]| p50(runs.iter().map(|r| r.bytes_fetched).collect());
    if improves(latency(baseline), latency(candidate), policy.min_gain_percent)
        || improves(bytes(baseline), bytes(candidate), policy.min_gain_percent)
    {
        Decision::Keep
    } else {
        Decision::Revert(RevertReason::NoMeasuredGain)
    }
}

/// Evaluate one shape's candidate against its baseline replays (MEDULLA-FEDERATED-QUERY-R091.2).
pub fn evaluate_shape(
    policy: &EvolutionPolicy,
    baseline: &[ReplayMeasure],
    candidate: &[ReplayMeasure],
) -> Decision {
    keep_on_measured_gain(policy, baseline, candidate)
}

/// Must a kept plan be re-validated: did the source statistic move by `drift_revalidate`
/// percent or more since the plan was kept?
pub fn drift_requires_revalidation(policy: &EvolutionPolicy, kept: f64, current: f64) -> bool {
    if kept == 0.0 {
        return current != 0.0;
    }
    ((current - kept).abs() / kept.abs()) * 100.0 >= policy.drift_revalidate_percent as f64
}

fn derived_digest(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

/// The `rewrite` generator: one candidate per top shape, a rewritten form of the baseline
/// plan named by the digest of the baseline and the shape's template.
pub fn rewrite(
    store: &ObservationStore,
    tenant: &str,
    policy: &EvolutionPolicy,
) -> Vec<PlanCandidate> {
    top_candidates(store, tenant, policy, CandidateKind::Rewrite, |shape, baseline| {
        derived_digest(&["rewrite", baseline, &shape.query_template])
    })
}

/// The `pushdown_choice` generator: for each top shape, the observed plan with the fewest
/// bytes fetched whose pushdowns differ from the baseline's.
pub fn pushdown_choice(
    store: &ObservationStore,
    tenant: &str,
    policy: &EvolutionPolicy,
) -> Vec<PlanCandidate> {
    let mut out = Vec::new();
    for (shape_digest, _) in store.top_shapes(tenant, policy.top_shapes) {
        let Some(baseline) = store.baseline_plan(tenant, &shape_digest) else {
            continue;
        };
        let baseline_pushdowns: Option<&Vec<String>> = store
            .observations(tenant, &shape_digest)
            .find(|o| o.plan_digest == baseline)
            .map(|o| &o.pushdowns);
        let best = store
            .observations(tenant, &shape_digest)
            .filter(|o| o.plan_digest != baseline && Some(&o.pushdowns) != baseline_pushdowns)
            .min_by(|a, b| {
                a.bytes_fetched
                    .cmp(&b.bytes_fetched)
                    .then_with(|| a.plan_digest.cmp(&b.plan_digest))
            });
        if let Some(best) = best {
            out.push(PlanCandidate {
                shape_digest,
                kind: CandidateKind::PushdownChoice,
                plan_digest: best.plan_digest.clone(),
                baseline_digest: baseline,
            });
        }
    }
    out
}

fn top_candidates(
    store: &ObservationStore,
    tenant: &str,
    policy: &EvolutionPolicy,
    kind: CandidateKind,
    plan_of: impl Fn(&PlanShape, &str) -> String,
) -> Vec<PlanCandidate> {
    let mut out = Vec::new();
    for (shape_digest, _) in store.top_shapes(tenant, policy.top_shapes) {
        let (Some(shape), Some(baseline)) = (
            store.shape(tenant, &shape_digest),
            store.baseline_plan(tenant, &shape_digest),
        ) else {
            continue;
        };
        out.push(PlanCandidate {
            plan_digest: plan_of(shape, &baseline),
            shape_digest,
            kind,
            baseline_digest: baseline,
        });
    }
    out
}

/// The sources of a shape's latest observation.
fn latest_sources(store: &ObservationStore, tenant: &str, shape_digest: &str) -> Vec<String> {
    store
        .observations(tenant, shape_digest)
        .last()
        .map(|o| o.sources.clone())
        .unwrap_or_default()
}

/// Shapes seen at least `min_replays` times, best first.
fn replayed_shapes(
    store: &ObservationStore,
    tenant: &str,
    policy: &EvolutionPolicy,
) -> Vec<String> {
    store
        .top_shapes(tenant, policy.top_shapes)
        .into_iter()
        .filter(|(_, seen)| *seen >= policy.min_replays)
        .map(|(digest, _)| digest)
        .collect()
}

/// `cache_subgraph` candidates (MEDULLA-FEDERATED-QUERY-R091.7): a shape seen at least
/// `min_replays` times whose every source has a fresh checkpoint in `scope`, so the
/// fragment result can be cached in the `FragmentCacheScope` with its existing time-to-live.
pub fn generate_cache_candidates(
    store: &ObservationStore,
    tenant: &str,
    policy: &EvolutionPolicy,
    scope: &FragmentCacheScope,
) -> Vec<PlanCandidate> {
    let mut out = Vec::new();
    for shape_digest in replayed_shapes(store, tenant, policy) {
        let sources = latest_sources(store, tenant, &shape_digest);
        if sources.is_empty() || sources.iter().any(|s| scope.watermark_of(s).is_none()) {
            continue;
        }
        let Some(baseline) = store.baseline_plan(tenant, &shape_digest) else {
            continue;
        };
        out.push(PlanCandidate {
            plan_digest: derived_digest(&["cache_subgraph", &baseline]),
            shape_digest,
            kind: CandidateKind::CacheSubgraph,
            baseline_digest: baseline,
        });
    }
    out
}

/// `materialize_subgraph` candidates (MEDULLA-FEDERATED-QUERY-R091.7): the candidate and the
/// [`AccelerationPolicy`] built from the shape's pushed predicate. It needs the source `copy`
/// capability (`supports_copy`) and a fresh freshness watermark.
pub fn generate_materialize_candidates(
    store: &ObservationStore,
    tenant: &str,
    policy: &EvolutionPolicy,
    scope: &FragmentCacheScope,
    supports_copy: &dyn Fn(&str) -> bool,
) -> Vec<(PlanCandidate, AccelerationPolicy)> {
    let mut out = Vec::new();
    for shape_digest in replayed_shapes(store, tenant, policy) {
        let sources = latest_sources(store, tenant, &shape_digest);
        let [source] = sources.as_slice() else {
            continue;
        };
        let predicate = store
            .observations(tenant, &shape_digest)
            .last()
            .map(|o| o.pushdowns.join(" AND "))
            .unwrap_or_default();
        let watermark = scope.watermark_of(source);
        if !supports_copy(source) || predicate.is_empty() || watermark.is_none() {
            continue;
        }
        let Some(baseline) = store.baseline_plan(tenant, &shape_digest) else {
            continue;
        };
        out.push((
            PlanCandidate {
                plan_digest: derived_digest(&["materialize_subgraph", &baseline]),
                shape_digest,
                kind: CandidateKind::MaterializeSubgraph,
                baseline_digest: baseline,
            },
            AccelerationPolicy {
                source_supports_copy: true,
                hot_subset_predicate: predicate,
                freshness_watermark: watermark,
            },
        ));
    }
    out
}

/// A suggestion for the operator to index one source column. The engine sends nothing to
/// the source (MEDULLA-FEDERATED-QUERY-R091.8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexSuggestion {
    pub source: String,
    pub column: String,
    pub shape_digest: String,
}

/// The prefix of a pushdown entry that names a filtered column: `filter:<column>`.
pub const FILTER_PUSHDOWN_PREFIX: &str = "filter:";

/// One suggestion per shape seen at least `min_replays` times whose latest observation
/// pushed a filter on a column to a single source.
pub fn generate_index_suggestions(
    store: &ObservationStore,
    tenant: &str,
    policy: &EvolutionPolicy,
) -> Vec<IndexSuggestion> {
    let mut out = Vec::new();
    for shape_digest in replayed_shapes(store, tenant, policy) {
        let sources = latest_sources(store, tenant, &shape_digest);
        let [source] = sources.as_slice() else {
            continue;
        };
        let column = store
            .observations(tenant, &shape_digest)
            .last()
            .and_then(|o| {
                o.pushdowns
                    .iter()
                    .find_map(|p| p.strip_prefix(FILTER_PUSHDOWN_PREFIX))
            })
            .map(str::to_string);
        if let Some(column) = column {
            out.push(IndexSuggestion {
                source: source.clone(),
                column,
                shape_digest,
            });
        }
    }
    out
}

fn global_store() -> &'static Mutex<ObservationStore> {
    static STORE: OnceLock<Mutex<ObservationStore>> = OnceLock::new();
    STORE.get_or_init(|| {
        let knobs = medulla_federation_limits();
        Mutex::new(ObservationStore::new(knobs))
    })
}

/// The store bounds when no composition root has resolved the knobs: the registry defaults.
fn medulla_federation_limits() -> StoreLimits {
    StoreLimits {
        observations_per_shape: 4_096,
        shapes_per_tenant: 1_024,
    }
}

/// The shape digest of the fragments of the source labelled `source_label`.
pub(super) fn shape_digest_of(source_label: &str) -> String {
    derived_digest(&["shape", source_label])
}

/// Record one finished fragment for `tenant` (MEDULLA-FEDERATED-QUERY-R091.3). The shape is the
/// fragment's source label and the plan is the fetch strategy with its pushdowns. `replay` is
/// what a later replay of the fragment reads.
pub fn observe(tenant: &str, trace: &FragmentTrace, replay: ReplayInput) {
    let shape_digest = shape_digest_of(&trace.source);
    let mut pushdowns = Vec::new();
    if let Some(limit) = trace.limit_pushed {
        pushdowns.push(format!("limit:{limit}"));
    }
    if trace.keys_pushed > 0 {
        pushdowns.push("keys".to_string());
    }
    let plan_digest = derived_digest(&["plan", &format!("{:?}", trace.strategy), &pushdowns.join(",")]);
    let shape = PlanShape {
        shape_digest: shape_digest.clone(),
        query_template: trace.source.clone(),
        language: "federation".into(),
        authority_ref: tenant.to_string(),
    };
    let observation = PlanObservation {
        shape_digest,
        plan_digest,
        sources: vec![trace.source.clone()],
        pushdowns,
        latency_us: trace.elapsed_ms.saturating_mul(1_000),
        rows_fetched: u64::try_from(trace.rows_fetched).unwrap_or(u64::MAX),
        bytes_fetched: 0,
        cache_hit: false,
        graph_revision: 0,
        strategy: trace.strategy,
        limit_pushed: trace.limit_pushed,
        keys_pushed: trace.keys_pushed,
        replay,
    };
    if let Ok(mut store) = global_store().lock() {
        store.observe(tenant, shape, observation);
    }
}

/// The bounded kept plans of every tenant: at most `shapes_per_tenant` per tenant, the oldest
/// kept shape dropped first. It is a planner cache, not a fact store: a restart starts with
/// none kept and replays rebuild it.
pub struct KeptStore {
    per_tenant: usize,
    tenants: HashMap<String, VecDeque<(String, KeptPlan)>>,
}

impl KeptStore {
    pub fn new(per_tenant: usize) -> Self {
        Self {
            per_tenant: per_tenant.max(1),
            tenants: HashMap::new(),
        }
    }

    /// Keep `plan` for `tenant`'s `shape_digest`, replacing a plan already kept for it.
    pub fn keep(&mut self, tenant: &str, shape_digest: &str, plan: KeptPlan) {
        let limit = self.per_tenant;
        let kept = self.tenants.entry(tenant.to_string()).or_default();
        kept.retain(|(shape, _)| shape != shape_digest);
        while kept.len() >= limit {
            kept.pop_front();
        }
        kept.push_back((shape_digest.to_string(), plan));
    }

    /// The plan kept for `tenant`'s `shape_digest`.
    pub fn get(&self, tenant: &str, shape_digest: &str) -> Option<KeptPlan> {
        self.tenants
            .get(tenant)?
            .iter()
            .find(|(shape, _)| shape == shape_digest)
            .map(|(_, plan)| plan.clone())
    }

    /// Drop the plan kept for `tenant`'s `shape_digest`.
    pub fn clear(&mut self, tenant: &str, shape_digest: &str) {
        if let Some(kept) = self.tenants.get_mut(tenant) {
            kept.retain(|(shape, _)| shape != shape_digest);
        }
    }

    /// How many plans `tenant` holds kept.
    pub fn len(&self, tenant: &str) -> usize {
        self.tenants.get(tenant).map_or(0, VecDeque::len)
    }

    pub fn is_empty(&self, tenant: &str) -> bool {
        self.len(tenant) == 0
    }
}

fn global_kept() -> &'static Mutex<KeptStore> {
    static KEPT: OnceLock<Mutex<KeptStore>> = OnceLock::new();
    KEPT.get_or_init(|| Mutex::new(KeptStore::new(medulla_federation_limits().shapes_per_tenant)))
}

/// The plan kept for `tenant`'s `shape_digest`, if a plan-evolution run kept one.
pub fn kept_plan(tenant: &str, shape_digest: &str) -> Option<KeptPlan> {
    global_kept().lock().ok()?.get(tenant, shape_digest)
}

/// Keep `plan` for `tenant`'s `shape_digest`.
pub fn keep(tenant: &str, shape_digest: &str, plan: KeptPlan) {
    if let Ok(mut store) = global_kept().lock() {
        store.keep(tenant, shape_digest, plan);
    }
}

/// Drop the plan kept for `tenant`'s `shape_digest`.
pub fn clear(tenant: &str, shape_digest: &str) {
    if let Ok(mut store) = global_kept().lock() {
        store.clear(tenant, shape_digest);
    }
}

/// Resolve a recorded fragment's source under `registry`, the replaying tenant's own. `None`
/// when the source no longer resolves there (MEDULLA-FEDERATED-QUERY-R091.3).
pub fn resolve_replay<'a>(
    source: &'a ForeignSourceSpec,
    registry: &'a ForeignSourceRegistry,
) -> Option<Box<dyn RemoteFetch + 'a>> {
    super::remote::resolve(source, Some(registry))
}

/// The tenants that hold plan observations, in order.
pub fn observed_tenants() -> Vec<String> {
    global_store()
        .lock()
        .map(|store| store.tenants())
        .unwrap_or_default()
}

/// One shape's candidate with the observations a replay needs: the baseline's and the
/// candidate's own.
#[derive(Clone, Debug)]
pub struct EvolutionWork {
    pub candidate: PlanCandidate,
    pub baseline: PlanObservation,
    pub observed: PlanObservation,
}

/// The `pushdown_choice` candidates of `tenant`, each with the observations of its baseline
/// plan and of the candidate plan. A candidate whose plan was observed only under another
/// shape cannot occur: both are read from the one shape.
pub fn pushdown_work(tenant: &str, policy: &EvolutionPolicy) -> Vec<EvolutionWork> {
    let Ok(store) = global_store().lock() else {
        return Vec::new();
    };
    pushdown_choice(&store, tenant, policy)
        .into_iter()
        .filter_map(|candidate| {
            let baseline = store
                .observation_of_plan(tenant, &candidate.shape_digest, &candidate.baseline_digest)?
                .clone();
            let observed = store
                .observation_of_plan(tenant, &candidate.shape_digest, &candidate.plan_digest)?
                .clone();
            Some(EvolutionWork {
                candidate,
                baseline,
                observed,
            })
        })
        .collect()
}

/// How many times `tenant`'s fragments of `source_label` have been recorded.
pub fn recorded_count(tenant: &str, source_label: &str) -> usize {
    let shape_digest = derived_digest(&["shape", source_label]);
    global_store()
        .lock()
        .map_or(0, |store| store.observations(tenant, &shape_digest).count())
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R091.4
    #[test]
    fn only_a_result_mismatch_reverts_with_a_registered_refusal() {
        assert_eq!(
            RevertReason::ResultsDiffer.refusal().map(|refusal| refusal.code().to_string()),
            Some("PLAN_RESULT_MISMATCH".to_string())
        );
        assert_eq!(RevertReason::TooFewReplays.refusal(), None);
        assert_eq!(RevertReason::NoMeasuredGain.refusal(), None);
        assert_eq!(
            PlanEvolutionRefusal::AuthorityExpired.code().to_string(),
            "PLAN_AUTHORITY_EXPIRED"
        );
    }

    const LIMITS: StoreLimits = StoreLimits {
        observations_per_shape: 3,
        shapes_per_tenant: 2,
    };

    fn shape(digest: &str) -> PlanShape {
        PlanShape {
            shape_digest: digest.into(),
            query_template: format!("SELECT ?x WHERE {{ ?x a <{digest}> }}"),
            language: "sparql".into(),
            authority_ref: "authority:1".into(),
        }
    }

    fn observation(digest: &str, plan: &str) -> PlanObservation {
        PlanObservation {
            shape_digest: digest.into(),
            plan_digest: plan.into(),
            sources: vec!["source-a".into(), "source-b".into()],
            pushdowns: vec!["filter".into()],
            latency_us: 1_500,
            rows_fetched: 20,
            bytes_fetched: 4_096,
            cache_hit: false,
            graph_revision: 7,
            strategy: FetchStrategy::FullFetch,
            limit_pushed: None,
            keys_pushed: 0,
            replay: ReplayInput::default(),
        }
    }

    fn plans(store: &ObservationStore, tenant: &str, digest: &str) -> Vec<String> {
        store
            .observations(tenant, digest)
            .map(|o| o.plan_digest.clone())
            .collect()
    }

    // spec: MEDULLA-FEDERATED-QUERY-R091.1
    #[test]
    fn observation_store_bounds() {
        // Every type round-trips and keeps every field.
        let shape_value = shape("s1");
        let encoded = serde_json::to_string(&shape_value).unwrap();
        assert_eq!(
            serde_json::from_str::<PlanShape>(&encoded).unwrap(),
            shape_value
        );
        let observed = observation("s1", "p1");
        let encoded = serde_json::to_string(&observed).unwrap();
        assert_eq!(
            serde_json::from_str::<PlanObservation>(&encoded).unwrap(),
            observed
        );
        for kind in [
            CandidateKind::Rewrite,
            CandidateKind::PushdownChoice,
            CandidateKind::CacheSubgraph,
            CandidateKind::MaterializeSubgraph,
            CandidateKind::IndexSuggestion,
        ] {
            let candidate = PlanCandidate {
                shape_digest: "s1".into(),
                kind,
                plan_digest: "p2".into(),
                baseline_digest: "p1".into(),
            };
            let encoded = serde_json::to_string(&candidate).unwrap();
            assert_eq!(
                serde_json::from_str::<PlanCandidate>(&encoded).unwrap(),
                candidate
            );
        }

        // An unknown kind and an unknown field are refused.
        let unknown_kind =
            r#"{"shape_digest":"s1","kind":"reorder","plan_digest":"p2","baseline_digest":"p1"}"#;
        assert!(serde_json::from_str::<PlanCandidate>(unknown_kind).is_err());
        let unknown_field = r#"{"shape_digest":"s1","query_template":"q","language":"uql","authority_ref":"a","extra":1}"#;
        assert!(serde_json::from_str::<PlanShape>(unknown_field).is_err());

        // A shape keeps its newest observations only.
        let mut store = ObservationStore::new(LIMITS);
        for plan in ["p1", "p2", "p3", "p4"] {
            store.observe("tenant-a", shape("s1"), observation("s1", plan));
        }
        assert_eq!(plans(&store, "tenant-a", "s1"), ["p2", "p3", "p4"]);

        // A tenant keeps its most recently seen shapes: s2 is the stalest when s3 arrives.
        store.observe("tenant-a", shape("s2"), observation("s2", "q1"));
        store.observe("tenant-a", shape("s1"), observation("s1", "p5"));
        store.observe("tenant-a", shape("s3"), observation("s3", "r1"));
        assert_eq!(store.shape_count("tenant-a"), 2);
        assert!(store.shape("tenant-a", "s2").is_none());
        assert!(store.shape("tenant-a", "s1").is_some());
        assert!(store.shape("tenant-a", "s3").is_some());

        // Another tenant's shapes are separate: nothing is visible across tenants.
        assert_eq!(store.shape_count("tenant-b"), 0);
        assert_eq!(store.observations("tenant-b", "s1").count(), 0);
        store.observe("tenant-b", shape("s1"), observation("s1", "b1"));
        assert_eq!(plans(&store, "tenant-b", "s1"), ["b1"]);
        assert_eq!(plans(&store, "tenant-a", "s1"), ["p3", "p4", "p5"]);
    }

    fn policy() -> EvolutionPolicy {
        EvolutionPolicy {
            top_shapes: 2,
            min_gain_percent: 10,
            min_replays: 3,
            drift_revalidate_percent: 25,
        }
    }

    fn runs(latency_us: u64, bytes: u64, results: &[&str], n: usize) -> Vec<ReplayMeasure> {
        (0..n)
            .map(|_| ReplayMeasure {
                latency_us,
                bytes_fetched: bytes,
                results: results.iter().map(|r| r.to_string()).collect(),
            })
            .collect()
    }

    // spec: MEDULLA-FEDERATED-QUERY-R091.2
    #[test]
    fn keep_or_revert_by_measurement() {
        let p = policy();
        let base = runs(1_000, 4_000, &["a", "b"], 3);
        // A faster candidate with an equal multiset (order aside) is kept.
        assert_eq!(
            keep_on_measured_gain(&p, &base, &runs(800, 4_000, &["b", "a"], 3)),
            Decision::Keep
        );
        // Fewer bytes alone is a gain.
        assert_eq!(
            evaluate_shape(&p, &base, &runs(1_000, 3_000, &["a", "b"], 3)),
            Decision::Keep
        );
        // A gain below the floor, or a slower candidate, reverts.
        assert_eq!(
            keep_on_measured_gain(&p, &base, &runs(950, 4_000, &["a", "b"], 3)),
            Decision::Revert(RevertReason::NoMeasuredGain)
        );
        assert_eq!(
            keep_on_measured_gain(&p, &base, &runs(2_000, 8_000, &["a", "b"], 3)),
            Decision::Revert(RevertReason::NoMeasuredGain)
        );
        // An unequal result multiset reverts even when it is much faster.
        assert_eq!(
            keep_on_measured_gain(&p, &base, &runs(100, 100, &["a"], 3)),
            Decision::Revert(RevertReason::ResultsDiffer)
        );
        // Too few replays revert.
        assert_eq!(
            keep_on_measured_gain(&p, &base, &runs(100, 100, &["a", "b"], 2)),
            Decision::Revert(RevertReason::TooFewReplays)
        );
        // Drift re-validates a kept plan at the knob.
        assert!(drift_requires_revalidation(&p, 100.0, 125.0));
        assert!(!drift_requires_revalidation(&p, 100.0, 120.0));
    }

    fn seeded(plans: &[(&str, &str, Vec<String>)], times: usize) -> ObservationStore {
        let mut store = ObservationStore::new(StoreLimits {
            observations_per_shape: 64,
            shapes_per_tenant: 8,
        });
        for _ in 0..times {
            for (plan, source, pushdowns) in plans {
                let mut obs = observation("s1", plan);
                obs.sources = vec![source.to_string()];
                obs.pushdowns = pushdowns.clone();
                store.observe("t", shape("s1"), obs);
            }
        }
        store
    }

    fn scope(sources: &[&str]) -> FragmentCacheScope {
        FragmentCacheScope::new(
            "salt".into(),
            sources
                .iter()
                .map(|name| {
                    (
                        name.to_string(),
                        super::super::cache::SourceWatermark {
                            watermark: "w1".into(),
                            valid_through_ms: u64::MAX,
                            entry_ttl_ms: None,
                        },
                    )
                })
                .collect(),
        )
    }

    // spec: MEDULLA-FEDERATED-QUERY-R091.7
    #[test]
    fn cache_and_materialize_candidates_respect_capability() {
        let p = policy();
        let store = seeded(&[("p1", "src", vec!["filter:status".into()])], 3);
        let fresh = scope(&["src"]);
        let cache = generate_cache_candidates(&store, "t", &p, &fresh);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache[0].kind, CandidateKind::CacheSubgraph);
        assert_eq!(cache[0].baseline_digest, "p1");
        // No checkpoint: no cache candidate. Too few sightings: none either.
        assert!(generate_cache_candidates(&store, "t", &p, &scope(&[])).is_empty());
        let rare = seeded(&[("p1", "src", vec!["filter:status".into()])], 2);
        assert!(generate_cache_candidates(&rare, "t", &p, &fresh).is_empty());

        let copy = |_: &str| true;
        let nocopy = |_: &str| false;
        let built = generate_materialize_candidates(&store, "t", &p, &fresh, &copy);
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].0.kind, CandidateKind::MaterializeSubgraph);
        assert_eq!(built[0].1.hot_subset_predicate, "filter:status");
        assert_eq!(built[0].1.freshness_watermark.as_deref(), Some("w1"));
        assert!(built[0].1.source_supports_copy);
        // Without the copy capability or a watermark nothing materializes.
        assert!(generate_materialize_candidates(&store, "t", &p, &fresh, &nocopy).is_empty());
        assert!(generate_materialize_candidates(&store, "t", &p, &scope(&[]), &copy).is_empty());
    }

    // spec: MEDULLA-FEDERATED-QUERY-R091.8
    #[test]
    fn index_suggestion_threshold() {
        let p = policy();
        let pushed = vec!["limit:5".to_string(), "filter:status".to_string()];
        let store = seeded(&[("p1", "src", pushed.clone())], 3);
        assert_eq!(
            generate_index_suggestions(&store, "t", &p),
            vec![IndexSuggestion {
                source: "src".into(),
                column: "status".into(),
                shape_digest: "s1".into(),
            }]
        );
        // Below the replay threshold, or with no filtered column, nothing is suggested.
        assert!(generate_index_suggestions(&seeded(&[("p1", "src", pushed)], 2), "t", &p).is_empty());
        let bare = seeded(&[("p1", "src", vec!["limit:5".into()])], 3);
        assert!(generate_index_suggestions(&bare, "t", &p).is_empty());
    }

    fn kept_plan(digest: &str) -> KeptPlan {
        KeptPlan {
            plan_digest: digest.into(),
            strategy: FetchStrategy::BindJoin,
            limit_pushed: false,
            receipt_job_id: "job-1".into(),
            evidence: Default::default(),
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R091.3
    #[test]
    fn kept_store_is_bounded_per_tenant_and_isolated() {
        let mut store = KeptStore::new(2);
        store.keep("tenant-a", "s1", kept_plan("p1"));
        store.keep("tenant-a", "s2", kept_plan("p2"));
        store.keep("tenant-a", "s3", kept_plan("p3"));
        assert_eq!(store.len("tenant-a"), 2);
        assert!(store.get("tenant-a", "s1").is_none(), "the oldest is dropped first");
        assert_eq!(store.get("tenant-a", "s3").unwrap().plan_digest, "p3");
        // Keeping again replaces; another tenant never sees the plan.
        store.keep("tenant-a", "s3", kept_plan("p4"));
        assert_eq!(store.len("tenant-a"), 2);
        assert_eq!(store.get("tenant-a", "s3").unwrap().plan_digest, "p4");
        assert!(store.get("tenant-b", "s3").is_none());
        store.clear("tenant-a", "s3");
        assert!(store.get("tenant-a", "s3").is_none());
        assert!(!store.is_empty("tenant-a"));
    }
}
