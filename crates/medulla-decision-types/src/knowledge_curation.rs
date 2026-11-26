//! Knowledge curation for the Learn stage (MEDULLA-DECISION-ENGINE-R709.1).
//!
//! A scheduled curator reads the open flags, the traces they were observed in, the human
//! corrections, the failure patterns and the source changes ([`CurationInputs`]), and proposes
//! edits ([`CurationProposal`]). The engine never calls a model: it serves the inputs, checks
//! every proposal for contradiction and records it as an evaluated revision. [`CurationOp`] is
//! the body of `Method::KnowledgeCuration`.

use serde::{Deserialize, Serialize};

use crate::knowledge_flag::{KnowledgeFlag, KnowledgeKind};
use medulla_types::refusal_code::RefusalCode;

/// Most bytes one proposed body holds.
pub const CURATION_BODY_MAX_BYTES: usize = 64 * 1024;
/// Most flags, traces, corrections, patterns or source changes one `Inputs` page returns.
pub const CURATION_PAGE_MAX: usize = 200;
/// The graph class of a proposal node (vocabulary R817).
pub const CURATION_PROPOSAL_CLASS: &str = "CurationProposal";

/// `CONTRADICTS_ACTIVE_DEFINITION`: the target has a different current definition and the edit
/// does not supersede it.
pub const CONTRADICTS_ACTIVE_DEFINITION: RefusalCode =
    RefusalCode::new("CONTRADICTS_ACTIVE_DEFINITION");
/// `CONTRADICTS_INVARIANT`: the invariant gate refuses the edit.
pub const CONTRADICTS_INVARIANT: RefusalCode = RefusalCode::new("CONTRADICTS_INVARIANT");
/// `REFERENCE_UNRESOLVED`: the edit names a source column or term the graph does not hold.
pub const REFERENCE_UNRESOLVED: RefusalCode = RefusalCode::new("REFERENCE_UNRESOLVED");
/// `UPDATE_BASE_STALE`: `before_digest` is not the current digest.
pub const UPDATE_BASE_STALE: RefusalCode = RefusalCode::new("UPDATE_BASE_STALE");
/// `CURATION_FLAG_UNKNOWN`: a `flag_id` that is not `Open`.
pub const CURATION_FLAG_UNKNOWN: RefusalCode = RefusalCode::new("CURATION_FLAG_UNKNOWN");
/// `CURATION_NOT_DUE`: `Inputs` before the cadence elapsed and `force` is false.
pub const CURATION_NOT_DUE: RefusalCode = RefusalCode::new("CURATION_NOT_DUE");
/// `CURATION_LEASE_REQUIRED`: the caller holds no live lease for the curation loop.
pub const CURATION_LEASE_REQUIRED: RefusalCode = RefusalCode::new("CURATION_LEASE_REQUIRED");
/// `CURATION_EDIT_TOO_LARGE`: a body over 64 KiB.
pub const CURATION_EDIT_TOO_LARGE: RefusalCode = RefusalCode::new("CURATION_EDIT_TOO_LARGE");
/// `CURATION_PROPOSAL_NOT_FOUND`: no proposal of that id is visible to the caller.
pub const CURATION_PROPOSAL_NOT_FOUND: RefusalCode =
    RefusalCode::new("CURATION_PROPOSAL_NOT_FOUND");
/// `CURATION_ROLE_REQUIRED`: the caller lacks scope `knowledge:curate`.
pub const CURATION_ROLE_REQUIRED: RefusalCode = RefusalCode::new("CURATION_ROLE_REQUIRED");

/// The `knowledge_curation` table of registered refusal codes.
pub const REFUSALS: &[RefusalCode] = &[
    CONTRADICTS_ACTIVE_DEFINITION,
    CONTRADICTS_INVARIANT,
    REFERENCE_UNRESOLVED,
    UPDATE_BASE_STALE,
    CURATION_FLAG_UNKNOWN,
    CURATION_NOT_DUE,
    CURATION_LEASE_REQUIRED,
    CURATION_EDIT_TOO_LARGE,
    CURATION_PROPOSAL_NOT_FOUND,
    CURATION_ROLE_REQUIRED,
];

/// A run trace a flag was observed in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TraceRef {
    /// The trace node.
    pub trace_node_id: String,
    /// The run the trace records.
    pub run_id: String,
}

