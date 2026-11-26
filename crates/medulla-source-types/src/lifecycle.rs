//! Software-lifecycle records (MEDULLA-SDLC): releases, deployments, defects, the
//! derived requirement-state answer and the lifecycle trace.
//!
//! Pure data and bounds. The derivation rules, the defect intake and the status
//! derivation live in `medulla-core`'s `lifecycle` module; these types are the
//! vocabulary they and the served methods share. Every closed set is a serde
//! enum, so an unknown value is refused at decode.

use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256, the digest form every lifecycle record uses.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The closed edge vocabulary (MEDULLA-SDLC-R001).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LifecycleEdgeKind {
    Changes,
    CitesRequirement,
    Implements,
    VerifiedBy,
    Exercises,
    Judges,
    ProducedBy,
    Affects,
    IntroducedBy,
    FixedBy,
    Contains,
    Deploys,
    ObservedIn,
    Raises,
}

impl LifecycleEdgeKind {
    /// Every edge kind, in declaration order.
    pub const ALL: [Self; 14] = [
        Self::Changes,
        Self::CitesRequirement,
        Self::Implements,
        Self::VerifiedBy,
        Self::Exercises,
        Self::Judges,
        Self::ProducedBy,
        Self::Affects,
        Self::IntroducedBy,
        Self::FixedBy,
        Self::Contains,
        Self::Deploys,
        Self::ObservedIn,
        Self::Raises,
    ];

    /// The wire and ontology name of the edge.
    pub fn name(self) -> &'static str {
        match self {
            Self::Changes => "changes",
            Self::CitesRequirement => "citesRequirement",
            Self::Implements => "implements",
            Self::VerifiedBy => "verifiedBy",
            Self::Exercises => "exercises",
            Self::Judges => "judges",
            Self::ProducedBy => "producedBy",
            Self::Affects => "affects",
            Self::IntroducedBy => "introducedBy",
            Self::FixedBy => "fixedBy",
            Self::Contains => "contains",
            Self::Deploys => "deploys",
            Self::ObservedIn => "observedIn",
            Self::Raises => "raises",
        }
    }

    /// The kinds a named derivation rule emits. A direct write of one of these
    /// refuses: the changeset log and the code graph are their only source.
    pub fn is_derived(self) -> bool {
        matches!(
            self,
            Self::Changes
                | Self::CitesRequirement
                | Self::Implements
                | Self::VerifiedBy
                | Self::Exercises
                | Self::Contains
                | Self::IntroducedBy
                | Self::FixedBy
                | Self::ProducedBy
        )
    }
}

/// The record kinds a lifecycle trace can reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleKind {
    Requirement,
    CodeSymbol,
    TestCase,
    Changeset,
    GateVerdict,
    RunTrace,
    Release,
    Deployment,
    Defect,
    Incident,
}

impl LifecycleKind {
    /// The lifecycle kind a stored node of graph `type` `node_type` stands for. The one
    /// mapping the changeset accept and the served trace share.
    pub fn from_node_type(node_type: &str) -> Option<Self> {
        match node_type {
            "Requirement" => Some(Self::Requirement),
            "CodeSymbolRef" | "SYMBOL" => Some(Self::CodeSymbol),
            "TestCase" | "Test" | "TestFile" => Some(Self::TestCase),
            "Changeset" => Some(Self::Changeset),
            "GateVerdict" => Some(Self::GateVerdict),
            "RunTrace" => Some(Self::RunTrace),
            "Release" => Some(Self::Release),
            "Deployment" => Some(Self::Deployment),
            "Defect" => Some(Self::Defect),
            "Incident" => Some(Self::Incident),
            _ => None,
        }
    }

    /// Parse a wire name; an unknown name carries the stable refusal code.
    pub fn parse(name: &str) -> Result<Self, LifecycleRefusal> {
        serde_json::from_value(serde_json::Value::String(name.to_owned()))
            .map_err(|_| LifecycleRefusal::KindUnknown(name.to_owned()))
    }
}

/// The stable refusal codes of the served release and deployment records
/// (MEDULLA-SDLC-R002.4, R002.5), in the order they are documented.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("RELEASE_HEAD_UNKNOWN"),
    RefusalCode::new("RELEASE_VERSION_TAKEN"),
    RefusalCode::new("RELEASE_SEALED"),
    RefusalCode::new("GOAL_TARGET_REQUIRED"),
    RefusalCode::new("PROPOSAL_KIND_UNKNOWN"),
    RefusalCode::new("MODE_APPROVAL_REQUIRED"),
    RefusalCode::new("PROPOSAL_EVIDENCE_REQUIRED"),
    RefusalCode::new("PROPOSAL_EVIDENCE_UNRESOLVED"),
    RefusalCode::new("PROPOSAL_EVIDENCE_DIGEST_MISMATCH"),
    RefusalCode::new("MODE_REFERENCE_NO_PROPOSALS"),
    RefusalCode::new("MODE_ORIGIN_NOT_ADMITTED"),
    RefusalCode::new("PROPOSAL_NOT_FOUND"),
    RefusalCode::new("PROPOSAL_NOT_OPEN"),
    RefusalCode::new("CHANGESET_NODE_CONFLICT"),
    RefusalCode::new("GATE_INPUT_DIGEST_MISMATCH"),
    RefusalCode::new("GATE_VERDICT_SEALED"),
    RefusalCode::new("LANDING_GATE_BLOCKED"),
    RefusalCode::new("LANDING_POLICY_APPROVAL_REQUIRED"),
    RefusalCode::new("COMPETENCY_SUBJECT_UNKNOWN"),
    RefusalCode::new("DEFECT_TRANSITION_ILLEGAL"),
    RefusalCode::new("DEFECT_UNVERIFIED"),
    RefusalCode::new("LIFECYCLE_DEPTH_INVALID"),
    RefusalCode::new("DELIVERY_SCOPE_EMPTY"),
    RefusalCode::new("SPEC_STATUS_REV_UNKNOWN"),
    RefusalCode::new("SPEC_TREE_ABSENT"),
    RefusalCode::new("SCENARIO_RUN_INCOMPLETE"),
    RefusalCode::new("RESEARCH_ENTRY_UNCITED"),
    RefusalCode::new("PROPOSAL_DIRECT_WRITE_REFUSED"),
    RefusalCode::new("RESEARCH_TOOL_UNKNOWN"),
];

