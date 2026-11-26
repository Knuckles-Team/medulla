//! The request of a lossless SKILL.md edit (MEDULLA-TYPED-PACKS-R100.100).
//!
//! A skill is edited by byte-range patches over its ORIGINAL bytes: nothing is reparsed or
//! reserialized, so every untouched byte (order, spacing, comments, line endings and unknown
//! frontmatter fields) stays identical. The pure patch function is
//! `medulla_catalog_types::skill_edit::apply_skill_patches`.

use serde::{Deserialize, Serialize};

use super::validate_text;
use crate::agent_library::AgentLibraryMutationContext;

/// Refusal: two patches overlap, or two insertions share one point.
pub const SKILL_PATCH_OVERLAP: &str = "SKILL_PATCH_OVERLAP";
/// Refusal: a patch boundary splits a character, or its replacement breaks the line-ending
/// style of the source.
pub const SKILL_PATCH_ANCHOR_INVALID: &str = "SKILL_PATCH_ANCHOR_INVALID";
/// Refusal: the named source digest is not the digest of the bytes the edit would patch.
pub const SKILL_SOURCE_STALE: &str = "SKILL_SOURCE_STALE";
/// Refusal: a span lies outside the source, or the edited result is over the bound.
pub const SKILL_SPAN_UNKNOWN: &str = "SKILL_SPAN_UNKNOWN";

/// The most patches one edit carries.
pub const MAX_SKILL_PATCHES: usize = 256;

/// One byte-range replacement over the original SKILL.md bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillSourcePatch {
    /// First replaced byte.
    pub range_start: u64,
    /// One past the last replaced byte; equal to `range_start` for an insertion.
    pub range_end: u64,
    pub replacement: String,
}

/// Edit one skill component's source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillSourceEditRequest {
    /// The base revision is `context.expected_revision`; the operation id is the wire
    /// idempotency key bound into `context.idempotency_key`.
    pub context: AgentLibraryMutationContext,
    pub component_id: String,
    /// `sha256:<hex>` of the source bytes the patches are ranges over.
    pub source_digest: String,
    pub patches: Vec<SkillSourcePatch>,
}

impl SkillSourceEditRequest {
    /// Refuse a malformed request before any read or write.
    pub fn validate(&self) -> Result<(), String> {
        self.context.validate()?;
        validate_text("component_id", &self.component_id)?;
        if self.context.expected_revision.is_none_or(|base| base == 0) {
            return Err(
                "INVALID_ARGUMENT: a skill edit names its base revision (expected_revision of \
                 at least 1)"
                    .to_string(),
            );
        }
        if self.patches.is_empty() || self.patches.len() > MAX_SKILL_PATCHES {
            return Err(format!(
                "INVALID_ARGUMENT: a skill edit carries 1 to {MAX_SKILL_PATCHES} patches"
            ));
        }
        Ok(())
    }
}