/// A human correction (R702) the curator may learn from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CorrectionRef {
    /// The correction node.
    pub correction_node_id: String,
    /// The correction id.
    pub correction_id: String,
    /// What the correction targets, when it names one.
    #[serde(default)]
    pub target: Option<String>,
}

/// A recurring failure pattern (R701) the curator may learn from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PatternRef {
    /// The gap node.
    pub gap_node_id: String,
    /// The pattern signature.
    pub signature: String,
    /// How many failed runs share it.
    pub count: u64,
}

/// A change a source ingest recorded since the watermark.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceChangeRef {
    /// The change node.
    pub change_node_id: String,
    /// The source that changed.
    pub source_id: String,
    /// When the change was recorded, Unix milliseconds.
    pub at_ms: u64,
}

/// What `Inputs` answers: everything the curator needs for one cycle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CurationInputs {
    /// The flags in state `Open`, oldest first.
    pub flags: Vec<KnowledgeFlag>,
    /// The traces the flags were observed in.
    pub traces: Vec<TraceRef>,
    /// The corrections since the watermark.
    pub corrections: Vec<CorrectionRef>,
    /// The failure patterns.
    pub patterns: Vec<PatternRef>,
    /// The source changes since the watermark.
    pub source_changes: Vec<SourceChangeRef>,
    /// The watermark this page was read since, Unix milliseconds.
    pub watermark_ms: u64,
    /// Whether a curation cycle is due.
    pub due: bool,
    /// The cursor of the next page of flags, when there is one.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// What an edit does to its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CurationEditOp {
    /// Replace the current definition.
    Update,
    /// Add a definition that does not exist.
    Create,
    /// Withdraw the current definition.
    Retire,
}

/// One proposed edit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CurationEdit {
    /// Update, create or retire.
    pub op: CurationEditOp,
    /// The digest the edit starts from; absent for a create.
    #[serde(default)]
    pub before_digest: Option<String>,
    /// The digest the edit produces; absent for a retire.
    #[serde(default)]
    pub after_digest: Option<String>,
    /// The digest of the proposed body.
    pub body_digest: String,
    /// Whether the edit replaces a different current definition on purpose.
    #[serde(default)]
    pub supersedes: bool,
    /// The source columns and terms the edit names; each must exist in the graph.
    #[serde(default)]
    pub references: Vec<String>,
}

/// One proposal: an edit with the flags it answers and the traces behind it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CurationProposal {
    /// The proposal id.
    pub proposal_id: String,
    /// The flags the proposal addresses, at least one.
    pub flag_ids: Vec<String>,
    /// The target of the edit.
    pub target_ref: String,
    /// Which kind of knowledge the edit changes.
    pub knowledge_kind: KnowledgeKind,
    /// The edit.
    pub edit: CurationEdit,
    /// The traces the curator read.
    #[serde(default)]
    pub rationale_trace_ids: Vec<String>,
    /// The digest of the evidence the curator read.
    pub evidence_digest: String,
}

/// One reason a proposal is refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ContradictionFinding {
    /// The registered code.
    pub code: String,
    /// What the proposal contradicts or fails to resolve.
    #[serde(default)]
    pub other_ref: Option<String>,
}

/// The body of `Method::KnowledgeCuration`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "op", deny_unknown_fields)]
pub enum CurationOp {
    /// Read the curation inputs. Needs a live lease on the curation loop.
    Inputs {
        /// The graph that holds flags, traces and corrections.
        graph: String,
        /// The curation loop the caller holds.
        loop_id: String,
        /// The lease epoch the caller holds.
        epoch: u64,
        /// Read even though the cadence has not elapsed.
        #[serde(default)]
        force: bool,
        /// The cursor of a previous page.
        #[serde(default)]
        cursor: Option<String>,
        /// Most flags returned, at most 200.
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Check one proposal and record it as an evaluated revision. Needs a live lease.
    Propose {
        /// The graph that holds the flags.
        graph: String,
        /// The curation loop the caller holds.
        loop_id: String,
        /// The lease epoch the caller holds.
        epoch: u64,
        /// The proposal.
        proposal: CurationProposal,
        /// The proposed body, at most 64 KiB; its digest is `edit.body_digest`.
        body: String,
    },
    /// Read one proposal and close the flags of an adopted proposal.
    Status {
        /// The graph that holds the flags.
        graph: String,
        /// The proposal id.
        proposal_id: String,
    },
    /// Read the closed-loop measures of a window.
    Metrics {
        /// The graph that holds the flags.
        graph: String,
        /// First millisecond of the window.
        from_ms: u64,
        /// Last millisecond of the window.
        to_ms: u64,
    },
}

impl CurationOp {
    /// Whether the op writes: `Propose` and `Status` do, `Inputs` and `Metrics` do not.
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Propose { .. } | Self::Status { .. })
    }

    /// The graph the op reads and writes.
    pub fn graph(&self) -> &str {
        match self {
            Self::Inputs { graph, .. }
            | Self::Propose { graph, .. }
            | Self::Status { graph, .. }
            | Self::Metrics { graph, .. } => graph,
        }
    }
}

