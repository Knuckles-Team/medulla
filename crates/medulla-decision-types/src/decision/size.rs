//! Source size and adequacy facts of an assembly candidate
//! (MEDULLA-DECISION-ENGINE-R014.100.1).
//!
//! A published component may declare, as entry attributes, how large its
//! source artifact is and whether a recorded evaluation found it adequate for
//! the task. `CandidateFacts::from_entry` reads them through the same one
//! constructor both sides of a decision use, so a commit rebuilds exactly the
//! facts the assembly read. Both are observations, never claims: each one
//! names the revision or evaluation that measured it.

use serde::{Deserialize, Serialize};

use medulla_agent_types::agent_component::AgentComponentEntry;

/// Attribute carrying the artifact size in bytes.
pub const SIZE_BYTES_ATTRIBUTE: &str = "size.artifact_bytes";
/// Attribute carrying the artifact line count.
pub const SIZE_LINES_ATTRIBUTE: &str = "size.line_count";
/// Attribute carrying the revision the size was measured at.
pub const SIZE_REVISION_ATTRIBUTE: &str = "size.measured_revision";
/// Attribute carrying `true` or `false`: whether the component is adequate.
pub const ADEQUATE_ATTRIBUTE: &str = "adequacy.adequate";
/// Attribute carrying the id of the evaluation that judged adequacy.
pub const ADEQUACY_EVALUATION_ATTRIBUTE: &str = "adequacy.evaluation_id";

/// Attribute carrying the observed cost of one run, in micros, read from the
/// usage ledger (MEDULLA-DECISION-ENGINE-R014.100.2).
pub const RUN_COST_MICROS_ATTRIBUTE: &str = "ledger.run_cost_micros";
/// Attribute carrying the ledger revision the run cost was read at.
pub const RUN_COST_REVISION_ATTRIBUTE: &str = "ledger.measured_revision";

/// How large a component's source artifact is, at a measured revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SourceSize {
    pub artifact_bytes: u64,
    pub line_count: u32,
    pub measured_revision: u64,
}

/// Whether a recorded evaluation found the component adequate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Adequacy {
    pub adequate: bool,
    pub evaluation_id: String,
}

/// What one run of the component cost, as the usage ledger recorded it
/// (MEDULLA-DECISION-ENGINE-R014.100.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ObservedRunCost {
    pub micros: u64,
    pub measured_revision: u64,
}

/// A malformed size or adequacy declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SizeFactError {
    /// A size attribute is present without its companions, or unparsable.
    SizeIncomplete,
    /// An adequacy attribute is present without its companion, or unparsable.
    AdequacyIncomplete,
    /// A ledger cost attribute is present without its companion, or unparsable.
    RunCostIncomplete,
}

/// The size the entry declares: `None` when it declares no size attribute.
pub fn size_from_entry(entry: &AgentComponentEntry) -> Result<Option<SourceSize>, SizeFactError> {
    let attrs = [
        entry.attributes.get(SIZE_BYTES_ATTRIBUTE),
        entry.attributes.get(SIZE_LINES_ATTRIBUTE),
        entry.attributes.get(SIZE_REVISION_ATTRIBUTE),
    ];
    if attrs.iter().all(Option::is_none) {
        return Ok(None);
    }
    let [Some(bytes), Some(lines), Some(revision)] = attrs else {
        return Err(SizeFactError::SizeIncomplete);
    };
    Ok(Some(SourceSize {
        artifact_bytes: bytes.parse().map_err(|_| SizeFactError::SizeIncomplete)?,
        line_count: lines.parse().map_err(|_| SizeFactError::SizeIncomplete)?,
        measured_revision: revision
            .parse()
            .map_err(|_| SizeFactError::SizeIncomplete)?,
    }))
}

/// The adequacy the entry declares: `None` when it declares none (unknown).
pub fn adequacy_from_entry(entry: &AgentComponentEntry) -> Result<Option<Adequacy>, SizeFactError> {
    let flag = entry.attributes.get(ADEQUATE_ATTRIBUTE);
    let evaluation = entry.attributes.get(ADEQUACY_EVALUATION_ATTRIBUTE);
    let (flag, evaluation) = match (flag, evaluation) {
        (None, None) => return Ok(None),
        (Some(flag), Some(evaluation)) => (flag, evaluation),
        _ => return Err(SizeFactError::AdequacyIncomplete),
    };
    let adequate = match flag.as_str() {
        "true" => true,
        "false" => false,
        _ => return Err(SizeFactError::AdequacyIncomplete),
    };
    Ok(Some(Adequacy {
        adequate,
        evaluation_id: evaluation.clone(),
    }))
}

/// The ledger run cost the entry declares: `None` when it declares none.
pub fn run_cost_from_entry(
    entry: &AgentComponentEntry,
) -> Result<Option<ObservedRunCost>, SizeFactError> {
    let micros = entry.attributes.get(RUN_COST_MICROS_ATTRIBUTE);
    let revision = entry.attributes.get(RUN_COST_REVISION_ATTRIBUTE);
    let (micros, revision) = match (micros, revision) {
        (None, None) => return Ok(None),
        (Some(micros), Some(revision)) => (micros, revision),
        _ => return Err(SizeFactError::RunCostIncomplete),
    };
    Ok(Some(ObservedRunCost {
        micros: micros
            .parse()
            .map_err(|_| SizeFactError::RunCostIncomplete)?,
        measured_revision: revision
            .parse()
            .map_err(|_| SizeFactError::RunCostIncomplete)?,
    }))
}
