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
    /// Discover neighbour expansion: neighbours kept per hop.
    discover_hop_fanout: u64 = "discover.hop_fanout", U64(32), [1, 256], "nodes", Tunable;
    /// Discover neighbour expansion: score decay per hop.
    discover_hop_decay: f64 = "discover.hop_decay", F64(0.5), [0.1, 0.9], "ratio", Tunable;
    /// Discover verdict: lowest mean hit score that still passes the quality gate.
    discover_min_composite_quality: f64 = "discover.min_composite_quality", F64(0.25), [0.0, 1.0], "score", Tunable;
    /// Discover verdict: lowest returned-to-requested hit ratio that still passes the gate.
    discover_min_population_ratio: f64 = "discover.min_population_ratio", F64(0.5), [0.0, 1.0], "ratio", Tunable;
    /// Discover verdict: most graph versions the index may trail the graph before it is stale.
    discover_max_version_lag: u64 = "discover.max_version_lag", U64(1000), [1, 1_000_000_000], "versions", Tunable;
    /// Discover deep mode: most expansions asked of the expansion port per request.
    discover_expansions: u64 = "discover.expansions", U64(4), [1, 8], "items", Tunable;
    /// Discover deep mode: reciprocal-rank-fusion damping constant.
    discover_rrf_k: u64 = "discover.rrf_k", U64(60), [1, 1000], "rank", Tunable;
    /// Discover recency: weight of the recency boost.
    discover_recency_weight: f64 = "discover.recency_weight", F64(0.5), [0.0, 2.0], "ratio", Tunable;
    /// Discover recency: default half life when the node type declares none.
    discover_recency_half_life_days: u64 = "discover.recency_half_life_days", U64(30), [1, 3650], "days", Tunable;
    /// Context bundle: reciprocal-rank-fusion damping constant.
    context_rrf_k: u64 = "context.rrf_k", U64(60), [1, 1000], "rank", Tunable;
    /// Context bundle: fewest items kept by the gap cut.
    context_min_items: u64 = "context.min_items", U64(3), [1, 64], "items", Tunable;
    /// Context bundle: solver node budget for the certified selection.
    context_solver_node_budget: u64 = "context.solver_node_budget", U64(100_000), [1000, 5_000_000], "nodes", Tunable;
    /// Context bundle: proof edges kept per item.
    context_proof_edges_per_item: u64 = "context.proof_edges_per_item", U64(3), [0, 16], "edges", Tunable;
    /// Context bundle: least value per token a chosen span must carry.
    context_value_floor_micro_per_token: u64 = "context.value_floor_micro_per_token", U64(0), [0, 1_000_000], "micro-units", Tunable;
    /// Executor schedule tick: seconds between passes over the due schedule windows.
    jobs_schedule_poll_s: u64 = "jobs.schedule_poll_s", U64(1), [1, 60], "seconds", Tunable;
    /// Schedule budget: default most run seconds one schedule's runs may use per budget period.
    jobs_schedule_budget_run_s: u64 = "jobs.schedule_budget_run_s", U64(3_600), [1, 31_536_000], "seconds", Tunable;
    /// Schedule budget: default most tokens one schedule's runs may use per budget period.
    jobs_schedule_budget_tokens: u64 = "jobs.schedule_budget_tokens", U64(1_000_000), [1, 1_000_000_000_000], "tokens", Tunable;
    /// Schedule budget: default length of one budget period.
    jobs_schedule_budget_period_s: u64 = "jobs.schedule_budget_period_s", U64(86_400), [1, 31_536_000], "seconds", Tunable;
    /// Schedule tick: how far back a new leader records missed windows.
    jobs_schedule_missed_lookback_s: u64 = "jobs.schedule_missed_lookback_s", U64(86_400), [60, 2_592_000], "seconds", Tunable;
    /// User tables: times an auto-commit UPDATE or DELETE retries after `TXN_CONFLICT` before `TXN_RETRY_EXHAUSTED` (MEDULLA-ENGINE-PARITY-R004.2).
    tables_occ_max_retries: u64 = "tables.occ_max_retries", U64(3), [0, 10], "retries", Tunable;
    /// Trace critique read: most nodes one page returns (MEDULLA-DECISION-ENGINE-R133.7.3).
    trace_critique_page_limit: u64 = "trace.critique_page_limit", U64(64), [1, 256], "rows", Tunable;
    /// Batch: most operations one served `Batch` carries; a larger batch refuses `BATCH_TOO_LARGE` before any operation runs (MEDULLA-DURABLE-KERNEL-R871).
    batch_max_operations: u64 = "batch.max_operations", U64(64), [1, 1024], "operations", Tunable;
    /// Retrieval: how many candidates the reranker scores.
    retrieval_rerank_k: u64 = "retrieval.rerank_k", U64(64), [8, 256], "candidates", Tunable;
    /// Retrieval round: fewest rows a sufficiency verdict accepts.
    retrieval_min_evidence: u64 = "retrieval.min_evidence", U64(3), [1, 64], "rows", Tunable;
    /// Retrieval round: most rounds one retrieval loop may run.
    retrieval_round_limit: u64 = "retrieval.round_limit", U64(4), [1, 16], "rounds", Tunable;
    /// Retrieval round: most tokens one retrieval loop may use.
    retrieval_token_limit: u64 = "retrieval.token_limit", U64(32_000), [1000, 1_000_000], "tokens", Tunable;
    /// Retrieval round: most inference calls one retrieval loop may make.
    retrieval_inference_limit: u64 = "retrieval.inference_limit", U64(8), [1, 64], "calls", Tunable;
    /// Retrieval round: most perspective queries one feedback part may carry.
    retrieval_feedback_queries: u64 = "retrieval.feedback_queries", U64(8), [1, 64], "queries", Tunable;
    /// Retrieval round: most terms one feedback part may carry.
    retrieval_feedback_terms: u64 = "retrieval.feedback_terms", U64(32), [1, 256], "terms", Tunable;
    /// Retrieval round: most cost one retrieval loop may incur.
    retrieval_cost_limit: u64 = "retrieval.cost_limit", U64(1_000_000), [1, 1_000_000_000_000], "micro-units", Tunable;
    /// Retrieval round: score change below which quality has saturated.
    retrieval_saturation_epsilon: f64 = "retrieval.saturation_epsilon", F64(0.01), [0.0, 0.2], "score", Tunable;
    /// Retrieval round: most chunks the reranker scores for selection.
    retrieval_rerank_pool: u64 = "retrieval.rerank_pool", U64(50), [1, 256], "chunks", Tunable;
    /// Retrieval round: most nDCG at 10 an adaptive selection policy may lose to its baseline.
    retrieval_selection_quality_floor: f64 = "retrieval.selection_quality_floor", F64(0.01), [0.0, 0.1], "nDCG", Tunable;
    /// Retrieval round: most the mean selected count may be of the fixed-count baseline's.
    retrieval_selection_cost_ratio_max: f64 = "retrieval.selection_cost_ratio_max", F64(0.9), [0.5, 1.0], "ratio", Tunable;
    /// Retrieval round: deepest document-section tree a traversal walks.
    retrieval_section_max_depth: u64 = "retrieval.section_max_depth", U64(32), [1, 64], "levels", Tunable;
    /// Retrieval round: most section records a traversal reads.
    retrieval_section_max_visited: u64 = "retrieval.section_max_visited", U64(4096), [1, 65536], "records", Tunable;
    /// Branch diff: most entries one `BranchDiff` page returns (MEDULLA-BRANCH-MERGE-R006).
    branch_diff_page_size: u64 = "branch.diff_page_size", U64(1000), [1, 10000], "entities", Tunable;
    /// Chunk encoding: tokens in one encoder window (MEDULLA-FEDERATED-QUERY-R039.2.2).
    retrieval_encode_window_tokens: u64 = "retrieval.encode_window_tokens", U64(512), [16, 8192], "tokens", Tunable;
    /// Chunk encoding: tokens shared by neighbouring windows; below the window size.
    retrieval_encode_window_overlap: u64 = "retrieval.encode_window_overlap", U64(64), [0, 512], "tokens", Tunable;
    /// Document state: bytes of document text in one state segment (MEDULLA-FEDERATED-QUERY-R081.101.2).
    retrieval_state_segment_bytes: u64 = "retrieval.state_segment_bytes", U64(512), [64, 8192], "bytes", Tunable;
    /// Document state: whether the deterministic built-in segment encoder is bound; the engine
    /// builds no state and a state rerank refuses while it is off (MEDULLA-FEDERATED-QUERY-R081.101.2).
    retrieval_state_builtin_encoder: bool = "retrieval.state_builtin_encoder", Bool(true), [false, true], "switch", Tunable;
    /// Retrieval round: L1 weight of the joint sparse selection.
    retrieval_joint_lambda: f64 = "retrieval.joint_lambda", F64(0.1), [0.0001, 10.0], "unitless", Tunable;
    /// Retrieval round: coefficient change below which the joint solve stops.
    retrieval_joint_tolerance: f64 = "retrieval.joint_tolerance", F64(0.000001), [0.000000001, 0.001], "unitless", Tunable;
    /// Retrieval round: most sweeps of the joint solve.
    retrieval_joint_max_iters: u64 = "retrieval.joint_max_iters", U64(1000), [10, 100000], "sweeps", Tunable;
    /// Pareto front: most candidates the front keeps; the smallest crowding distance leaves first (MEDULLA-DECISION-ENGINE-R800.2).
    front_max_size: u64 = "front.max_size", U64(256), [16, 1024], "candidates", Tunable;
    /// Chunk encoding: most chunks one document may take (MEDULLA-FEDERATED-QUERY-R039.2.6).
    retrieval_encode_budget_chunks: u64 = "retrieval.encode_budget_chunks", U64(4096), [1, 100_000], "chunks", Tunable;
    /// Source coverage: hops searched from the queried roots when the request names no budget (MEDULLA-SRC-AUTH-R004.6.1).
    source_coverage_hop_budget: u64 = "source_coverage.hop_budget", U64(3), [0, 8], "hops", Tunable;
    /// Source coverage: most classes one request may name.
    source_coverage_max_classes: u64 = "source_coverage.max_classes", U64(64), [1, 512], "classes", Tunable;
    /// RESP listener: compare-and-swap attempts one `INCR` makes before it answers `INCR_CONFLICT` (MEDULLA-DURABLE-KERNEL-R860.3).
    resp_incr_retry_limit: u64 = "resp.incr_retry_limit", U64(8), [1, 64], "attempts", Tunable;
    /// Embedding provider: most items in one call (MEDULLA-FEDERATED-QUERY-R040.5.4).
    embed_batch_items: u64 = "embed.batch_items", U64(64), [1, 1024], "items", Tunable;
    /// Embedding provider: most source bytes in one call.
    embed_batch_bytes: u64 = "embed.batch_bytes", U64(1_048_576), [4096, 67_108_864], "bytes", Tunable;
    /// Embedding provider: calls in flight at once.
    embed_concurrency: u64 = "embed.concurrency", U64(4), [1, 64], "calls", Tunable;
    /// Embedding provider: calls one run may spend.
    embed_calls_max: u64 = "embed.calls_max", U64(1000), [1, 1_000_000], "calls", Tunable;
    /// Embedding provider: resident cache entries.
    embed_cache_entries: u64 = "embed.cache_entries", U64(10_000), [0, 10_000_000], "entries", Tunable;
    /// Embedding provider: resident cache bytes.
    embed_cache_bytes: u64 = "embed.cache_bytes", U64(67_108_864), [0, 4_294_967_296], "bytes", Tunable;
    /// Council agreement quorum.
    council_quorum_fraction: f64 = "council.quorum_fraction", F64(0.5), [0.5, 1.0], "fraction", Tunable;
    /// Edit concentration: shipped edits on one dimension inside the window that revert a new edit (MEDULLA-DECISION-ENGINE-R173.3); the default when a policy names none.
    concentration_min_shipped: u64 = "concentration.min_shipped", U64(3), [2, 20], "edits", Tunable;
    /// Edit concentration: rounds in the window the shipped edits are counted over; the default when a policy names none.
    concentration_window_rounds: u64 = "concentration.window_rounds", U64(5), [2, 50], "rounds", Tunable;
    /// Designation reward decay.
    designation_reward_decay: f64 = "designation.reward_decay", F64(0.99), [0.5, 1.0], "factor", Tunable;
    /// Designation required-capability cap.
    designation_max_required_capabilities: u64 = "designation.max_required_capabilities", U64(32), [32, 32], "count", Fixed;
    /// Concept lineage read: most direct successors one `Read` returns before it reports `truncated` (MEDULLA-DURABLE-KERNEL-R094.7.1).
    concept_lineage_read_max_successors: u64 = "concept_lineage.read_max_successors", U64(1_024), [1, 65_536], "successors", Tunable;
    /// Time a cross-shard COMMIT decision may wait after its request before it is refused (MEDULLA-DECISION-ENGINE-R036.105.2).
    xshard_decision_deadline_ms: u64 = "xshard.decision_deadline_ms", U64(30_000), [1_000, 600_000], "ms", Tunable;
    /// Designation feed: Agent Library outbox rows one sweep claims.
    designation_feed_claim_limit: u64 = "designation.feed_claim_limit", U64(16), [1, 256], "outbox rows", Tunable;
    /// Designation feed: how long a claimed outbox row stays leased.
    designation_feed_lease_ms: u64 = "designation.feed_lease_ms", U64(120_000), [1000, 3_600_000], "milliseconds", Tunable;
    /// Designation feed: idle back-off between sweeps.
    designation_feed_idle_ms: u64 = "designation.feed_idle_ms", U64(500), [10, 60_000], "milliseconds", Tunable;
    /// Topology slot width cap.
    topology_max_width: u64 = "topology.max_width", U64(16), [1, 16], "count", Tunable;
    /// Topology depth cap.
    topology_max_depth: u64 = "topology.max_depth", U64(8), [1, 8], "count", Tunable;
    /// Topology round cap.
    topology_max_rounds: u64 = "topology.max_rounds", U64(8), [1, 8], "count", Tunable;
    /// Topology analogy score floor.
    topology_analogy_min: f64 = "topology.analogy_min", F64(0.6), [0.1, 1.0], "ratio", Tunable;
    /// How long the capacity leases a committed topology plan acquires live (MEDULLA-DECISION-ENGINE-R113.2.5).
    topology_lease_ttl_ms: u64 = "topology.lease_ttl_ms", U64(900_000), [1_000, 86_400_000], "ms", Tunable;
    /// Primitive graph edge cap (MEDULLA-DECISION-ENGINE-R036.100.2).
    primitive_max_edges: u64 = "primitive.max_edges", U64(256), [16, 1_024], "edges", Tunable;
    /// Oldest coding-agent discovery receipt a plan accepts (MEDULLA-DECISION-ENGINE-R036.101).
    coding_agent_max_receipt_age: u64 = "coding_agent.max_receipt_age", U64(3_600), [3_600, 3_600], "seconds", Fixed;
    /// Topology candidate-set cap.
    topology_max_candidates: u64 = "topology.max_candidates", U64(1_000), [1, 100_000], "sets", Tunable;
    /// Schema-repair shadow ingest sample.
    schema_repair_shadow_sample_nodes: u64 = "schema_repair.shadow_sample_nodes", U64(10_000), [100, 1_000_000], "nodes", Tunable;
    /// Most ids an ontology diff lists per impact category (MEDULLA-TYPED-PACKS-R804.5); counts stay exact.
    ontology_diff_max_listed: u64 = "ontology_diff.max_listed", U64(100), [10, 1_000], "ids", Tunable;
    /// Reputation minimum independent labels.
    reputation_min_support: u64 = "reputation.min_support", U64(10), [5, 1_000], "labels", Tunable;
    /// Fewest backbones a transfer estimate may declare (MEDULLA-DECISION-ENGINE-R059.1.2).
    transfer_min_backbones: u64 = "transfer.min_backbones", U64(2), [2, 16], "count", Tunable;
    /// Fewest independent labels per backbone in a transfer estimate (MEDULLA-DECISION-ENGINE-R059.1.2).
    transfer_min_samples: u64 = "transfer.min_samples", U64(30), [10, 10_000], "labels", Tunable;
    /// Independent successes of one option under one question before it is a skill candidate (MEDULLA-DECISION-ENGINE-R059.2).
    transfer_skill_after_successes: u64 = "transfer.skill_after_successes", U64(3), [1, 100], "labels", Tunable;
    /// Fewest distinct records behind a skill candidate's successes (MEDULLA-DECISION-ENGINE-R059.2).
    transfer_min_rollouts: u64 = "transfer.min_rollouts", U64(3), [1, 100], "records", Tunable;
    /// Lifetime of a signed memory snapshot list cursor (MEDULLA-DURABLE-KERNEL-R085.2).
    memory_sync_cursor_ttl_s: u64 = "memory_sync.cursor_ttl_s", U64(900), [60, 86_400], "seconds", Tunable;
    /// Memory search-state size cap.
    memory_search_state_max_bytes: u64 = "memory.search_state_max_bytes", U64(1_048_576), [65_536, 1_048_576], "bytes", Tunable;
    /// TOC line-index stride.
    toc_line_index_stride: u64 = "toc.line_index_stride", U64(64), [16, 1_024], "lines", Tunable;
    /// A source record without retention is refused (MEDULLA-INGEST-WORKER-R900.2).
    ingest_retention_required: bool = "ingest.retention_required", Bool(false), [false, true], "flag", Tunable;
    /// Age after which a record's synced access counts as stale (MEDULLA-INGEST-WORKER-R902.2).
    ingest_acl_stale_after_s: u64 = "ingest.acl_stale_after_s", U64(3_600), [60, 604_800], "s", Tunable;
    /// Largest process or architecture exchange document the engine reads (MEDULLA-CQ-ORCH-R702.2).
    process_document_max_bytes: u64 = "process.document_max_bytes", U64(16_777_216), [1_024, 268_435_456], "bytes", Tunable;
    /// Deepest element nesting a process or architecture exchange document may have.
    process_document_max_depth: u64 = "process.document_max_depth", U64(64), [4, 256], "levels", Tunable;
    /// Most source records one `SourceSync` reconciles (MEDULLA-SRC-AUTH-R012.6); a larger read is refused.
    sync_max_records: u64 = "sync.max_records", U64(50_000), [1, 1_000_000], "records", Tunable;
    /// Grace window a held record missing from a complete enumeration waits before the sweep deletes it (MEDULLA-INGEST-WORKER-R903.2).
    ingest_reconcile_grace_s: u64 = "ingest.reconcile_grace_s", U64(86_400), [3_600, 2_592_000], "s", Tunable;
    /// Interval of the reconcile sweep job (MEDULLA-INGEST-WORKER-R903.2).
    ingest_reconcile_sweep_interval_s: u64 = "ingest.reconcile_sweep_interval_s", U64(300), [10, 86_400], "s", Tunable;
    /// Most nodes one document delete cascade may remove (MEDULLA-UNIFIED-DATA-PLANE-R651.4).
    document_max_cascade_nodes: u64 = "document.max_cascade_nodes", U64(100_000), [1, 10_000_000], "nodes", Tunable;
    /// TOC maximum served span.
    toc_max_span_lines: u64 = "toc.max_span_lines", U64(2_000), [10, 20_000], "lines", Tunable;
    /// Events each graph keeps in its change-feed ring; a cursor older than the window is `CURSOR_EXPIRED` (MEDULLA-ENGINE-PARITY-R015.2).
    cdc_ring_events: u64 = "cdc.ring_events", U64(65_536), [16, 16_777_216], "events", Tunable;
    /// Largest page of `ReadGraphChanges` (MEDULLA-DURABLE-KERNEL-R043.100.3).
    graph_change_feed_max_page: u64 = "graph_change_feed.max_page", U64(256), [1, 256], "events", Tunable;
    /// Lowest risk tier (0 low, 1 medium, 2 high) a write-back proposal queues for approval at.
    writeback_queue_min_tier: u64 = "writeback.queue_min_tier", U64(2), [0, 2], "tier", Fixed;
    /// Most proposals one approval queue may hold (MEDULLA-DECISION-ENGINE-R615.3).
    writeback_queue_max_proposals: u64 = "writeback.queue_max_proposals", U64(256), [1, 4_096], "proposals", Tunable;
    /// Nodes re-embedded per generation-cycle batch (MEDULLA-DECISION-ENGINE-R174.2).
    generation_batch_nodes: u64 = "generation.batch_nodes", U64(256), [16, 4_096], "nodes", Tunable;
    /// Directions a scoped query adapter fits (MEDULLA-DECISION-ENGINE-R174.7.2).
    adapter_rank: u64 = "adapter.rank", U64(8), [1, 8], "directions", Tunable;
    /// Share of judged decisions held out when fitting an adapter.
    adapter_holdout_pct: u64 = "adapter.holdout_pct", U64(20), [5, 50], "percent", Tunable;
    /// Fewest held-out items a scoped adapter fit needs.
    adapter_min_eval_items: u64 = "adapter.min_eval_items", U64(30), [10, 10_000], "items", Tunable;
    /// Most bytes one resident adapter may hold.
    adapter_max_resident_bytes: u64 = "adapter.max_resident_bytes", U64(40_960), [4_096, 1_048_576], "bytes", Tunable;
    /// Fewest post-activation outcomes before the monitor may roll back (MEDULLA-DECISION-ENGINE-R174.7.2.1).
    adapter_monitor_min_outcomes: u64 = "adapter.monitor_min_outcomes", U64(50), [10, 10_000], "outcomes", Tunable;
    /// Most adapters active per tenant (MEDULLA-DECISION-ENGINE-R174.7.4).
    adapter_max_per_tenant: u64 = "adapter.max_per_tenant", U64(64), [1, 1_024], "adapters", Tunable;
    /// Steps kept before the failing step when a failure pattern is reduced (MEDULLA-DECISION-ENGINE-R701.2).
    failure_context_steps: u64 = "failure.context_steps", U64(2), [0, 8], "steps", Tunable;
    /// Records a failure pattern needs before it files a gap.
    failure_min_count: u64 = "failure.min_count", U64(3), [2, 1_000], "records", Tunable;
    /// Fewest outcomes a tool needs before its success rate orders usage guidance steps (MEDULLA-DECISION-ENGINE-R177.2).
    usage_min_trials: u64 = "usage.min_trials", U64(5), [1, 1_000], "outcomes", Tunable;
    /// Most sources a research citation walk reads (MEDULLA-DECISION-ENGINE-R705.2).
    research_node_budget: u64 = "research.node_budget", U64(1_000), [10, 100_000], "nodes", Tunable;
    /// Most citation links a research walk follows from a seed.
    research_depth_limit: u64 = "research.depth_limit", U64(3), [1, 10], "links", Tunable;
    /// A research walk stops when the best expected value is at or below this.
    research_value_floor_micro: u64 = "research.value_floor_micro", U64(0), [0, 1_000_000], "micro-units", Tunable;
    /// Weight of token similarity in a feature pair score (MEDULLA-DECISION-ENGINE-R707.2).
    match_w_token: u64 = "match.w_token", U64(400_000), [0, 1_000_000], "ppm", Tunable;
    /// Weight of ontology label equality in a feature pair score.
    match_w_label: u64 = "match.w_label", U64(300_000), [0, 1_000_000], "ppm", Tunable;
    /// Weight of embedding cosine in a feature pair score.
    match_w_embedding: u64 = "match.w_embedding", U64(300_000), [0, 1_000_000], "ppm", Tunable;
    /// Most features scored in one same-label block.
    match_max_block: u64 = "match.max_block", U64(256), [8, 4_096], "features", Tunable;
    /// Lowest pair score that counts as a match.
    match_score_min: u64 = "match.score_min", U64(600_000), [0, 1_000_000], "ppm", Tunable;
    /// Name entropy at which a name alone never merges two features.
    match_entropy_mbit: u64 = "match.entropy_mbit", U64(3_500), [1_000, 6_000], "millibits", Tunable;
    /// Seconds over which a refresh budget's request allowance refills (MEDULLA-DECISION-ENGINE-R174.9.2).
    freshness_window: u64 = "freshness.window", U64(60), [1, 3_600], "seconds", Tunable;
    /// Team signal maximum age.
    team_signal_max_age: u64 = "team.signal_max_age", U64(30), [1, 3_600], "seconds", Tunable;
    /// Held-item minimum passes.
    held_item_min_passes: u64 = "held_item.min_passes", U64(2), [1, 20], "count", Tunable;
    /// Measured-fact minimum cases.
    measured_fact_min_cases: u64 = "measured_fact.min_cases", U64(30), [1, 100_000], "cases", Tunable;
    /// Red-team probe cap.
    red_team_max_probes: u64 = "red_team.max_probes", U64(50), [1, 500], "probes", Tunable;
    /// Revision proposal list page size: the default and the ceiling a request's limit is held to (MEDULLA-DECISION-ENGINE-R817).
    revision_proposals_page_limit: u64 = "revision_proposals.page_limit", U64(100), [1, 1_000], "rows", Tunable;
    /// Identity ownership writer share.
    identity_ownership_writer_share: f64 = "identity.ownership_writer_share", F64(0.10), [0.01, 0.5], "fraction", Tunable;
    /// Retrieval lineage depth.
    retrieval_lineage_depth: u64 = "retrieval.lineage_depth", U64(8), [1, 16], "hops", Tunable;
    /// Schema drift gate: most records of one stream sampled to infer a delivery shape.
    drift_sample_max: u64 = "drift.sample_max", U64(1000), [10, 10_000], "records", Tunable;
    /// Connector conformance evidence: how long one onboarding import's evidence stands.
    conformance_evidence_ttl_ms: u64 = "conformance.evidence_ttl_ms", U64(2_592_000_000), [60_000, 31_536_000_000], "ms", Tunable;
    /// Regime forecast: largest horizon a request may ask for.
    finance_regime_forecast_max_horizon: u64 = "finance.regime_forecast.max_horizon", U64(10_000), [1, 1_000_000], "steps", Tunable;
    /// Regime forecast: tolerance on every probability vector and matrix row sum.
    finance_regime_forecast_sum_tolerance: f64 = "finance.regime_forecast.sum_tolerance", F64(0.000000001), [0.000000000001, 0.000001], "probability", Tunable;
    /// Regime forecast: the ceiling of a policy `max_brier` (the largest possible Brier score); a policy supplies the threshold and none is defaulted from this.
    finance_regime_forecast_max_brier: f64 = "finance.regime_forecast.max_brier", F64(2.0), [0.0, 2.0], "squared probability", Tunable;
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
    /// Most enhancement proposals one `RankProposals` call keeps
    /// (MEDULLA-DECISION-ENGINE-R614.2.2).
    enrichment_proposal_top_n: u64 = "enrichment.proposal_top_n", U64(20), [1, 200], "proposals", Tunable;
    /// Lowest confidence, in basis points, at which `Classify` writes an assignment
    /// (MEDULLA-DECISION-ENGINE-R614.2.3).
    enrichment_topic_min_confidence_bp: u64 = "enrichment.topic_min_confidence_bp", U64(3_000), [0, 10_000], "basis points", Tunable;
    /// Most nodes one `Project` call reads (MEDULLA-DECISION-ENGINE-R614.2.4).
    enrichment_projection_batch: u64 = "enrichment.projection_batch", U64(10_000), [100, 100_000], "nodes", Tunable;
    /// Longest `instructions` text of an assistant profile
    /// (MEDULLA-DECISION-ENGINE-R656.1).
    profile_instructions_max_chars: u64 = "profile.instructions_max_chars", U64(8000), [200, 64_000], "characters", Tunable;
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
    /// Deepest variable-length openCypher path a graph view compiles (MEDULLA-UNIFIED-DATA-PLANE-R038.6.2).
    federation_cypher_max_path_depth: u64 = "federation.cypher_max_path_depth", U64(8), [1, 32], "hops", Tunable;
    /// Longest a web metasearch result is served from the cache (MEDULLA-UNIFIED-DATA-PLANE-R038.7.2).
    federation_web_cache_ttl_s: u64 = "federation.web_cache_ttl_s", U64(900), [0, 86_400], "s", Tunable;
    /// Bits per character at or above which a name is an opaque token that never merges on the name alone (MEDULLA-UNIFIED-DATA-PLANE-R030.701).
    entity_link_entropy_bits: f64 = "entity_link.entropy_bits", F64(3.5), [2.0, 6.0], "bits per character", Tunable;
    /// Calibrated probability at or above which an entity pair is a Match (MEDULLA-UNIFIED-DATA-PLANE-R030).
    entity_link_match_min: f64 = "entity_link.match_min", F64(0.95), [0.5, 1.0], "ratio", Tunable;
    /// Calibrated probability at or below which an entity pair is a NonMatch (MEDULLA-UNIFIED-DATA-PLANE-R030).
    entity_link_nonmatch_max: f64 = "entity_link.nonmatch_max", F64(0.05), [0.0, 0.5], "ratio", Tunable;
    /// Embedding cosine at or above which a name match plus the embedding proposes a merge (MEDULLA-UNIFIED-DATA-PLANE-R030.701).
    entity_link_embedding_min: f64 = "entity_link.embedding_min", F64(0.9), [0.5, 1.0], "cosine", Tunable;
    /// Relevance below which a context source is omitted as irrelevant (MEDULLA-DECISION-ENGINE-R029.2.1).
    context_budget_relevance_min: f64 = "context_budget.relevance_min", F64(0.5), [0.0, 1.0], "ratio", Tunable;
    /// Longest a federated source statement may run.
    federation_statement_timeout_ms: u64 = "federation.statement_timeout_ms", U64(30_000), [1_000, 600_000], "ms", Tunable;
    /// Most rows a copy-served fragment may hold (MEDULLA-UNIFIED-DATA-PLANE-R038.2.2).
    acceleration_max_copy_rows: u64 = "acceleration.max_copy_rows", U64(1_000_000), [1, 100_000_000], "rows", Tunable;
    /// Belief revision per-weight cap (MEDULLA-DECISION-ENGINE-R700.2).
    belief_weight_cap_micro: u64 = "belief.weight_cap_micro", U64(6_000_000), [1_000_000, 12_000_000], "micro-nats", Tunable;
    /// Token-Jaccard floor for two claims to be compared as opposed (MEDULLA-DECISION-ENGINE-R700.2).
    belief_opposition_similarity_min: u64 = "belief.opposition_similarity_min", U64(800_000), [500_000, 1_000_000], "ppm", Tunable;
    /// Posterior below which a revision proposes a retraction (MEDULLA-DECISION-ENGINE-R700.4).
    belief_retraction_threshold_micro: i64 = "belief.retraction_threshold_micro", I64(-1_386_294), [-6_000_000, 0], "micro-nats", Tunable;
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
    /// Loop lease time to live (MEDULLA-DECISION-ENGINE-R653.2).
    loops_lease_ttl_s: u64 = "loops.lease_ttl", U64(60), [5, 3_600], "s", Tunable;
    /// Cap of a derived offer rate (MEDULLA-DECISION-ENGINE-R653.7).
    loops_offer_rate_cap_ppm: u64 = "loops.offer_rate_cap_ppm", U64(1_000_000), [0, 1_000_000], "ppm", Tunable;
    /// Open offers at which offer saturation reaches 100% (MEDULLA-DECISION-ENGINE-R653.7).
    loops_offer_capacity: u64 = "loops.offer_capacity", U64(8), [1, 256], "count", Tunable;
    /// Longest mined failure subsequence (MEDULLA-DECISION-ENGINE-R653.8).
    loops_miner_max_len: u64 = "loops.miner_max_len", U64(4), [2, 8], "steps", Tunable;
    /// Fewest failure sequences a mined pattern needs (MEDULLA-DECISION-ENGINE-R653.8).
    loops_miner_min_support: u64 = "loops.miner_min_support", U64(3), [2, 1_000], "sequences", Tunable;
    /// Most patterns one mine keeps (MEDULLA-DECISION-ENGINE-R653.8).
    loops_miner_max_patterns: u64 = "loops.miner_max_patterns", U64(64), [1, 1_024], "patterns", Tunable;
    /// Fewest samples a ratchet side needs (MEDULLA-DECISION-ENGINE-R653.8).
    loops_ratchet_min_samples: u64 = "loops.ratchet_min_samples", U64(3), [1, 1_000], "samples", Tunable;
    /// Smallest gain a ratchet keeps (MEDULLA-DECISION-ENGINE-R653.8).
    loops_ratchet_min_gain_micro: i64 = "loops.ratchet_min_gain_micro", I64(0), [-1_000_000, 1_000_000], "micro", Tunable;
    /// Smallest evidence support, in micro-nats, a claim needs to count as supported (MEDULLA-DECISION-ENGINE-R652.2).
    claims_support_min_micro: i64 = "claims.support_min_micro", I64(1_000_000), [0, 6_000_000], "micro-nats", Tunable;
    /// Longest wait for a stable WAL snapshot of an attached SQLite file (MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.1).
    foreign_sqlite_busy_timeout_ms: u64 = "foreign.sqlite.busy_timeout_ms", U64(1_000), [0, 60_000], "ms", Tunable;
    /// Watermark poll interval of an attached SQLite file (MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.2).
    foreign_sqlite_poll_interval_ms: u64 = "foreign.sqlite.poll_interval_ms", U64(5_000), [100, 3_600_000], "ms", Tunable;
    /// Most connections one catalog read of an attached MySQL or MariaDB source opens (MEDULLA-UNIFIED-DATA-PLANE-R014.2.2).
    foreign_mysql_pool_size: u64 = "foreign.mysql.pool_size", U64(4), [1, 64], "connections", Tunable;
    /// Largest hop bound of one column lineage impact walk (MEDULLA-UNIFIED-DATA-PLANE-R763.3).
    lineage_impact_max_depth: u64 = "lineage.impact_max_depth", U64(8), [1, 32], "hops", Tunable;
    /// Most columns one column lineage impact walk may reach (MEDULLA-UNIFIED-DATA-PLANE-R763.3).
    lineage_impact_max_columns: u64 = "lineage.impact_max_columns", U64(10_000), [100, 1_000_000], "columns", Tunable;
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
    /// Fraction of the answer-name tokens a search-task clue may expose (MEDULLA-FEDERATED-QUERY-R702.2).
    search_task_expose_overlap: f64 = "search_task.expose_overlap", F64(0.5), [0.1, 1.0], "ratio", Tunable;
    /// Answer-node degree that trips the prior-knowledge shortcut detector (MEDULLA-FEDERATED-QUERY-R702.2).
    search_task_prior_degree: u64 = "search_task.prior_degree", U64(50), [5, 10_000], "edges", Tunable;
    /// Refinement rounds a search task may take before it must stand (MEDULLA-FEDERATED-QUERY-R702.2).
    search_task_refine_rounds: u32 = "search_task.refine_rounds", U64(3), [0, 8], "rounds", Tunable;
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
    // The source access gate lives below this crate; it reads the resolved staleness bound.
    medulla_graph_core::rls::install_source_acl_stale_after_s(config.ingest_acl_stale_after_s);
    let _ = INSTALLED.set(config);
}