/// A typed refusal from the lifecycle records.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LifecycleRefusal {
    #[error("LIFECYCLE_KIND_UNKNOWN: {0}")]
    KindUnknown(String),
    #[error("LIFECYCLE_DEPTH_INVALID: depth {0} is outside 1 and the trace depth ceiling")]
    TraceDepth(u8),
    #[error("LIFECYCLE_EDGE_DERIVED_ONLY: {0}")]
    EdgeDerivedOnly(&'static str),
    #[error("LIFECYCLE_ENDPOINT_UNKNOWN: {0}")]
    EndpointUnknown(String),
    #[error("RELEASE_HEAD_UNKNOWN: {0}")]
    ReleaseHeadUnknown(String),
    #[error("RELEASE_VERSION_TAKEN: {0}")]
    ReleaseVersionTaken(String),
    #[error("RELEASE_SEALED: {0}")]
    ReleaseSealed(String),
    #[error("GOAL_TARGET_REQUIRED: {0}")]
    GoalTargetRequired(String),
    #[error("PROPOSAL_KIND_UNKNOWN: {0}")]
    ProposalKindUnknown(String),
    #[error("PROPOSAL_EVIDENCE_REQUIRED: {0}")]
    ProposalEvidenceRequired(String),
    #[error("PROPOSAL_EVIDENCE_UNRESOLVED: {0}")]
    ProposalEvidenceUnresolved(String),
    #[error("PROPOSAL_EVIDENCE_DIGEST_MISMATCH: {0}")]
    ProposalEvidenceDigestMismatch(String),
    #[error("MODE_REFERENCE_NO_PROPOSALS: {0}")]
    ModeReferenceNoProposals(String),
    #[error("MODE_ORIGIN_NOT_ADMITTED: {0}")]
    ModeOriginNotAdmitted(String),
    #[error("PROPOSAL_NOT_FOUND: {0}")]
    ProposalNotFound(String),
    #[error("PROPOSAL_NOT_OPEN: {0}")]
    ProposalNotOpen(String),
    #[error("CHANGESET_NODE_CONFLICT: {0}")]
    ChangesetNodeConflict(String),
    #[error("GATE_INPUT_DIGEST_MISMATCH: {0}")]
    GateInputDigestMismatch(String),
    #[error("GATE_VERDICT_SEALED: {0}")]
    GateVerdictSealed(String),
    #[error("LANDING_GATE_BLOCKED: {0}")]
    LandingGateBlocked(String),
    #[error("LANDING_POLICY_APPROVAL_REQUIRED: {0}")]
    LandingPolicyApprovalRequired(String),
    #[error("COMPETENCY_SUBJECT_UNKNOWN: {0}")]
    CompetencySubjectUnknown(String),
    #[error("DEFECT_TRANSITION_ILLEGAL: {0}")]
    DefectTransitionIllegal(String),
    #[error("DEFECT_UNVERIFIED: {0}")]
    DefectUnverified(String),
    #[error("PARAMETER_INVALID: {0}")]
    ParameterInvalid(String),
    #[error("INVALID_ARGUMENT: {0}")]
    InvalidArgument(String),
    #[error("MODE_APPROVAL_REQUIRED: {0}")]
    ModeApprovalRequired(String),
    #[error("DELIVERY_SCOPE_EMPTY: {0}")]
    DeliveryScopeEmpty(String),
    #[error("SPEC_STATUS_REV_UNKNOWN: {0}")]
    SpecStatusRevUnknown(String),
    #[error("SPEC_TREE_ABSENT: {0}")]
    SpecTreeAbsent(String),
    #[error("SCENARIO_RUN_INCOMPLETE: {0}")]
    ScenarioRunIncomplete(String),
    #[error("RESEARCH_ENTRY_UNCITED: {0}")]
    ResearchEntryUncited(String),
    #[error("PROPOSAL_DIRECT_WRITE_REFUSED: {0}")]
    ProposalDirectWriteRefused(String),
    #[error("RESEARCH_TOOL_UNKNOWN: {0}")]
    ResearchToolUnknown(String),
}

impl LifecycleRefusal {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::KindUnknown(_) => "LIFECYCLE_KIND_UNKNOWN",
            Self::TraceDepth(_) => "LIFECYCLE_DEPTH_INVALID",
            Self::EdgeDerivedOnly(_) => "LIFECYCLE_EDGE_DERIVED_ONLY",
            Self::EndpointUnknown(_) => "LIFECYCLE_ENDPOINT_UNKNOWN",
            Self::ReleaseHeadUnknown(_) => "RELEASE_HEAD_UNKNOWN",
            Self::ReleaseVersionTaken(_) => "RELEASE_VERSION_TAKEN",
            Self::ReleaseSealed(_) => "RELEASE_SEALED",
            Self::GoalTargetRequired(_) => "GOAL_TARGET_REQUIRED",
            Self::ProposalKindUnknown(_) => "PROPOSAL_KIND_UNKNOWN",
            Self::ProposalEvidenceRequired(_) => "PROPOSAL_EVIDENCE_REQUIRED",
            Self::ProposalEvidenceUnresolved(_) => "PROPOSAL_EVIDENCE_UNRESOLVED",
            Self::ProposalEvidenceDigestMismatch(_) => "PROPOSAL_EVIDENCE_DIGEST_MISMATCH",
            Self::ModeReferenceNoProposals(_) => "MODE_REFERENCE_NO_PROPOSALS",
            Self::ModeOriginNotAdmitted(_) => "MODE_ORIGIN_NOT_ADMITTED",
            Self::ProposalNotFound(_) => "PROPOSAL_NOT_FOUND",
            Self::ProposalNotOpen(_) => "PROPOSAL_NOT_OPEN",
            Self::ChangesetNodeConflict(_) => "CHANGESET_NODE_CONFLICT",
            Self::GateInputDigestMismatch(_) => "GATE_INPUT_DIGEST_MISMATCH",
            Self::GateVerdictSealed(_) => "GATE_VERDICT_SEALED",
            Self::LandingGateBlocked(_) => "LANDING_GATE_BLOCKED",
            Self::LandingPolicyApprovalRequired(_) => "LANDING_POLICY_APPROVAL_REQUIRED",
            Self::CompetencySubjectUnknown(_) => "COMPETENCY_SUBJECT_UNKNOWN",
            Self::DefectTransitionIllegal(_) => "DEFECT_TRANSITION_ILLEGAL",
            Self::DefectUnverified(_) => "DEFECT_UNVERIFIED",
            Self::ParameterInvalid(_) => "PARAMETER_INVALID",
            Self::InvalidArgument(_) => "INVALID_ARGUMENT",
            Self::ModeApprovalRequired(_) => "MODE_APPROVAL_REQUIRED",
            Self::DeliveryScopeEmpty(_) => "DELIVERY_SCOPE_EMPTY",
            Self::SpecStatusRevUnknown(_) => "SPEC_STATUS_REV_UNKNOWN",
            Self::SpecTreeAbsent(_) => "SPEC_TREE_ABSENT",
            Self::ScenarioRunIncomplete(_) => "SCENARIO_RUN_INCOMPLETE",
            Self::ResearchEntryUncited(_) => "RESEARCH_ENTRY_UNCITED",
            Self::ProposalDirectWriteRefused(_) => "PROPOSAL_DIRECT_WRITE_REFUSED",
            Self::ResearchToolUnknown(_) => "RESEARCH_TOOL_UNKNOWN",
        }
    }
}

/// A published release of one repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub repo: String,
    pub version: String,
    pub git_ref: String,
    pub changeset_head: String,
    pub artifact_digests: Vec<String>,
    pub published_at: u64,
}

impl Release {
    /// The stable record id a deployment links to.
    pub fn id(&self) -> String {
        format!("{}@{}", self.repo, self.version)
    }

    fn names_digest(&self, digest: &str) -> bool {
        self.artifact_digests.iter().any(|d| d == digest)
    }
}

/// How a deployment ended. `Unattributed` means no release names its image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DeploymentOutcome {
    Succeeded,
    Failed,
    RolledBack,
    Unattributed,
}

/// One rollout of an image to an environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deployment {
    pub environment: String,
    pub workload: String,
    pub image_digest: String,
    pub release_ref: Option<String>,
    pub started_at: u64,
    pub finished_at: u64,
    pub outcome: DeploymentOutcome,
}

/// The caller's view of a deployment, before the engine resolves its release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DeploymentInput {
    pub environment: String,
    pub workload: String,
    pub image_digest: String,
    pub started_at: u64,
    pub finished_at: u64,
    pub outcome: DeploymentOutcome,
}

/// Resolve a deployment to its release by image digest. A digest no release
/// names stays `unattributed` with no release; the engine never guesses one.
pub fn record_deployment(releases: &[Release], input: DeploymentInput) -> Deployment {
    let release = releases
        .iter()
        .find(|release| release.names_digest(&input.image_digest));
    let (release_ref, outcome) = match release {
        Some(release) => (Some(release.id()), input.outcome),
        None => (None, DeploymentOutcome::Unattributed),
    };
    Deployment {
        environment: input.environment,
        workload: input.workload,
        image_digest: input.image_digest,
        release_ref,
        started_at: input.started_at,
        finished_at: input.finished_at,
        outcome,
    }
}

/// The changesets a release contains: those after the previous release head up
/// to and including its own head, on the repository chain (oldest first).
pub fn release_range(
    chain: &[String],
    previous_head: Option<&str>,
    head: &str,
) -> Result<Vec<String>, LifecycleRefusal> {
    let position = |id: &str| chain.iter().position(|c| c == id);
    let end =
        position(head).ok_or_else(|| LifecycleRefusal::ReleaseHeadUnknown(head.to_owned()))?;
    let start = match previous_head {
        Some(previous) => position(previous)
            .map(|index| index + 1)
            .ok_or_else(|| LifecycleRefusal::ReleaseHeadUnknown(previous.to_owned()))?,
        None => 0,
    };
    Ok(chain[start.min(end + 1)..=end].to_vec())
}

