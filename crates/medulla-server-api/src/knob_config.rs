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
    fn to_value(self) -> KnobValue;
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
    fn to_value(self) -> KnobValue {
        KnobValue::U64(self)
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
    fn to_value(self) -> KnobValue {
        KnobValue::U64(u64::from(self))
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
    fn to_value(self) -> KnobValue {
        KnobValue::I64(self)
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
    fn to_value(self) -> KnobValue {
        KnobValue::F64(self)
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
    fn to_value(self) -> KnobValue {
        KnobValue::Bool(self)
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

        impl KnobConfig {
            /// The resolved value of the knob `name`, or `None` for a name the table does
            /// not hold (MEDULLA-OBSERVE-R006.12).
            pub fn value_of(&self, name: &str) -> Option<KnobValue> {
                match name {
                    $($name => Some(<$ty as KnobType>::to_value(self.$field)),)+
                    _ => None,
                }
            }
        }

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
    /// Metadata catalog: trailing window the `USAGE QUERIES` / `USAGE ASSETS` views rank over.
    catalog_top_queries_window_days: u64 = "catalog.top_queries_window_days", U64(30), [1, 365], "days", Tunable;
    /// KV-cache hot tier capacity.
    kvcache_hot_bytes: u64 = "kvcache.hot_bytes", U64(1_073_741_824), [67_108_864, 1_099_511_627_776], "bytes", Tunable;
    /// KV-cache warm tier capacity.
    kvcache_warm_bytes: u64 = "kvcache.warm_bytes", U64(4_294_967_296), [67_108_864, 1_099_511_627_776], "bytes", Tunable;
    /// KV-cache cold tier capacity.
    kvcache_cold_bytes: u64 = "kvcache.cold_bytes", U64(68_719_476_736), [67_108_864, 1_099_511_627_776], "bytes", Tunable;
    /// KV-cache admission floor: milliseconds saved per byte per hit.
    kvcache_min_worth: f64 = "kvcache.min_worth", F64(0.001), [0.0, 1000.0], "ms per byte per hit", Tunable;
    /// KV-cache demotion score weight of graph importance: score = recency + weight * importance.
    kvcache_importance_weight: f64 = "kvcache.importance_weight", F64(1.0), [0.0, 10.0], "ratio", Tunable;
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
    /// Retrieval feedback: most explicit events read per document.
    feedback_events_per_doc: u64 = "feedback.events_per_doc", U64(1000), [10, 100_000], "events", Tunable;
    /// Retrieval feedback: most stored retrieval outcomes read for implicit events.
    feedback_implicit_outcomes: u64 = "feedback.implicit_outcomes", U64(256), [16, 4096], "outcomes", Tunable;
    /// Retrieval feedback: weight of an implicit `cited` event.
    feedback_cited_weight: f64 = "feedback.cited_weight", F64(0.05), [0.0, 0.5], "weight", Tunable;
    /// Retrieval feedback: weight of an implicit `ignored` event.
    feedback_ignored_weight: f64 = "feedback.ignored_weight", F64(-0.02), [-0.5, 0.0], "weight", Tunable;
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
    /// Connector-pack projection: attempts one head record's projection gets before its retryable failure becomes the recorded terminal failure (MEDULLA-TYPED-PACKS-R096.2).
    connector_pack_projection_max_attempts: u64 = "connector_pack.projection_max_attempts", U64(5), [1, 50], "attempts", Tunable;
    /// Connector-pack projection: delay before the first retry; each later retry doubles it (MEDULLA-TYPED-PACKS-R096.2).
    connector_pack_projection_backoff_ms: u64 = "connector_pack.projection_backoff_ms", U64(1_000), [10, 600_000], "milliseconds", Tunable;
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
    /// Lifecycle edges: most `Release` nodes one derivation or release record reads (MEDULLA-SDLC-R001.3).
    lifecycle_release_scan_limit: u64 = "lifecycle.release_scan_limit", U64(100_000), [1, 10_000_000], "releases", Tunable;
    /// User tables: times an auto-commit UPDATE or DELETE retries after `TXN_CONFLICT` before `TXN_RETRY_EXHAUSTED` (MEDULLA-ENGINE-PARITY-R004.2).
    tables_occ_max_retries: u64 = "tables.occ_max_retries", U64(3), [0, 10], "retries", Tunable;
    /// User tables: most deferred foreign keys one transaction may touch; a larger transaction refuses `DEFERRED_FK_PENDING_EXCEEDED` (MEDULLA-ENGINE-PARITY-R011.1).
    tables_deferred_fk_pending_max: u64 = "tables.deferred_fk_pending_max", U64(medulla_tables::DEFERRED_FK_PENDING_MAX_DEFAULT), [1, 10_000_000], "keys", Tunable;
    /// User tables: deepest chain of triggers one write may nest; a deeper chain refuses `TRIGGER_DEPTH` (MEDULLA-ENGINE-PARITY-R012.2).
    tables_trigger_depth_max: u64 = "tables.trigger_depth_max", U64(medulla_tables::TRIGGER_DEPTH_MAX_DEFAULT), [1, 64], "levels", Tunable;
    /// Trace critique read: most nodes one page returns (MEDULLA-DECISION-ENGINE-R133.7.3).
    trace_critique_page_limit: u64 = "trace.critique_page_limit", U64(64), [1, 256], "rows", Tunable;
    /// Graph integrity validation: most nodes one integrity-shape scan covers; a larger graph reports a coverage gap (MEDULLA-FEDERATED-QUERY-R005.700).
    validate_shape_node_budget: u64 = "validate_shape.node_budget", U64(1_000_000), [1000, 10_000_000], "nodes", Tunable;
    /// Batch: most operations one served `Batch` carries; a larger batch refuses `BATCH_TOO_LARGE` before any operation runs (MEDULLA-DURABLE-KERNEL-R871).
    batch_max_operations: u64 = "batch.max_operations", U64(64), [1, 1024], "operations", Tunable;
    /// Wire logins: seconds a resolved wire-login record stays cached (MEDULLA-ENGINE-PARITY-R007.1).
    wire_login_cache_ttl_s: u64 = "wire_login.cache_ttl_s", U64(5), [0, 60], "s", Tunable;
    /// Wire listener: milliseconds a pgwire client may take to finish startup and authentication before the connection is dropped (MEDULLA-ENGINE-PARITY-R006.2).
    pgwire_startup_timeout_ms: u64 = "pgwire.startup_timeout_ms", U64(60000), [1000, 600000], "ms", Tunable;
    /// Graph node history: how many replaced node images each graph keeps so a row recorded at a graph version can be re-read at that version (MEDULLA-DECISION-ENGINE-R121.2). 0 keeps none.
    graph_node_history_revisions: u64 = "graph.node_history_revisions", U64(200_000), [0, 100_000_000], "revisions", Tunable;
    /// Data product proposals: principals that must each have queried every asset of a cluster.
    data_product_cluster_min_principals: u64 = "data_product.cluster_min_principals", U64(3), [2, 50], "principals", Tunable;
    /// Retrieval: how many candidates the reranker scores.
    retrieval_rerank_k: u64 = "retrieval.rerank_k", U64(64), [8, 256], "candidates", Tunable;
    /// Rerank calibration: fewest judged (score, relevant) pairs a fit accepts (MEDULLA-DECISION-ENGINE-R822).
    retrieval_calibration_min_pairs: u64 = "retrieval.calibration_min_pairs", U64(500), [50, 1_000_000], "pairs", Tunable;
    /// Retrieval round: fewest rows a sufficiency verdict accepts.
    retrieval_min_evidence: u64 = "retrieval.min_evidence", U64(3), [1, 64], "rows", Tunable;
    /// Retrieval round: most rounds one retrieval loop may run.
    retrieval_round_limit: u64 = "retrieval.round_limit", U64(4), [1, 16], "rounds", Tunable;
    /// Retrieval round: most tokens one retrieval loop may use.
    retrieval_token_limit: u64 = "retrieval.token_limit", U64(32_000), [1000, 1_000_000], "tokens", Tunable;
    /// Retrieval round: lowest measured score a transfer receipt may carry to move a query strategy to another retriever.
    retrieval_transfer_min_threshold: f64 = "retrieval.transfer_min_threshold", F64(0.6), [0.0, 1.0], "score", Tunable;
    /// Retrieval round: most inference calls one retrieval loop may make.
    retrieval_inference_limit: u64 = "retrieval.inference_limit", U64(8), [1, 64], "calls", Tunable;
    /// Retrieval round: most perspective queries one feedback part may carry.
    retrieval_feedback_queries: u64 = "retrieval.feedback_queries", U64(8), [1, 64], "queries", Tunable;
    /// Retrieval round: most terms one feedback part may carry.
    retrieval_feedback_terms: u64 = "retrieval.feedback_terms", U64(32), [1, 256], "terms", Tunable;
    /// Retrieval round: confidence gap a supported answer must exceed to win over another supported answer.
    retrieval_arbitration_margin: f64 = "retrieval.arbitration_margin", F64(0.10), [0.0, 0.5], "confidence", Tunable;
    /// Sufficiency (the fraction of requested slots filled with supported evidence) below which a second pass is planned (MEDULLA-FEDERATED-QUERY-R081.10).
    retrieval_second_pass_threshold: f64 = "retrieval.second_pass_threshold", F64(0.5), [0.0, 1.0], "fraction", Tunable;
    /// Most queries one second pass builds from the unmet slots (MEDULLA-FEDERATED-QUERY-R081.10).
    retrieval_second_pass_queries: u64 = "retrieval.second_pass_queries", U64(3), [1, 8], "queries", Tunable;
    /// Retrieval round: most cost one retrieval loop may incur.
    retrieval_cost_limit: u64 = "retrieval.cost_limit", U64(1_000_000), [1, 1_000_000_000_000], "micro-units", Tunable;
    /// Retrieval round: score change below which quality has saturated.
    retrieval_saturation_epsilon: f64 = "retrieval.saturation_epsilon", F64(0.01), [0.0, 0.2], "score", Tunable;
    /// Evidence forest: most similarity neighbours one memory contributes to the candidate walk (MEDULLA-FEDERATED-QUERY-R736.2).
    retrieval_forest_similar_k: u64 = "retrieval.forest.similar_k", U64(8), [1, 64], "neighbours", Tunable;
    /// Evidence forest: how much a candidate's gain shrinks per unit of similarity to a chosen candidate (MEDULLA-FEDERATED-QUERY-R736.3).
    retrieval_forest_redundancy_weight: f64 = "retrieval.forest.redundancy_weight", F64(0.5), [0.0, 4.0], "weight", Tunable;
    /// Evidence forest: above zero, a candidate in conflict with a chosen candidate is dropped (MEDULLA-FEDERATED-QUERY-R736.3).
    retrieval_forest_conflict_weight: f64 = "retrieval.forest.conflict_weight", F64(1.0), [0.0, 4.0], "weight", Tunable;
    /// Evidence forest: how many hops the candidate walk takes from the seeds (MEDULLA-FEDERATED-QUERY-R736.4).
    retrieval_forest_hops: u64 = "retrieval.forest.hops", U64(2), [1, 8], "hops", Tunable;
    /// Evidence forest: the most candidates the walk may collect before it refuses (MEDULLA-FEDERATED-QUERY-R736.4).
    retrieval_forest_candidate_cap: u64 = "retrieval.forest.candidate_cap", U64(2000), [1, 100000], "candidates", Tunable;
    /// Retrieval round: most chunks the reranker scores for selection.
    retrieval_rerank_pool: u64 = "retrieval.rerank_pool", U64(50), [1, 256], "chunks", Tunable;
    /// Retrieval round: most nDCG at 10 an adaptive selection policy may lose to its baseline.
    retrieval_selection_quality_floor: f64 = "retrieval.selection_quality_floor", F64(0.01), [0.0, 0.1], "nDCG", Tunable;
    /// Retrieval round: most the mean selected count may be of the fixed-count baseline's.
    retrieval_selection_cost_ratio_max: f64 = "retrieval.selection_cost_ratio_max", F64(0.9), [0.5, 1.0], "ratio", Tunable;
    /// Retrieval round: least held-out nDCG at 10 a distilled rerank head reaches to become champion (MEDULLA-FEDERATED-QUERY-R081.102).
    retrieval_distill_min_quality: f64 = "retrieval.distill_min_quality", F64(0.8), [0.0, 1.0], "nDCG", Tunable;
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
    /// Pareto front: fewest test-split examples a reflective job's evaluations are admitted over (MEDULLA-DECISION-ENGINE-R800.4).
    front_min_eval_examples: u64 = "front.min_eval_examples", U64(30), [10, 10000], "examples", Tunable;
    /// Chunk encoding: most chunks one document may take (MEDULLA-FEDERATED-QUERY-R039.2.6).
    retrieval_encode_budget_chunks: u64 = "retrieval.encode_budget_chunks", U64(4096), [1, 100_000], "chunks", Tunable;
    /// Source coverage: hops searched from the queried roots when the request names no budget (MEDULLA-SRC-AUTH-R004.6.1).
    source_coverage_hop_budget: u64 = "source_coverage.hop_budget", U64(3), [0, 8], "hops", Tunable;
    /// Source coverage: most classes one request may name.
    source_coverage_max_classes: u64 = "source_coverage.max_classes", U64(64), [1, 512], "classes", Tunable;
    /// Stored queries: shortest periodic refresh a put accepts (MEDULLA-FEDERATED-QUERY-R708.11).
    stored_query_min_period_s: u64 = "stored_query.min_period_s", U64(300), [60, 86400], "seconds", Tunable;
    /// Retrieval: the first fused candidates an agent's knowledge scope is tested on; the scope never reads the corpus (MEDULLA-FEDERATED-QUERY-R704.6).
    retrieval_scope_candidates: u64 = "retrieval.scope_candidates", U64(1024), [64, 16384], "candidates", Tunable;
    /// Stored queries: how often the schedule tick re-lints the stored versions against the trust flags (MEDULLA-FEDERATED-QUERY-R708.9).
    stored_query_relint_period_s: u64 = "stored_query.relint_period_s", U64(3600), [60, 86400], "seconds", Tunable;
    /// Bookmark relations: most edges one read window holds; a page beyond it or an answer above it refuses `BOOKMARK_LIMIT_EXCEEDED` (MEDULLA-MEDIA-VOCAB-R001.102.3).
    bookmark_relations_window: u64 = "bookmark.relations_window", U64(5000), [1, 100_000], "edges", Tunable;
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
    /// Share of a promoted rule's named resolutions that must be human (MEDULLA-DECISION-ENGINE-R129.5).
    rule_promotion_min_agreement: f64 = "rule_promotion.min_agreement", F64(0.8), [0.5, 1.0], "fraction", Fixed;
    /// Fewest human resolutions a rule promotion needs (MEDULLA-DECISION-ENGINE-R129.5).
    rule_promotion_min_overrides: u64 = "rule_promotion.min_overrides", U64(5), [1, 1_000], "resolutions", Fixed;
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
    /// Most rows the steps and workflow parts of a composed execution answer keep (MEDULLA-CQ-ORCH-R008.2).
    execution_answer_max_rows_per_part: u64 = "execution_answer.max_rows_per_part", U64(256), [1, 4_096], "rows", Tunable;
    /// Oldest coding-agent discovery receipt a plan accepts (MEDULLA-DECISION-ENGINE-R036.101).
    coding_agent_max_receipt_age: u64 = "coding_agent.max_receipt_age", U64(3_600), [3_600, 3_600], "seconds", Fixed;
    /// Topology candidate-set cap.
    topology_max_candidates: u64 = "topology.max_candidates", U64(1_000), [1, 100_000], "sets", Tunable;
    /// Most nodes a topology analytic accepts (MEDULLA-FEDERATED-QUERY-R607.4).
    topology_max_nodes: u64 = "topology.max_nodes", U64(200_000), [1_000, 5_000_000], "nodes", Tunable;
    /// Most edges a topology analytic accepts (MEDULLA-FEDERATED-QUERY-R607.4).
    topology_max_edges: u64 = "topology.max_edges", U64(2_000_000), [10_000, 50_000_000], "edges", Tunable;
    /// Schema-repair shadow ingest sample.
    schema_repair_shadow_sample_nodes: u64 = "schema_repair.shadow_sample_nodes", U64(10_000), [100, 1_000_000], "nodes", Tunable;
    /// Most ids an ontology diff lists per impact category (MEDULLA-TYPED-PACKS-R804.5); counts stay exact.
    ontology_diff_max_listed: u64 = "ontology_diff.max_listed", U64(100), [10, 1_000], "ids", Tunable;
    /// Most breaking changes a pending pack release reports (MEDULLA-TYPED-PACKS-R107.4).
    catalog_contract_break_report_max: u64 = "catalog.contract_break_report_max", U64(100), [10, 1_000], "items", Tunable;
    /// Most triples one catalog onboarding batch admits (MEDULLA-TYPED-PACKS-R106.2).
    catalog_onboard_batch_max_triples: u64 = "catalog.onboard_batch_max_triples", U64(50_000), [1_000, 500_000], "triples", Tunable;
    /// Reputation minimum independent labels.
    reputation_min_support: u64 = "reputation.min_support", U64(10), [5, 1_000], "labels", Tunable;
    /// Fewest backbones a transfer estimate may declare (MEDULLA-DECISION-ENGINE-R059.1.2).
    transfer_min_backbones: u64 = "transfer.min_backbones", U64(2), [2, 16], "count", Tunable;
    /// Fewest independent labels per backbone in a transfer estimate (MEDULLA-DECISION-ENGINE-R059.1.2).
    transfer_min_samples: u64 = "transfer.min_samples", U64(30), [10, 10_000], "labels", Tunable;
    /// Independent successes of one option under one question before it is a skill candidate (MEDULLA-DECISION-ENGINE-R059.2).
    transfer_skill_after_successes: u64 = "transfer.skill_after_successes", U64(3), [1, 100], "labels", Tunable;
    /// Most key groups one UQL `GROUP` stage may produce before it refuses `UQL_GROUP_LIMIT`
    /// (MEDULLA-FEDERATED-QUERY-R733).
    uql_group_max_groups: u64 = "uql.group.max_groups", U64(100_000), [1, 1_000_000], "groups", Tunable;
    /// Fewest distinct records behind a skill candidate's successes (MEDULLA-DECISION-ENGINE-R059.2).
    transfer_min_rollouts: u64 = "transfer.min_rollouts", U64(3), [1, 100], "records", Tunable;
    /// Lifetime of a signed memory snapshot list cursor (MEDULLA-DURABLE-KERNEL-R085.2).
    memory_sync_cursor_ttl_s: u64 = "memory_sync.cursor_ttl_s", U64(900), [60, 86_400], "seconds", Tunable;
    /// Seconds a resolved candidate or a superseded revision stays held before its blob is released (MEDULLA-DURABLE-KERNEL-R085.7).
    memory_sync_history_retention_s: u64 = "memory_sync.history_retention_s", U64(2_592_000), [3_600, 31_536_000], "seconds", Tunable;
    /// Most revisions one memory document keeps held; older ones release oldest first (MEDULLA-DURABLE-KERNEL-R085.7).
    memory_sync_max_revisions_per_doc: u64 = "memory_sync.max_revisions_per_doc", U64(64), [1, 4_096], "revisions", Tunable;
    /// Most bytes of memory snapshot content one tenant holds; a write over it is refused (MEDULLA-DURABLE-KERNEL-R085.7).
    memory_sync_capacity_bytes: u64 = "memory_sync.capacity_bytes", U64(1_073_741_824), [1_048_576, 1_099_511_627_776], "bytes", Tunable;
    /// Busy ratio at or above which a tokio worker, across two windows, is flagged spinning (MEDULLA-OBSERVE-R006.3).
    observe_worker_spin_busy_ratio: f64 = "observe.worker_spin_busy_ratio", F64(0.9), [0.5, 1.0], "ratio", Tunable;
    /// A lock held this long logs at WARN with its holder (MEDULLA-OBSERVE-R006.4).
    observe_lock_hold_warn_ms: u64 = "observe.lock_hold_warn_ms", U64(5_000), [100, 600_000], "ms", Tunable;
    /// A Running or Progressing task whose last progress is older than this is marked stalled by the watchdog (MEDULLA-OBSERVE-R005.2).
    observe_task_stall_after_ms: u64 = "observe.task_stall_after_ms", U64(60_000), [100, 3_600_000], "ms", Tunable;
    /// Time between two scans of the live tasks by the stall watchdog (MEDULLA-OBSERVE-R005.2).
    observe_stall_check_interval_ms: u64 = "observe.stall_check_interval_ms", U64(5_000), [50, 60_000], "ms", Tunable;
    /// Longest the `Health` call and the `/readyz` probe wait for the server state before they answer degraded (MEDULLA-OBSERVE-R007.3).
    observe_health_deadline_ms: u64 = "observe.health_deadline_ms", U64(2_000), [100, 30_000], "ms", Tunable;
    /// Transition events the ring keeps; the oldest leave first (MEDULLA-OBSERVE-R004.4).
    observe_event_ring_capacity: u64 = "observe.event_ring_capacity", U64(4_096), [256, 65_536], "events", Tunable;
    /// Live entries the task registry lists; a job that does not fit still runs unlisted (MEDULLA-OBSERVE-R003.1).
    observe_task_registry_capacity: u64 = "observe.task_registry_capacity", U64(4_096), [64, 65_536], "tasks", Tunable;
    /// Equal cursors in a row after which a progress guard fails its loop with NO_PROGRESS (MEDULLA-OBSERVE-R005.1).
    observe_progress_idle_limit: u64 = "observe.progress_idle_limit", U64(2), [1, 100], "iterations", Tunable;
    /// Boot reports kept in the control store (MEDULLA-OBSERVE-R008.2).
    observe_startup_reports_kept: u64 = "observe.startup_reports_kept", U64(8), [1, 64], "reports", Tunable;
    /// Sampling rate of the stack profiler (MEDULLA-OBSERVE-R010.1).
    observe_profile_hz: u64 = "observe.profile_hz", U64(99), [10, 1_000], "Hz", Tunable;
    /// Longest one profile call may sample; fixed (MEDULLA-OBSERVE-R010.1).
    observe_profile_max_seconds: u64 = "observe.profile_max_seconds", U64(60), [60, 60], "s", Fixed;
    /// Memory search-state size cap.
    memory_search_state_max_bytes: u64 = "memory.search_state_max_bytes", U64(1_048_576), [65_536, 1_048_576], "bytes", Tunable;
    /// Turns after which a pending derivation range is due at once (MEDULLA-DECISION-ENGINE-R651.11).
    memory_derive_batch_turns: u64 = "memory.derive_batch_turns", U64(8), [1, 64], "turns", Tunable;
    /// Quiet time after the last turn before a pending derivation range is due (MEDULLA-DECISION-ENGINE-R651.11).
    memory_derive_debounce: u64 = "memory.derive_debounce", U64(30_000), [1_000, 600_000], "ms", Tunable;
    /// Pending derivation tasks one tenant may hold before new ones are skipped (MEDULLA-DECISION-ENGINE-R651.11).
    memory_derive_max_pending: u64 = "memory.derive_max_pending", U64(10_000), [100, 1_000_000], "tasks", Tunable;
    /// Conclusions one derive job keeps (MEDULLA-DECISION-ENGINE-R651.12).
    memory_derive_max_conclusions: u64 = "memory.derive_max_conclusions", U64(8), [1, 32], "claims", Tunable;
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
    /// Most containers one served architecture view may hold (MEDULLA-CQ-ORCH-R010.2).
    architecture_view_max_containers: u64 = "architecture_view.max_containers", U64(512), [16, 4_096], "containers", Tunable;
    /// Least property coverage an induced ontology proposal needs to be admitted (MEDULLA-INGEST-WORKER-R002.3).
    ingest_proposal_min_coverage: f64 = "ingest.proposal_min_coverage", F64(0.8), [0.5, 1.0], "ratio", Tunable;
    /// Seconds between paper intake firings, aligned to UTC (MEDULLA-REPO-INGEST-R006.6).
    paper_intake_interval: u64 = "paper_intake.interval", U64(21_600), [3_600, 86_400], "seconds", Tunable;
    /// Most bytes one served media object may hold; a larger put is refused `MEDIA_TOO_LARGE` (MEDULLA-MEDIA-VOCAB-R657.1).
    media_max_bytes: u64 = "media.max_bytes", U64(33_554_432), [1_024, 67_108_864], "bytes", Tunable;
    /// Most records one drain page task holds (MEDULLA-INGEST-WORKER-R605.2).
    ingest_drain_page_records: u64 = "ingest.drain_page_records", U64(500), [1, 1_024], "records", Tunable;
    /// Most class redundancy an induced ontology proposal may carry and still be admitted (MEDULLA-INGEST-WORKER-R002.3).
    ingest_proposal_max_redundancy: f64 = "ingest.proposal_max_redundancy", F64(0.1), [0.0, 0.5], "ratio", Tunable;
    /// TOC maximum served span.
    toc_max_span_lines: u64 = "toc.max_span_lines", U64(2_000), [10, 20_000], "lines", Tunable;
    /// Events each graph keeps in its change-feed ring; a cursor older than the window is `CURSOR_EXPIRED` (MEDULLA-ENGINE-PARITY-R015.2).
    cdc_ring_events: u64 = "cdc.ring_events", U64(65_536), [16, 16_777_216], "events", Tunable;
    /// Largest page of `ReadGraphChanges` (MEDULLA-DURABLE-KERNEL-R043.100.3).
    graph_change_feed_max_page: u64 = "graph_change_feed.max_page", U64(256), [1, 256], "events", Tunable;
    /// Revision of the live-graph latency profile; `GraphLiveLimits` answers it and every change-feed event names it as `latency_profile_revision` (MEDULLA-DURABLE-KERNEL-R043.100.5).
    graph_live_profile_revision: u64 = "graph_live.profile_revision", U64(1), [1, 1_000_000], "revision", Tunable;
    /// 95th-percentile bound from a commit to its first authorized read, in milliseconds (MEDULLA-DURABLE-KERNEL-R043.100.5).
    graph_live_commit_to_visible_p95_ms: f64 = "graph_live.commit_to_visible_p95_ms", F64(1_000.0), [1.0, 60_000.0], "ms", Tunable;
    /// 95th-percentile bound of one live-graph frame, in milliseconds (MEDULLA-DURABLE-KERNEL-R043.100.5).
    graph_live_frame_p95_ms: f64 = "graph_live.frame_p95_ms", F64(16.7), [1.0, 1_000.0], "ms", Tunable;
    /// Bound on a live-graph layout settling after a change, in milliseconds (MEDULLA-DURABLE-KERNEL-R043.100.5).
    graph_live_layout_settle_ms: f64 = "graph_live.layout_settle_ms", F64(2_000.0), [1.0, 60_000.0], "ms", Tunable;
    /// Lowest risk tier (0 low, 1 medium, 2 high) a write-back proposal queues for approval at.
    writeback_queue_min_tier: u64 = "writeback.queue_min_tier", U64(2), [2, 2], "tier", Fixed;
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
    /// Most open knowledge flags one run may raise (MEDULLA-DECISION-ENGINE-R708.2).
    flag_max_open_per_run: u64 = "flag.max_open_per_run", U64(20), [1, 200], "flags", Tunable;
    /// Seconds between curation cycles; sets `due` (MEDULLA-DECISION-ENGINE-R709.7).
    curation_cadence_seconds: u64 = "curation.cadence_seconds", U64(86_400), [300, 604_800], "seconds", Tunable;
    /// Pass rate below which a skill suite is too low (MEDULLA-DECISION-ENGINE-R711.2).
    eval_pass_band_low_ppm: u64 = "eval.pass_band.low_ppm", U64(700_000), [0, 1_000_000], "ppm", Tunable;
    /// Pass rate above which a skill suite is suspiciously saturated.
    eval_pass_band_high_ppm: u64 = "eval.pass_band.high_ppm", U64(900_000), [0, 1_000_000], "ppm", Tunable;
    /// Wanted share of golden cases in a skill suite.
    eval_mix_golden_ppm: u64 = "eval.mix.golden_ppm", U64(50_000), [0, 1_000_000], "ppm", Tunable;
    /// Wanted share of real-prompt cases in a skill suite.
    eval_mix_real_ppm: u64 = "eval.mix.real_ppm", U64(800_000), [0, 1_000_000], "ppm", Tunable;
    /// Wanted share of author-judged cases in a skill suite.
    eval_mix_judged_ppm: u64 = "eval.mix.judged_ppm", U64(150_000), [0, 1_000_000], "ppm", Tunable;
    /// How far a suite share may sit from its wanted share.
    eval_mix_tolerance_ppm: u64 = "eval.mix.tolerance_ppm", U64(100_000), [0, 1_000_000], "ppm", Tunable;
    /// Fewest cases a skill suite needs before it counts.
    eval_min_cases: u64 = "eval.min_cases", U64(20), [1, 10_000], "cases", Tunable;
    /// Lowest judge agreement that lets a judged case count.
    eval_judge_min_kappa_ppm: u64 = "eval.judge.min_kappa_ppm", U64(600_000), [0, 1_000_000], "ppm", Tunable;
    /// Identical runs a noise band needs (MEDULLA-DECISION-ENGINE-R712.2).
    eval_ablation_identical_runs: u64 = "eval.ablation.identical_runs", U64(5), [2, 20], "runs", Tunable;
    /// Share of traffic a canary challenger takes.
    canary_traffic_ppm: u64 = "canary.traffic_ppm", U64(50_000), [1_000, 500_000], "ppm", Tunable;
    /// Eval runs of prior weight in the skill quality score (MEDULLA-DECISION-ENGINE-R713.5).
    skill_quality_prior_runs: u64 = "skill.quality.prior_runs", U64(20), [1, 1_000], "runs", Tunable;
    /// Fewest cohort principals before a peer recommendation answers (MEDULLA-DECISION-ENGINE-R713.6).
    skill_peers_min_peers: u64 = "skill.peers.min_peers", U64(5), [3, 100], "principals", Tunable;
    /// Salt of the principal pseudonym of a skill invocation (MEDULLA-DECISION-ENGINE-R713.1).
    skill_pseudonym_salt: u64 = "skill.pseudonym_salt", U64(1), [0, 1_000_000_000_000], "salt", Tunable;
    /// Most always-on skills per tenant (MEDULLA-DECISION-ENGINE-R714.2).
    skills_always_on_max: u64 = "skills.always_on_max", U64(100), [1, 500], "skills", Tunable;
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
    /// Seconds of inbound events a playbook correlation keeps per service (MEDULLA-DECISION-ENGINE-R704.2).
    playbook_window: u64 = "playbook.window", U64(300), [10, 86_400], "seconds", Tunable;
    /// State-change events inside the window that make a service flapping.
    playbook_flap_min_events: u64 = "playbook.flap_min_events", U64(3), [2, 100], "events", Tunable;
    /// Most neighbours entity-resolution candidate generation returns per node (MEDULLA-DECISION-ENGINE-R706.2).
    er_candidate_k: u64 = "er.candidate_k", U64(16), [1, 64], "neighbours", Tunable;
    /// Reviews in each window the entity-resolution accept rate is compared over.
    er_drift_window: u64 = "er.drift_window", U64(200), [20, 5_000], "reviews", Tunable;
    /// Absolute accept-rate change between two windows that reports drift.
    er_drift_max_ppm: u64 = "er.drift_max_ppm", U64(150_000), [10_000, 500_000], "ppm", Tunable;
    /// Most nodes one entity-resolution candidate index is built over (MEDULLA-DECISION-ENGINE-R706.3).
    er_index_max_nodes: u64 = "er.index_max_nodes", U64(50_000), [100, 1_000_000], "nodes", Tunable;
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
    /// Code context: default breadth-first depth of a `CodeContext` walk.
    code_context_depth: u64 = "code.context_depth", U64(3), [1, 6], "hops", Tunable;
    /// Code health: default step budget of one `CodeHealthSweep` traversal.
    code_health_step_budget: u64 = "code.health_step_budget", U64(5_000_000), [10_000, 1_000_000_000], "steps", Tunable;
    /// Code context: visited-node cap of a `CodeContext` walk.
    code_max_nodes: u64 = "code.max_nodes", U64(512), [32, 8192], "nodes", Tunable;
    /// Retrieval lineage depth.
    retrieval_lineage_depth: u64 = "retrieval.lineage_depth", U64(8), [1, 16], "hops", Tunable;
    /// Topic fusion weight `w` of `score = (1 - w) * dense + w * topic` (MEDULLA-FEDERATED-QUERY-R040.2.2).
    retrieval_topic_weight: f64 = "retrieval.topic_weight", F64(0.3), [0.0, 1.0], "ratio", Tunable;
    /// Mixed traversal and vector ranking: most widening rounds one `k` replenishment takes (MEDULLA-FEDERATED-QUERY-R039).
    retrieval_replenish_max_rounds: u64 = "retrieval.replenish_max_rounds", U64(6), [1, 32], "rounds", Tunable;
    /// Mixed traversal and vector ranking: the first candidate window is `k` times this (MEDULLA-FEDERATED-QUERY-R039).
    retrieval_oversample_factor: u64 = "retrieval.oversample_factor", U64(4), [1, 16], "factor", Tunable;
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
    /// Feedback rate limit per principal.
    feedback_rate_limit_per_hour: u64 = "feedback.rate_limit_per_hour", U64(100), [100, 100], "events per hour", Fixed;
    /// Federated raw-scan cost per KiB.
    federated_raw_scan_cost_per_kib: f64 = "federated.raw_scan_cost_per_kib", F64(1.0), [1.0, 100.0], "cost units per KiB", Tunable;
    /// Interactive query node budget.
    query_interactive_node_budget: u64 = "query.interactive_node_budget", U64(100_000), [1_000, 10_000_000], "nodes", Tunable;
    /// Plan records kept per tenant by the plan record store; the oldest is dropped first (MEDULLA-FEDERATED-QUERY-R732.1).
    query_plan_records: u64 = "query.plan_records", U64(4_096), [256, 65_536], "records", Tunable;
    /// Fraction of the digest space of interactive plans that also run under a challenger planner revision in shadow (MEDULLA-FEDERATED-QUERY-R732.3).
    query_planner_shadow_fraction: f64 = "query.planner_shadow_fraction", F64(0.05), [0.0, 0.2], "fraction", Tunable;
    /// Plan records or shadow comparisons a planner revision needs before the evolution job acts (MEDULLA-FEDERATED-QUERY-R732.4).
    query_planner_min_records: u64 = "query.planner_min_records", U64(64), [16, 4_096], "records", Tunable;
    /// Unified planner gate (MEDULLA-FEDERATED-QUERY-R730): arm A passes when its p95 latency is at most this multiple of arm B's.
    planner_gate_max_p95_ratio: f64 = "planner.gate.max_p95_ratio", F64(0.7), [0.7, 0.7], "ratio", Fixed;
    /// Unified planner gate: the number of top vector matches whose recall must be equal in both arms.
    planner_gate_recall_k: u64 = "planner.gate.recall_k", U64(50), [50, 50], "matches", Fixed;
    /// Unified planner gate: claims in the workload.
    planner_gate_claims: u64 = "planner.gate.claims", U64(10_000), [10_000, 10_000], "claims", Fixed;
    /// Unified planner gate: embedding dimension of every claim.
    planner_gate_embedding_dim: u64 = "planner.gate.embedding_dim", U64(1_024), [1_024, 1_024], "dimensions", Fixed;
    /// Unified planner gate: attributes on every claim.
    planner_gate_attributes: u64 = "planner.gate.attributes", U64(8), [8, 8], "attributes", Fixed;
    /// Unified planner gate: concurrent writers during the measured queries.
    planner_gate_writers: u64 = "planner.gate.writers", U64(4), [4, 4], "writers", Fixed;
    /// Unified planner gate: writes each concurrent writer issues; every one must be readable afterwards.
    planner_gate_writes_per_writer: u64 = "planner.gate.writes_per_writer", U64(250), [250, 250], "writes", Fixed;
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
    /// Most IRIs one document set may name or resolve to
    /// (MEDULLA-DECISION-ENGINE-R656.3).
    profile_document_set_max_members: u64 = "profile.document_set_max_members", U64(5000), [10, 100_000], "IRIs", Tunable;
    /// Most steps plus flows of one imported canonical workflow (MEDULLA-CQ-ORCH-R701.2).
    process_import_max_elements: u64 = "process.import_max_elements", U64(100_000), [100, 1_000_000], "elements", Tunable;
    /// Team score: weight of the prior, as a count of outcomes. (MEDULLA-ORCH-VOCAB-R751.751).
    /// Most columns one `DatasetProfile` job profiles (MEDULLA-UNIFIED-DATA-PLANE-R761.3).
    profile_max_columns: u64 = "profile.max_columns", U64(256), [1, 4_096], "columns", Tunable;
    /// Connect and read timeout of a profile read on an attached SQL source
    /// (MEDULLA-UNIFIED-DATA-PLANE-R761.3).
    profile_source_timeout_ms: u64 = "profile.source_timeout_ms", U64(10_000), [1_000, 300_000], "ms", Tunable;
    /// Non-null values the sensitive scan samples per column (MEDULLA-TYPED-PACKS-R803.2).
    sensitive_max_sample_values: u64 = "sensitive.max_sample_values", U64(10_000), [100, 1_000_000], "values", Tunable;
    /// Compiled-size limit of one detector pattern (MEDULLA-TYPED-PACKS-R803.2).
    sensitive_regex_size_limit: u64 = "sensitive.regex_size_limit", U64(1_048_576), [65_536, 16_777_216], "bytes", Tunable;
    /// Values a model detector compares per column (MEDULLA-TYPED-PACKS-R803.2).
    sensitive_model_sample_values: u64 = "sensitive.model_sample_values", U64(200), [10, 5_000], "values", Tunable;
    /// Chunks of one Document the sensitive scan reads (MEDULLA-TYPED-PACKS-R803.7).
    sensitive_max_chunks: u64 = "sensitive.max_chunks", U64(500), [10, 10_000], "chunks", Tunable;
    /// Rows the reservoir of a sampled column profile keeps (MEDULLA-UNIFIED-DATA-PLANE-R761.2).
    profile_sample_rows: u64 = "profile.sample_rows", U64(100_000), [1_000, 10_000_000], "rows", Tunable;
    /// Most frequent values one column profile keeps (MEDULLA-UNIFIED-DATA-PLANE-R761.2).
    profile_top_k: u64 = "profile.top_k", U64(10), [1, 1_000], "values", Tunable;
    /// Equal-width histogram buckets of a numeric column profile (MEDULLA-UNIFIED-DATA-PLANE-R761.2).
    profile_histogram_buckets: u64 = "profile.histogram_buckets", U64(20), [1, 1_000], "buckets", Tunable;
    /// Consecutive failing scores of one quality rule that open an incident
    /// (MEDULLA-UNIFIED-DATA-PLANE-R762.2).
    quality_incident_after_failures: u64 = "quality.incident_after_failures", U64(3), [1, 100], "failures", Tunable;
    /// Correlation window of the incident a failing quality rule records
    /// (MEDULLA-UNIFIED-DATA-PLANE-R762.3).
    quality_incident_window_ms: u64 = "quality.incident_window_ms", U64(86_400_000), [60_000, 2_592_000_000], "ms", Tunable;
    /// Most focus nodes one shape rule of a `QualityEvaluate` job validates
    /// (MEDULLA-UNIFIED-DATA-PLANE-R762.9).
    quality_max_focus_nodes: u64 = "quality.max_focus_nodes", U64(100_000), [1, 10_000_000], "nodes", Tunable;
    /// Most rules one `QualityEvaluate` job evaluates (MEDULLA-UNIFIED-DATA-PLANE-R762.3).
    quality_max_rules: u64 = "quality.max_rules", U64(1_000), [1, 100_000], "rules", Tunable;
    /// Most incidents one `QualityEvaluate` job loads for correlation
    /// (MEDULLA-UNIFIED-DATA-PLANE-R762.3).
    quality_max_incidents: u64 = "quality.max_incidents", U64(100_000), [1_000, 1_000_000], "incidents", Tunable;
    /// Context facts kept per node.
    context_facts_max_facts_per_node: u64 = "context_facts.max_facts_per_node", U64(16), [1, 256], "facts", Tunable;
    /// Bytes per token in the context facts estimate.
    context_facts_bytes_per_token: u64 = "context_facts.bytes_per_token", U64(4), [2, 8], "bytes", Tunable;
    /// Bands of the locality-sensitive duplicate index (MEDULLA-INGEST-WORKER-R702.1).
    ingest_lsh_bands: u64 = "ingest.lsh_bands", U64(20), [4, 64], "count", Tunable;
    /// Rows per band of the locality-sensitive duplicate index (MEDULLA-INGEST-WORKER-R702.1).
    ingest_lsh_rows: u64 = "ingest.lsh_rows", U64(5), [2, 16], "count", Tunable;
    /// Least Jaccard estimate at which an indexed document is a near duplicate (MEDULLA-INGEST-WORKER-R702.2).
    ingest_lsh_jaccard_threshold: f64 = "ingest.lsh_jaccard_threshold", F64(0.8), [0.5, 1.0], "ratio", Tunable;
    /// Most words an atom holds before its chunk splits at sentence ends (MEDULLA-INGEST-WORKER-R003.2).
    ingest_atom_max_tokens: u64 = "ingest.atom_max_tokens", U64(200), [20, 2_000], "words", Tunable;
    /// Words of atoms an atom context read returns (MEDULLA-INGEST-WORKER-R003.2).
    ingest_context_token_budget: u64 = "ingest.context_token_budget", U64(2_000), [100, 32_000], "words", Tunable;
    /// Ingest link minimum shared mentions.
    ingest_link_min_shared_mentions: u64 = "ingest.link_min_shared_mentions", U64(2), [1, 20], "count", Tunable;
    /// Largest source a document reader accepts, and the base of its memory bound (MEDULLA-INGEST-WORKER-R700.2).
    ingest_reader_max_bytes: u64 = "ingest.reader_max_bytes", U64(67_108_864), [1_048_576, 268_435_456], "bytes", Tunable;
    /// Threads a document reader uses for independent page and sheet units (MEDULLA-INGEST-WORKER-R700.2).
    ingest_reader_parallelism: u64 = "ingest.reader_parallelism", U64(4), [1, 32], "threads", Tunable;
    /// Media blob size cap.
    media_max_blob_bytes: u64 = "media.max_blob_bytes", U64(33_554_432), [1_048_576, 268_435_456], "bytes", Tunable;
    /// Synthesis context budget.
    synthesis_context_budget_tokens: u64 = "synthesis.context_budget_tokens", U64(8_192), [512, 1_048_576], "tokens", Tunable;
    /// Evolving mode requires approval.
    sdlc_mode_evolving_requires_approval: bool = "sdlc.mode.evolving_requires_approval", Bool(true), [true, true], "flag", Fixed;
    /// Byte bound of one task run context.
    sdlc_task_context_max_bytes: u64 = "sdlc.task_context_max_bytes", U64(65_536), [4_096, 4_194_304], "bytes", Tunable;
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
    /// Requests in flight against one registered foreign source at once, when its registration names no quota (MEDULLA-UNIFIED-DATA-PLANE-R031.2).
    foreign_default_quota_max_concurrent: u64 = "foreign.default_quota.max_concurrent", U64(8), [1, 256], "requests", Tunable;
    /// Rows one registered foreign source may return to one query, when its registration names no quota (MEDULLA-UNIFIED-DATA-PLANE-R031.2).
    foreign_default_quota_max_rows: u64 = "foreign.default_quota.max_rows", U64(250_000), [1, 10_000_000], "rows", Tunable;
    /// Wall-clock time one request to a registered foreign source may take, when its registration names no quota (MEDULLA-UNIFIED-DATA-PLANE-R031.2).
    foreign_default_quota_max_wall_ms: u64 = "foreign.default_quota.max_wall_ms", U64(60_000), [100, 600_000], "ms", Tunable;
    /// Deepest variable-length openCypher path a graph view compiles (MEDULLA-UNIFIED-DATA-PLANE-R038.6.2).
    federation_cypher_max_path_depth: u64 = "federation.cypher_max_path_depth", U64(8), [1, 32], "hops", Tunable;
    /// Longest a web metasearch result is served from the cache (MEDULLA-UNIFIED-DATA-PLANE-R038.7.2).
    federation_web_cache_ttl_s: u64 = "federation.web_cache_ttl_s", U64(900), [0, 86_400], "s", Tunable;
    /// Most plan observations one query shape keeps; the oldest is dropped first (MEDULLA-FEDERATED-QUERY-R091.1).
    federation_observations_per_shape: u64 = "federation.observations_per_shape", U64(4_096), [256, 65_536], "items", Tunable;
    /// Most query shapes one tenant keeps observations for; the least recently seen is evicted (MEDULLA-FEDERATED-QUERY-R091.1).
    federation_shapes_per_tenant: u64 = "federation.shapes_per_tenant", U64(1_024), [64, 16_384], "shapes", Tunable;
    /// Most frequent query shapes one plan-evolution run replays (MEDULLA-FEDERATED-QUERY-R091.2).
    federation_evolution_top_shapes: u64 = "federation.evolution_top_shapes", U64(32), [1, 256], "shapes", Tunable;
    /// Percent p50 latency or bytes fetched must improve for a candidate plan to be kept (MEDULLA-FEDERATED-QUERY-R091.2).
    federation_min_gain: u64 = "federation.min_gain", U64(10), [1, 90], "percent", Tunable;
    /// Fewest replays a candidate needs before it can be kept (MEDULLA-FEDERATED-QUERY-R091.2).
    federation_min_replays: u64 = "federation.min_replays", U64(20), [5, 1_000], "runs", Tunable;
    /// Percent source statistics may drift before a kept plan is re-validated (MEDULLA-FEDERATED-QUERY-R091.2).
    federation_drift_revalidate: u64 = "federation.drift_revalidate", U64(25), [5, 100], "percent", Tunable;
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
    /// How long past its watermark a copy stays valid (MEDULLA-UNIFIED-DATA-PLANE-R038.2.2).
    acceleration_watermark_ttl_ms: u64 = "acceleration.watermark_ttl_ms", U64(300_000), [1_000, 86_400_000], "ms", Tunable;
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
    /// Table names one metastore listing page asks for (MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.1).
    metastore_list_page_size: u64 = "metastore.list_page_size", U64(500), [1, 100_000], "tables", Tunable;
    /// Most partitions one metastore scan may read after pruning (MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.1).
    metastore_max_partitions: u64 = "metastore.max_partitions", U64(10_000), [1, 10_000_000], "partitions", Tunable;
    /// Longest wait for one metastore connect, send or reply (MEDULLA-UNIFIED-DATA-PLANE-R038.4.3).
    metastore_timeout_ms: u64 = "metastore.timeout_ms", U64(10_000), [100, 600_000], "ms", Tunable;
    /// Most connections one catalog read of an attached MySQL or MariaDB source opens (MEDULLA-UNIFIED-DATA-PLANE-R014.2.2).
    foreign_mysql_pool_size: u64 = "foreign.mysql.pool_size", U64(4), [1, 64], "connections", Tunable;
    /// Fewest text-search uses before column advice ranks anything (MEDULLA-FEDERATED-QUERY-R077.6.2).
    sql_advice_min_samples: u64 = "sql_advice.min_samples", U64(50), [1, 100_000], "observations", Tunable;
    /// Weight of a column's share of text-search uses in its advice score (MEDULLA-FEDERATED-QUERY-R077.6.2).
    sql_advice_w_demand: f64 = "sql_advice.w_demand", F64(0.6), [0.0, 1.0], "weight", Tunable;
    /// Weight of a column's average text length in its advice score (MEDULLA-FEDERATED-QUERY-R077.6.2).
    sql_advice_w_length: f64 = "sql_advice.w_length", F64(0.25), [0.0, 1.0], "weight", Tunable;
    /// Weight of a column's distinct-value ratio in its advice score (MEDULLA-FEDERATED-QUERY-R077.6.2).
    sql_advice_w_distinct: f64 = "sql_advice.w_distinct", F64(0.15), [0.0, 1.0], "weight", Tunable;
    /// Average text length at which the length term of an advice score saturates (MEDULLA-FEDERATED-QUERY-R077.6.2).
    sql_advice_length_norm: u64 = "sql_advice.length_norm", U64(512), [1, 65_536], "bytes", Tunable;
    /// Most rows one advice listing returns (MEDULLA-FEDERATED-QUERY-R077.6.2).
    sql_advice_max_rows: u64 = "sql_advice.max_rows", U64(1000), [1, 10_000], "rows", Tunable;
    /// Hours of text-search usage the column advice window covers (MEDULLA-FEDERATED-QUERY-R077.6.3).
    sql_advice_window_hours: u64 = "sql_advice.window_hours", U64(24), [1, 720], "hours", Tunable;
    /// Most `(table, column)` usage entries one tenant keeps (MEDULLA-FEDERATED-QUERY-R077.6.3).
    sql_advice_max_columns: u64 = "sql_advice.max_columns", U64(100_000), [1, 1_000_000], "columns", Tunable;
    /// Most rows one advice read samples per table for length and distinct ratio (MEDULLA-FEDERATED-QUERY-R077.6.3).
    sql_advice_stats_sample_rows: u64 = "sql_advice.stats_sample_rows", U64(10_000), [1, 1_000_000], "rows", Tunable;
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
    /// Applied changes between two shadow checks of one incremental view; there is no off value (MEDULLA-INCREMENTAL-VIEWS-R008.1).
    view_shadow_every: u64 = "view.shadow_every", U64(1_000), [10, 1_000_000], "changes", Tunable;
    /// Period of the task that runs due shadow checks (MEDULLA-INCREMENTAL-VIEWS-R008.1).
    view_shadow_tick_ms: u64 = "view.shadow_tick_ms", U64(1_000), [100, 60_000], "ms", Tunable;
    /// Ontology naming convention mode: 0 warns, 1 refuses (MEDULLA-TYPED-PACKS-R651.2).
    ontology_naming_mode: u64 = "ontology.naming_mode", U64(0), [0, 1], "mode", Tunable;
    /// Orchestration statement page size: the ceiling a request's `page_size` is held to (MEDULLA-ORCH-VOCAB-R010.101).
    orchestration_limits_page_size_max: u64 = "orchestration_limits.page_size_max", U64(100), [1, 1_000], "rows", Tunable;
    /// Most sessions (records plus unavailable entries) one `IndexHarnessSessionBatch` carries (MEDULLA-ORCH-VOCAB-R009.101.1).
    orchestration_limits_session_batch_max: u64 = "orchestration_limits.session_batch_max", U64(500), [1, 5_000], "sessions", Tunable;
    /// Fraction of the answer-name tokens a search-task clue may expose (MEDULLA-FEDERATED-QUERY-R702.2).
    search_task_expose_overlap: f64 = "search_task.expose_overlap", F64(0.5), [0.1, 1.0], "ratio", Tunable;
    /// Answer-node degree that trips the prior-knowledge shortcut detector (MEDULLA-FEDERATED-QUERY-R702.2).
    search_task_prior_degree: u64 = "search_task.prior_degree", U64(50), [5, 10_000], "edges", Tunable;
    /// Refinement rounds a search task may take before it must stand (MEDULLA-FEDERATED-QUERY-R702.2).
    search_task_refine_rounds: u32 = "search_task.refine_rounds", U64(3), [0, 8], "rounds", Tunable;
    /// Scheduler: Worker pool size the scheduler admits against. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_total_workers: u32 = "scheduler.total_workers", U64(4), [0, 1_024], "workers", Tunable;
    /// Scheduler: Workers reserved for the general lane; 0 reserves none. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_general_lane_floor: u32 = "scheduler.general_lane_floor", U64(0), [0, 1_024], "workers", Tunable;
    /// Scheduler: Workers reserved for the hydration lane; 0 reserves none. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_hydration_lane_floor: u32 = "scheduler.hydration_lane_floor", U64(0), [0, 1_024], "workers", Tunable;
    /// Scheduler: Fraction of a worker's cores the scheduler grants. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_cores_per_mille: u64 = "scheduler.cores_per_mille", U64(360), [0, 1_000], "per-mille", Tunable;
    /// Scheduler: Memory budget per worker. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_memory_per_worker_bytes: u64 = "scheduler.memory_per_worker_bytes", U64(3_221_225_472), [1, 1_099_511_627_776], "bytes", Tunable;
    /// Scheduler: Deepest task queue the scheduler admits. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_max_queue_depth: u64 = "scheduler.max_queue_depth", U64(1_024), [0, 100_000_000], "tasks", Tunable;
    /// Scheduler: Task lease held by a claim. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_task_lease_ms: u64 = "scheduler.task_lease_ms", U64(60_000), [5_000, 300_000], "ms", Tunable;
    /// Scheduler: Back-off hint carried by an admission refusal. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_retry_after_ms: u64 = "scheduler.retry_after_ms", U64(250), [50, 30_000], "ms", Tunable;
    /// Scheduler: A worker silent for longer than this expires. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_worker_ttl_ms: u64 = "scheduler.worker_ttl_ms", U64(30_000), [1_000, 600_000], "ms", Tunable;
    /// Scheduler: Slots the analytics pool may hold. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_analytics_slots: u32 = "scheduler.analytics_slots", U64(1), [0, 32], "slots", Tunable;
    /// Scheduler: Lease a colocated analytics worker holds on a claimed job. (MEDULLA-DURABLE-KERNEL-R082.9).
    scheduler_analytics_lease_ms: u64 = "scheduler.analytics_lease_ms", U64(60_000), [5_000, 300_000], "ms", Tunable;
    /// Scheduler: Poll interval of an idle colocated analytics worker. (MEDULLA-DURABLE-KERNEL-R082.9).
    scheduler_analytics_poll_ms: u64 = "scheduler.analytics_poll_ms", U64(250), [50, 30_000], "ms", Tunable;
    /// Scheduler: Largest baseline-hydration scope admitted. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_hydration_scope_cap: u32 = "scheduler.hydration_scope_cap", U64(10_000), [100, 1_000_000], "items", Tunable;
    /// Scheduler: Target file count of one repository split part. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_hydration_split_target_files: u32 = "scheduler.hydration_split_target_files", U64(2_000), [100, 100_000], "files", Tunable;
    /// Scheduler: Containers above which an architecture view is refused. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_architecture_view_max_containers: u32 = "scheduler.architecture_view_max_containers", U64(512), [16, 4_096], "containers", Tunable;
    /// Scheduler: Bound of one repository enrichment run. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_enrich_timeout_ms: u64 = "scheduler.enrich_timeout_ms", U64(5_000), [100, 600_000], "ms", Tunable;
    /// Downtime: Foreground request rate at or under which an idle window may open. (MEDULLA-DURABLE-KERNEL-R101.2).
    downtime_idle_max_rps: u32 = "downtime.idle_max_rps", U64(1), [0, 100], "requests/s", Tunable;
    /// Downtime: Seconds the foreground rate must stay quiet before an idle window opens. (MEDULLA-DURABLE-KERNEL-R101.2).
    downtime_idle_quiet_secs: u32 = "downtime.idle_quiet_secs", U64(60), [10, 3_600], "s", Tunable;
    /// Downtime: Share of one core, in permille, a downtime window may spend per second. (MEDULLA-DURABLE-KERNEL-R101.2).
    downtime_window_cpu_permille: u32 = "downtime.window_cpu_permille", U64(250), [10, 1_000], "permille", Tunable;
    /// Downtime: Memory a downtime window may hold. (MEDULLA-DURABLE-KERNEL-R101.2).
    downtime_window_memory_bytes: u64 = "downtime.window_memory_bytes", U64(2_147_483_648), [268_435_456, 68_719_476_736], "bytes", Tunable;
    /// Downtime: Seconds between downtime scheduler ticks. (MEDULLA-DURABLE-KERNEL-R101.3).
    downtime_tick_secs: u64 = "downtime.tick_secs", U64(1), [1, 60], "s", Tunable;
    /// Downtime: Milliseconds a running downtime job has to stop after foreground activity. (MEDULLA-DURABLE-KERNEL-R101.6).
    downtime_preempt_deadline_ms: u64 = "downtime.preempt_deadline_ms", U64(250), [50, 5_000], "ms", Tunable;
    /// Scheduler: Working-memory bound of one incremental index job. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_index_max_working_bytes: u64 = "scheduler.index_max_working_bytes", U64(1_073_741_824), [67_108_864, 68_719_476_736], "bytes", Tunable;
    /// Scheduler: Most commits one history run applies. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_history_max_commits: u64 = "scheduler.history_max_commits", U64(100_000), [1, 1_000_000], "commits", Tunable;
    /// Scheduler: Fewest co-changing commits for a coupling edge. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_history_support_floor: u64 = "scheduler.history_support_floor", U64(3), [1, 1_000], "commits", Tunable;
    /// Scheduler: Lowest coupling confidence kept. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_history_confidence_floor_milli: u64 = "scheduler.history_confidence_floor_milli", U64(300), [0, 1_000], "milli", Tunable;
    /// Scheduler: Most files of one commit that enter pair counting. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_history_max_files_per_commit: u64 = "scheduler.history_max_files_per_commit", U64(200), [2, 10_000], "files", Tunable;
    /// Scheduler: Most steps plus flows of one imported process. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_process_import_max_elements: u64 = "scheduler.process_import_max_elements", U64(100_000), [100, 1_000_000], "elements", Tunable;
    /// Scheduler: Most nodes of one process compiled from the graph. (MEDULLA-CQ-ORCH-R700.2).
    scheduler_process_compile_max_nodes: u64 = "scheduler.process_compile_max_nodes", U64(100_000), [100, 1_000_000], "nodes", Tunable;
    /// Scheduler: Largest document an `EnrichDocument` reads. (MEDULLA-REPO-INGEST-R652.4).
    scheduler_document_max_bytes: u64 = "scheduler.document_max_bytes", U64(67_108_864), [1_048_576, 1_073_741_824], "bytes", Tunable;
    /// Scheduler: Largest process document read. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_process_document_max_bytes: u64 = "scheduler.process_document_max_bytes", U64(16_777_216), [65_536, 268_435_456], "bytes", Tunable;
    /// Scheduler: Deepest element nesting of a process document. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_process_document_max_depth: u32 = "scheduler.process_document_max_depth", U64(64), [8, 256], "levels", Tunable;
    /// Scheduler: Largest submitted changeset blob. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_changeset_max_blob_bytes: u64 = "scheduler.changeset_max_blob_bytes", U64(134_217_728), [1_048_576, 4_294_967_296], "bytes", Tunable;
    /// Scheduler: Changesets between two stored tree snapshots (MEDULLA-SCM-R004).
    scheduler_tree_checkpoint_interval: u64 = "scheduler.tree_checkpoint_interval", U64(1000), [10, 100_000], "changesets", Tunable;
    /// Scheduler: Byte ceiling of one low-storage ANN artifact. (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_ann_storage_ceiling_bytes: u64 = "scheduler.ann_storage_ceiling_bytes", U64(2_147_483_648), [1_048_576, 1_099_511_627_776], "bytes", Tunable;
    /// Scheduler: hub-preserving pruning of compact ANN artifacts (MEDULLA-FEDERATED-QUERY-R040.5.5).
    scheduler_ann_hub_pruning: bool = "scheduler.ann_hub_pruning", Bool(false), [false, true], "switch", Tunable;
    /// Fraction of clues one clue's evidence may decide alone before a served search task is refused (MEDULLA-FEDERATED-QUERY-R702.3).
    search_task_single_clue_max: f64 = "search_task.single_clue_max", F64(0.5), [0.0, 1.0], "ratio", Tunable;
    /// Fraction of evidence nodes that may be adjacent to two other clues' nodes before a served search task is refused (MEDULLA-FEDERATED-QUERY-R702.3).
    search_task_co_coverage_max: f64 = "search_task.co_coverage_max", F64(0.5), [0.0, 1.0], "ratio", Tunable;
    /// Hops from the answer node the served evidence subgraph reaches (MEDULLA-FEDERATED-QUERY-R702.3).
    search_task_evidence_hops: u32 = "search_task.evidence_hops", U64(3), [1, 8], "hops", Tunable;
    /// Evidence nodes one served synthesis reads; a larger neighbourhood is refused (MEDULLA-FEDERATED-QUERY-R702.3).
    search_task_max_evidence_nodes: u64 = "search_task.max_evidence_nodes", U64(10_000), [10, 1_000_000], "nodes", Tunable;
    /// Archiver: upper bound on one encoded archive segment (MEDULLA-ENGINE-PARITY-R016.1).
    archive_segment_bytes: u64 = "archive.segment_bytes", U64(67_108_864), [1_048_576, 1_073_741_824], "bytes", Tunable;
    /// Archiver: how long archive segments are kept (MEDULLA-ENGINE-PARITY-R016.1).
    archive_retain_s: u64 = "archive.retain_s", U64(2_592_000), [3_600, 31_536_000], "s", Tunable;
    /// Archiver: seconds between archive passes (MEDULLA-ENGINE-PARITY-R016.1).
    archive_interval_s: u64 = "archive.interval_s", U64(300), [10, 86_400], "s", Tunable;
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
        if !ROOT_LIST_KEYS.contains(&name)
            && name != DEFAULT_INGEST_GRAPH_KEY
            && !KNOB_TABLE.iter().any(|spec| spec.name == name)
        {
            return Err(refusal(
                ServerErrorCode::KnobUnknown,
                name,
                "no knob table declares this name".to_string(),
            ));
        }
        cli.push((name.to_string(), value.to_string()));
    }
    let config = resolve_all(&cli, &env)?;
    // The scheduler policy is checked as a whole (lane floors against the pool
    // size), so a setting that is valid alone but not with the others refuses
    // here, before any state is built from it (MEDULLA-DURABLE-KERNEL-R076.2).
    scheduler_config_from(&config).map_err(|error| {
        refusal(
            ServerErrorCode::SchedulerConfigInvalid,
            "scheduler",
            error.to_string(),
        )
    })?;
    Ok(config)
}

/// The list-valued server config key naming the directories an attached SQLite file may
/// live under (MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.3). It is a path list, not a scalar, so
/// it is read here beside the scalar table; the default is the empty list, which refuses
/// every path.
pub const FOREIGN_SQLITE_ALLOWED_ROOTS: &str = "foreign.sqlite.allowed_roots";

/// The list-valued key naming the directories an attached DuckDB database file may live
/// under (MEDULLA-UNIFIED-DATA-PLANE-R020.2.2); the default is the empty list.
pub const FOREIGN_DUCKDB_ALLOWED_ROOTS: &str = "foreign.duckdb.allowed_roots";

/// The string-valued server config key naming the graph an ingest is written to when no
/// connector, repository or tenant routing record names one (MEDULLA-INGEST-WORKER-R605.2).
pub const DEFAULT_INGEST_GRAPH_KEY: &str = "ingest.default_graph";

/// The graph an ingest uses when nothing routes it elsewhere.
pub const DEFAULT_INGEST_GRAPH_FALLBACK: &str = "main";

static DEFAULT_INGEST_GRAPH: OnceLock<String> = OnceLock::new();

/// Read `ingest.default_graph`: the last `--knob`, else `MEDULLA_INGEST_DEFAULT_GRAPH`, else
/// `main`. A name outside `[A-Za-z0-9._:-]{1,128}` refuses startup with `KNOB_INVALID`.
pub fn load_default_ingest_graph(
    args: &[String],
    env: impl Fn(&str) -> Option<String>,
) -> Result<String, KnobRefusal> {
    let from_cli = args.iter().rev().find_map(|arg| {
        let (name, value) = arg.split_once('=')?;
        (name.trim() == DEFAULT_INGEST_GRAPH_KEY).then(|| value.trim().to_string())
    });
    let raw = from_cli
        .or_else(|| env("MEDULLA_INGEST_DEFAULT_GRAPH"))
        .unwrap_or_else(|| DEFAULT_INGEST_GRAPH_FALLBACK.to_string());
    let valid = !raw.is_empty()
        && raw.len() <= 128
        && raw
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'));
    if valid {
        Ok(raw)
    } else {
        Err(refusal(
            ServerErrorCode::KnobInvalid,
            DEFAULT_INGEST_GRAPH_KEY,
            "graph must match [A-Za-z0-9._:-]{1,128}".to_string(),
        ))
    }
}

/// Hand the startup-loaded default ingest graph to the handlers; the first install wins.
pub fn install_default_ingest_graph(graph: String) {
    let _ = DEFAULT_INGEST_GRAPH.set(graph);
}

/// The graph an ingest uses when nothing routes it elsewhere; `main` until installed.
pub fn default_ingest_graph() -> &'static str {
    DEFAULT_INGEST_GRAPH
        .get()
        .map_or(DEFAULT_INGEST_GRAPH_FALLBACK, String::as_str)
}

/// Every list-valued path key, read beside the scalar table.
pub const ROOT_LIST_KEYS: [&str; 2] = [FOREIGN_SQLITE_ALLOWED_ROOTS, FOREIGN_DUCKDB_ALLOWED_ROOTS];

/// The environment name a path-list key falls back to: `foreign.sqlite.allowed_roots`
/// reads `MEDULLA_FOREIGN_SQLITE_ALLOWED_ROOTS`.
fn root_list_env_name(key: &str) -> String {
    format!("MEDULLA_{}", key.to_uppercase().replace('.', "_"))
}

/// Read the path-list `key` (one of [`ROOT_LIST_KEYS`]): the last `--knob` for the key,
/// else its environment name, split on `:`. Every entry must be an absolute path; a
/// relative one refuses startup with `KNOB_INVALID`.
pub fn load_allowed_roots(
    key: &str,
    args: &[String],
    env: impl Fn(&str) -> Option<String>,
) -> Result<Vec<std::path::PathBuf>, KnobRefusal> {
    let from_cli = args.iter().rev().find_map(|arg| {
        let (name, value) = arg.split_once('=')?;
        (name.trim() == key).then(|| value.to_string())
    });
    let raw = from_cli.or_else(|| env(&root_list_env_name(key)));
    let mut roots = Vec::new();
    for entry in raw.iter().flat_map(|raw| raw.split(':')) {
        if entry.trim().is_empty() {
            continue;
        }
        let path = std::path::PathBuf::from(entry.trim());
        if !path.is_absolute() {
            return Err(refusal(
                ServerErrorCode::KnobInvalid,
                key,
                format!("root {entry:?} is not an absolute path"),
            ));
        }
        roots.push(path);
    }
    Ok(roots)
}

static SQLITE_ROOTS: OnceLock<Vec<std::path::PathBuf>> = OnceLock::new();
static DUCKDB_ROOTS: OnceLock<Vec<std::path::PathBuf>> = OnceLock::new();

fn roots_slot(key: &str) -> Option<&'static OnceLock<Vec<std::path::PathBuf>>> {
    match key {
        FOREIGN_SQLITE_ALLOWED_ROOTS => Some(&SQLITE_ROOTS),
        FOREIGN_DUCKDB_ALLOWED_ROOTS => Some(&DUCKDB_ROOTS),
        _ => None,
    }
}

/// Hand the startup-loaded roots of `key` to the registration gate; the first install wins.
pub fn install_allowed_roots(key: &str, roots: Vec<std::path::PathBuf>) {
    if let Some(slot) = roots_slot(key) {
        let _ = slot.set(roots);
    }
}

/// The roots of `key` an attached file may live under; empty until installed (and for an
/// unknown key), so every path is refused.
pub fn allowed_roots(key: &str) -> &'static [std::path::PathBuf] {
    roots_slot(key)
        .and_then(OnceLock::get)
        .map_or(&[], Vec::as_slice)
}