/// The knobs loaded at startup, or the defaults where no loader ran.
pub fn knobs() -> &'static KnobConfig {
    INSTALLED.get_or_init(KnobConfig::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R036.105.2
    #[test]
    fn decision_deadline_knob_bounds() {
        let spec = KNOB_TABLE
            .iter()
            .find(|spec| spec.name == "xshard.decision_deadline_ms")
            .expect("the decision deadline is a registry knob");
        assert_eq!(spec.default, KnobValue::U64(30_000));
        assert_eq!(spec.min, KnobValue::U64(1_000));
        assert_eq!(spec.max, KnobValue::U64(600_000));
        assert_eq!(spec.unit, "ms");
        assert_eq!(spec.class, KnobClass::Tunable);
        let none = |_: &str| None;
        assert_eq!(
            load_knobs(&[], none).unwrap().xshard_decision_deadline_ms,
            30_000
        );
        for (value, expected) in [("1000", 1_000), ("600000", 600_000), ("45000", 45_000)] {
            let args = vec![format!("xshard.decision_deadline_ms={value}")];
            assert_eq!(
                load_knobs(&args, none).unwrap().xshard_decision_deadline_ms,
                expected
            );
        }
        // A value outside the range refuses at startup, naming the knob.
        for value in ["999", "600001", "0"] {
            let args = vec![format!("xshard.decision_deadline_ms={value}")];
            let refusal = load_knobs(&args, none).unwrap_err();
            assert_eq!(refusal.code, ServerErrorCode::KnobOutOfRange);
            assert_eq!(refusal.knob, "xshard.decision_deadline_ms");
        }
        let refusal =
            load_knobs(&["xshard.decision_deadline_ms=soon".to_string()], none).unwrap_err();
        assert_eq!(refusal.code, ServerErrorCode::KnobInvalid);
    }

    // spec: MEDULLA-DURABLE-KERNEL-R871
    #[test]
    fn batch_max_operations_knob_bounds() {
        let spec = KNOB_TABLE
            .iter()
            .find(|spec| spec.name == "batch.max_operations")
            .expect("the batch operation cap is a registry knob");
        assert_eq!(spec.default, KnobValue::U64(64));
        assert_eq!(spec.min, KnobValue::U64(1));
        assert_eq!(spec.max, KnobValue::U64(1024));
        assert_eq!(spec.unit, "operations");
        assert_eq!(spec.class, KnobClass::Tunable);
        let none = |_: &str| None;
        assert_eq!(load_knobs(&[], none).unwrap().batch_max_operations, 64);
        for (value, expected) in [("1", 1), ("1024", 1024), ("8", 8)] {
            let args = vec![format!("batch.max_operations={value}")];
            assert_eq!(
                load_knobs(&args, none).unwrap().batch_max_operations,
                expected
            );
        }
        for value in ["0", "1025"] {
            let args = vec![format!("batch.max_operations={value}")];
            let refusal = load_knobs(&args, none).unwrap_err();
            assert_eq!(refusal.code, ServerErrorCode::KnobOutOfRange);
            assert_eq!(refusal.knob, "batch.max_operations");
        }
    }
}
