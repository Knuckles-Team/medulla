//! Authoritative scheduler policy data (MEDULLA-DURABLE-KERNEL-R076.1).
//!
//! The served setting, admission wiring and persistence are R076.2. Consumers
//! must validate this model before use; workers never choose their own policy.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Default fraction of host cores, in thousandths.
pub const DEFAULT_CORES_PER_MILLE: u16 = 360;
/// Default memory budget per worker: exactly three binary gibibytes.
pub const DEFAULT_MEMORY_PER_WORKER_BYTES: u64 = 3 * 1024 * 1024 * 1024;

/// Default hydration scope cap in work items (MEDULLA-REPO-INGEST-R650.2).
pub const DEFAULT_HYDRATION_SCOPE_CAP: u32 = 10_000;
/// Inclusive bounds of `hydration_scope_cap`.
pub const HYDRATION_SCOPE_CAP_BOUNDS: (u32, u32) = (100, 1_000_000);
/// Default target file count of one repository split part (MEDULLA-REPO-INGEST-R650.2).
pub const DEFAULT_HYDRATION_SPLIT_TARGET_FILES: u32 = 2_000;
/// Inclusive bounds of `hydration_split_target_files`.
pub const HYDRATION_SPLIT_TARGET_FILES_BOUNDS: (u32, u32) = (100, 100_000);
/// Environment key overriding `hydration_scope_cap` at startup.
pub const ENV_HYDRATION_SCOPE_CAP: &str = "MEDULLA_HYDRATION_SCOPE_CAP";
/// Environment key overriding `hydration_split_target_files` at startup.
pub const ENV_HYDRATION_SPLIT_TARGET_FILES: &str = "MEDULLA_HYDRATION_SPLIT_TARGET_FILES";

fn default_hydration_scope_cap() -> u32 {
    DEFAULT_HYDRATION_SCOPE_CAP
}

fn default_hydration_split_target_files() -> u32 {
    DEFAULT_HYDRATION_SPLIT_TARGET_FILES
}

fn default_cores_per_mille() -> u16 {
    DEFAULT_CORES_PER_MILLE
}

fn default_memory_per_worker_bytes() -> u64 {
    DEFAULT_MEMORY_PER_WORKER_BYTES
}

/// Default bound of one repository enrichment run in milliseconds
/// (MEDULLA-REPO-INGEST-R600.3).
pub const DEFAULT_ENRICH_TIMEOUT_MS: u64 = 5_000;
/// Inclusive bounds of `enrich_timeout_ms`.
pub const ENRICH_TIMEOUT_MS_BOUNDS: (u64, u64) = (100, 600_000);
/// Environment key overriding `enrich_timeout_ms` at startup.
pub const ENV_ENRICH_TIMEOUT_MS: &str = "MEDULLA_ENRICH_TIMEOUT_MS";

fn default_enrich_timeout_ms() -> u64 {
    DEFAULT_ENRICH_TIMEOUT_MS
}

