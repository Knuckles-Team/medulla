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
];

/// A typed refusal from the lifecycle records.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LifecycleRefusal {
    #[error("LIFECYCLE_KIND_UNKNOWN: {0}")]
    KindUnknown(String),
    #[error("LIFECYCLE_TRACE_DEPTH: depth {0} is below 1")]
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
    #[error("INVALID_ARGUMENT: {0}")]
    InvalidArgument(String),
    #[error("MODE_APPROVAL_REQUIRED: {0}")]
    ModeApprovalRequired(String),
}

impl LifecycleRefusal {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::KindUnknown(_) => "LIFECYCLE_KIND_UNKNOWN",
            Self::TraceDepth(_) => "LIFECYCLE_TRACE_DEPTH",
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
            Self::InvalidArgument(_) => "INVALID_ARGUMENT",
            Self::ModeApprovalRequired(_) => "MODE_APPROVAL_REQUIRED",
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

/// How a defect reached the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

/// One observation of a defect, with its intake path and source record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Symptom {
    pub intake: DefectIntake,
    pub source_record_id: String,
    pub observed_at: u64,
    pub text: String,
}

/// A code fault, deduplicated by the digest of its normalized symptom.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}
