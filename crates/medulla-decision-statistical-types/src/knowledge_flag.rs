//! Knowledge flags for the Learn stage (MEDULLA-DECISION-ENGINE-R708.1).
//!
//! Any agent may flag knowledge that is missing or wrong: a skill, a domain term or a data
//! access source. A [`KnowledgeFlag`] names one target, says whether the knowledge is
//! [`FlagKind::Missing`] or [`FlagKind::Wrong`] and may carry a suggested correction. A flag is
//! agent-asserted and never counts as an independent outcome; only the curation cycle of
//! R709 turns a flag into a change. [`KnowledgeFlagOp`] is the body of `Method::KnowledgeFlag`.
//!
//! The state of a flag is the explicit machine [`KNOWLEDGE_FLAG_MACHINE`] (organization
//! constitution Article XXXI; ontology description `KnowledgeFlagMachine`).

use serde::{Deserialize, Serialize};

use medulla_types::contract::Digest256;
use medulla_types::refusal_code::RefusalCode;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// Most bytes `what_observed` holds.
pub const WHAT_OBSERVED_MAX_BYTES: usize = 2 * 1024;
/// Most bytes `suggested_correction` holds.
pub const SUGGESTED_CORRECTION_MAX_BYTES: usize = 8 * 1024;
/// Most flags one `List` returns.
pub const FLAG_LIST_MAX: usize = 200;

/// The graph class of a flag node (vocabulary R817).
pub const KNOWLEDGE_FLAG_CLASS: &str = "KnowledgeFlag";
/// The `evidence_class` every flag carries: a flag is never an independent outcome.
pub const EVIDENCE_AGENT_ASSERTED: &str = "agent_asserted";

/// `FLAG_TARGET_UNKNOWN`: a `Wrong` flag whose target does not resolve in the caller's tenant graph.
pub const FLAG_TARGET_UNKNOWN: RefusalCode = RefusalCode::new("FLAG_TARGET_UNKNOWN");
/// `FLAG_RUN_UNKNOWN`: a `run_id` that is not an open or recorded run of the caller.
pub const FLAG_RUN_UNKNOWN: RefusalCode = RefusalCode::new("FLAG_RUN_UNKNOWN");
/// `FLAG_TEXT_TOO_LARGE`: text over its bound.
pub const FLAG_TEXT_TOO_LARGE: RefusalCode = RefusalCode::new("FLAG_TEXT_TOO_LARGE");
/// `FLAG_RUN_QUOTA_EXCEEDED`: more open flags in one run than `flag.max_open_per_run`.
pub const FLAG_RUN_QUOTA_EXCEEDED: RefusalCode = RefusalCode::new("FLAG_RUN_QUOTA_EXCEEDED");
/// `FLAG_RESOLVE_ROLE_REQUIRED`: `Resolve` without scope `knowledge:curate`.
pub const FLAG_RESOLVE_ROLE_REQUIRED: RefusalCode = RefusalCode::new("FLAG_RESOLVE_ROLE_REQUIRED");
/// `FLAG_STATE_ILLEGAL`: an event the machine does not allow.
pub const FLAG_STATE_ILLEGAL: RefusalCode = RefusalCode::new("FLAG_STATE_ILLEGAL");
/// `FLAG_NOT_FOUND`: no flag of that id is visible to the caller.
pub const FLAG_NOT_FOUND: RefusalCode = RefusalCode::new("FLAG_NOT_FOUND");
/// `FLAG_TARGET_INVALID`: a `target_ref` that is none of the four forms.
pub const FLAG_TARGET_INVALID: RefusalCode = RefusalCode::new("FLAG_TARGET_INVALID");

/// The `knowledge_flag` table of registered refusal codes.
pub const REFUSALS: &[RefusalCode] = &[
    FLAG_TARGET_UNKNOWN,
    FLAG_RUN_UNKNOWN,
    FLAG_TEXT_TOO_LARGE,
    FLAG_RUN_QUOTA_EXCEEDED,
    FLAG_RESOLVE_ROLE_REQUIRED,
    FLAG_STATE_ILLEGAL,
    FLAG_NOT_FOUND,
    FLAG_TARGET_INVALID,
];

