//! Friction reports for the Learn stage (MEDULLA-DECISION-ENGINE-R710.1, R710.2).
//!
//! An always-on skill lets an agent file what it tried, what it expected, what happened and the
//! fix it suggests ([`FrictionReport`]). The engine groups the reports of one skill
//! ([`FrictionGroup`]) and each group moves along the explicit machine [`FRICTION_MACHINE`]:
//! triage, root cause, fix, evaluation, approval, measurement and regression check. Every move
//! names its evidence ([`FrictionTransition`]). [`FrictionOp`] is the body of
//! `Method::FrictionReport`.

use serde::{Deserialize, Serialize};

use medulla_types::refusal_code::RefusalCode;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// Most bytes each text field of a report holds.
pub const FRICTION_TEXT_MAX_BYTES: usize = 2 * 1024;
/// Most groups one `Group` read returns.
pub const FRICTION_GROUP_LIST_MAX: usize = 200;
/// The graph class of a report node (vocabulary R817).
pub const FRICTION_REPORT_CLASS: &str = "FrictionReport";
/// The graph class of a transition node (vocabulary R817).
pub const FRICTION_STEP_CLASS: &str = "FrictionStep";
/// The graph class of a group node.
pub const FRICTION_GROUP_CLASS: &str = "FrictionGroup";
/// The scope that lets a caller approve a group.
pub const SCOPE_APPROVE: &str = "knowledge:approve";

/// `FRICTION_SKILL_UNKNOWN`: a `skill_ref` that is not a published revision.
pub const FRICTION_SKILL_UNKNOWN: RefusalCode = RefusalCode::new("FRICTION_SKILL_UNKNOWN");
/// `FRICTION_TRANSITION_ILLEGAL`: a move outside the machine.
pub const FRICTION_TRANSITION_ILLEGAL: RefusalCode =
    RefusalCode::new("FRICTION_TRANSITION_ILLEGAL");
/// `FRICTION_EVIDENCE_REQUIRED`: a move without its `evidence_ref`.
pub const FRICTION_EVIDENCE_REQUIRED: RefusalCode =
    RefusalCode::new("FRICTION_EVIDENCE_REQUIRED");
/// `FRICTION_APPROVAL_ROLE_REQUIRED`: `Approved` without scope `knowledge:approve`.
pub const FRICTION_APPROVAL_ROLE_REQUIRED: RefusalCode =
    RefusalCode::new("FRICTION_APPROVAL_ROLE_REQUIRED");
/// `FRICTION_TEXT_TOO_LARGE`: a text field over its bound.
pub const FRICTION_TEXT_TOO_LARGE: RefusalCode = RefusalCode::new("FRICTION_TEXT_TOO_LARGE");
/// `FRICTION_GROUP_NOT_FOUND`: no group of that reference is visible to the caller.
pub const FRICTION_GROUP_NOT_FOUND: RefusalCode = RefusalCode::new("FRICTION_GROUP_NOT_FOUND");

/// The `friction` table of registered refusal codes.
pub const REFUSALS: &[RefusalCode] = &[
    FRICTION_SKILL_UNKNOWN,
    FRICTION_TRANSITION_ILLEGAL,
    FRICTION_EVIDENCE_REQUIRED,
    FRICTION_APPROVAL_ROLE_REQUIRED,
    FRICTION_TEXT_TOO_LARGE,
    FRICTION_GROUP_NOT_FOUND,
];

/// The closed state set of `FrictionMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FrictionState {
    /// An agent filed a report.
    Reported,
    /// The group was looked at.
    Triaged,
    /// A recurring failure pattern explains it.
    RootCaused,
    /// A revision fixes it.
    Fixed,
    /// Held-out evidence measured the fix.
    Evaluated,
    /// A human approved the fix.
    Approved,
    /// Two measure windows show the effect.
    Measured,
    /// A regression verdict cleared it.
    RegressionChecked,
    /// The group is done.
    Closed,
    /// The group was turned down before approval.
    Rejected,
}

/// The closed event set of `FrictionMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrictionEvent {
    /// `Reported` to `Triaged`.
    Triage,
    /// `Triaged` to `RootCaused`.
    RootCause,
    /// `RootCaused` to `Fixed`.
    Fix,
    /// `Fixed` to `Evaluated`.
    Evaluate,
    /// `Evaluated` to `Approved`.
    Approve,
    /// `Approved` to `Measured`.
    Measure,
    /// `Measured` to `RegressionChecked`.
    CheckRegression,
    /// `RegressionChecked` to `Closed`.
    Close,
    /// Any state before `Approved` to `Rejected`.
    Reject,
}