/// Everything the served `RecordRelease` commits for one release
/// (MEDULLA-SDLC-R002.3): the sealed release record and the changesets it
/// contains, oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseWrite {
    pub release: Release,
    /// The seal over the release record (see [`release_seal`]).
    pub seal: String,
    /// The changeset ids the release contains (`contains.release_range`).
    pub contains: Vec<String>,
    /// True when the same record was already stored, so the write replays.
    pub replayed: bool,
}

/// The seal of a release record: the SHA-256 of its canonical JSON. A readback
/// re-derives it, so a stored record that differs from what was written shows.
pub fn release_seal(release: &Release) -> String {
    sha256_hex(
        serde_json::to_string(release)
            .unwrap_or_default()
            .as_bytes(),
    )
}

/// The graph node id of a release. `@` is refused in node ids by the
/// persistence privacy policy, so it differs from [`Release::id`], which
/// deployments carry as their `release_ref`.
pub fn release_node_id(release: &Release) -> String {
    format!("release:{}:{}", release.repo, release.version)
}

/// The graph node id of a deployment: a digest over what makes it distinct.
pub fn deployment_node_id(deployment: &Deployment) -> String {
    let key = format!(
        "{}|{}|{}|{}",
        deployment.environment, deployment.workload, deployment.image_digest, deployment.started_at
    );
    format!("deployment:{}", &sha256_hex(key.as_bytes())[..32])
}

/// `RecordRelease` (MEDULLA-SDLC-R002.3 to R002.5): decide what one release
/// writes, or refuse before anything is written.
///
/// `existing` is every stored release, `chain` the repository's changeset
/// chain ending at `release.changeset_head` or absent from it (oldest first).
/// A head that is not on the chain refuses `RELEASE_HEAD_UNKNOWN`. A stored
/// release with the same repository and version refuses
/// `RELEASE_VERSION_TAKEN` unless it is byte-identical, which replays. The
/// previous release of the repository whose head is on the chain bounds the
/// range, so the new release contains only the changesets after it.
pub fn handle_record_release(
    existing: &[Release],
    chain: &[String],
    release: Release,
) -> Result<ReleaseWrite, LifecycleRefusal> {
    let mut replayed = false;
    if let Some(stored) = existing.iter().find(|r| r.id() == release.id()) {
        if *stored != release {
            return Err(LifecycleRefusal::ReleaseVersionTaken(release.id()));
        }
        replayed = true;
    }
    let previous_head = existing
        .iter()
        .filter(|r| r.repo == release.repo && r.id() != release.id())
        .filter(|r| chain.contains(&r.changeset_head))
        .max_by_key(|r| r.published_at)
        .map(|r| r.changeset_head.as_str());
    let contains = release_range(chain, previous_head, &release.changeset_head)?;
    Ok(ReleaseWrite {
        seal: release_seal(&release),
        release,
        contains,
        replayed,
    })
}

/// `RecordDeployment` (MEDULLA-SDLC-R002.3): resolve the deployment to its
/// release by image digest. Reuses [`record_deployment`]; there is no second
/// digest lookup.
pub fn handle_record_deployment(releases: &[Release], input: DeploymentInput) -> Deployment {
    record_deployment(releases, input)
}

/// What `RecordRelease` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReleaseReceipt {
    pub release_id: String,
    pub node_id: String,
    pub seal: String,
    pub contains: u64,
    pub replayed: bool,
}

/// What `RecordDeployment` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DeploymentReceipt {
    pub deployment_id: String,
    pub release_ref: Option<String>,
    pub outcome: DeploymentOutcome,
}

/// What a goal aims at: a requirement row of a repository or a competency question.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalTarget {
    /// A requirement row, by owner repository and row id.
    Row { repo: String, id: String },
    /// A competency question, by id.
    CompetencyQuestion { id: String },
}

impl GoalTarget {
    /// The graph node the `targetsGoal` edge starts from. A row target is the
    /// requirement node of the spec projection (`requirement:<repo>:<id>`).
    pub fn node_id(&self) -> String {
        match self {
            Self::Row { repo, id } => format!("requirement:{repo}:{id}"),
            Self::CompetencyQuestion { id } => format!("competency_question:{id}"),
        }
    }
}

/// A goal of one project (MEDULLA-SDLC-R017.1): a statement and the rows or
/// competency questions that target it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Goal {
    pub project: String,
    pub statement: String,
    pub targets: Vec<GoalTarget>,
}

/// What `GoalPut` writes: the goal, its node id and its distinct targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalWrite {
    pub goal: Goal,
    pub node_id: String,
    /// The distinct targets, in order.
    pub targets: Vec<GoalTarget>,
}

/// What `GoalPut` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GoalReceipt {
    pub node_id: String,
    pub targets: u64,
}

/// The graph node id of a goal: the project and a digest of its statement.
pub fn goal_node_id(goal: &Goal) -> String {
    let key = format!("{}|{}", goal.project, goal.statement);
    format!("goal:{}", &sha256_hex(key.as_bytes())[..32])
}

/// `GoalPut` (MEDULLA-SDLC-R017.1): decide what one goal writes, or refuse
/// before anything is written. A goal that names no target refuses
/// `GOAL_TARGET_REQUIRED`; a repeated target counts once.
pub fn handle_goal_put(goal: Goal) -> Result<GoalWrite, LifecycleRefusal> {
    let mut targets: Vec<GoalTarget> = Vec::with_capacity(goal.targets.len());
    for target in &goal.targets {
        if !targets.contains(target) {
            targets.push(target.clone());
        }
    }
    if targets.is_empty() {
        return Err(LifecycleRefusal::GoalTargetRequired(goal.project.clone()));
    }
    Ok(GoalWrite {
        node_id: goal_node_id(&goal),
        goal,
        targets,
    })
}

/// How a codebase takes part in the learning loop (MEDULLA-SDLC-R012.8). A
/// project with no record reads as `reference`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProjectMode {
    /// Queryable and used for research; it never receives a proposal.
    #[default]
    Reference,
    /// Receives proposals for review.
    Development,
    /// Receives proposals unprompted toward its goals.
    Evolving,
}

impl ProjectMode {
    /// The snake case wire name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Reference => "reference",
            Self::Development => "development",
            Self::Evolving => "evolving",
        }
    }
}

/// The operator approval behind a move to `evolving`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProjectModeApproval {
    pub approver: String,
    pub approved_at_ms: u64,
    pub reason: String,
}

/// `ProjectModeSet` request: set the mode of one project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProjectModeSet {
    pub project: String,
    pub mode: ProjectMode,
    #[serde(default)]
    pub approval: Option<ProjectModeApproval>,
}

/// What `ProjectModeSet` answers; `previous` is `Reference` when no record existed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProjectModeReceipt {
    pub node_id: String,
    pub mode: ProjectMode,
    pub previous: ProjectMode,
}

/// What `ProjectModeSet` writes (not on the wire).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectModeWrite {
    pub request: ProjectModeSet,
    pub node_id: String,
    pub previous: ProjectMode,
}

/// The graph node id of a project's mode record: the first 32 hex digits of the
/// SHA-256 of the project name.
pub fn project_mode_node_id(project: &str) -> String {
    format!("projectmode:{}", &sha256_hex(project.as_bytes())[..32])
}

/// `ProjectModeGet` request (MEDULLA-SDLC-R012.9): read the mode of one project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProjectModeGetRequest {
    pub project: String,
}

/// What `ProjectModeGet` answers. A project with no record of the verified
/// tenant answers `mode: reference`, `recorded: false` and no approval, the same
/// answer for an absent record and for a record the caller may not see.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProjectModeRecord {
    pub node_id: String,
    pub project: String,
    pub mode: ProjectMode,
    pub recorded: bool,
    #[serde(default)]
    pub approver: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub approved_at_ms: Option<u64>,
}

