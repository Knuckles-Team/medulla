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
    /// The semantic indexes the engine maintains for its own vocabularies, one row each
    /// (MEDULLA-MEDIA-VOCAB-R001.102.2): `bookmark_index` reads `bound` or
    /// `unbound (no text embedder)`, so a missing embedder is never silent.
    Indexes,
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
            Self::Indexes => "indexes",
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

/// What one `Profile` call reads (MEDULLA-OBSERVE-R010.1).
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    /// Sample the stack of every thread for `seconds` and answer folded stacks.
    Stacks,
    /// One atomic snapshot of the tasks, requests, locks and the last events.
    TaskDump,
}

/// One frame of a sampled stack. A frame is never a raw address alone: it names its module and
/// the offset inside it, so the stack can be symbolized offline from the symbol artifact of the
/// module's build id (MEDULLA-OBSERVE-R010.2).
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileFrame {
    /// The path of the module (executable or shared library) the address lies in.
    pub module: String,
    /// The module's GNU build id as lowercase hex, when it carries one.
    pub build_id: Option<String>,
    /// The offset of the address inside the module.
    pub offset: u64,
    /// The function name, when the module holds a dynamic symbol for it.
    pub function: Option<String>,
    /// `file:line`, when the module holds debug information.
    pub file_line: Option<String>,
}

/// One distinct stack of one thread and how many samples hit it.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoldedStack {
    /// The thread's name.
    pub thread: String,
    /// The frames, outermost first.
    pub frames: Vec<ProfileFrame>,
    /// How many samples hit this stack.
    pub count: u64,
}

/// The answer of `Profile`. A `stacks` report fills `stacks`; a `task_dump` report fills the four
/// row lists, each read once under the one sequence number `seq`.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileReport {
    /// What was read.
    pub kind: ProfileKind,
    /// The snapshot sequence number of a `task_dump`; 0 for `stacks`.
    pub seq: u64,
    /// The seconds the sampler ran; 0 for a `task_dump`.
    pub seconds: u16,
    /// The sampling rate; 0 for a `task_dump`.
    pub hz: u16,
    /// The samples taken across every thread.
    pub samples: u64,
    /// The folded stacks of a `stacks` report, hottest first.
    pub stacks: Vec<FoldedStack>,
    /// The task rows of a `task_dump`, as `Introspect tasks` shows them.
    pub tasks: Vec<serde_json::Value>,
    /// The in-flight request rows of a `task_dump`.
    pub requests: Vec<serde_json::Value>,
    /// The lock rows of a `task_dump`.
    pub locks: Vec<serde_json::Value>,
    /// The last events of a `task_dump`.
    pub events: Vec<serde_json::Value>,
}