impl FrictionState {
    /// Every state, in declaration order.
    pub const ALL: [FrictionState; 10] = [
        Self::Reported,
        Self::Triaged,
        Self::RootCaused,
        Self::Fixed,
        Self::Evaluated,
        Self::Approved,
        Self::Measured,
        Self::RegressionChecked,
        Self::Closed,
        Self::Rejected,
    ];

    /// The one transition function of `FrictionMachine`.
    pub fn apply(self, event: FrictionEvent) -> Result<Self, TransitionRefused> {
        transition(&FRICTION_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// The state whose stored text is `text`.
    pub fn from_stored(text: &str) -> Option<Self> {
        FRICTION_MACHINE.state_from_stored(text)
    }

    /// Whether the group has ended.
    pub fn is_terminal(self) -> bool {
        FRICTION_MACHINE.is_terminal(self)
    }

    /// The event that moves a group from its state to `to`, when the machine has one.
    pub fn event_to(self, to: FrictionState) -> Option<FrictionEvent> {
        FRICTION_MACHINE
            .edges_from(self)
            .find(|edge| edge.to == to)
            .map(|edge| edge.event)
    }
}

impl MachineState for FrictionState {
    fn stored(self) -> &'static str {
        match self {
            Self::Reported => "reported",
            Self::Triaged => "triaged",
            Self::RootCaused => "root_caused",
            Self::Fixed => "fixed",
            Self::Evaluated => "evaluated",
            Self::Approved => "approved",
            Self::Measured => "measured",
            Self::RegressionChecked => "regression_checked",
            Self::Closed => "closed",
            Self::Rejected => "rejected",
        }
    }
}

impl MachineEvent for FrictionEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Triage => "Triage",
            Self::RootCause => "RootCause",
            Self::Fix => "Fix",
            Self::Evaluate => "Evaluate",
            Self::Approve => "Approve",
            Self::Measure => "Measure",
            Self::CheckRegression => "CheckRegression",
            Self::Close => "Close",
            Self::Reject => "Reject",
        }
    }
}

const fn edge(
    id: &'static str,
    from: FrictionState,
    event: FrictionEvent,
    to: FrictionState,
) -> Edge<FrictionState, FrictionEvent, &'static str> {
    Edge {
        id,
        from,
        event,
        guard: None,
        to,
    }
}

/// `FrictionMachine`: the closed edge table. Every other pair is refused.
pub static FRICTION_MACHINE: MachineDef<FrictionState, FrictionEvent, &'static str> = MachineDef {
    name: "FrictionMachine",
    states: &FrictionState::ALL,
    initial: FrictionState::Reported,
    terminals: &[FrictionState::Closed, FrictionState::Rejected],
    events: &[
        FrictionEvent::Triage,
        FrictionEvent::RootCause,
        FrictionEvent::Fix,
        FrictionEvent::Evaluate,
        FrictionEvent::Approve,
        FrictionEvent::Measure,
        FrictionEvent::CheckRegression,
        FrictionEvent::Close,
        FrictionEvent::Reject,
    ],
    guards: &[],
    edges: &[
        edge("FR01", FrictionState::Reported, FrictionEvent::Triage, FrictionState::Triaged),
        edge("FR02", FrictionState::Triaged, FrictionEvent::RootCause, FrictionState::RootCaused),
        edge("FR03", FrictionState::RootCaused, FrictionEvent::Fix, FrictionState::Fixed),
        edge("FR04", FrictionState::Fixed, FrictionEvent::Evaluate, FrictionState::Evaluated),
        edge("FR05", FrictionState::Evaluated, FrictionEvent::Approve, FrictionState::Approved),
        edge("FR06", FrictionState::Approved, FrictionEvent::Measure, FrictionState::Measured),
        edge(
            "FR07",
            FrictionState::Measured,
            FrictionEvent::CheckRegression,
            FrictionState::RegressionChecked,
        ),
        edge("FR08", FrictionState::RegressionChecked, FrictionEvent::Close, FrictionState::Closed),
        edge("FR09", FrictionState::Reported, FrictionEvent::Reject, FrictionState::Rejected),
        edge("FR10", FrictionState::Triaged, FrictionEvent::Reject, FrictionState::Rejected),
        edge("FR11", FrictionState::RootCaused, FrictionEvent::Reject, FrictionState::Rejected),
        edge("FR12", FrictionState::Fixed, FrictionEvent::Reject, FrictionState::Rejected),
        edge("FR13", FrictionState::Evaluated, FrictionEvent::Reject, FrictionState::Rejected),
    ],
};