/// The three kinds of knowledge a flag names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeKind {
    /// A skill revision.
    Skill,
    /// A domain term.
    Domain,
    /// A data access source.
    DataAccess,
}

/// Whether the knowledge is missing or wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FlagKind {
    /// The knowledge does not exist.
    Missing,
    /// The knowledge exists and is wrong.
    Wrong,
}

impl FlagKind {
    /// The stored text.
    pub fn stored(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Wrong => "wrong",
        }
    }
}

impl KnowledgeKind {
    /// The stored text.
    pub fn stored(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Domain => "domain",
            Self::DataAccess => "data_access",
        }
    }

    /// The kind whose stored text is `text`.
    pub fn from_stored(text: &str) -> Option<Self> {
        [Self::Skill, Self::Domain, Self::DataAccess]
            .into_iter()
            .find(|kind| kind.stored() == text)
    }
}

/// The closed state set of `KnowledgeFlagMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FlagState {
    /// Raised and waiting for the curation cycle.
    Open,
    /// A curation proposal fixing it was adopted.
    Curated,
    /// The curator or a human dismissed it.
    Dismissed,
}

/// The closed event set of `KnowledgeFlagMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlagEvent {
    /// A curation proposal fixing the flag was adopted: `Open` to `Curated`.
    Adopt,
    /// The flag was dismissed: `Open` to `Dismissed`.
    Dismiss,
    /// The same flag was raised again after it was curated: `Curated` to `Open`.
    Reraise,
    /// The same flag was raised again with different evidence after a dismissal:
    /// `Dismissed` to `Open`.
    ReraiseChanged,
}

impl MachineState for FlagState {
    fn stored(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Curated => "curated",
            Self::Dismissed => "dismissed",
        }
    }
}

impl MachineEvent for FlagEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Adopt => "Adopt",
            Self::Dismiss => "Dismiss",
            Self::Reraise => "Reraise",
            Self::ReraiseChanged => "ReraiseChanged",
        }
    }
}

/// `KnowledgeFlagMachine`: the closed edge table. Every other pair is refused.
pub static KNOWLEDGE_FLAG_MACHINE: MachineDef<FlagState, FlagEvent, &'static str> = MachineDef {
    name: "KnowledgeFlagMachine",
    states: &[FlagState::Open, FlagState::Curated, FlagState::Dismissed],
    initial: FlagState::Open,
    terminals: &[],
    events: &[
        FlagEvent::Adopt,
        FlagEvent::Dismiss,
        FlagEvent::Reraise,
        FlagEvent::ReraiseChanged,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "KF01",
            from: FlagState::Open,
            event: FlagEvent::Adopt,
            guard: None,
            to: FlagState::Curated,
        },
        Edge {
            id: "KF02",
            from: FlagState::Open,
            event: FlagEvent::Dismiss,
            guard: None,
            to: FlagState::Dismissed,
        },
        Edge {
            id: "KF03",
            from: FlagState::Curated,
            event: FlagEvent::Reraise,
            guard: None,
            to: FlagState::Open,
        },
        Edge {
            id: "KF04",
            from: FlagState::Dismissed,
            event: FlagEvent::ReraiseChanged,
            guard: None,
            to: FlagState::Open,
        },
    ],
};

impl FlagState {
    /// Every state, in declaration order.
    pub const ALL: [FlagState; 3] = [FlagState::Open, FlagState::Curated, FlagState::Dismissed];

