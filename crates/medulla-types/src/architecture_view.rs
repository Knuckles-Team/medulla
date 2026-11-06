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

/// The scope of one served `ArchitectureView` read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ViewScope {
    /// The ingested repository whose graph evidence the view is derived from.
    pub repository_id: String,
    /// The repository-snapshot digest (64 lowercase hex characters) of the
    /// newest `IndexRepository` envelope the caller read; `None` reads the newest.
    #[serde(default)]
    pub revision: Option<String>,
}

/// The served answer: the view, the snapshot it was read at, and the SHA-256
/// digest of the canonical view (64 lowercase hex characters).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ArchitectureViewResponse {
    /// The derived view.
    pub view: ArchitectureView,
    /// The repository-snapshot digest the view was derived at.
    pub revision: Option<String>,
    /// SHA-256 of the canonical view, 64 lowercase hex characters.
    pub digest: String,
}

/// Domain of the canonical view digest.
pub const VIEW_DIGEST_DOMAIN: &str = "eg/architecture-view/v1";

/// The SHA-256 digest of the canonical view, 64 lowercase hex characters.
pub fn view_digest(view: &ArchitectureView) -> String {
    let text = crate::solve_scalar::digest_text(VIEW_DIGEST_DOMAIN, view);
    text.strip_prefix(crate::solve_scalar::DIGEST_TEXT_PREFIX)
        .map(str::to_owned)
        .unwrap_or(text)
}

impl ArchitectureViewResponse {
    /// Wrap a derived view with its revision and canonical digest.
    pub fn new(view: ArchitectureView, revision: Option<String>) -> Self {
        let digest = view_digest(&view);
        Self {
            view,
            revision,
            digest,
        }
    }
}

/// Why a view was refused. The derivation never invents a container and never
/// returns a partial view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ViewAbstained {
    /// The scope holds no ingested code and no diagram element.
    NoSupportingSource,
    /// The repository id is invisible to the tenant.
    UnknownRepository,
    /// The stated revision differs from the newest snapshot digest.
    StaleRevision { stated: String, newest: String },
    /// More containers than `architecture_view_max_containers`.
    TooLarge { containers: usize, limit: usize },
}

impl ViewAbstained {
    /// The stable wire error code of this refusal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoSupportingSource => "VIEW_NO_SUPPORTING_SOURCE",
            Self::UnknownRepository => "VIEW_UNKNOWN_REPOSITORY",
            Self::StaleRevision { .. } => "VIEW_STALE_REVISION",
            Self::TooLarge { .. } => "VIEW_TOO_LARGE",
        }
    }
}

impl std::fmt::Display for ViewAbstained {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSupportingSource => write!(f, "{}: no ingested source in scope", self.code()),
            Self::UnknownRepository => write!(f, "{}: repository is unknown", self.code()),
            Self::StaleRevision { stated, newest } => write!(
                f,
                "{}: stated revision {stated} differs from newest {newest}",
                self.code()
            ),
            Self::TooLarge { containers, limit } => write!(
                f,
                "{}: {containers} containers exceed the limit {limit}",
                self.code()
            ),
        }
    }
}

impl std::error::Error for ViewAbstained {}

/// Every refusal code of the served `ArchitectureView` read.
pub const REFUSALS: &[&str] = &[
    "VIEW_NO_SUPPORTING_SOURCE",
    "VIEW_UNKNOWN_REPOSITORY",
    "VIEW_STALE_REVISION",
    "VIEW_TOO_LARGE",
];

/// Refuse a stated revision that is not the newest snapshot digest.
pub fn check_revision(scope: &ViewScope, newest: Option<&str>) -> Result<(), ViewAbstained> {
    match (scope.revision.as_deref(), newest) {
        (Some(stated), Some(newest)) if stated != newest => Err(ViewAbstained::StaleRevision {
            stated: stated.to_owned(),
            newest: newest.to_owned(),
        }),
        _ => Ok(()),
    }
}