/// What `Propose` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CurationAdmitted {
    /// The proposal id.
    pub proposal_id: String,
    /// The id of the revision the proposal was recorded as.
    pub revision_id: String,
    /// The stage the revision stands at.
    pub stage: String,
    /// The receipt id every node and edge of the proposal shares.
    pub receipt_id: String,
}

/// One flag of a proposal and its state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FlagStateRow {
    /// The flag.
    pub flag_id: String,
    /// Its state.
    pub state: crate::knowledge_flag::FlagState,
}

/// What `Status` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CurationStatus {
    /// The proposal id.
    pub proposal_id: String,
    /// The stage of its revision.
    pub stage: String,
    /// The addressed flags and their states after this read.
    pub flags: Vec<FlagStateRow>,
    /// The flags this read moved to `Curated`.
    pub adopted: Vec<String>,
}

/// The closed-loop measures of one tenant and window (R709.7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct LoopMetrics {
    /// Flags raised in the window.
    pub flags_raised: u64,
    /// Of those, the flags now `Curated`.
    pub flags_curated: u64,
    /// Of those, the flags now `Dismissed`.
    pub flags_dismissed: u64,
    /// The median of resolve time minus raise time over the curated flags, milliseconds.
    pub median_flag_to_fix_ms: u64,
    /// The share of ever-curated flags that returned to `Open` through a reraise, in ppm.
    pub recurrence_ppm: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN_PROPOSAL: &str = r#"{"proposal_id":"p1","flag_ids":["flag:a"],"target_ref":"source:orders","knowledge_kind":"data_access","edit":{"op":"update","before_digest":"b","after_digest":"a","body_digest":"d","supersedes":false,"references":[]},"rationale_trace_ids":["t1"],"evidence_digest":"e"}"#;

    // spec: MEDULLA-DECISION-ENGINE-R709.1
    #[test]
    fn curation_types_round_trip_with_golden_vector() {
        let proposal: CurationProposal = serde_json::from_str(GOLDEN_PROPOSAL).expect("decode");
        assert_eq!(proposal.edit.op, CurationEditOp::Update);
        assert_eq!(proposal.knowledge_kind, KnowledgeKind::DataAccess);
        assert_eq!(
            serde_json::to_string(&proposal).expect("encode"),
            GOLDEN_PROPOSAL
        );
        let op = CurationOp::Metrics {
            graph: "g".into(),
            from_ms: 1,
            to_ms: 2,
        };
        let text = serde_json::to_string(&op).expect("encode op");
        assert_eq!(serde_json::from_str::<CurationOp>(&text).unwrap(), op);
        assert!(!op.is_mutation());
        let unknown = GOLDEN_PROPOSAL.replace("\"evidence_digest\":\"e\"", "\"evidence_digest\":\"e\",\"x\":1");
        assert!(serde_json::from_str::<CurationProposal>(&unknown).is_err());
        let inputs = CurationInputs {
            flags: Vec::new(),
            traces: vec![TraceRef {
                trace_node_id: "t".into(),
                run_id: "r".into(),
            }],
            corrections: Vec::new(),
            patterns: Vec::new(),
            source_changes: Vec::new(),
            watermark_ms: 7,
            due: true,
            next_cursor: None,
        };
        let text = serde_json::to_string(&inputs).expect("encode inputs");
        assert_eq!(serde_json::from_str::<CurationInputs>(&text).unwrap(), inputs);
    }
}
