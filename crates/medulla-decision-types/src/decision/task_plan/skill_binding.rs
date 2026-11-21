//! The skill binding a `task_plan` Decide can pin (MEDULLA-REPO-INGEST-R002.1.3).
//!
//! One typed parameter names the stored `SkillCompilation` node a plan was built
//! from and the document digest the caller bound it at; the engine refuses the
//! plan when the stored binding no longer matches.

use serde::{Deserialize, Serialize};

/// The parameter name a `task_plan` request carries a skill binding under.
pub const SKILL_BINDING_PARAM: &str = "skill_binding";

/// A skill binding to check: the compilation node and the document digest the
/// caller bound the plan at (lowercase hex).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillBindingPin {
    pub node_id: String,
    pub document_digest: String,
}