/// `ProjectModeGet` (MEDULLA-SDLC-R012.9): check the bounds of the request. A
/// blank project refuses `INVALID_ARGUMENT`.
pub fn handle_project_mode_get(
    request: ProjectModeGetRequest,
) -> Result<ProjectModeGetRequest, LifecycleRefusal> {
    if request.project.trim().is_empty() {
        return Err(LifecycleRefusal::InvalidArgument(
            "a project mode read names its project".to_owned(),
        ));
    }
    Ok(request)
}

/// The answer of `ProjectModeGet` from the stored `ProjectMode` node properties
/// (`None` when no record of the verified tenant is visible).
pub fn project_mode_record(
    project: &str,
    stored: Option<&serde_json::Value>,
) -> ProjectModeRecord {
    let node_id = project_mode_node_id(project);
    let Some(node) = stored else {
        return ProjectModeRecord {
            node_id,
            project: project.to_owned(),
            mode: ProjectMode::default(),
            recorded: false,
            approver: None,
            reason: None,
            approved_at_ms: None,
        };
    };
    let text = |key: &str| {
        node.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    ProjectModeRecord {
        node_id,
        project: project.to_owned(),
        mode: node
            .get("mode")
            .and_then(serde_json::Value::as_str)
            .and_then(|name| {
                serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
            })
            .unwrap_or_default(),
        recorded: true,
        approver: text("approver"),
        reason: text("reason"),
        approved_at_ms: node
            .get("approved_at_ms")
            .and_then(serde_json::Value::as_u64),
    }
}

/// `ProjectModeSet` (MEDULLA-SDLC-R012.8): decide what one mode change writes,
/// or refuse before anything is written. A blank project refuses
/// `INVALID_ARGUMENT`; a move to `evolving` while `evolving_requires_approval`
/// needs an approval with a named approver, a reason and a time above zero.
pub fn handle_project_mode_set(
    request: ProjectModeSet,
    previous: ProjectMode,
    evolving_requires_approval: bool,
) -> Result<ProjectModeWrite, LifecycleRefusal> {
    if request.project.trim().is_empty() {
        return Err(LifecycleRefusal::InvalidArgument(
            "a project mode names its project".to_owned(),
        ));
    }
    if request.mode == ProjectMode::Evolving
        && previous != ProjectMode::Evolving
        && evolving_requires_approval
    {
        let approved = request.approval.as_ref().is_some_and(|approval| {
            !approval.approver.trim().is_empty()
                && !approval.reason.trim().is_empty()
                && approval.approved_at_ms > 0
        });
        if !approved {
            return Err(LifecycleRefusal::ModeApprovalRequired(
                request.project.clone(),
            ));
        }
    }
    Ok(ProjectModeWrite {
        node_id: project_mode_node_id(&request.project),
        request,
        previous,
    })
}

/// The closed kind of a proposal (MEDULLA-SDLC-R017.2). Row kinds change one
/// requirement row, the plan kind edits a plan section, task kinds change the
/// task list and the other kinds add a contract, resolve a clarification marker
/// or tune a knob.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    /// Add a requirement row.
    RowAdd,
    /// Update a requirement row.
    RowUpdate,
    /// Split a requirement row.
    RowSplit,
    /// Prune a requirement row.
    RowPrune,
    /// Retire a requirement row.
    RowRetire,
    /// Edit a plan section.
    EditPlanSection,
    /// Add a task.
    TaskAdd,
    /// Reorder tasks.
    TaskReorder,
    /// Split a task.
    TaskSplit,
    /// Add a contract.
    AddContract,
    /// Resolve a `[NEEDS CLARIFICATION]` marker.
    ResolveClarification,
    /// Tune a knob.
    TuneKnob,
}

impl ProposalKind {
    /// Every kind, in documented order.
    pub const ALL: [ProposalKind; 12] = [
        Self::RowAdd,
        Self::RowUpdate,
        Self::RowSplit,
        Self::RowPrune,
        Self::RowRetire,
        Self::EditPlanSection,
        Self::TaskAdd,
        Self::TaskReorder,
        Self::TaskSplit,
        Self::AddContract,
        Self::ResolveClarification,
        Self::TuneKnob,
    ];

    /// Parse a wire name; a name outside the closed set refuses
    /// `PROPOSAL_KIND_UNKNOWN`.
    pub fn parse(name: &str) -> Result<Self, LifecycleRefusal> {
        serde_json::from_value(serde_json::Value::String(name.to_owned()))
            .map_err(|_| LifecycleRefusal::ProposalKindUnknown(name.to_owned()))
    }
}

/// A change the learning loop proposes to the specs of one project
/// (MEDULLA-SDLC-R017.2): one closed kind, the record it targets and a
/// statement. A proposal that resolves a `[NEEDS CLARIFICATION]` marker names
/// the marker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub project: String,
    pub kind: ProposalKind,
    /// The row, plan section, task or knob the proposal changes.
    pub target: String,
    pub statement: String,
    /// The marker a `resolve_clarification` proposal resolves.
    #[serde(default)]
    pub marker: Option<String>,
    /// The code-graph facts, research citations and gate results behind the
    /// proposal (MEDULLA-SDLC-R017.3), at most [`MAX_PROPOSAL_EVIDENCE`].
    #[serde(default)]
    pub evidence: Vec<ProposalEvidence>,
    /// Whether the proposal was asked for or raised unprompted.
    #[serde(default)]
    pub origin: ProposalOrigin,
}

/// The most evidence entries one proposal carries.
pub const MAX_PROPOSAL_EVIDENCE: usize = 64;

/// What a piece of proposal evidence is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    CodeGraphFact,
    ResearchCitation,
    GateResult,
}

/// One cited record: the id of a graph node and the content digest it had.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProposalEvidence {
    pub kind: EvidenceKind,
    pub record_id: String,
    pub digest: String,
}

/// Whether a proposal was asked for or raised unprompted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProposalOrigin {
    #[default]
    Requested,
    Unprompted,
}

impl ProposalOrigin {
    /// The snake case wire name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Unprompted => "unprompted",
        }
    }
}

/// What `ProposalPut` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProposalReceipt {
    pub node_id: String,
    pub evidence: u64,
}

/// What `ProposalPut` writes (not on the wire).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalWrite {
    pub proposal: Proposal,
    pub node_id: String,
}