static INSTALLED: OnceLock<KnobConfig> = OnceLock::new();

/// Hand the startup-loaded knobs to the listeners and served handlers; the
/// first install wins.
pub fn install(config: KnobConfig) {
    // The source access gate lives below this crate; it reads the resolved staleness bound.
    medulla_graph_core::rls::install_source_acl_stale_after_s(config.ingest_acl_stale_after_s);
    // The observability registries sit below this crate and cannot read the knobs.
    medulla_observe::runtime_sampler().set_spin_busy_ratio(config.observe_worker_spin_busy_ratio);
    medulla_observe::locks().set_hold_warn_ms(config.observe_lock_hold_warn_ms);
    medulla_observe::settings::set_task_stall_after_ms(config.observe_task_stall_after_ms);
    medulla_observe::settings::set_stall_check_interval_ms(config.observe_stall_check_interval_ms);
    medulla_observe::settings::set_health_deadline_ms(config.observe_health_deadline_ms);
    medulla_observe::settings::set_startup_reports_kept(config.observe_startup_reports_kept);
    medulla_observe::settings::set_profile_hz(config.observe_profile_hz);
    medulla_observe::events().set_capacity(config.observe_event_ring_capacity as usize);
    medulla_observe::global().set_capacity(config.observe_task_registry_capacity as usize);
    medulla_observe::progress::set_progress_idle_limit(
        u32::try_from(config.observe_progress_idle_limit).unwrap_or(u32::MAX),
    );
    if INSTALLED.set(config).is_ok() {
        // The graph cores read the retention bound at write time and cannot see this
        // crate, so the resolved value is handed to them once, here.
        medulla_graph_core::graph::install_node_history_limit(
            INSTALLED.get().map_or(0, |installed| {
                installed.graph_node_history_revisions as usize
            }),
        );
    }
}

