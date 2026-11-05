//! Pure data for an architecture view derived from graph evidence.
//!
//! The derivation and served read path live above `medulla-types`.  These structs
//! deliberately carry only the stable view contract: deployable containers,
//! supporting source paths, and call edges.

use serde::{Deserialize, Serialize};

/// A graph-derived architecture view.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ArchitectureView {
    /// Deployable containers present in the view.
    pub containers: Vec<ViewContainer>,
    /// Cross-container calls represented by the view.
    pub edges: Vec<ViewEdge>,
}

/// One deployable container and the ingested source paths that support it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ViewContainer {
    /// Stable display/name key for the container.
    pub name: String,
    /// Technology label associated with the container.
    pub technology: String,
    /// Source paths whose ingested evidence supports the container.
    pub support: Vec<String>,
}

/// A directed call edge between two architecture containers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ViewEdge {
    /// Source container name.
    pub from: String,
    /// Destination container name.
    pub to: String,
    /// Kind of relationship represented by the edge.
    pub kind: String,
    /// Number of supporting calls for this relationship.
    pub call_count: u64,
}

/// The scope of one served `ArchitectureView` read (MEDULLA-CQ-ORCH-R010.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ViewScope {
    /// The repository the view is derived for.
    pub repository_id: String,
    /// Repository-snapshot digest (64 lowercase hex characters) of the newest
    /// `IndexRepository` envelope; absent means "the newest".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

impl ViewScope {
    /// Whether `revision` is absent or a 64-character lowercase hex digest.
    pub fn revision_is_valid(&self) -> bool {
        self.revision.as_ref().is_none_or(|r| {
            r.len() == 64 && r.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
    }
}

/// Why a view was not derived. A refusal never carries a partial view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ViewAbstained {
    /// The scope holds no ingested code and no diagram element.
    NoSupportingSource,
    /// The repository id is invisible to the tenant.
    UnknownRepository,
    /// The stated revision differs from the newest snapshot digest.
    StaleRevision,
    /// More containers than `architecture_view_max_containers`.
    TooLarge,
}

impl ViewAbstained {
    /// The wire error code of this refusal.
    pub fn code(self) -> &'static str {
        match self {
            Self::NoSupportingSource => "VIEW_NO_SUPPORTING_SOURCE",
            Self::UnknownRepository => "VIEW_UNKNOWN_REPOSITORY",
            Self::StaleRevision => "VIEW_STALE_REVISION",
            Self::TooLarge => "VIEW_TOO_LARGE",
        }
    }
}