/// The graph node id of a proposal: the first 32 hex digits of the SHA-256 of
/// `<project>|<kind>|<target>|<statement>`, so a repeat is one node.
pub fn proposal_node_id(proposal: &Proposal) -> String {
    let kind = serde_json::to_value(proposal.kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    let key = format!(
        "{}|{}|{}|{}",
        proposal.project, kind, proposal.target, proposal.statement
    );
    format!("proposal:{}", &sha256_hex(key.as_bytes())[..32])
}

/// `ProposalPut` (MEDULLA-SDLC-R017.3): decide what one proposal writes, or
/// refuse before anything is written. `resolved` holds `(record_id, content
/// digest)` for each cited record the router found visible. In order: a blank
/// project or statement, or more than [`MAX_PROPOSAL_EVIDENCE`] entries, refuses
/// `INVALID_ARGUMENT`; no evidence refuses `PROPOSAL_EVIDENCE_REQUIRED`; a cited
/// record that was not found refuses `PROPOSAL_EVIDENCE_UNRESOLVED`; one whose
/// digest differs refuses `PROPOSAL_EVIDENCE_DIGEST_MISMATCH`.
pub fn handle_proposal_put(
    proposal: Proposal,
    resolved: &[(String, String)],
) -> Result<ProposalWrite, LifecycleRefusal> {
    if proposal.project.trim().is_empty() || proposal.statement.trim().is_empty() {
        return Err(LifecycleRefusal::InvalidArgument(
            "a proposal names its project and a statement".to_owned(),
        ));
    }
    // MEDULLA-SDLC-R017.4: a proposal names the row, section, task or knob it changes. One that
    // names a spec file asks for a direct write of that file, which only a changeset may make.
    if is_spec_file_path(&proposal.target) {
        return Err(LifecycleRefusal::ProposalDirectWriteRefused(format!(
            "{}: a proposal names a row, section, task or knob, never a spec file",
            proposal.target
        )));
    }
    if proposal.evidence.len() > MAX_PROPOSAL_EVIDENCE {
        return Err(LifecycleRefusal::InvalidArgument(format!(
            "a proposal cites at most {MAX_PROPOSAL_EVIDENCE} records"
        )));
    }
    if proposal.evidence.is_empty() {
        return Err(LifecycleRefusal::ProposalEvidenceRequired(
            proposal.project.clone(),
        ));
    }
    for cited in &proposal.evidence {
        match resolved.iter().find(|(id, _)| *id == cited.record_id) {
            None => {
                return Err(LifecycleRefusal::ProposalEvidenceUnresolved(
                    cited.record_id.clone(),
                ))
            }
            Some((_, digest)) if *digest != cited.digest => {
                return Err(LifecycleRefusal::ProposalEvidenceDigestMismatch(
                    cited.record_id.clone(),
                ))
            }
            Some(_) => {}
        }
    }
    Ok(ProposalWrite {
        node_id: proposal_node_id(&proposal),
        proposal,
    })
}

/// The path prefix of a repository's spec tree (MEDULLA-SDLC-R017.4).
pub const SPEC_TREE_PREFIX: &str = "specs/";

/// Whether `path` is a file of the spec tree: a spec, plan or task file lives under `specs/`.
pub fn is_spec_file_path(path: &str) -> bool {
    path.trim_start_matches("./").starts_with(SPEC_TREE_PREFIX)
}

/// The proposal ids a commit message names in its `Proposal:` trailers, in order. Only the last
/// paragraph is read, as for the `Spec:` trailers; a blank id is dropped.
pub fn proposal_trailers(message: &str) -> Vec<String> {
    let Some(block) = message.trim_end().rsplit("\n\n").next() else {
        return Vec::new();
    };
    block
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(key, _)| key.trim() == "Proposal")
        .flat_map(|(_, value)| value.split(','))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Where a proposal stands in review (MEDULLA-SDLC-R017.6). A proposal is
/// written `open`; a reviewer moves it to `approved` or `rejected`; it reaches
/// `landed` only after its pull request merges (MEDULLA-SDLC-R017.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProposalState {
    Open,
    Approved,
    Rejected,
    Landed,
}

impl ProposalState {
    /// The snake case wire name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Landed => "landed",
        }
    }

    /// The value of the `status` property of a stored `Proposal` node. An open
    /// proposal is stored `proposed` (MEDULLA-SDLC-R017.3); the others keep their
    /// wire name.
    pub fn stored(self) -> &'static str {
        match self {
            Self::Open => "proposed",
            other => other.name(),
        }
    }

    /// Parse a stored `status` value; `None` for a value this record never wrote.
    pub fn from_stored(stored: &str) -> Option<Self> {
        match stored {
            "proposed" => Some(Self::Open),
            other => serde_json::from_value(serde_json::Value::String(other.to_owned()))
                .ok()
                .filter(|state| *state != Self::Open),
        }
    }
}

/// `ProposalList` request (MEDULLA-SDLC-R017.6): the proposals of one project,
/// optionally narrowed to one state and one kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProposalListRequest {
    /// The project (repository) the proposals name.
    pub repo: String,
    #[serde(default)]
    pub state: Option<ProposalState>,
    #[serde(default)]
    pub kind: Option<ProposalKind>,
}

/// One entry of the `ProposalList` answer: what a reviewer needs to decide.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProposalListEntry {
    pub id: String,
    pub kind: ProposalKind,
    pub target: String,
    pub evidence: Vec<ProposalEvidence>,
    pub state: ProposalState,
}

/// A reviewer's decision on one open proposal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProposalDecision {
    Approve,
    Reject,
}

impl ProposalDecision {
    /// The state this decision moves an open proposal to.
    pub fn state(self) -> ProposalState {
        match self {
            Self::Approve => ProposalState::Approved,
            Self::Reject => ProposalState::Rejected,
        }
    }
}

/// `ProposalDecide` request (MEDULLA-SDLC-R017.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProposalDecideRequest {
    /// The proposal node id, as `ProposalList` returns it.
    pub id: String,
    pub decision: ProposalDecision,
    pub reviewer: String,
    pub reason: String,
}

/// What `ProposalDecide` answers. `pull_request` stays absent until the
/// approved proposal has a pull request (MEDULLA-SDLC-R017.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProposalDecideReceipt {
    pub id: String,
    pub state: ProposalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<String>,
}

/// `ProposalDecide` (MEDULLA-SDLC-R017.6): the state one decision moves a
/// proposal to. A blank reviewer or reason refuses `INVALID_ARGUMENT`; a
/// proposal that is not `open` refuses `PROPOSAL_NOT_OPEN`.
pub fn handle_proposal_decide(
    current: ProposalState,
    request: &ProposalDecideRequest,
) -> Result<ProposalState, LifecycleRefusal> {
    if request.reviewer.trim().is_empty() || request.reason.trim().is_empty() {
        return Err(LifecycleRefusal::InvalidArgument(
            "a decision names its reviewer and a reason".to_owned(),
        ));
    }
    if current != ProposalState::Open {
        return Err(LifecycleRefusal::ProposalNotOpen(request.id.clone()));
    }
    Ok(request.decision.state())
}

/// How a defect reached the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DefectIntake {
    GateFailure,
    RunFailure,
    Incident,
    ConnectorIssue,
    Direct,
}

/// A defect's place in its life cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DefectStatus {
    New,
    Triaged,
    Fixing,
    Fixed,
    Verified,
    Closed,
    Rejected,
}

/// How serious a defect is judged to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

/// One observation of a defect, with its intake path and source record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Symptom {
    pub intake: DefectIntake,
    pub source_record_id: String,
    pub observed_at: u64,
    pub text: String,
}

/// A code fault, deduplicated by the digest of its normalized symptom.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Defect {
    pub id: String,
    pub repo: String,
    pub title: String,
    pub intake: DefectIntake,
    pub signature_digest: String,
    pub severity: Severity,
    pub status: DefectStatus,
    pub symptoms: Vec<Symptom>,
    pub created_at: u64,
}

/// The graph `type` of a stored defect node.
pub const DEFECT_TYPE: &str = "Defect";
/// The property holding the canonical JSON of the stored [`Defect`], symptoms included.
pub const DEFECT_RECORD_PROPERTY: &str = "defect_record";
/// The longest defect title and symptom text accepted.
pub const MAX_DEFECT_TITLE_BYTES: usize = 512;
/// The longest symptom text accepted.
pub const MAX_SYMPTOM_TEXT_BYTES: usize = 64 * 1024;

/// The node id of a defect: `defect:<repo>/<id>`, unique across the graph.
pub fn defect_node_id(repo: &str, id: &str) -> String {
    format!("defect:{repo}/{id}")
}

/// One filing of a symptom (MEDULLA-SDLC-R003.3): the request of `FileDefect` and the
/// input of the in-process `file_defect`. Each call carries the intake path, the source
/// record id and the time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FileDefectRequest {
    /// The repository the defect belongs to.
    pub repo: String,
    /// How the symptom reached the engine.
    pub intake: DefectIntake,
    /// The title a new defect takes.
    pub title: String,
    /// The severity a new defect takes; `medium` when absent.
    #[serde(default)]
    pub severity: Option<Severity>,
    /// The record the symptom came from (a verdict node id, a run trace id, an incident id).
    pub source_record_id: String,
    /// When the symptom was observed, in milliseconds.
    pub observed_at: u64,
    /// The symptom text; equal normalized texts land on one defect.
    pub text: String,
}

/// What `FileDefect` did with a symptom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DefectFileOutcome {
    /// A new defect was opened.
    Created,
    /// An open defect with the same signature took the symptom.
    Absorbed,
    /// A closed defect with the same signature reopened.
    Reopened,
}

