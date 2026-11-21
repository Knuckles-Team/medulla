//! The reply of the served `DowntimeSelections` method (MEDULLA-DURABLE-KERNEL-R101.3).

use serde::{Deserialize, Serialize};

/// One downtime job type the latest tick submitted a job for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DowntimeSelected {
    /// The IRI of the selected `dw:JobType`.
    pub iri: String,
    /// The intent whose job the tick submitted.
    pub intent: String,
    /// The id of the submitted job.
    pub job_id: String,
}

/// What the latest downtime tick decided.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DowntimeSelections {
    /// Engine time of the tick, in milliseconds; 0 before the first tick.
    pub at_ms: i64,
    /// The types a job was submitted for, in run order.
    pub selected: Vec<DowntimeSelected>,
    /// Types skipped because a precondition was false.
    pub skipped_precondition: Vec<String>,
    /// Types skipped because their budget did not fit the window.
    pub skipped_budget: Vec<String>,
    /// Types selected but not submitted because no background lease was granted.
    pub skipped_lease: Vec<String>,
}