/// The knobs loaded at startup, or the defaults where no loader ran.
pub fn knobs() -> &'static KnobConfig {
    INSTALLED.get_or_init(KnobConfig::default)
}

/// The memory-sync limits the resolved knobs carry (MEDULLA-DURABLE-KERNEL-R085.7).
/// The receipt lifetime is not a knob; it keeps the contract default.
pub fn memory_sync_config() -> medulla_mutation_types::memory_sync::MemorySyncConfig {
    let knobs = knobs();
    medulla_mutation_types::memory_sync::MemorySyncConfig {
        cursor_ttl_s: knobs.memory_sync_cursor_ttl_s,
        history_retention_s: knobs.memory_sync_history_retention_s,
        max_revisions_per_doc: u32::try_from(knobs.memory_sync_max_revisions_per_doc)
            .unwrap_or(u32::MAX),
        capacity_bytes: knobs.memory_sync_capacity_bytes,
        ..medulla_mutation_types::memory_sync::MemorySyncConfig::default()
    }
}

/// The scheduler policy built from the resolved knob values (MEDULLA-DURABLE-KERNEL-R076.2).
/// Built once at startup and held in `ServerState`; there is no second settings store.
pub fn scheduler_config_from(
    knobs: &KnobConfig,
) -> Result<
    medulla_work_types::scheduler_config::SchedulerConfig,
    medulla_work_types::scheduler_config::InvalidSchedulerConfig,