    /// The one transition function of `KnowledgeFlagMachine`.
    pub fn apply(self, event: FlagEvent) -> Result<Self, TransitionRefused> {
        transition(&KNOWLEDGE_FLAG_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The state whose stored text is `text`.
    pub fn from_stored(text: &str) -> Option<Self> {
        KNOWLEDGE_FLAG_MACHINE.state_from_stored(text)
    }
}

/// One flag, as the engine stores and returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct KnowledgeFlag {
    /// `flag:` plus the first 32 hex characters of the flag key.
    pub flag_id: String,
    /// The tenant that owns the flag.
    pub tenant_id: String,
    /// Which kind of knowledge the flag names.
    pub knowledge_kind: KnowledgeKind,
    /// Whether the knowledge is missing or wrong.
    pub flag_kind: FlagKind,
    /// `skill:<name>@<revision>`, `term:<iri>`, `source:<source_id>` or `topic:<slug>`.
    pub target_ref: String,
    /// What the agent saw, at most 2 KiB.
    pub what_observed: String,
    /// What the agent suggests instead, at most 8 KiB.
    #[serde(default)]
    pub suggested_correction: Option<String>,
    /// The principal that raised the flag.
    pub raised_by: String,
    /// The run in which the agent saw it.
    pub run_id: String,
    /// The run-trace node of that run, when it has one.
    #[serde(default)]
    pub trace_node_id: Option<String>,
    /// When the flag was first raised, Unix milliseconds.
    pub raised_at_ms: u64,
    /// The machine state.
    pub state: FlagState,
    /// How many times the same flag was raised.
    pub raise_count: u64,
    /// Whether the flag returned to `Open` after a curation or a dismissal.
    pub reopened: bool,
}

/// Why a `Resolve` ends the flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FlagResolution {
    /// A curation proposal fixing the flag was adopted.
    Adopt,
    /// The flag is dismissed.
    Dismiss,
}

/// The body of `Method::KnowledgeFlag`. Every op names the caller `graph` that holds the
/// flags, the runs and the targets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "op", deny_unknown_fields)]
pub enum KnowledgeFlagOp {
    /// Raise one flag. Scope `knowledge:flag`.
    Raise {
        /// The graph that holds the runs and receives the flag.
        graph: String,
        /// Which kind of knowledge is flagged.
        knowledge_kind: KnowledgeKind,
        /// Missing or wrong.
        flag_kind: FlagKind,
        /// The target.
        target_ref: String,
        /// What the agent observed.
        what_observed: String,
        /// A suggested correction.
        #[serde(default)]
        suggested_correction: Option<String>,
        /// The run in which the agent observed it.
        run_id: String,
        /// The run-trace node of that run.
        #[serde(default)]
        trace_node_id: Option<String>,
        /// When the agent raised it, Unix milliseconds.
        at_ms: u64,
    },
    /// Move a flag out of `Open`. Scope `knowledge:curate`.
    Resolve {
        /// The graph that holds the flag.
        graph: String,
        /// The flag id.
        flag_id: String,
        /// Adopt or dismiss.
        resolution: FlagResolution,
        /// The human correction (R702) that resolved the flag, when there is one.
        #[serde(default)]
        correction_id: Option<String>,
        /// The adopted revision that fixed the flag, when there is one.
        #[serde(default)]
        fixed_by: Option<String>,
    },
    /// Read one flag with its links.
    Get {
        /// The graph that holds the flag.
        graph: String,
        /// The flag id.
        flag_id: String,
    },
    /// List flags, newest first.
    List {
        /// The graph that holds the flags.
        graph: String,
        /// Only flags in this state.
        #[serde(default)]
        state: Option<FlagState>,
        /// Most flags returned, at most 200.
        #[serde(default)]
        limit: Option<u32>,
    },
}

impl KnowledgeFlagOp {
    /// Whether the op writes: `Raise` and `Resolve` do, `Get` and `List` do not.
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Raise { .. } | Self::Resolve { .. })
    }

    /// The graph the op reads and writes.
    pub fn graph(&self) -> &str {
        match self {
            Self::Raise { graph, .. }
            | Self::Resolve { graph, .. }
            | Self::Get { graph, .. }
            | Self::List { graph, .. } => graph,
        }
    }
}

/// What a `Raise` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RaiseOutcome {
    /// A new flag node was written.
    Created,
    /// The same flag existed: `raise_count` grew by one and no second node was written.
    Deduplicated,
    /// The same flag returned to `Open` from `Curated` or `Dismissed`.
    Reopened,
}

/// What `Raise` and `Resolve` answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct KnowledgeFlagReceipt {
    /// The flag after the op.
    pub flag: KnowledgeFlag,
    /// What a `Raise` did; absent for `Resolve`.
    #[serde(default)]
    pub outcome: Option<RaiseOutcome>,
    /// The receipt id every node and edge of the op shares.
    pub receipt_id: String,
}

