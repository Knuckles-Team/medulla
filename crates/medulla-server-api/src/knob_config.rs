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
    /// Memory search-state size cap.
    memory_search_state_max_bytes: u64 = "memory.search_state_max_bytes", U64(1_048_576), [65_536, 1_048_576], "bytes", Tunable;
    /// TOC line-index stride.
    toc_line_index_stride: u64 = "toc.line_index_stride", U64(64), [16, 1_024], "lines", Tunable;
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
        if !KNOB_TABLE.iter().any(|spec| spec.name == name) {
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
