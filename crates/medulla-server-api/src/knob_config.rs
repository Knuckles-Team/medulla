//! Typed server-config knob loader (MEDULLA-CONTRACT-R058).
//!
//! Every engine knob is a typed field of [`KnobConfig`]. [`KNOB_TABLE`] declares
//! each knob's dotted name, type, default, inclusive range, unit and class once.
//! [`load_knobs`] reads each knob from the repeatable `--knob <name>=<value>`
//! option first, then from `MEDULLA_<NAME>` (upper-cased, dots to underscores),
//! then the default, and checks the bounds once at startup. Decision knobs are
//! not here: they live only in the decision-engine knob registry.

use std::sync::OnceLock;

use medulla_types::contract::ServerErrorCode;

/// Whether the evolution loop may change a knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnobClass {
    /// May be tuned within its range.
    Tunable,
    /// A one-value range; never changed.
    Fixed,
}

/// A typed knob literal (default and bounds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KnobValue {
    /// Unsigned integer.
    U64(u64),
    /// Signed integer.
    I64(i64),
    /// Floating point.
    F64(f64),
    /// Flag.
    Bool(bool),
}

/// One row of the knob table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KnobSpec {
    /// Dotted knob name, e.g. `topology.max_width`.
    pub name: &'static str,
    /// Default value.
    pub default: KnobValue,
    /// Inclusive lower bound.
    pub min: KnobValue,
    /// Inclusive upper bound.
    pub max: KnobValue,
    /// Unit of the value.
    pub unit: &'static str,
    /// Tunable or Fixed.
    pub class: KnobClass,
}

impl KnobSpec {
    /// The environment variable that carries this knob.
    pub fn env_name(&self) -> String {
        format!(
            "MEDULLA_{}",
            self.name.to_ascii_uppercase().replace('.', "_")
        )
    }
}

/// A startup refusal; the server exits non-zero before it binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnobRefusal {
    /// `KNOB_INVALID`, `KNOB_OUT_OF_RANGE` or `KNOB_UNKNOWN`.
    pub code: ServerErrorCode,
    /// The knob the refusal names.
    pub knob: String,
    /// Human detail.
    pub detail: String,
}

impl std::fmt::Display for KnobRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: knob {}: {}", self.code, self.knob, self.detail)
    }
}

impl std::error::Error for KnobRefusal {}

trait KnobType: Copy + PartialOrd + std::fmt::Display {
    fn parse(raw: &str) -> Option<Self>;
    fn of(value: KnobValue) -> Self;
}

impl KnobType for u64 {
    fn parse(raw: &str) -> Option<Self> {
        raw.trim().parse().ok()
    }
    fn of(value: KnobValue) -> Self {
        match value {
            KnobValue::U64(v) => v,
            other => unreachable!("knob table type mismatch: {other:?}"),
        }
    }
}

impl KnobType for u32 {
    fn parse(raw: &str) -> Option<Self> {
        raw.trim().parse().ok()
    }
    fn of(value: KnobValue) -> Self {
        match value {
            KnobValue::U64(v) => u32::try_from(v).expect("u32 knob bounds fit in u32"),
            other => unreachable!("knob table type mismatch: {other:?}"),
        }
    }
}

impl KnobType for i64 {
    fn parse(raw: &str) -> Option<Self> {
        raw.trim().parse().ok()
    }
    fn of(value: KnobValue) -> Self {
        match value {
            KnobValue::I64(v) => v,
            other => unreachable!("knob table type mismatch: {other:?}"),
        }
    }
}

impl KnobType for f64 {
    fn parse(raw: &str) -> Option<Self> {
        raw.trim().parse::<f64>().ok().filter(|v| v.is_finite())
    }
    fn of(value: KnobValue) -> Self {
        match value {
            KnobValue::F64(v) => v,
            other => unreachable!("knob table type mismatch: {other:?}"),
        }
    }
}