/// What the evidence of a move must be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceKind {
    /// No evidence is required.
    None,
    /// A `FailurePattern` gap id of R701.
    FailurePattern,
    /// A revision of R805.
    Revision,
    /// A held-out evidence receipt of R805.9.
    HeldOutReceipt,
    /// The human principal that approved.
    HumanPrincipal,
    /// Two measure windows of R713, comma separated.
    MeasureWindows,
    /// A regression verdict of R805.2.
    RegressionVerdict,
}

/// The evidence the move into `to` requires.
pub fn required_evidence(to: FrictionState) -> EvidenceKind {
    match to {
        FrictionState::RootCaused => EvidenceKind::FailurePattern,
        FrictionState::Fixed => EvidenceKind::Revision,
        FrictionState::Evaluated => EvidenceKind::HeldOutReceipt,
        FrictionState::Approved => EvidenceKind::HumanPrincipal,
        FrictionState::Measured => EvidenceKind::MeasureWindows,
        FrictionState::RegressionChecked => EvidenceKind::RegressionVerdict,
        _ => EvidenceKind::None,
    }
}

/// One report an agent filed about a skill.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FrictionReport {
    /// The report id; a repeat of the same id is not counted twice.
    pub report_id: String,
    /// The tenant: the engine overwrites it with the verified tenant.
    pub tenant_id: String,
    /// The skill revision: `skill:<name>@<revision>`.
    pub skill_ref: String,
    /// The version of the skill the agent used.
    pub skill_version: String,
    /// What the agent tried, at most 2 KiB.
    pub tried: String,
    /// What it expected, at most 2 KiB.
    pub expected: String,
    /// What happened, at most 2 KiB.
    pub happened: String,
    /// The fix the agent suggests, at most 2 KiB.
    #[serde(default)]
    pub proposed_fix: Option<String>,
    /// The run in which the agent saw it.
    pub run_id: String,
    /// The principal: the engine overwrites it with the verified principal.
    pub reported_by: String,
    /// When the agent filed it, Unix milliseconds.
    pub at_ms: u64,
}

impl FrictionReport {
    /// The text field that is over its bound, when there is one.
    pub fn oversized_field(&self) -> Option<&'static str> {
        [
            ("tried", Some(&self.tried)),
            ("expected", Some(&self.expected)),
            ("happened", Some(&self.happened)),
            ("proposed_fix", self.proposed_fix.as_ref()),
        ]
        .into_iter()
        .find(|(_, text)| text.is_some_and(|text| text.len() > FRICTION_TEXT_MAX_BYTES))
        .map(|(name, _)| name)
    }
}

/// The reports of one skill, as one group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FrictionGroup {
    /// The group reference: `friction:<skill_ref>#<generation>`.
    pub group_ref: String,
    /// The skill revision.
    pub skill_ref: String,
    /// How many distinct reports the group holds.
    pub report_count: u64,
    /// How many distinct runs they came from.
    pub distinct_runs: u64,
    /// When the first report was filed.
    pub first_ms: u64,
    /// When the last report was filed.
    pub last_ms: u64,
    /// The machine state.
    pub state: FrictionState,
}

/// One requested move of a group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FrictionTransition {
    /// The group.
    pub group_ref: String,
    /// The state to move to.
    pub to: FrictionState,
    /// The evidence of the move.
    #[serde(default)]
    pub evidence_ref: Option<String>,
}

/// The body of `Method::FrictionReport`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "op", deny_unknown_fields)]
pub enum FrictionOp {
    /// File one report. Scope `knowledge:flag`.
    Report {
        /// The graph that holds the skills, the runs and the groups.
        graph: String,
        /// The report.
        report: FrictionReport,
    },
    /// Move one group. Scope `knowledge:curate`; into `Approved`, scope `knowledge:approve`.
    Transition {
        /// The graph that holds the group.
        graph: String,
        /// The move.
        transition: FrictionTransition,
        /// When the move happened, Unix milliseconds.
        at_ms: u64,
    },
    /// List groups by report count, most first.
    Group {
        /// The graph that holds the groups.
        graph: String,
        /// Only the groups of this skill.
        #[serde(default)]
        skill_ref: Option<String>,
        /// Most groups returned, at most 200.
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Read one group with its reports and steps.
    Get {
        /// The graph that holds the group.
        graph: String,
        /// The group.
        group_ref: String,
    },
}

impl FrictionOp {
    /// Whether the op writes: `Report` and `Transition` do, `Group` and `Get` do not.
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Report { .. } | Self::Transition { .. })
    }

    /// The graph the op reads and writes.
    pub fn graph(&self) -> &str {
        match self {
            Self::Report { graph, .. }
            | Self::Transition { graph, .. }
            | Self::Group { graph, .. }
            | Self::Get { graph, .. } => graph,
        }
    }
}