/// What `FileDefect` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FileDefectReceipt {
    /// The defect node id, `defect:<repo>/<id>`.
    pub node_id: String,
    /// The defect id inside its repository.
    pub defect_id: String,
    /// What the filing did.
    pub outcome: DefectFileOutcome,
    /// The defect's status after the filing.
    pub status: DefectStatus,
    /// How many symptoms the defect now lists.
    pub symptoms: u64,
}

/// What one filing writes (not on the wire): the defect as stored after the filing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefectWrite {
    /// The defect with its full symptom list.
    pub defect: Defect,
    /// What the filing did.
    pub outcome: DefectFileOutcome,
}

impl DefectWrite {
    /// The defect node id.
    pub fn node_id(&self) -> String {
        defect_node_id(&self.defect.repo, &self.defect.id)
    }

    /// The flat properties of the one `Defect` node of this filing, without the tenant stamp. The
    /// status, severity and signature are flat beside the canonical record so a UQL read needs
    /// no JSON decode. `culprit_changeset` is not written here (a bisect writes it, MEDULLA-SCM-R009).
    pub fn node_properties(&self) -> Result<serde_json::Map<String, serde_json::Value>, String> {
        let defect = &self.defect;
        let body = serde_json::to_string(defect).map_err(|_| "defect record encoding failed".to_string())?;
        let enum_name = |value: Result<serde_json::Value, serde_json::Error>| {
            value.ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
        };
        let mut properties = serde_json::Map::new();
        properties.insert("type".into(), DEFECT_TYPE.into());
        properties.insert(DEFECT_RECORD_PROPERTY.into(), body.into());
        properties.insert("repo".into(), defect.repo.clone().into());
        properties.insert("defect_id".into(), defect.id.clone().into());
        properties.insert("title".into(), defect.title.clone().into());
        properties.insert("signature_digest".into(), defect.signature_digest.clone().into());
        properties.insert("status".into(), enum_name(serde_json::to_value(defect.status)).into());
        properties.insert("severity".into(), enum_name(serde_json::to_value(defect.severity)).into());
        properties.insert("intake".into(), enum_name(serde_json::to_value(defect.intake)).into());
        properties.insert("symptom_count".into(), (defect.symptoms.len() as u64).into());
        properties.insert("created_at".into(), defect.created_at.into());
        Ok(properties)
    }

    /// The receipt of this filing.
    pub fn receipt(&self) -> FileDefectReceipt {
        FileDefectReceipt {
            node_id: self.node_id(),
            defect_id: self.defect.id.clone(),
            outcome: self.outcome,
            status: self.defect.status,
            symptoms: self.defect.symptoms.len() as u64,
        }
    }
}

/// The stored defect of one decoded `Defect` node, when it belongs to `tenant`. A node of
/// another owner, or without the canonical record, is not read.
pub fn defect_from_node(node: &serde_json::Value, tenant: &str) -> Option<Defect> {
    if node.get("tenant_id").and_then(serde_json::Value::as_str).is_some_and(|owner| owner != tenant) {
        return None;
    }
    serde_json::from_str::<Defect>(node.get(DEFECT_RECORD_PROPERTY)?.as_str()?).ok()
}

/// One status move of a defect (MEDULLA-SDLC-R003.4): the request of `TransitionDefect`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TransitionDefectRequest {
    /// The repository the defect belongs to.
    pub repo: String,
    /// The defect id inside its repository.
    pub defect_id: String,
    /// The status to move to; it must be one edge of the defect machine from the stored status.
    pub to: DefectStatus,
    /// The changeset that fixes the defect; required for the move to `fixed`, and the changeset
    /// whose gate verdict a move to `verified` reads.
    #[serde(default)]
    pub fix_changeset: Option<String>,
    /// The bound test's gate on the fix changeset; required for the move to `verified`.
    #[serde(default)]
    pub gate_id: Option<String>,
    /// The recorded reason; required for a move to `rejected` and for a reopening.
    #[serde(default)]
    pub reason: Option<String>,
}

/// What `TransitionDefect` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TransitionDefectReceipt {
    /// The defect node id.
    pub node_id: String,
    /// The status before the move.
    pub from: DefectStatus,
    /// The status after the move.
    pub to: DefectStatus,
}

/// The request of `GetDefect` (MEDULLA-SDLC-R003.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct GetDefectRequest {
    /// The repository the defect belongs to.
    pub repo: String,
    /// The defect id inside its repository.
    pub defect_id: String,
}

/// What `GetDefect` answers: the stored defect with its symptom list, each symptom carrying
/// its intake path, source record id and time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GetDefectReceipt {
    /// The defect node id.
    pub node_id: String,
    /// The fix changeset recorded by the move to `fixed`, when there is one.
    pub fix_changeset: Option<String>,
    /// The defect as stored.
    pub defect: Defect,
}

/// What one status move writes (not on the wire).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefectMove {
    /// The defect with its new status.
    pub defect: Defect,
    /// The status before the move.
    pub from: DefectStatus,
    /// The fix changeset the defect carries after the move.
    pub fix_changeset: Option<String>,
}

impl DefectMove {
    /// The receipt of this move.
    pub fn receipt(&self) -> TransitionDefectReceipt {
        TransitionDefectReceipt {
            node_id: defect_node_id(&self.defect.repo, &self.defect.id),
            from: self.from,
            to: self.defect.status,
        }
    }
}

/// Check the bounds of a `GetDefect` request.
pub fn handle_get_defect(request: GetDefectRequest) -> Result<GetDefectRequest, LifecycleRefusal> {
    if request.repo.trim().is_empty() || request.defect_id.trim().is_empty() {
        return Err(LifecycleRefusal::InvalidArgument(
            "a defect read names its repo and defect id".to_owned(),
        ));
    }
    Ok(request)
}

/// Decide one status move against the stored `defect` (MEDULLA-SDLC-R003.4). The move must be
/// an edge of the defect machine from the stored status, else `DEFECT_TRANSITION_ILLEGAL`; a
/// guard that does not hold (a fix changeset, a reason) refuses the same way. The move to
/// `verified` needs a passing verdict of the bound test on the stored fix changeset
/// (`verdict_passed`, read by the caller from the sealed gate verdict), else
/// `DEFECT_UNVERIFIED`. A refusal changes nothing.
pub fn handle_transition_defect(
    request: &TransitionDefectRequest,
    defect: &Defect,
    stored_fix_changeset: Option<&str>,
    verdict_passed: bool,
) -> Result<DefectMove, LifecycleRefusal> {
    use crate::defect_machine::{DefectEvent, DefectFacts, DEFECT_MACHINE};
    let filled = |value: &Option<String>| value.as_deref().is_some_and(|text| !text.trim().is_empty());
    let from = defect.status;
    let edge = DEFECT_MACHINE
        .edges
        .iter()
        .find(|edge| edge.from == from && edge.to == request.to)
        .ok_or_else(|| {
            LifecycleRefusal::DefectTransitionIllegal(format!(
                "a defect in {from:?} does not move to {:?}",
                request.to
            ))
        })?;
    if edge.event == DefectEvent::Verify && !verdict_passed {
        return Err(LifecycleRefusal::DefectUnverified(
            "no passing verdict of the bound test on the fix changeset".to_owned(),
        ));
    }
    let facts = DefectFacts {
        fix_reference_recorded: filled(&request.fix_changeset),
        verification_evidence_recorded: verdict_passed,
        reason_recorded: filled(&request.reason),
    };
    let status = from
        .apply(edge.event, facts)
        .map_err(|refused| LifecycleRefusal::DefectTransitionIllegal(refused.to_string()))?;
    let fix_changeset = match edge.event {
        DefectEvent::Fix => request.fix_changeset.clone(),
        _ => stored_fix_changeset.map(str::to_owned),
    };
    Ok(DefectMove {
        defect: Defect {
            status,
            ..defect.clone()
        },
        from,
        fix_changeset,
    })
}