/// The startup value of `enrich_timeout_ms`: the default, or the
/// `MEDULLA_ENRICH_TIMEOUT_MS` override; out of range refuses startup.
pub fn enrich_timeout_ms_from(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<u64, InvalidSchedulerConfig> {
    let value = match lookup(ENV_ENRICH_TIMEOUT_MS) {
        Some(raw) => raw
            .trim()
            .parse()
            .map_err(|_| InvalidSchedulerConfig::InvalidEnvValue(ENV_ENRICH_TIMEOUT_MS))?,
        None => DEFAULT_ENRICH_TIMEOUT_MS,
    };
    if !(ENRICH_TIMEOUT_MS_BOUNDS.0..=ENRICH_TIMEOUT_MS_BOUNDS.1).contains(&value) {
        return Err(InvalidSchedulerConfig::EnrichTimeoutOutOfRange);
    }
    Ok(value)
}

/// Commit-history enrichment settings (MEDULLA-REPO-INGEST-R601.3), as
/// `(key, environment key, default, inclusive bounds)`.
pub const HISTORY_KNOBS: [(&str, &str, u64, (u64, u64)); 4] = [
    (
        "history_max_commits",
        "MEDULLA_HISTORY_MAX_COMMITS",
        100_000,
        (1, 1_000_000),
    ),
    (
        "history_support_floor",
        "MEDULLA_HISTORY_SUPPORT_FLOOR",
        3,
        (1, 1_000),
    ),
    (
        "history_confidence_floor_milli",
        "MEDULLA_HISTORY_CONFIDENCE_FLOOR_MILLI",
        300,
        (0, 1_000),
    ),
    (
        "history_max_files_per_commit",
        "MEDULLA_HISTORY_MAX_FILES_PER_COMMIT",
        200,
        (2, 10_000),
    ),
];

fn default_history_max_commits() -> u64 {
    HISTORY_KNOBS[0].2
}

fn default_history_support_floor() -> u64 {
    HISTORY_KNOBS[1].2
}

fn default_history_confidence_floor_milli() -> u64 {
    HISTORY_KNOBS[2].2
}

fn default_history_max_files_per_commit() -> u64 {
    HISTORY_KNOBS[3].2
}

/// The startup values of the four history keys, in [`HISTORY_KNOBS`] order:
/// the defaults, or the `MEDULLA_HISTORY_*` overrides; out of range refuses
/// startup.
pub fn history_knobs_from(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<[u64; 4], InvalidSchedulerConfig> {
    let mut values = [0; 4];
    for (slot, (_, env, default, bounds)) in values.iter_mut().zip(HISTORY_KNOBS) {
        *slot = match lookup(env) {
            Some(raw) => raw
                .trim()
                .parse()
                .map_err(|_| InvalidSchedulerConfig::InvalidEnvValue(env))?,
            None => default,
        };
        if !(bounds.0..=bounds.1).contains(slot) {
            return Err(InvalidSchedulerConfig::HistoryKnobOutOfRange(env));
        }
    }
    Ok(values)
}

/// Default bound on the containers of one architecture view (MEDULLA-CQ-ORCH-R010.2).
pub const DEFAULT_ARCHITECTURE_VIEW_MAX_CONTAINERS: u32 = 512;
/// Accepted range of `architecture_view_max_containers`.
pub const ARCHITECTURE_VIEW_MAX_CONTAINERS_RANGE: std::ops::RangeInclusive<u32> = 16..=4096;

fn default_architecture_view_max_containers() -> u32 {
    DEFAULT_ARCHITECTURE_VIEW_MAX_CONTAINERS
}

/// Default bound on the steps plus flows of one imported process (MEDULLA-CQ-ORCH-R701.2).
pub const DEFAULT_PROCESS_IMPORT_MAX_ELEMENTS: u64 = 100_000;
/// Accepted range of `process_import_max_elements`.
pub const PROCESS_IMPORT_MAX_ELEMENTS_RANGE: std::ops::RangeInclusive<u64> = 100..=1_000_000;

fn default_process_import_max_elements() -> u64 {
    DEFAULT_PROCESS_IMPORT_MAX_ELEMENTS
}

/// Default bound on one process document, in bytes (MEDULLA-CQ-ORCH-R702.2).
pub const DEFAULT_PROCESS_DOCUMENT_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Accepted range of `process_document_max_bytes`.
pub const PROCESS_DOCUMENT_MAX_BYTES_RANGE: std::ops::RangeInclusive<u64> =
    64 * 1024..=256 * 1024 * 1024;
/// Default bound on the element depth of one process document.
pub const DEFAULT_PROCESS_DOCUMENT_MAX_DEPTH: u32 = 64;
/// Accepted range of `process_document_max_depth`.
pub const PROCESS_DOCUMENT_MAX_DEPTH_RANGE: std::ops::RangeInclusive<u32> = 8..=256;

fn default_process_document_max_bytes() -> u64 {
    DEFAULT_PROCESS_DOCUMENT_MAX_BYTES
}

fn default_process_document_max_depth() -> u32 {
    DEFAULT_PROCESS_DOCUMENT_MAX_DEPTH
}

/// Default task lease held by a claim, in milliseconds
/// (MEDULLA-DURABLE-KERNEL-R076.2).
pub const DEFAULT_TASK_LEASE_MS: u64 = 60_000;
/// Inclusive bounds of `task_lease_ms`.
pub const TASK_LEASE_MS_BOUNDS: (u64, u64) = (5_000, 300_000);
/// Default admission back-off, in milliseconds (MEDULLA-DURABLE-KERNEL-R076.2).
pub const DEFAULT_RETRY_AFTER_MS: u64 = 250;
/// Inclusive bounds of `retry_after_ms`.
pub const RETRY_AFTER_MS_BOUNDS: (u64, u64) = (50, 30_000);

fn default_task_lease_ms() -> u64 {
    DEFAULT_TASK_LEASE_MS
}

fn default_retry_after_ms() -> u64 {
    DEFAULT_RETRY_AFTER_MS
}

/// Engine-owned policy. Pool size and queue bound must be configured explicitly.
/// Zero workers, zero CPU share, or zero queue depth can disable that resource;
/// no implicit minimum may override those limits. Lane IDs are engine-defined
/// worker lanes, not the decision engine's fast/medium/slow ingestion lanes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchedulerConfig {
    pub total_workers: u32,
    #[serde(default)]
    pub lane_floors: BTreeMap<String, u32>,
    /// Task lease held by a claim, in milliseconds; the admission reply carries
    /// it as `lease_seconds` (MEDULLA-DURABLE-KERNEL-R075, R076.2).
    #[serde(default = "default_task_lease_ms")]
    pub task_lease_ms: u64,
    /// Back-off hint for an admission refusal, in milliseconds; the admission
    /// reply carries it as `retry_after_seconds` (MEDULLA-DURABLE-KERNEL-R075,
    /// R076.2).
    #[serde(default = "default_retry_after_ms")]
    pub retry_after_ms: u64,
    #[serde(default = "default_cores_per_mille")]
    pub cores_per_mille: u16,
    #[serde(default = "default_memory_per_worker_bytes")]
    pub memory_per_worker_bytes: u64,
    pub max_queue_depth: u64,
    /// Largest baseline-hydration scope admitted, in work items.
    #[serde(default = "default_hydration_scope_cap")]
    pub hydration_scope_cap: u32,
    /// Target file count of one repository split part.
    #[serde(default = "default_hydration_split_target_files")]
    pub hydration_split_target_files: u32,
    /// Above this many containers an architecture view is refused as too large.
    #[serde(default = "default_architecture_view_max_containers")]
    pub architecture_view_max_containers: u32,
    /// Bound of one repository enrichment run, in milliseconds.
    #[serde(default = "default_enrich_timeout_ms")]
    pub enrich_timeout_ms: u64,
    /// Most commits one history run applies.
    #[serde(default = "default_history_max_commits")]
    pub history_max_commits: u64,
    /// Fewest co-changing commits for a coupling edge.
    #[serde(default = "default_history_support_floor")]
    pub history_support_floor: u64,
    /// Lowest coupling confidence kept, in thousandths.
    #[serde(default = "default_history_confidence_floor_milli")]
    pub history_confidence_floor_milli: u64,
    /// Most files of one commit that enter pair counting.
    #[serde(default = "default_history_max_files_per_commit")]
    pub history_max_files_per_commit: u64,
    /// Most steps plus flows of one imported process.
    #[serde(default = "default_process_import_max_elements")]
    pub process_import_max_elements: u64,
    /// Largest process document read, in bytes.
    #[serde(default = "default_process_document_max_bytes")]
    pub process_document_max_bytes: u64,
    /// Deepest element nesting of a process document.
    #[serde(default = "default_process_document_max_depth")]
    pub process_document_max_depth: u32,
}

/// Invalid scheduler policy, refused before it can govern admission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidSchedulerConfig {
    BlankLane,
    CoresPerMilleOutOfRange,
    ZeroMemoryPerWorker,
    ArchitectureViewMaxContainersOutOfRange,
    ProcessImportMaxElementsOutOfRange,
    ProcessDocumentBoundOutOfRange(&'static str),
    OversubscribedFloors { total_workers: u32 },
    HydrationScopeCapOutOfRange,
    HydrationSplitTargetOutOfRange,
    EnrichTimeoutOutOfRange,
    HistoryKnobOutOfRange(&'static str),
    InvalidEnvValue(&'static str),
}

impl std::fmt::Display for InvalidSchedulerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlankLane => write!(f, "scheduler lane must not be blank"),
            Self::CoresPerMilleOutOfRange => {
                write!(f, "scheduler cores_per_mille must be at most 1000")
            }
            Self::ZeroMemoryPerWorker => {
                write!(f, "scheduler memory_per_worker_bytes must be nonzero")
            }
            Self::ArchitectureViewMaxContainersOutOfRange => write!(
                f,
                "scheduler architecture_view_max_containers must be in [16, 4096]"
            ),
            Self::ProcessImportMaxElementsOutOfRange => write!(
                f,
                "scheduler process_import_max_elements must be in [100, 1000000]"
            ),
            Self::ProcessDocumentBoundOutOfRange(key) => {
                write!(f, "scheduler {key} is outside its accepted range")
            }
            Self::OversubscribedFloors { total_workers } => {
                write!(
                    f,
                    "scheduler lane floors exceed total workers {total_workers}"
                )
            }
            Self::HydrationScopeCapOutOfRange => write!(
                f,
                "scheduler hydration_scope_cap must be within [{}, {}]",
                HYDRATION_SCOPE_CAP_BOUNDS.0, HYDRATION_SCOPE_CAP_BOUNDS.1
            ),
            Self::HydrationSplitTargetOutOfRange => write!(
                f,
                "scheduler hydration_split_target_files must be within [{}, {}]",
                HYDRATION_SPLIT_TARGET_FILES_BOUNDS.0, HYDRATION_SPLIT_TARGET_FILES_BOUNDS.1
            ),
            Self::EnrichTimeoutOutOfRange => write!(
                f,
                "scheduler enrich_timeout_ms must be within [{}, {}]",
                ENRICH_TIMEOUT_MS_BOUNDS.0, ENRICH_TIMEOUT_MS_BOUNDS.1
            ),
            Self::HistoryKnobOutOfRange(key) => {
                write!(f, "{key} is outside its accepted range")
            }
            Self::InvalidEnvValue(key) => write!(f, "{key} must be an unsigned integer"),
        }
    }
}

