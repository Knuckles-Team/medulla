//! The competency questions over the spec artifact graph (MEDULLA-SDLC-R014.6): the closed
//! question set and the answer a served `SpecQuestion` returns. Pure data; the answers are
//! computed by `medulla-sdlc` over a pre-read graph view.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The closed set of questions, snake_case on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SpecQuestion {
    /// Requirement rows with no commit that implements them and no test that verifies them.
    UnreachableRequirements,
    /// Code symbols no requirement row points at.
    UncoveredCodeItems,
    /// Tasks whose row crosses a seam to a contract whose provider row is not yet verified.
    TasksBlockedByContract,
    /// Constitution principles that govern the unverified rows a changeset cites.
    PrinciplesAtRisk,
    /// Changesets that cite a row targeting one goal.
    ChangesAdvancingGoal,
    /// The next task of one phase that nothing blocks.
    NextUnblockedTask,
    /// Requirement rows with no `verifiedBy` edge (MEDULLA-SDLC-R006.3).
    UnboundRequirements,
    /// The deployments a requirement reaches through the symbols that implement it, the
    /// changesets that change them and the releases that contain those (MEDULLA-SDLC-R006.3).
    DeploymentOfRequirement,
    /// Open defects whose affected symbol lies in a changeset one release contains
    /// (MEDULLA-SDLC-R006.3).
    OpenDefectsInRelease,
    /// The changesets one deployment's release contains and another's does not
    /// (MEDULLA-SDLC-R006.3).
    ChangesetsBetweenDeployments,
    /// The open task rows of one release bundle in the bundle's train order
    /// (MEDULLA-SDLC-R018.2).
    OpenTasksOfBundle,
    /// What an implementer needs for one task: its row, phase, covering plan sections,
    /// contracts, code items and what blocks it (MEDULLA-SDLC-R014.15). The subject is the
    /// task node id.
    TaskContext,
    /// One plan section with its span, phase and the requirement rows it covers
    /// (MEDULLA-SDLC-R014.15). The subject is the plan section node id.
    PlanSection,
    /// One contract with its wire shape, providers and the rows that cross it
    /// (MEDULLA-SDLC-R014.15). The subject is the contract node id.
    Contract,
    /// For each contract one requirement row crosses, which repository and row provides it
    /// (MEDULLA-SDLC-R014.15). The subject is the requirement node id.
    SeamRouting,
    /// The phase of one plan section and the goals each requirement it covers targets
    /// (MEDULLA-SDLC-R014.15). The subject is the plan section node id.
    PlanToGoalPhase,
    /// The unresolved items of the Phase 0 plan sections of one repository, ordered by file
    /// and line (MEDULLA-SDLC-R016.1).
    PlanUnknowns,
    /// The tools the tool-use contracts of the research task name, one row per connector and
    /// tool (MEDULLA-SDLC-R016.4). Read from the graph; no list lives in code.
    ResearchTools,
}

impl SpecQuestion {
    /// Whether the question takes a subject: the phase, the goal id, the changeset id, the
    /// requirement or release node id, or `<deployment>,<deployment>` for the two deployments
    /// compared.
    pub fn takes_subject(self) -> bool {
        matches!(
            self,
            Self::PrinciplesAtRisk
                | Self::ChangesAdvancingGoal
                | Self::NextUnblockedTask
                | Self::DeploymentOfRequirement
                | Self::OpenDefectsInRelease
                | Self::ChangesetsBetweenDeployments
                | Self::OpenTasksOfBundle
                | Self::TaskContext
                | Self::PlanSection
                | Self::Contract
                | Self::SeamRouting
                | Self::PlanToGoalPhase
        )
    }
}

/// What a served `SpecQuestion` answers: one row per answer, and the digests of the records
/// the answer read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SpecQuestionAnswer {
    /// The answer rows, each a flat map of named values.
    pub rows: Vec<BTreeMap<String, String>>,
    /// The SHA-256 digests of the stored records the answer read, sorted and unique.
    pub digests: Vec<String>,
}
