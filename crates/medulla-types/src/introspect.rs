//! The runtime introspection page (MEDULLA-OBSERVE-R006.1).
//!
//! `Method::Introspect` reads one section of the engine's runtime registries and answers one
//! [`IntrospectPage`]: the rows ascending by their key, a cursor for the next page and the
//! registry sequence number the page was read at. Rows carry no payload and no token.

use serde::{Deserialize, Serialize};

/// The section of the runtime registries one `Introspect` call reads.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntrospectSection {
    /// In-flight requests.
    Requests,
    /// Registered background tasks, live and recently finished.
    Tasks,
    /// Runtime worker statistics.
    Runtime,
    /// Lock holders and waiters.
    Locks,
    /// Per-graph readiness.
    Readiness,
    /// Startup steps.
    Startup,
    /// Build information.
    Build,
    /// The recent event ring.
    Events,
}

impl IntrospectSection {
    /// The wire name of the section.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requests => "requests",
            Self::Tasks => "tasks",
            Self::Runtime => "runtime",
            Self::Locks => "locks",
            Self::Readiness => "readiness",
            Self::Startup => "startup",
            Self::Build => "build",
            Self::Events => "events",
        }
    }
}

/// One page of one section. `rows` are JSON objects whose shape the section defines
/// (`specs/engine-observability/data-model.md`), ascending by the section's key.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntrospectPage {
    /// The section the rows belong to.
    pub section: IntrospectSection,
    /// The rows of this page.
    pub rows: Vec<serde_json::Value>,
    /// The key of the page's last row when more rows follow; `None` on the last page.
    pub next_cursor: Option<String>,
    /// The registry sequence number the page was read at.
    pub seq: u64,
}