/// What `Get` answers: the flag and the nodes it links to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct KnowledgeFlagView {
    /// The flag.
    pub flag: KnowledgeFlag,
    /// The `evidence_class` property of the flag node.
    pub evidence_class: String,
    /// The node the flag names, when it exists.
    #[serde(default)]
    pub flags: Option<String>,
    /// The run-trace node the flag was observed in.
    #[serde(default)]
    pub observed_in: Option<String>,
    /// The principal that raised it.
    pub raised_by: String,
    /// The correction that resolved it.
    #[serde(default)]
    pub resolved_by: Option<String>,
    /// The revision that fixed it.
    #[serde(default)]
    pub fixed_by: Option<String>,
}

/// What `List` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct KnowledgeFlagList {
    /// The flags, newest first.
    pub flags: Vec<KnowledgeFlag>,
}

/// The flag key: `sha256(tenant_id, target_ref, flag_kind, digest of suggested_correction or
/// what_observed)` as 64 lowercase hex characters.
pub fn flag_key(flag: &KnowledgeFlag) -> String {
    let evidence = flag
        .suggested_correction
        .as_deref()
        .unwrap_or(&flag.what_observed);
    let evidence_digest = Digest256::sha256(evidence.as_bytes()).to_hex();
    Digest256::framed(
        b"medulla.knowledge_flag.v1",
        &[
            flag.tenant_id.as_bytes(),
            flag.target_ref.as_bytes(),
            flag.flag_kind.stored().as_bytes(),
            evidence_digest.as_bytes(),
        ],
    )
    .expect("a flag key frames four short fields")
    .to_hex()
}

/// The id of a flag: `flag:` plus the first 32 hex characters of its key.
pub fn flag_id_of(key: &str) -> String {
    format!("flag:{}", &key[..32])
}

/// The digest of what the agent observed, kept on the node to tell a changed re-raise.
pub fn evidence_digest(what_observed: &str) -> String {
    Digest256::sha256(what_observed.as_bytes()).to_hex()
}

/// Whether `target_ref` is one of the four forms and what the target names.
pub fn target_form(target_ref: &str) -> Option<(&'static str, &str)> {
    ["skill:", "term:", "source:", "topic:"]
        .into_iter()
        .find_map(|prefix| {
            target_ref
                .strip_prefix(prefix)
                .filter(|rest| !rest.is_empty())
                .map(|rest| (prefix, rest))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN_FLAG: &str = r#"{"flag_id":"flag:00","tenant_id":"t1","knowledge_kind":"data_access","flag_kind":"wrong","target_ref":"source:orders","what_observed":"column amount is renamed","suggested_correction":"use total_amount","raised_by":"agent-7","run_id":"run-1","trace_node_id":"trace-1","raised_at_ms":1700000000000,"state":"open","raise_count":2,"reopened":false}"#;

    // spec: MEDULLA-DECISION-ENGINE-R708.1
    #[test]
    fn flag_types_round_trip_with_golden_vector() {
        let flag: KnowledgeFlag = serde_json::from_str(GOLDEN_FLAG).expect("decode");
        assert_eq!(flag.knowledge_kind, KnowledgeKind::DataAccess);
        assert_eq!(flag.flag_kind, FlagKind::Wrong);
        assert_eq!(flag.state, FlagState::Open);
        assert_eq!(serde_json::to_string(&flag).expect("encode"), GOLDEN_FLAG);
        let op = KnowledgeFlagOp::Raise {
            graph: "g".into(),
            knowledge_kind: KnowledgeKind::Skill,
            flag_kind: FlagKind::Missing,
            target_ref: "skill:triage@3".into(),
            what_observed: "no step covers refunds".into(),
            suggested_correction: None,
            run_id: "run-1".into(),
            trace_node_id: None,
            at_ms: 5,
        };
        let text = serde_json::to_string(&op).expect("encode op");
        assert_eq!(serde_json::from_str::<KnowledgeFlagOp>(&text).unwrap(), op);
        assert!(op.is_mutation());
        // Unknown fields are refused.
        let unknown = GOLDEN_FLAG.replace("\"reopened\":false", "\"reopened\":false,\"x\":1");
        assert!(serde_json::from_str::<KnowledgeFlag>(&unknown).is_err());
        assert_eq!(target_form("topic:refunds"), Some(("topic:", "refunds")));
        assert_eq!(target_form("refunds"), None);
        assert_eq!(FlagState::from_stored("curated"), Some(FlagState::Curated));
    }
}
