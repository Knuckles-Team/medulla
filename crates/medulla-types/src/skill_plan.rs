//! Skill-document compilation records (MEDULLA-REPO-INGEST-R002.1.1).
//!
//! A `SkillCompilation` binds a skill directory to the digests of the document,
//! the capability registry and the type registry it was compiled against, plus
//! one `SkillStepBinding` per step of the document's `## Steps` section. It
//! stores digests and spans only, never the instruction text.

use serde::{Deserialize, Serialize};

use crate::contract::Digest256;

/// The step grammar this compiler reads: an ordered list under `## Steps`.
pub const SKILL_GRAMMAR_REVISION: &str = "skill-steps-1";

/// One compiled step: where it sits in the document and which capability runs it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillStepBinding {
    pub step_id: String,
    pub span_start: u64,
    pub span_end: u64,
    pub instruction_digest: Digest256,
    pub capability_id: Option<String>,
}

/// The compilation of one skill document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillCompilation {
    pub directory_ref: String,
    pub document_digest: Digest256,
    pub grammar_revision: String,
    pub plan_digest: Digest256,
    pub capability_digest: Digest256,
    pub type_digest: Digest256,
    pub steps: Vec<SkillStepBinding>,
}

impl SkillCompilation {
    /// True when the document or either registry digest differs from the one
    /// this compilation was bound to (R002.1.3 marks such a binding stale).
    pub fn is_stale_against(
        &self,
        document: &Digest256,
        capability: &Digest256,
        types: &Digest256,
    ) -> bool {
        self.document_digest != *document
            || self.capability_digest != *capability
            || self.type_digest != *types
    }
}