impl std::error::Error for InvalidSchedulerConfig {}

impl SchedulerConfig {
    /// Validate all policy bounds without allocating workers or changing state.
    pub fn validate(&self) -> Result<(), InvalidSchedulerConfig> {
        if self.cores_per_mille > 1000 {
            return Err(InvalidSchedulerConfig::CoresPerMilleOutOfRange);
        }
        if self.memory_per_worker_bytes == 0 {
            return Err(InvalidSchedulerConfig::ZeroMemoryPerWorker);
        }
        if !(HYDRATION_SCOPE_CAP_BOUNDS.0..=HYDRATION_SCOPE_CAP_BOUNDS.1)
            .contains(&self.hydration_scope_cap)
        {
            return Err(InvalidSchedulerConfig::HydrationScopeCapOutOfRange);
        }
        if !(HYDRATION_SPLIT_TARGET_FILES_BOUNDS.0..=HYDRATION_SPLIT_TARGET_FILES_BOUNDS.1)
            .contains(&self.hydration_split_target_files)
        {
            return Err(InvalidSchedulerConfig::HydrationSplitTargetOutOfRange);
        }
        if !ARCHITECTURE_VIEW_MAX_CONTAINERS_RANGE.contains(&self.architecture_view_max_containers)
        {
            return Err(InvalidSchedulerConfig::ArchitectureViewMaxContainersOutOfRange);
        }
        if !PROCESS_IMPORT_MAX_ELEMENTS_RANGE.contains(&self.process_import_max_elements) {
            return Err(InvalidSchedulerConfig::ProcessImportMaxElementsOutOfRange);
        }
        if !PROCESS_DOCUMENT_MAX_BYTES_RANGE.contains(&self.process_document_max_bytes) {
            return Err(InvalidSchedulerConfig::ProcessDocumentBoundOutOfRange(
                "process_document_max_bytes",
            ));
        }
        if !PROCESS_DOCUMENT_MAX_DEPTH_RANGE.contains(&self.process_document_max_depth) {
            return Err(InvalidSchedulerConfig::ProcessDocumentBoundOutOfRange(
                "process_document_max_depth",
            ));
        }
        if !(ENRICH_TIMEOUT_MS_BOUNDS.0..=ENRICH_TIMEOUT_MS_BOUNDS.1)
            .contains(&self.enrich_timeout_ms)
        {
            return Err(InvalidSchedulerConfig::EnrichTimeoutOutOfRange);
        }
        let history = [
            self.history_max_commits,
            self.history_support_floor,
            self.history_confidence_floor_milli,
            self.history_max_files_per_commit,
        ];
        for (value, (_, env, _, bounds)) in history.into_iter().zip(HISTORY_KNOBS) {
            if !(bounds.0..=bounds.1).contains(&value) {
                return Err(InvalidSchedulerConfig::HistoryKnobOutOfRange(env));
            }
        }
        let mut available = self.total_workers;
        for (lane, floor) in &self.lane_floors {
            if lane.trim().is_empty() {
                return Err(InvalidSchedulerConfig::BlankLane);
            }
            // Subtract rather than sum: even u32::MAX floors cannot wrap.
            available = available.checked_sub(*floor).ok_or(
                InvalidSchedulerConfig::OversubscribedFloors {
                    total_workers: self.total_workers,
                },
            )?;
        }
        Ok(())
    }

