//! The wire request of a skill upgrade admission (MEDULLA-TYPED-PACKS-R100.101).
//!
//! The judgement itself is the pure rule in the catalog layer, which sits above this
//! crate, so the request here carries plain wire fields: digests as lowercase hex, a
//! contract as its declared fields. The served handler converts them into the catalog
//! layer's typed input and refuses a malformed one before anything is written.

use serde::{Deserialize, Serialize};

use super::validate_text;

/// Most fields one contract may carry on the wire; the catalog layer holds the same bound.
pub const MAX_SKILL_ADMISSION_CONTRACT_FIELDS: usize = 256;
/// Most consumers one request may name.
pub const MAX_SKILL_ADMISSION_COHORT: usize = 256;

/// One field a consumer relies on, as declared by the consumer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillContractFieldWire {
    pub name: String,
    pub type_name: String,
    pub required: bool,
}

/// A migration the skill author declared for a breaking change, with its receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillMigrationWire {
    pub migration_id: String,
    /// Lowercase hex SHA-256 of the migration definition.
    pub migration_digest: String,
    /// Lowercase hex package digest the migration starts from.
    pub previous_package_digest: String,
    /// Lowercase hex package digest the migration produces.
    pub new_package_digest: String,
    /// Lowercase hex digest of the consumer contract it was proven against.
    pub consumer_contract_digest: String,
    /// Whether the migration receipt passed.
    pub receipt_passed: bool,
}

/// Admit one skill upgrade, or re-check a held admission against what is now in force.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillAdmissionRequest {
    pub tenant_id: String,
    /// The skill component the admission is about.
    pub component_id: String,
    /// Lowercase hex content digest of the package already admitted.
    pub previous_package_digest: String,
    /// Lowercase hex content digest of the package being admitted.
    pub new_package_digest: String,
    /// The contract the previous package was admitted against.
    pub previous_contract: Vec<SkillContractFieldWire>,
    /// The consumer-declared contract now in force.
    pub new_contract: Vec<SkillContractFieldWire>,
    /// The consumers observed using the skill.
    #[serde(default)]
    pub cohort: Vec<String>,
    /// The declared migration, when the change is breaking.
    #[serde(default)]
    pub migration: Option<SkillMigrationWire>,
    /// When set, no new admission is taken: the stored admission of this revision is
    /// re-checked against `new_package_digest` and `new_contract`, and refused when
    /// either changed since it was taken.
    #[serde(default)]
    pub held_admission_revision: Option<u64>,
}

impl SkillAdmissionRequest {
    /// Shape checks that need no catalog types.
    pub fn validate(&self) -> Result<(), String> {
        validate_text("tenant_id", &self.tenant_id)?;
        validate_text("component_id", &self.component_id)?;
        validate_text("previous_package_digest", &self.previous_package_digest)?;
        validate_text("new_package_digest", &self.new_package_digest)?;
        if self.previous_contract.len() > MAX_SKILL_ADMISSION_CONTRACT_FIELDS
            || self.new_contract.len() > MAX_SKILL_ADMISSION_CONTRACT_FIELDS
        {
            return Err(format!(
                "a contract carries at most {MAX_SKILL_ADMISSION_CONTRACT_FIELDS} fields"
            ));
        }
        if self.cohort.len() > MAX_SKILL_ADMISSION_COHORT {
            return Err(format!(
                "a cohort names at most {MAX_SKILL_ADMISSION_COHORT} consumers"
            ));
        }
        Ok(())
    }
}