/// Check one filing's bounds; the served `FileDefect` and every in-process caller go through
/// this one check before the defect book sees the symptom.
pub fn handle_file_defect(request: FileDefectRequest) -> Result<FileDefectRequest, LifecycleRefusal> {
    let blank = |value: &str| value.trim().is_empty();
    if blank(&request.repo) || blank(&request.title) || blank(&request.source_record_id) || blank(&request.text) {
        return Err(LifecycleRefusal::InvalidArgument(
            "a defect filing names its repo, title, source record and symptom text".to_owned(),
        ));
    }
    if request.title.len() > MAX_DEFECT_TITLE_BYTES || request.text.len() > MAX_SYMPTOM_TEXT_BYTES {
        return Err(LifecycleRefusal::InvalidArgument(
            "a defect title or symptom text is over its bound".to_owned(),
        ));
    }
    Ok(request)
}

/// The derived state of a requirement row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SpecState {
    Specified,
    Landed,
    Verified,
}

/// One requirement row with the evidence behind its state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecRowState {
    pub id: String,
    pub state: SpecState,
    pub landed_by: Vec<String>,
    pub verified_by: Vec<String>,
    pub withdrawn: bool,
}

/// The served answer of `SpecStatus`: every row of one repository at one revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecStatusAnswer {
    pub repo: String,
    pub rev: String,
    pub rows: Vec<SpecRowState>,
}

/// The longest `repo`, `rev` or `spec_id` a `SpecStatus` request names.
pub const MAX_SPEC_STATUS_FIELD_BYTES: usize = 512;

/// A `SpecStatus` request (MEDULLA-SDLC-R005.3): the repository, the changeset id the answer
/// is derived at, and an optional spec id that keeps only the rows of that spec
/// (`<spec_id>-R...`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecStatusRequest {
    pub repo: String,
    pub rev: String,
    #[serde(default)]
    pub spec_id: Option<String>,
}

/// Check one `SpecStatus` request's bounds before the handler reads the graph; the served
/// method and every in-process caller go through this one check.
pub fn handle_spec_status(request: SpecStatusRequest) -> Result<SpecStatusRequest, LifecycleRefusal> {
    let blank = |value: &str| value.trim().is_empty();
    let over = |value: &str| value.len() > MAX_SPEC_STATUS_FIELD_BYTES;
    if blank(&request.repo) || blank(&request.rev) {
        return Err(LifecycleRefusal::InvalidArgument(
            "a status request names its repo and its rev".to_owned(),
        ));
    }
    if over(&request.repo)
        || over(&request.rev)
        || request.spec_id.as_deref().is_some_and(|id| blank(id) || over(id))
    {
        return Err(LifecycleRefusal::InvalidArgument(
            "a status request field is blank or over its bound".to_owned(),
        ));
    }
    Ok(request)
}

/// The derived state of one task line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TaskState {
    Open,
    Blocked,
    Done,
}

/// One task with the evidence behind its state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRowState {
    pub id: String,
    pub state: TaskState,
    /// The requirement row the task refers to, when it has one.
    pub row: Option<String>,
    /// The unresolved targets that block the task, sorted: a `dependsOn`
    /// row id absent from the tree, and the task's row when an
    /// unresolved contract (R014.4) touches it.
    pub blocked_by: Vec<String>,
}

/// The served answer of the task states: every task of one repository at one revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskStatusAnswer {
    pub repo: String,
    pub rev: String,
    pub tasks: Vec<TaskRowState>,
}

/// The result-size limits of one trace, resolved from the lifecycle knobs
/// (MEDULLA-SDLC-R006.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceLimits {
    pub max_depth: u8,
    pub node_budget: usize,
}

/// A request for the records reachable from one anchor. `depth` is checked
/// only against its lower bound at decode; its ceiling is the
/// `lifecycle.trace_max_depth` knob, verified by the served handler, and
/// `policy` names the policy whose knobs bound the answer
/// (MEDULLA-SDLC-R006.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LifecycleTraceRequestWire")]
pub struct LifecycleTraceRequest {
    pub anchor: String,
    pub depth: u8,
    pub kinds: Vec<LifecycleKind>,
    pub policy: DecisionPolicyRef,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LifecycleTraceRequestWire {
    anchor: String,
    depth: u8,
    #[serde(default)]
    kinds: Vec<LifecycleKind>,
    #[serde(default = "default_policy_ref")]
    policy: DecisionPolicyRef,
}

fn default_policy_ref() -> DecisionPolicyRef {
    DecisionPolicyRef::Default
}

impl TryFrom<LifecycleTraceRequestWire> for LifecycleTraceRequest {
    type Error = LifecycleRefusal;

    fn try_from(wire: LifecycleTraceRequestWire) -> Result<Self, Self::Error> {
        let mut request = Self::new(wire.anchor, wire.depth, wire.kinds)?;
        request.policy = wire.policy;
        Ok(request)
    }
}

impl LifecycleTraceRequest {
    /// Build a request; a depth below 1 refuses. The depth ceiling is the
    /// `lifecycle.trace_max_depth` knob, checked by the served handler, not at
    /// decode (MEDULLA-SDLC-R006.7).
    pub fn new(
        anchor: String,
        depth: u8,
        kinds: Vec<LifecycleKind>,
    ) -> Result<Self, LifecycleRefusal> {
        if depth < 1 {
            return Err(LifecycleRefusal::TraceDepth(depth));
        }
        Ok(Self {
            anchor,
            depth,
            kinds,
            policy: DecisionPolicyRef::Default,
        })
    }
}

/// A record a trace reached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceRecord {
    pub id: String,
    pub kind: LifecycleKind,
}

/// An edge a trace reached, with the rule that derived it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEdge {
    pub kind: LifecycleEdgeKind,
    pub from: String,
    pub to: String,
    pub rule: String,
    pub rule_version: u32,
}

/// The records and edges reachable from the anchor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleTraceAnswer {
    pub records: Vec<TraceRecord>,
    pub edges: Vec<TraceEdge>,
    pub truncated: bool,
    /// The digest of the resolved decision policy whose knobs bounded the walk
    /// (MEDULLA-SDLC-R006.5).
    #[serde(default)]
    pub policy_digest: String,
}

/// Check one trace request against the resolved limits before the walk reads the graph
/// (MEDULLA-SDLC-R006.4): a depth above the `lifecycle.trace_max_depth` ceiling refuses
/// `LIFECYCLE_DEPTH_INVALID`; the lower bound was checked at decode.
pub fn handle_lifecycle_trace(
    request: LifecycleTraceRequest,
    limits: TraceLimits,
) -> Result<LifecycleTraceRequest, LifecycleRefusal> {
    if request.depth < 1 || request.depth > limits.max_depth {
        return Err(LifecycleRefusal::TraceDepth(request.depth));
    }
    if request.anchor.trim().is_empty() {
        return Err(LifecycleRefusal::InvalidArgument(
            "a trace names its anchor".to_owned(),
        ));
    }
    Ok(request)
}

/// The changeset node an accept writes as the endpoint of every derived edge of the
/// changeset (MEDULLA-SDLC-R001.3). The node is the plan; the router applies it, ahead of the
/// derived edges, in the one mutation batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangesetNodeWrite {
    /// `commit:<changeset.id>`.
    pub node_id: String,
    pub changeset_id: String,
    /// The 1-based position of the changeset on its default-branch chain.
    pub seq: u64,
    /// The properties only the accept of a native changeset writes. The repository index
    /// route writes them itself, so there this is `None` and the node gets no second writer.
    pub facts: Option<ChangesetNodeFacts>,
}

/// The properties of a native changeset node beyond `changeset_id` and `seq`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangesetNodeFacts {
    pub repo: String,
    pub author: String,
    pub timestamp_ms: u64,
    /// The comma-joined parent changeset ids, the encoding of the repository index.
    pub parents: String,
}