    /// Apply the `MEDULLA_<NAME>` startup overrides of the hydration keys and
    /// re-validate, so an out-of-range value refuses startup.
    pub fn apply_env_overrides(
        &mut self,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<(), InvalidSchedulerConfig> {
        for (key, slot) in [
            (ENV_HYDRATION_SCOPE_CAP, &mut self.hydration_scope_cap),
            (
                ENV_HYDRATION_SPLIT_TARGET_FILES,
                &mut self.hydration_split_target_files,
            ),
        ] {
            if let Some(raw) = lookup(key) {
                *slot = raw
                    .trim()
                    .parse()
                    .map_err(|_| InvalidSchedulerConfig::InvalidEnvValue(key))?;
            }
        }
        if lookup(ENV_ENRICH_TIMEOUT_MS).is_some() {
            self.enrich_timeout_ms = enrich_timeout_ms_from(&lookup)?;
        }
        if HISTORY_KNOBS
            .iter()
            .any(|(_, env, ..)| lookup(env).is_some())
        {
            [
                self.history_max_commits,
                self.history_support_floor,
                self.history_confidence_floor_milli,
                self.history_max_files_per_commit,
            ] = history_knobs_from(&lookup)?;
        }
        self.validate()
    }
}

/// Default of `knob reasoning.rule_max_steps` (MEDULLA-FEDERATED-QUERY-R608.2).
pub const DEFAULT_RULE_MAX_STEPS: u64 = 100_000;
/// Bounds of `knob reasoning.rule_max_steps`, in rule evaluations.
pub const RULE_MAX_STEPS_RANGE: (u64, u64) = (1_000, 10_000_000);
/// Default of `knob reasoning.causal_nrmse_max` (MEDULLA-FEDERATED-QUERY-R608.2).
pub const DEFAULT_CAUSAL_NRMSE_MAX: f64 = 0.25;
/// Bounds of `knob reasoning.causal_nrmse_max`, a ratio.
pub const CAUSAL_NRMSE_MAX_RANGE: (f64, f64) = (0.01, 1.0);

/// Reasoning knobs of the server configuration, read from
/// `MEDULLA_REASONING_RULE_MAX_STEPS` and `MEDULLA_REASONING_CAUSAL_NRMSE_MAX`
/// and bounds-checked at startup.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningKnobs {
    pub rule_max_steps: u64,
    pub causal_nrmse_max: f64,
}

