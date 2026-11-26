//! Lifecycle records committed through the decision log (MEDULLA-SDLC-R013, R019.4).
//!
//! `DecisionLog` is the one decision mechanism: a `would_dispatch` decision of the delivery
//! loop and a `ScenarioRun` of a delivery phase are records of it, not nodes of a second log.
//! A write stores one tenant-visible record under a key its own content determines, so a
//! repeat is a replay; a read lists the stored records of one kind, newest first.

use serde::{Deserialize, Serialize};

use medulla_source_types::delivery::WouldDispatch;
use medulla_source_types::lifecycle::{ScenarioRun, StoredScenarioRun};

/// Format identity of a lifecycle record body.
pub const LIFECYCLE_RECORD_SCHEMA_VERSION: u16 = 1;
/// Most records one read answers.
pub const MAX_LIFECYCLE_RECORDS_READ: usize = 1_024;

/// The closed set of lifecycle record kinds the log holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum LifecycleRecordKind {
    WouldDispatch,
    ScenarioRun,
}

impl LifecycleRecordKind {
    /// The stored-key segment of the kind.
    pub fn segment(self) -> &'static str {
        match self {
            Self::WouldDispatch => "would_dispatch",
            Self::ScenarioRun => "scenario_run",
        }
    }
}

/// What a `LifecycleRecord` op writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "record_kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum LifecycleRecordWrite {
    /// The slice an unprompted run would have dispatched (MEDULLA-SDLC-R019.4).
    WouldDispatch { record: WouldDispatch },
    /// One scenario run of a delivery phase (MEDULLA-SDLC-R013).
    ScenarioRun { record: ScenarioRun },
}

impl LifecycleRecordWrite {
    pub fn kind(&self) -> LifecycleRecordKind {
        match self {
            Self::WouldDispatch { .. } => LifecycleRecordKind::WouldDispatch,
            Self::ScenarioRun { .. } => LifecycleRecordKind::ScenarioRun,
        }
    }
}

/// The refs of one stored lifecycle record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LifecycleRecordStored {
    pub schema_version: u16,
    pub record_kind: LifecycleRecordKind,
    pub record_id: String,
    /// The engine instant the record was first committed at.
    pub recorded_ms: u64,
    /// Whether this answer repeats an earlier identical write.
    pub replayed: bool,
}

/// Which stored lifecycle records a read lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LifecycleRecordQuery {
    pub record_kind: LifecycleRecordKind,
    /// Only scenario runs of this phase.
    #[serde(default)]
    pub phase: Option<String>,
    /// Only scenario runs on this branch.
    #[serde(default)]
    pub branch: Option<String>,
}

/// A stored would-dispatch record with its id and instant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StoredWouldDispatch {
    pub id: String,
    pub record: WouldDispatch,
    pub recorded_ms: u64,
}

/// The stored lifecycle records a read found, newest first; at most
/// [`MAX_LIFECYCLE_RECORDS_READ`]. Only the list of the queried kind is filled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LifecycleRecordsFound {
    pub record_kind: LifecycleRecordKind,
    #[serde(default)]
    pub would_dispatch: Vec<StoredWouldDispatch>,
    #[serde(default)]
    pub scenario_runs: Vec<StoredScenarioRun>,
}