/// What `Report` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FrictionFiled {
    /// The group after the report.
    pub group: FrictionGroup,
    /// Whether the report was new (false for a repeat of the same id).
    pub counted: bool,
    /// The receipt id every node and edge of the report shares.
    pub receipt_id: String,
}

/// One recorded move of a group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FrictionStepRow {
    /// The step node.
    pub step_id: String,
    /// Who moved the group.
    pub actor: String,
    /// The state the group moved to.
    pub to: FrictionState,
    /// The evidence of the move.
    pub evidence_ref: Option<String>,
    /// When it happened.
    pub at_ms: u64,
}

/// What `Transition` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FrictionMoved {
    /// The group after the move.
    pub group: FrictionGroup,
    /// The step the move recorded.
    pub step: FrictionStepRow,
}

/// What `Group` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FrictionGroups {
    /// The groups ordered by `report_count` descending, then `skill_ref`.
    pub groups: Vec<FrictionGroup>,
}

/// What `Get` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FrictionGroupView {
    /// The group.
    pub group: FrictionGroup,
    /// The ids of its reports, in id order.
    pub report_ids: Vec<String>,
    /// Its recorded moves, oldest first.
    pub steps: Vec<FrictionStepRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN_GROUP: &str = r#"{"group_ref":"friction:skill:triage@3#1","skill_ref":"skill:triage@3","report_count":3,"distinct_runs":2,"first_ms":1,"last_ms":9,"state":"root_caused"}"#;

    // spec: MEDULLA-DECISION-ENGINE-R710.1
    #[test]
    fn friction_types_round_trip_with_golden_vector() {
        let group: FrictionGroup = serde_json::from_str(GOLDEN_GROUP).expect("decode");
        assert_eq!(group.state, FrictionState::RootCaused);
        assert_eq!(serde_json::to_string(&group).expect("encode"), GOLDEN_GROUP);
        let op = FrictionOp::Transition {
            graph: "g".into(),
            transition: FrictionTransition {
                group_ref: group.group_ref.clone(),
                to: FrictionState::Fixed,
                evidence_ref: Some("rev:1".into()),
            },
            at_ms: 4,
        };
        let text = serde_json::to_string(&op).expect("encode op");
        assert_eq!(serde_json::from_str::<FrictionOp>(&text).unwrap(), op);
        assert!(op.is_mutation());
        let unknown = GOLDEN_GROUP.replace("\"state\"", "\"x\":1,\"state\"");
        assert!(serde_json::from_str::<FrictionGroup>(&unknown).is_err());
        let mut report = FrictionReport {
            report_id: "r".into(),
            tenant_id: "t".into(),
            skill_ref: "skill:triage@3".into(),
            skill_version: "3".into(),
            tried: "a".into(),
            expected: "b".into(),
            happened: "c".into(),
            proposed_fix: None,
            run_id: "run".into(),
            reported_by: "p".into(),
            at_ms: 1,
        };
        assert_eq!(report.oversized_field(), None);
        report.proposed_fix = Some("x".repeat(FRICTION_TEXT_MAX_BYTES + 1));
        assert_eq!(report.oversized_field(), Some("proposed_fix"));
    }

    // spec: MEDULLA-DECISION-ENGINE-R710.2
    #[test]
    fn friction_machine_allows_only_the_listed_moves() {
        use FrictionState::*;
        let legal = [
            (Reported, Triaged),
            (Triaged, RootCaused),
            (RootCaused, Fixed),
            (Fixed, Evaluated),
            (Evaluated, Approved),
            (Approved, Measured),
            (Measured, RegressionChecked),
            (RegressionChecked, Closed),
            (Reported, Rejected),
            (Triaged, Rejected),
            (RootCaused, Rejected),
            (Fixed, Rejected),
            (Evaluated, Rejected),
        ];
        for from in FrictionState::ALL {
            for to in FrictionState::ALL {
                let moved = from.event_to(to).is_some_and(|event| from.apply(event).is_ok());
                assert_eq!(moved, legal.contains(&(from, to)), "{from:?} -> {to:?}");
            }
        }
        assert!(Closed.is_terminal() && Rejected.is_terminal() && !Approved.is_terminal());
        assert_eq!(required_evidence(Approved), EvidenceKind::HumanPrincipal);
        assert_eq!(required_evidence(Triaged), EvidenceKind::None);
    }
}