impl Default for ReasoningKnobs {
    fn default() -> Self {
        Self {
            rule_max_steps: DEFAULT_RULE_MAX_STEPS,
            causal_nrmse_max: DEFAULT_CAUSAL_NRMSE_MAX,
        }
    }
}

/// A reasoning knob outside its range or unparseable; refuses startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidKnob {
    pub knob: &'static str,
    pub value: String,
}

impl std::fmt::Display for InvalidKnob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "knob {} has invalid value {:?}", self.knob, self.value)
    }
}

impl std::error::Error for InvalidKnob {}

const RULE_MAX_STEPS_KNOB: &str = "reasoning.rule_max_steps";
const CAUSAL_NRMSE_MAX_KNOB: &str = "reasoning.causal_nrmse_max";

impl ReasoningKnobs {
    /// Read the knobs through `lookup` (the environment in production), apply
    /// defaults for unset ones and check every bound.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, InvalidKnob> {
        let mut knobs = Self::default();
        if let Some(raw) = lookup("MEDULLA_REASONING_RULE_MAX_STEPS") {
            knobs.rule_max_steps = raw.trim().parse().map_err(|_| InvalidKnob {
                knob: RULE_MAX_STEPS_KNOB,
                value: raw.clone(),
            })?;
        }
        if let Some(raw) = lookup("MEDULLA_REASONING_CAUSAL_NRMSE_MAX") {
            knobs.causal_nrmse_max = raw.trim().parse().map_err(|_| InvalidKnob {
                knob: CAUSAL_NRMSE_MAX_KNOB,
                value: raw.clone(),
            })?;
        }
        knobs.validate()?;
        Ok(knobs)
    }

    /// Read the knobs from the process environment.
    pub fn from_env() -> Result<Self, InvalidKnob> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Check every knob against its range.
    pub fn validate(&self) -> Result<(), InvalidKnob> {
        let (lo, hi) = RULE_MAX_STEPS_RANGE;
        if !(lo..=hi).contains(&self.rule_max_steps) {
            return Err(InvalidKnob {
                knob: RULE_MAX_STEPS_KNOB,
                value: self.rule_max_steps.to_string(),
            });
        }
        let (lo, hi) = CAUSAL_NRMSE_MAX_RANGE;
        if !(lo..=hi).contains(&self.causal_nrmse_max) {
            return Err(InvalidKnob {
                knob: CAUSAL_NRMSE_MAX_KNOB,
                value: self.causal_nrmse_max.to_string(),
            });
        }
        Ok(())
    }
}