impl ChangesetNodeWrite {
    /// The node's properties as the string values the repository index writes.
    pub fn properties(&self) -> std::collections::BTreeMap<String, String> {
        let mut properties = std::collections::BTreeMap::from([
            ("type".to_owned(), crate::source_changeset::COMMIT_TYPE.to_owned()),
            ("changeset_id".to_owned(), self.changeset_id.clone()),
            ("seq".to_owned(), self.seq.to_string()),
        ]);
        if let Some(facts) = &self.facts {
            properties.insert("repo".to_owned(), facts.repo.clone());
            properties.insert("author".to_owned(), facts.author.clone());
            properties.insert("timestamp_ms".to_owned(), facts.timestamp_ms.to_string());
            properties.insert("parents".to_owned(), facts.parents.clone());
        }
        properties
    }
}

/// The `changeset_id` and `seq` of a changeset node the graph already holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoredChangesetNode {
    pub changeset_id: Option<String>,
    pub seq: Option<String>,
}

/// Decide the changeset node of an accept (MEDULLA-SDLC-R001.3). `native` is true for the
/// accept of a submitted changeset and false for a commit the repository index ingested. An
/// existing node whose `changeset_id` or `seq` differs from the new value refuses the whole
/// batch with `CHANGESET_NODE_CONFLICT`; equal values make the write a no-op.
pub fn plan_changeset_node(
    changeset: &crate::source_changeset::Changeset,
    seq: u64,
    existing: Option<&StoredChangesetNode>,
    native: bool,
) -> Result<ChangesetNodeWrite, LifecycleRefusal> {
    let node_id = crate::source_changeset::commit_node_id(&changeset.id);
    if let Some(stored) = existing {
        let differs = |held: &Option<String>, new: &str| held.as_deref().is_some_and(|v| v != new);
        if differs(&stored.changeset_id, &changeset.id) || differs(&stored.seq, &seq.to_string()) {
            return Err(LifecycleRefusal::ChangesetNodeConflict(node_id));
        }
    }
    Ok(ChangesetNodeWrite {
        node_id,
        changeset_id: changeset.id.clone(),
        seq,
        facts: native.then(|| ChangesetNodeFacts {
            repo: changeset.repo.clone(),
            author: changeset.author_principal.clone(),
            timestamp_ms: changeset.created_at,
            parents: changeset.parents.join(","),
        }),
    })
}

/// Format identity of a [`ScenarioRun`] record.
pub const SCENARIO_RUN_SCHEMA_VERSION: u16 = 1;

/// The branch a [`ScenarioRun`] is on when its writer names none: the gated one.
pub const SCENARIO_RUN_DEFAULT_BRANCH: &str = "main";

/// How one scenario run ended (MEDULLA-SDLC-R013).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ScenarioResult {
    Passed,
    Failed,
}

fn scenario_run_schema_version() -> u16 {
    SCENARIO_RUN_SCHEMA_VERSION
}

fn scenario_run_default_branch() -> String {
    SCENARIO_RUN_DEFAULT_BRANCH.to_owned()
}

/// One scenario run of a delivery phase (MEDULLA-SDLC-R013). It gates the phase it names: a
/// release bundle is complete only when the latest run of its phase on `main` passed.
/// `branch` is the branch the commit was run on; a writer that names none runs on `main`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ScenarioRun {
    #[serde(default = "scenario_run_schema_version")]
    pub schema_version: u16,
    pub phase: String,
    pub scenario_id: String,
    pub commit: String,
    #[serde(default = "scenario_run_default_branch")]
    pub branch: String,
    pub result: ScenarioResult,
    /// The requirement ids the scenario exercised.
    #[serde(default)]
    pub requirement_ids: Vec<String>,
    /// The digests of the evidence the run produced.
    #[serde(default)]
    pub evidence_digests: Vec<String>,
}

impl ScenarioRun {
    /// A record without a phase, a commit or a scenario id is incomplete and refuses
    /// `SCENARIO_RUN_INCOMPLETE`; the list fields are sorted and deduplicated so one run has
    /// one canonical form.
    pub fn normalized(mut self) -> Result<Self, LifecycleRefusal> {
        let blank = |text: &str| text.trim().is_empty();
        let missing: Vec<&str> = [
            ("phase", blank(&self.phase)),
            ("scenario_id", blank(&self.scenario_id)),
            ("commit", blank(&self.commit)),
            ("branch", blank(&self.branch)),
        ]
        .into_iter()
        .filter_map(|(name, absent)| absent.then_some(name))
        .collect();
        if !missing.is_empty() {
            return Err(LifecycleRefusal::ScenarioRunIncomplete(format!(
                "a scenario run names its {}",
                missing.join(", ")
            )));
        }
        if self.schema_version != SCENARIO_RUN_SCHEMA_VERSION {
            return Err(LifecycleRefusal::ScenarioRunIncomplete(format!(
                "schema_version {} is not {SCENARIO_RUN_SCHEMA_VERSION}",
                self.schema_version
            )));
        }
        self.requirement_ids.sort();
        self.requirement_ids.dedup();
        self.evidence_digests.sort();
        self.evidence_digests.dedup();
        Ok(self)
    }

    /// The stable record id: the digest of the phase, scenario, commit and branch, so a second
    /// write of the same run is the same record.
    pub fn id(&self) -> String {
        format!(
            "scenario-run:{}",
            &sha256_hex(
                format!(
                    "{}\n{}\n{}\n{}",
                    self.phase, self.scenario_id, self.commit, self.branch
                )
                .as_bytes()
            )[..32]
        )
    }
}

/// A stored [`ScenarioRun`]: the record and the engine instant it was committed at. The
/// instant orders the runs of one phase; the writer's own clock never does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StoredScenarioRun {
    pub id: String,
    pub run: ScenarioRun,
    pub recorded_ms: u64,
}

#[cfg(test)]
mod defect_transition_tests {
    use super::*;

    fn defect(status: DefectStatus) -> Defect {
        Defect {
            id: "def-0001".into(),
            repo: "r".into(),
            title: "t".into(),
            intake: DefectIntake::Direct,
            signature_digest: "d".into(),
            severity: Severity::Medium,
            status,
            symptoms: Vec::new(),
            created_at: 1,
        }
    }

    fn request(to: DefectStatus) -> TransitionDefectRequest {
        TransitionDefectRequest {
            repo: "r".into(),
            defect_id: "def-0001".into(),
            to,
            fix_changeset: None,
            gate_id: None,
            reason: None,
        }
    }

    // spec: MEDULLA-SDLC-R003.4
    #[test]
    fn a_skipped_state_refuses_illegal_and_a_failed_verification_refuses_unverified() {
        let skipped = handle_transition_defect(&request(DefectStatus::Verified), &defect(DefectStatus::New), None, true);
        assert_eq!(skipped.unwrap_err().code(), "DEFECT_TRANSITION_ILLEGAL");
        let unverified =
            handle_transition_defect(&request(DefectStatus::Verified), &defect(DefectStatus::Fixed), Some("c1"), false);
        assert_eq!(unverified.unwrap_err().code(), "DEFECT_UNVERIFIED");
        let unfixed = handle_transition_defect(&request(DefectStatus::Fixed), &defect(DefectStatus::Fixing), None, false);
        assert_eq!(unfixed.unwrap_err().code(), "DEFECT_TRANSITION_ILLEGAL");
    }

    // spec: MEDULLA-SDLC-R003.4
    #[test]
    fn a_legal_move_changes_only_the_status_and_keeps_the_fix_reference() {
        let mut fix = request(DefectStatus::Fixed);
        fix.fix_changeset = Some("c1".into());
        let fixed = handle_transition_defect(&fix, &defect(DefectStatus::Fixing), None, false).expect("fixed");
        assert_eq!((fixed.from, fixed.defect.status), (DefectStatus::Fixing, DefectStatus::Fixed));
        assert_eq!(fixed.fix_changeset.as_deref(), Some("c1"));
        let verified =
            handle_transition_defect(&request(DefectStatus::Verified), &fixed.defect, Some("c1"), true).expect("verified");
        assert_eq!(verified.defect.status, DefectStatus::Verified);
        assert_eq!(verified.fix_changeset.as_deref(), Some("c1"));
        assert_eq!(verified.defect.symptoms, fixed.defect.symptoms);
    }
}