> {
    use medulla_work_types::scheduler_config::{AnnHubPruning, SchedulerConfig};
    let mut lane_floors = std::collections::BTreeMap::new();
    if knobs.scheduler_general_lane_floor > 0 {
        lane_floors.insert("general".to_string(), knobs.scheduler_general_lane_floor);
    }
    if knobs.scheduler_hydration_lane_floor > 0 {
        lane_floors.insert(
            "hydration".to_string(),
            knobs.scheduler_hydration_lane_floor,
        );
    }
    let config = SchedulerConfig {
        total_workers: knobs.scheduler_total_workers,
        lane_floors,
        task_lease_ms: knobs.scheduler_task_lease_ms,
        retry_after_ms: knobs.scheduler_retry_after_ms,
        worker_ttl_ms: knobs.scheduler_worker_ttl_ms,
        analytics_slots: knobs.scheduler_analytics_slots,
        // One knob feeds both fields: `scheduler.analytics_slots` is the only source.
        analytics_workers: knobs.scheduler_analytics_slots,
        analytics_lease_ms: knobs.scheduler_analytics_lease_ms,
        analytics_poll_ms: knobs.scheduler_analytics_poll_ms,
        cores_per_mille: u16::try_from(knobs.scheduler_cores_per_mille).unwrap_or(u16::MAX),
        memory_per_worker_bytes: knobs.scheduler_memory_per_worker_bytes,
        max_queue_depth: knobs.scheduler_max_queue_depth,
        hydration_scope_cap: knobs.scheduler_hydration_scope_cap,
        hydration_split_target_files: knobs.scheduler_hydration_split_target_files,
        architecture_view_max_containers: knobs.scheduler_architecture_view_max_containers,
        enrich_timeout_ms: knobs.scheduler_enrich_timeout_ms,
        index_max_working_bytes: knobs.scheduler_index_max_working_bytes,
        history_max_commits: knobs.scheduler_history_max_commits,
        history_support_floor: knobs.scheduler_history_support_floor,
        history_confidence_floor_milli: knobs.scheduler_history_confidence_floor_milli,
        history_max_files_per_commit: knobs.scheduler_history_max_files_per_commit,
        process_import_max_elements: knobs.scheduler_process_import_max_elements,
        process_compile_max_nodes: knobs.scheduler_process_compile_max_nodes,
        document_max_bytes: knobs.scheduler_document_max_bytes,
        process_document_max_bytes: knobs.scheduler_process_document_max_bytes,
        process_document_max_depth: knobs.scheduler_process_document_max_depth,
        changeset_max_blob_bytes: knobs.scheduler_changeset_max_blob_bytes,
        tree_checkpoint_interval: knobs.scheduler_tree_checkpoint_interval,
        ann_storage_ceiling_bytes: knobs.scheduler_ann_storage_ceiling_bytes,
        ann_hub_pruning: if knobs.scheduler_ann_hub_pruning {
            AnnHubPruning::Enabled
        } else {
            AnnHubPruning::Disabled
        },
        orchestration_limits: Default::default(),
        ingest_retry: Default::default(),
        admit_retry_ms: medulla_work_types::scheduler_config::DEFAULT_ADMIT_RETRY_MS,
    };
    config.validate()?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-OBSERVE-R011
    #[test]
    fn observe_knobs_have_defaults_bounds_and_units() {
        // name, default, minimum, maximum, unit, class: the rows of data-model.md.
        let expect: [(&str, KnobValue, KnobValue, KnobValue, &str, KnobClass); 11] = [
            ("observe.task_stall_after_ms", KnobValue::U64(60_000), KnobValue::U64(100), KnobValue::U64(3_600_000), "ms", KnobClass::Tunable),
            ("observe.stall_check_interval_ms", KnobValue::U64(5_000), KnobValue::U64(50), KnobValue::U64(60_000), "ms", KnobClass::Tunable),
            ("observe.health_deadline_ms", KnobValue::U64(2_000), KnobValue::U64(100), KnobValue::U64(30_000), "ms", KnobClass::Tunable),
            ("observe.event_ring_capacity", KnobValue::U64(4_096), KnobValue::U64(256), KnobValue::U64(65_536), "events", KnobClass::Tunable),
            ("observe.task_registry_capacity", KnobValue::U64(4_096), KnobValue::U64(64), KnobValue::U64(65_536), "tasks", KnobClass::Tunable),
            ("observe.progress_idle_limit", KnobValue::U64(2), KnobValue::U64(1), KnobValue::U64(100), "iterations", KnobClass::Tunable),
            ("observe.worker_spin_busy_ratio", KnobValue::F64(0.9), KnobValue::F64(0.5), KnobValue::F64(1.0), "ratio", KnobClass::Tunable),
            ("observe.lock_hold_warn_ms", KnobValue::U64(5_000), KnobValue::U64(100), KnobValue::U64(600_000), "ms", KnobClass::Tunable),
            ("observe.startup_reports_kept", KnobValue::U64(8), KnobValue::U64(1), KnobValue::U64(64), "reports", KnobClass::Tunable),
            ("observe.profile_hz", KnobValue::U64(99), KnobValue::U64(10), KnobValue::U64(1_000), "Hz", KnobClass::Tunable),
            ("observe.profile_max_seconds", KnobValue::U64(60), KnobValue::U64(60), KnobValue::U64(60), "s", KnobClass::Fixed),
        ];
        for (name, default, min, max, unit, class) in expect {
            let spec = KNOB_TABLE
                .iter()
                .find(|spec| spec.name == name)
                .unwrap_or_else(|| panic!("{name} is not a row of the knob table"));
            assert_eq!(spec.default, default, "{name} default");
            assert_eq!(spec.min, min, "{name} minimum");
            assert_eq!(spec.max, max, "{name} maximum");
            assert_eq!(spec.unit, unit, "{name} unit");
            assert_eq!(spec.class, class, "{name} class");
        }
        // Every row of the observe group is one the data model lists.
        let observe_rows = KNOB_TABLE
            .iter()
            .filter(|spec| spec.name.starts_with("observe."))
            .count();
        assert_eq!(observe_rows, expect.len());
        // The defaults of the registries match the knob defaults, so a registry that never
        // saw the server's knobs runs as documented.
        assert_eq!(
            medulla_observe::settings::DEFAULT_TASK_STALL_AFTER_MS,
            60_000
        );
        assert_eq!(medulla_observe::settings::DEFAULT_HEALTH_DEADLINE_MS, 2_000);
        assert_eq!(
            medulla_observe::events::DEFAULT_EVENT_RING_CAPACITY,
            4_096
        );
        // A bound is enforced: a value below the minimum is refused with the knob's name.
        let refused = resolve_all(
            &[("observe.event_ring_capacity".to_string(), "255".to_string())],
            &|_| None,
        )
        .expect_err("a value under the minimum is refused");
        assert_eq!(refused.knob, "observe.event_ring_capacity");
    }

    // spec: MEDULLA-DURABLE-KERNEL-R085.7
    #[test]
    fn memory_sync_retention_knobs_register() {
        let expect = [
            (
                "memory_sync.history_retention_s",
                2_592_000,
                3_600,
                31_536_000,
                "seconds",
            ),
            (
                "memory_sync.max_revisions_per_doc",
                64,
                1,
                4_096,
                "revisions",
            ),
            (
                "memory_sync.capacity_bytes",
                1_073_741_824,
                1_048_576,
                1_099_511_627_776,
                "bytes",
            ),
        ];
        for (name, default, min, max, unit) in expect {
            let spec = KNOB_TABLE
                .iter()
                .find(|spec| spec.name == name)
                .unwrap_or_else(|| panic!("{name} is registered"));
            assert_eq!(spec.default, KnobValue::U64(default));
            assert_eq!(spec.min, KnobValue::U64(min));
            assert_eq!(spec.max, KnobValue::U64(max));
            assert_eq!(spec.unit, unit);
            assert_eq!(spec.class, KnobClass::Tunable);
        }
        let none = |_: &str| None;
        let loaded = load_knobs(&[], none).unwrap();
        assert_eq!(loaded.memory_sync_max_revisions_per_doc, 64);
        let args = vec!["memory_sync.max_revisions_per_doc=4097".to_string()];
        assert!(
            load_knobs(&args, none).is_err(),
            "above the bound refuses at startup"
        );
        let config = memory_sync_config();
        assert_eq!(config.validate(), Ok(()));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.2.2.3
    #[test]
    fn acceleration_watermark_ttl_knob_default_and_range() {
        let spec = KNOB_TABLE
            .iter()
            .find(|spec| spec.name == "acceleration.watermark_ttl_ms")
            .expect("the watermark knob is registered");
        assert_eq!(spec.default, KnobValue::U64(300_000));
        assert_eq!(spec.min, KnobValue::U64(1_000));
        assert_eq!(spec.max, KnobValue::U64(86_400_000));
        assert_eq!(spec.unit, "ms");
        assert_eq!(spec.class, KnobClass::Tunable);
        let none = |_: &str| None;
        assert_eq!(
            load_knobs(&[], none).unwrap().acceleration_watermark_ttl_ms,
            300_000
        );
        let below = vec!["acceleration.watermark_ttl_ms=999".to_string()];
        assert!(
            load_knobs(&below, none).is_err(),
            "below the range refuses at startup"
        );
    }

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
}

#[cfg(test)]
mod scheduler_knob_tests {
    use super::*;

    #[test]
    fn scheduler_policy_is_built_from_resolved_knobs() {
        let args = vec![
            ("scheduler.task_lease_ms".to_string(), "90000".to_string()),
            ("scheduler.worker_ttl_ms".to_string(), "45000".to_string()),
            ("scheduler.analytics_slots".to_string(), "3".to_string()),
            ("scheduler.general_lane_floor".to_string(), "2".to_string()),
        ];
        let resolved = resolve_all(&args, &|_| None).expect("knobs resolve");
        let config = scheduler_config_from(&resolved).expect("policy validates");
        assert_eq!(config.task_lease_ms, 90_000);
        assert_eq!(config.worker_ttl_ms, 45_000);
        assert_eq!(config.analytics_slots, 3);
        assert_eq!(config.lane_floors.get("general"), Some(&2));
        assert!(!config.lane_floors.contains_key("hydration"));
        assert_eq!(config.retry_after_ms, 250);
    }

    // spec: MEDULLA-DURABLE-KERNEL-R082.9
    #[test]
    fn analytics_lease_and_poll_knobs_reach_the_scheduler_policy() {
        let env = |name: &str| match name {
            "MEDULLA_SCHEDULER_ANALYTICS_LEASE_MS" => Some("9000".to_string()),
            "MEDULLA_SCHEDULER_ANALYTICS_POLL_MS" => Some("75".to_string()),
            _ => None,
        };
        let resolved = resolve_all(&[], &env).expect("knobs resolve");
        let config = scheduler_config_from(&resolved).expect("policy validates");
        assert_eq!(config.analytics_lease_ms, 9_000);
        assert_eq!(config.analytics_poll_ms, 75);

        let defaults = scheduler_config_from(&KnobConfig::default()).expect("defaults validate");
        assert_eq!(defaults.analytics_lease_ms, 60_000);
        assert_eq!(defaults.analytics_poll_ms, 250);

        let refused = resolve_all(&[], &|name: &str| {
            (name == "MEDULLA_SCHEDULER_ANALYTICS_LEASE_MS").then(|| "300001".to_string())
        })
        .expect_err("out of range refuses");
        assert_eq!(refused.code, ServerErrorCode::KnobOutOfRange);
        assert_eq!(refused.knob, "scheduler.analytics_lease_ms");
    }

    #[test]
    fn scheduler_knobs_refuse_out_of_range_values() {
        for (name, bad) in [
            ("scheduler.task_lease_ms", "4999"),
            ("scheduler.retry_after_ms", "49"),
            ("scheduler.worker_ttl_ms", "999"),
            ("scheduler.analytics_slots", "33"),
        ] {
            let args = vec![(name.to_string(), bad.to_string())];
            let refusal = resolve_all(&args, &|_| None).expect_err("out of range refuses");
            assert_eq!(refusal.knob, name);
        }
    }

    #[test]
    fn default_scheduler_policy_is_valid() {
        scheduler_config_from(&KnobConfig::default()).expect("defaults validate");
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

#[cfg(test)]
mod root_list_tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R020.2.2
    #[test]
    fn each_root_list_key_reads_its_own_cli_value_and_environment_name() {
        let args = vec!["foreign.duckdb.allowed_roots=/data/a:/data/b".to_string()];
        let duck = load_allowed_roots(FOREIGN_DUCKDB_ALLOWED_ROOTS, &args, |_| None).unwrap();
        assert_eq!(duck.len(), 2);
        // The other key does not see this key's value.
        let sqlite = load_allowed_roots(FOREIGN_SQLITE_ALLOWED_ROOTS, &args, |_| None).unwrap();
        assert!(sqlite.is_empty());
        let from_env = load_allowed_roots(FOREIGN_DUCKDB_ALLOWED_ROOTS, &[], |name| {
            (name == "MEDULLA_FOREIGN_DUCKDB_ALLOWED_ROOTS").then(|| "/data/c".to_string())
        })
        .unwrap();
        assert_eq!(from_env, vec![std::path::PathBuf::from("/data/c")]);
        let relative = vec!["foreign.duckdb.allowed_roots=data/a".to_string()];
        assert!(load_allowed_roots(FOREIGN_DUCKDB_ALLOWED_ROOTS, &relative, |_| None).is_err());
    }

    #[test]
    fn an_unknown_root_list_key_has_no_roots() {
        install_allowed_roots("foreign.other.allowed_roots", vec!["/x".into()]);
        assert!(allowed_roots("foreign.other.allowed_roots").is_empty());
    }
}
