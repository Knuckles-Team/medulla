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