static REASONING_KNOBS: std::sync::OnceLock<ReasoningKnobs> = std::sync::OnceLock::new();

/// Install the startup-validated reasoning knobs; the first install wins.
pub fn install_reasoning_knobs(knobs: ReasoningKnobs) {
    let _ = REASONING_KNOBS.set(knobs);
}

/// The installed reasoning knobs, or the defaults when startup installed none
/// (embedded and test construction).
pub fn reasoning_knobs() -> ReasoningKnobs {
    REASONING_KNOBS.get().copied().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> SchedulerConfig {
        serde_json::from_value(json!({
            "total_workers": 4,
            "lane_floors": {"general": 1, "hydration": 2},
            "max_queue_depth": 100
        }))
        .unwrap()
    }

    // spec: MEDULLA-PLACEMENT-R002.2
    // spec: MEDULLA-DURABLE-KERNEL-R076.1
    #[test]
    fn defaults_and_lane_floors_round_trip() {
        let config = config();
        assert_eq!(config.cores_per_mille, 360);
        assert_eq!(config.memory_per_worker_bytes, 3_221_225_472);
        assert_eq!(config.validate(), Ok(()));
        let encoded = serde_json::to_string(&config).unwrap();
        assert_eq!(
            serde_json::from_str::<SchedulerConfig>(&encoded).unwrap(),
            config
        );
    }

    // spec: MEDULLA-PLACEMENT-R002.2
    // spec: MEDULLA-DURABLE-KERNEL-R076.1
    #[test]
    fn explicit_autosize_and_queue_limits_round_trip() {
        let mut config = config();
        config.cores_per_mille = 1000;
        config.memory_per_worker_bytes = 1;
        config.max_queue_depth = u64::MAX;
        assert_eq!(config.validate(), Ok(()));
        let encoded = serde_json::to_value(&config).unwrap();
        assert_eq!(
            serde_json::from_value::<SchedulerConfig>(encoded).unwrap(),
            config
        );
    }

    // spec: MEDULLA-DURABLE-KERNEL-R076.1
    #[test]
    fn exact_floor_sum_is_accepted_and_oversubscription_is_refused() {
        let mut config = config();
        config.total_workers = 3;
        assert_eq!(config.validate(), Ok(()));
        config.total_workers = 2;
        assert_eq!(
            config.validate(),
            Err(InvalidSchedulerConfig::OversubscribedFloors { total_workers: 2 })
        );
    }

    // spec: MEDULLA-DURABLE-KERNEL-R076.1
    #[test]
    fn huge_floors_cannot_wrap_into_an_accepted_sum() {
        let mut config = config();
        config.total_workers = u32::MAX;
        config.lane_floors = BTreeMap::from([("a".into(), u32::MAX), ("b".into(), 1)]);
        assert_eq!(
            config.validate(),
            Err(InvalidSchedulerConfig::OversubscribedFloors {
                total_workers: u32::MAX
            })
        );
    }

    // spec: MEDULLA-DURABLE-KERNEL-R076.1
    #[test]
    fn zero_limits_disable_resources_without_an_implicit_floor() {
        let config: SchedulerConfig = serde_json::from_value(json!({
            "total_workers": 0, "cores_per_mille": 0, "max_queue_depth": 0
        }))
        .unwrap();
        assert!(config.lane_floors.is_empty());
        assert_eq!(config.validate(), Ok(()));
    }

    // spec: MEDULLA-DURABLE-KERNEL-R076.1
    #[test]
    fn invalid_policy_bounds_are_refused() {
        let mut config = config();
        config.cores_per_mille = 1001;
        assert_eq!(
            config.validate(),
            Err(InvalidSchedulerConfig::CoresPerMilleOutOfRange)
        );
        config.cores_per_mille = 360;
        config.memory_per_worker_bytes = 0;
        assert_eq!(
            config.validate(),
            Err(InvalidSchedulerConfig::ZeroMemoryPerWorker)
        );
        config.memory_per_worker_bytes = 1;
        config.lane_floors.insert("  ".into(), 0);
        assert_eq!(config.validate(), Err(InvalidSchedulerConfig::BlankLane));
    }

    // spec: MEDULLA-DURABLE-KERNEL-R076.1
    #[test]
    fn wire_requires_explicit_limits_and_rejects_unknown_or_negative_values() {
        for value in [
            json!({"total_workers": 1}),
            json!({"max_queue_depth": 1}),
            json!({"total_workers": 1, "max_queue_depth": 1, "worker_policy": true}),
            json!({"total_workers": -1, "max_queue_depth": 1}),
            json!({"total_workers": 1, "max_queue_depth": -1}),
            json!({"total_workers": 1, "max_queue_depth": 1, "cores_per_mille": 65536}),
        ] {
            assert!(serde_json::from_value::<SchedulerConfig>(value).is_err());
        }
    }

    // spec: MEDULLA-REPO-INGEST-R650.2
    #[test]
    fn hydration_knobs_default_override_and_refuse_out_of_range() {
        let mut config = config();
        assert_eq!(config.hydration_scope_cap, 10_000);
        assert_eq!(config.hydration_split_target_files, 2_000);
        config
            .apply_env_overrides(|key| (key == ENV_HYDRATION_SCOPE_CAP).then(|| "500".into()))
            .unwrap();
        assert_eq!(config.hydration_scope_cap, 500);
        assert_eq!(
            config.apply_env_overrides(|key| {
                (key == ENV_HYDRATION_SPLIT_TARGET_FILES).then(|| "99".into())
            }),
            Err(InvalidSchedulerConfig::HydrationSplitTargetOutOfRange)
        );
        let mut config = self::config();
        assert_eq!(
            config.apply_env_overrides(|key| (key == ENV_HYDRATION_SCOPE_CAP).then(|| "x".into())),
            Err(InvalidSchedulerConfig::InvalidEnvValue(
                ENV_HYDRATION_SCOPE_CAP
            ))
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R608.2
    #[test]
    fn reasoning_knobs_default_and_refuse_out_of_range() {
        let none = ReasoningKnobs::from_lookup(|_| None).unwrap();
        assert_eq!(none, ReasoningKnobs::default());
        let set = ReasoningKnobs::from_lookup(|k| match k {
            "MEDULLA_REASONING_RULE_MAX_STEPS" => Some("1000".into()),
            "MEDULLA_REASONING_CAUSAL_NRMSE_MAX" => Some("1.0".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!((set.rule_max_steps, set.causal_nrmse_max), (1_000, 1.0));
        let low = ReasoningKnobs::from_lookup(|k| {
            (k == "MEDULLA_REASONING_RULE_MAX_STEPS").then(|| "999".into())
        });
        assert_eq!(low.unwrap_err().knob, "reasoning.rule_max_steps");
        let high = ReasoningKnobs::from_lookup(|k| {
            (k == "MEDULLA_REASONING_CAUSAL_NRMSE_MAX").then(|| "1.5".into())
        });
        assert_eq!(high.unwrap_err().knob, "reasoning.causal_nrmse_max");
    }
}