impl KnobType for bool {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }
    fn of(value: KnobValue) -> Self {
        match value {
            KnobValue::Bool(v) => v,
            other => unreachable!("knob table type mismatch: {other:?}"),
        }
    }
}

macro_rules! knobs {
    ($(
        $(#[$doc:meta])*
        $field:ident: $ty:ident = $name:literal, $variant:ident($default:expr),
            [$min:expr, $max:expr], $unit:literal, $class:ident;
    )+) => {
        /// Every engine knob, one typed field per [`KNOB_TABLE`] row.
        #[derive(Debug, Clone, PartialEq)]
        pub struct KnobConfig {
            $($(#[$doc])* pub $field: $ty,)+
        }

        /// The knob table: name, type, default, inclusive range, unit and class.
        pub const KNOB_TABLE: &[KnobSpec] = &[$(KnobSpec {
            name: $name,
            default: KnobValue::$variant($default),
            min: KnobValue::$variant($min),
            max: KnobValue::$variant($max),
            unit: $unit,
            class: KnobClass::$class,
        }),+];

        impl Default for KnobConfig {
            fn default() -> Self {
                Self { $($field: $default,)+ }
            }
        }

        fn resolve_all(
            cli: &[(String, String)],
            env: &dyn Fn(&str) -> Option<String>,
        ) -> Result<KnobConfig, KnobRefusal> {
            let mut specs = KNOB_TABLE.iter();
            Ok(KnobConfig {
                $($field: resolve::<$ty>(
                    specs.next().expect("one table row per field"),
                    cli,
                    env,
                )?,)+
            })
        }
    };
}

knobs! {
    /// KV-cache hot tier capacity.
    kvcache_hot_bytes: u64 = "kvcache.hot_bytes", U64(1_073_741_824), [67_108_864, 1_099_511_627_776], "bytes", Tunable;
    /// KV-cache warm tier capacity.
    kvcache_warm_bytes: u64 = "kvcache.warm_bytes", U64(4_294_967_296), [67_108_864, 1_099_511_627_776], "bytes", Tunable;
    /// KV-cache cold tier capacity.
    kvcache_cold_bytes: u64 = "kvcache.cold_bytes", U64(68_719_476_736), [67_108_864, 1_099_511_627_776], "bytes", Tunable;
    /// KV-cache admission floor: milliseconds saved per byte per hit.
    kvcache_min_worth: f64 = "kvcache.min_worth", F64(0.001), [0.0, 1000.0], "ms per byte per hit", Tunable;
    /// KV-cache served-answer tier capacity.
    kvcache_answer_max_bytes: u64 = "kvcache.answer_max_bytes", U64(268_435_456), [1_048_576, 68_719_476_736], "bytes", Tunable;
    /// Council agreement quorum.
    council_quorum_fraction: f64 = "council.quorum_fraction", F64(0.5), [0.5, 1.0], "fraction", Tunable;
    /// Designation reward decay.
    designation_reward_decay: f64 = "designation.reward_decay", F64(0.99), [0.5, 1.0], "factor", Tunable;
    /// Designation required-capability cap.
    designation_max_required_capabilities: u64 = "designation.max_required_capabilities", U64(32), [32, 32], "count", Fixed;
    /// Topology slot width cap.
    topology_max_width: u64 = "topology.max_width", U64(16), [1, 16], "count", Tunable;
    /// Topology depth cap.
    topology_max_depth: u64 = "topology.max_depth", U64(8), [1, 8], "count", Tunable;
    /// Topology round cap.
    topology_max_rounds: u64 = "topology.max_rounds", U64(8), [1, 8], "count", Tunable;
    /// Topology analogy score floor.
    topology_analogy_min: f64 = "topology.analogy_min", F64(0.6), [0.1, 1.0], "ratio", Tunable;
    /// Topology candidate-set cap.
    topology_max_candidates: u64 = "topology.max_candidates", U64(1_000), [1, 100_000], "sets", Tunable;
    /// Schema-repair shadow ingest sample.
    schema_repair_shadow_sample_nodes: u64 = "schema_repair.shadow_sample_nodes", U64(10_000), [100, 1_000_000], "nodes", Tunable;
    /// Reputation minimum independent labels.
    reputation_min_support: u64 = "reputation.min_support", U64(10), [5, 1_000], "labels", Tunable;
    /// Lifetime of a signed memory snapshot list cursor (MEDULLA-DURABLE-KERNEL-R085.2).
    memory_sync_cursor_ttl_s: u64 = "memory_sync.cursor_ttl_s", U64(900), [60, 86_400], "seconds", Tunable;
    /// Memory search-state size cap.
    memory_search_state_max_bytes: u64 = "memory.search_state_max_bytes", U64(1_048_576), [65_536, 1_048_576], "bytes", Tunable;
    /// TOC line-index stride.
    toc_line_index_stride: u64 = "toc.line_index_stride", U64(64), [16, 1_024], "lines", Tunable;
    /// TOC maximum served span.
    toc_max_span_lines: u64 = "toc.max_span_lines", U64(2_000), [10, 20_000], "lines", Tunable;
    /// Team signal maximum age.
    team_signal_max_age: u64 = "team.signal_max_age", U64(30), [1, 3_600], "seconds", Tunable;
    /// Held-item minimum passes.
    held_item_min_passes: u64 = "held_item.min_passes", U64(2), [1, 20], "count", Tunable;
    /// Measured-fact minimum cases.
    measured_fact_min_cases: u64 = "measured_fact.min_cases", U64(30), [1, 100_000], "cases", Tunable;
    /// Red-team probe cap.
    red_team_max_probes: u64 = "red_team.max_probes", U64(50), [1, 500], "probes", Tunable;
    /// Identity ownership writer share.
    identity_ownership_writer_share: f64 = "identity.ownership_writer_share", F64(0.10), [0.01, 0.5], "fraction", Tunable;
    /// Retrieval lineage depth.
    retrieval_lineage_depth: u64 = "retrieval.lineage_depth", U64(8), [1, 16], "hops", Tunable;
    /// Feedback events kept per document.
    feedback_events_per_doc: u64 = "feedback.events_per_doc", U64(1_000), [10, 100_000], "events", Tunable;
    /// Feedback rate limit per principal.
    feedback_rate_limit_per_hour: u64 = "feedback.rate_limit_per_hour", U64(100), [100, 100], "events per hour", Fixed;
    /// Federated raw-scan cost per KiB.
    federated_raw_scan_cost_per_kib: f64 = "federated.raw_scan_cost_per_kib", F64(1.0), [1.0, 100.0], "cost units per KiB", Tunable;
    /// Interactive query node budget.
    query_interactive_node_budget: u64 = "query.interactive_node_budget", U64(100_000), [1_000, 10_000_000], "nodes", Tunable;
    /// Cap on the search nodes `ArgumentMapEvaluate` explores under `preferred`
    /// semantics (MEDULLA-FEDERATED-QUERY-R700.3).
    argument_node_budget: u64 = "argument.node_budget", U64(100_000), [1_000, 10_000_000], "nodes", Tunable;
    /// Context facts kept per node.
    context_facts_max_facts_per_node: u64 = "context_facts.max_facts_per_node", U64(16), [1, 256], "facts", Tunable;
    /// Bytes per token in the context facts estimate.
    context_facts_bytes_per_token: u64 = "context_facts.bytes_per_token", U64(4), [2, 8], "bytes", Tunable;
    /// Ingest link minimum shared mentions.
    ingest_link_min_shared_mentions: u64 = "ingest.link_min_shared_mentions", U64(2), [1, 20], "count", Tunable;
    /// Media blob size cap.
    media_max_blob_bytes: u64 = "media.max_blob_bytes", U64(33_554_432), [1_048_576, 268_435_456], "bytes", Tunable;
    /// Synthesis context budget.
    synthesis_context_budget_tokens: u64 = "synthesis.context_budget_tokens", U64(8_192), [512, 1_048_576], "tokens", Tunable;
    /// Evolving mode requires approval.
    sdlc_mode_evolving_requires_approval: bool = "sdlc.mode.evolving_requires_approval", Bool(true), [true, true], "flag", Fixed;
    /// Unprompted delivery is shadow in development.
    sdlc_delivery_shadow_unprompted_in_development: bool = "sdlc.delivery.shadow_unprompted_in_development", Bool(true), [true, true], "flag", Fixed;
    /// Automated review round cap.
    sdlc_delivery_max_automated_review_rounds: u64 = "sdlc.delivery.max_automated_review_rounds", U64(3), [1, 20], "rounds", Tunable;
    /// Owner approval is required for delivery.
    sdlc_delivery_owner_approval_required: bool = "sdlc.delivery.owner_approval_required", Bool(true), [true, true], "flag", Fixed;
    /// Doji body-to-range ceiling (MEDULLA-FINANCE-PRIMITIVES-R750.751).
    pattern_doji_body_max: f64 = "pattern.doji_body_max", F64(0.1), [0.01, 0.3], "ratio", Tunable;
    /// Hammer and shooting-star shadow-to-body floor.
    pattern_shadow_ratio_min: f64 = "pattern.shadow_ratio_min", F64(2.0), [1.5, 4.0], "ratio", Tunable;
    /// Opposite-shadow ceiling as a fraction of the range.
    pattern_wick_max: f64 = "pattern.wick_max", F64(0.1), [0.0, 0.3], "ratio", Tunable;
    /// Claim pacing bucket capacity for the background class (MEDULLA-PLACEMENT-R750.753).
    claim_pacing_background_capacity: u32 = "claim_pacing.background_capacity", U64(64), [1, 100_000], "tokens", Tunable;
    /// Claim pacing refill rate for the background class.
    claim_pacing_background_refill_per_s: u32 = "claim_pacing.background_refill_per_s", U64(32), [1, 100_000], "tokens per second", Tunable;
    /// Claim pacing bucket capacity for the low class (MEDULLA-PLACEMENT-R750.753).
    claim_pacing_low_capacity: u32 = "claim_pacing.low_capacity", U64(64), [1, 100_000], "tokens", Tunable;
    /// Claim pacing refill rate for the low class.
    claim_pacing_low_refill_per_s: u32 = "claim_pacing.low_refill_per_s", U64(32), [1, 100_000], "tokens per second", Tunable;
    /// Claim pacing bucket capacity for the normal class (MEDULLA-PLACEMENT-R750.753).
    claim_pacing_normal_capacity: u32 = "claim_pacing.normal_capacity", U64(64), [1, 100_000], "tokens", Tunable;
    /// Claim pacing refill rate for the normal class.
    claim_pacing_normal_refill_per_s: u32 = "claim_pacing.normal_refill_per_s", U64(32), [1, 100_000], "tokens per second", Tunable;
    /// Claim pacing bucket capacity for the foreground class (MEDULLA-PLACEMENT-R750.753).
    claim_pacing_foreground_capacity: u32 = "claim_pacing.foreground_capacity", U64(64), [1, 100_000], "tokens", Tunable;
    /// Claim pacing refill rate for the foreground class.
    claim_pacing_foreground_refill_per_s: u32 = "claim_pacing.foreground_refill_per_s", U64(32), [1, 100_000], "tokens per second", Tunable;
    /// Read-audit queue depth: a full queue refuses the read (MEDULLA-IDENTITY-R610.5).
    identity_read_audit_queue_depth: u64 = "identity.read_audit_queue_depth", U64(4_096), [64, 65_536], "entries", Tunable;
    /// Metric look-back of the tenant telemetry snapshot read (MEDULLA-ORCH-VOCAB-R804).
    telemetry_snapshot_window: u64 = "telemetry.snapshot_window", U64(300), [10, 86_400], "seconds", Tunable;
    /// Usage ledger batch size cap.
    usage_max_batch: u64 = "usage.max_batch", U64(1_000), [1, 10_000], "records", Tunable;
    /// OTLP log batch record cap (MEDULLA-CONTRACT-R050).
    obs_max_batch_records: u64 = "obs.max_batch_records", U64(1_000), [1, 10_000], "records", Tunable;
    /// Longest lag of a feature-power IC decay curve (MEDULLA-DECISION-ENGINE-R117.1.2).
    eval_ic_max_lag: u64 = "eval.ic_max_lag", U64(5), [1, 64], "lags", Tunable;
    /// Bootstrap resamples of a feature-power IC interval.
    eval_bootstrap_resamples: u64 = "eval.bootstrap_resamples", U64(1_000), [100, 10_000], "resamples", Tunable;
    /// Coverage of a feature-power IC interval.
    eval_ci_level: f64 = "eval.ci_level", F64(0.95), [0.5, 0.999], "fraction", Tunable;
    /// Longest replay lag at which a declared replica may serve a snapshot read.
    federation_replica_lag_bound_ms: u64 = "federation.replica_lag_bound_ms", U64(5_000), [0, 600_000], "ms", Tunable;
    /// Longest a federated source statement may run.
    federation_statement_timeout_ms: u64 = "federation.statement_timeout_ms", U64(30_000), [1_000, 600_000], "ms", Tunable;
    /// Most rows a copy-served fragment may hold (MEDULLA-UNIFIED-DATA-PLANE-R038.2.2).
    acceleration_max_copy_rows: u64 = "acceleration.max_copy_rows", U64(1_000_000), [1, 100_000_000], "rows", Tunable;
    /// Claim evidence weight cap (MEDULLA-DECISION-ENGINE-R652.2).
    claims_weight_cap_ppm: u64 = "claims.weight_cap_ppm", U64(6_000_000), [1_000_000, 12_000_000], "ppm", Tunable;
    /// Claim log-odds floor to enter `accepted`.
    claims_accept_min_micro: i64 = "claims.accept_min_micro", I64(2_944_439), [0, 12_000_000], "micro-nats", Tunable;
    /// Claim log-odds ceiling to enter `contested`.
    claims_contest_max_micro: i64 = "claims.contest_max_micro", I64(-1_000_000), [-12_000_000, 0], "micro-nats", Tunable;
    /// Claim research-feed relevance floor (MEDULLA-DECISION-ENGINE-R652.7).
    claims_feed_relevance_min: u64 = "claims.feed_relevance_min", U64(300_000), [0, 1_000_000], "ppm", Tunable;
    /// Claim channel prior for health evidence.
    claims_prior_health: u64 = "claims.prior_health", U64(600_000), [0, 1_000_000], "ppm", Tunable;
    /// Claim channel prior for mined evidence.
    claims_prior_mined: u64 = "claims.prior_mined", U64(300_000), [0, 1_000_000], "ppm", Tunable;
    /// Claim channel prior for optimization evidence.
    claims_prior_optimization: u64 = "claims.prior_optimization", U64(800_000), [0, 1_000_000], "ppm", Tunable;
    /// Claim channel prior for outcome evidence.
    claims_prior_outcome: u64 = "claims.prior_outcome", U64(1_000_000), [0, 1_000_000], "ppm", Tunable;
    /// Claim channel prior for process evidence.
    claims_prior_process: u64 = "claims.prior_process", U64(500_000), [0, 1_000_000], "ppm", Tunable;
    /// Claim log-odds floor to enter `supported`.
    claims_support_min_micro: i64 = "claims.support_min_micro", I64(1_000_000), [0, 6_000_000], "micro-nats", Tunable;
    /// Longest wait for a stable WAL snapshot of an attached SQLite file (MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.1).
    foreign_sqlite_busy_timeout_ms: u64 = "foreign.sqlite.busy_timeout_ms", U64(1_000), [0, 60_000], "ms", Tunable;
    /// Watermark poll interval of an attached SQLite file (MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.2).
    foreign_sqlite_poll_interval_ms: u64 = "foreign.sqlite.poll_interval_ms", U64(5_000), [100, 3_600_000], "ms", Tunable;
    /// Hop bound of the reverse `DEPENDS_ON` walk of an ecosystem finding (MEDULLA-SUPPLY-VOCAB-R009.3).
    ecosystem_max_dependency_depth: u64 = "ecosystem.max_dependency_depth", U64(8), [1, 64], "hops", Tunable;
    /// Largest n-ary link-prediction tuple (MEDULLA-FEDERATED-QUERY-R086.100.1).
    graphlearn_nary_max_arity: u64 = "graphlearn.nary_max_arity", U64(6), [3, 12], "members", Tunable;
    /// Most candidate tuples one n-ary link-prediction request carries.
    graphlearn_nary_max_candidates: u64 = "graphlearn.nary_max_candidates", U64(1_000), [1, 100_000], "tuples", Tunable;
    /// Lowest probability an n-ary hypothesis may score and still be returned.
    graphlearn_nary_min_score: f64 = "graphlearn.nary_min_score", F64(0.5), [0.0, 1.0], "probability", Tunable;
    /// Refresh records kept per live artifact (MEDULLA-FEDERATED-QUERY-R701.5).
    artifact_refresh_history: u64 = "artifact.refresh_history", U64(20), [1, 1_000], "records", Tunable;
    /// Largest maintained state of one incremental view; past it the view demotes to recompute (MEDULLA-INCREMENTAL-VIEWS-R007.2).
    view_state_bytes: u64 = "view.state_bytes", U64(268_435_456), [1_048_576, 17_179_869_184], "bytes", Tunable;
    /// Resident maintained state of all incremental views; past it the least recently changed view spills (MEDULLA-INCREMENTAL-VIEWS-R007.3).
    views_memory_bytes: u64 = "views.memory_bytes", U64(1_073_741_824), [16_777_216, 68_719_476_736], "bytes", Tunable;
    /// Ontology naming convention mode: 0 warns, 1 refuses (MEDULLA-TYPED-PACKS-R651.2).
    ontology_naming_mode: u64 = "ontology.naming_mode", U64(0), [0, 1], "mode", Tunable;
}

fn refusal(code: ServerErrorCode, knob: &str, detail: String) -> KnobRefusal {
    KnobRefusal {
        code,
        knob: knob.to_string(),
        detail,
    }
}

fn resolve<T: KnobType>(
    spec: &KnobSpec,
    cli: &[(String, String)],
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<T, KnobRefusal> {
    // The last `--knob` for a name wins; then the environment; then the default.
    let raw = cli
        .iter()
        .rev()
        .find(|(name, _)| name == spec.name)
        .map(|(_, value)| (value.clone(), "--knob"))
        .or_else(|| env(&spec.env_name()).map(|value| (value, "env")));
    let Some((raw, source)) = raw else {
        return Ok(T::of(spec.default));
    };
    let value = T::parse(&raw).ok_or_else(|| {
        refusal(
            ServerErrorCode::KnobInvalid,
            spec.name,
            format!("{source} value {raw:?} does not parse"),
        )
    })?;
    let (min, max) = (T::of(spec.min), T::of(spec.max));
    if value < min || value > max {
        return Err(refusal(
            ServerErrorCode::KnobOutOfRange,
            spec.name,
            format!(
                "{source} value {value} outside [{min}, {max}] {}",
                spec.unit
            ),
        ));
    }
    Ok(value)
}

/// Read every knob: `--knob <name>=<value>` first, then `MEDULLA_<NAME>`, then
/// the default; refuse a bad value or an unknown `--knob` name. Only `--knob`
/// names are checked against the table; other `MEDULLA_*` variables are
/// untouched.
pub fn load_knobs(
    args: &[String],
    env: impl Fn(&str) -> Option<String>,
) -> Result<KnobConfig, KnobRefusal> {
    let mut cli = Vec::with_capacity(args.len());
    for arg in args {
        let Some((name, value)) = arg.split_once('=') else {
            return Err(refusal(
                ServerErrorCode::KnobInvalid,
                arg,
                "expected --knob <name>=<value>".to_string(),
            ));
        };
        let name = name.trim();
        if name != FOREIGN_SQLITE_ALLOWED_ROOTS && !KNOB_TABLE.iter().any(|spec| spec.name == name)
        {
            return Err(refusal(
                ServerErrorCode::KnobUnknown,
                name,
                "no knob table declares this name".to_string(),
            ));
        }
        cli.push((name.to_string(), value.to_string()));
    }
    resolve_all(&cli, &env)
}

/// The list-valued server config key naming the directories an attached SQLite file may
/// live under (MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.3). It is a path list, not a scalar, so
/// it is read here beside the scalar table; the default is the empty list, which refuses
/// every path.
pub const FOREIGN_SQLITE_ALLOWED_ROOTS: &str = "foreign.sqlite.allowed_roots";

/// Read `foreign.sqlite.allowed_roots`: the last `--knob` for the key, else
/// `MEDULLA_FOREIGN_SQLITE_ALLOWED_ROOTS`, split on `:`. Every entry must be an absolute
/// path; a relative one refuses startup with `KNOB_INVALID`.
pub fn load_allowed_roots(
    args: &[String],
    env: impl Fn(&str) -> Option<String>,
) -> Result<Vec<std::path::PathBuf>, KnobRefusal> {
    let from_cli = args.iter().rev().find_map(|arg| {
        let (name, value) = arg.split_once('=')?;
        (name.trim() == FOREIGN_SQLITE_ALLOWED_ROOTS).then(|| value.to_string())
    });
    let raw = from_cli.or_else(|| env("MEDULLA_FOREIGN_SQLITE_ALLOWED_ROOTS"));
    let mut roots = Vec::new();
    for entry in raw.iter().flat_map(|raw| raw.split(':')) {
        if entry.trim().is_empty() {
            continue;
        }
        let path = std::path::PathBuf::from(entry.trim());
        if !path.is_absolute() {
            return Err(refusal(
                ServerErrorCode::KnobInvalid,
                FOREIGN_SQLITE_ALLOWED_ROOTS,
                format!("root {entry:?} is not an absolute path"),
            ));
        }
        roots.push(path);
    }
    Ok(roots)
}

static ALLOWED_ROOTS: OnceLock<Vec<std::path::PathBuf>> = OnceLock::new();

/// Hand the startup-loaded SQLite roots to the registration gate; the first install wins.
pub fn install_allowed_roots(roots: Vec<std::path::PathBuf>) {
    let _ = ALLOWED_ROOTS.set(roots);
}

/// The roots an attached SQLite file may live under; empty until installed, so every
/// path is refused.
pub fn allowed_roots() -> &'static [std::path::PathBuf] {
    ALLOWED_ROOTS.get().map_or(&[], Vec::as_slice)
}

static INSTALLED: OnceLock<KnobConfig> = OnceLock::new();

/// Hand the startup-loaded knobs to the listeners and served handlers; the
/// first install wins.
pub fn install(config: KnobConfig) {
    let _ = INSTALLED.set(config);
}

/// The knobs loaded at startup, or the defaults where no loader ran.
pub fn knobs() -> &'static KnobConfig {
    INSTALLED.get_or_init(KnobConfig::default)
}
