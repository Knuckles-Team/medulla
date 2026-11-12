//! Skill consumer compatibility and declared-migration admission, the pure rule
//! (MEDULLA-TYPED-PACKS-R100.101).
//!
//! Compatibility is judged against the contracts consumers declare, never against version
//! equality, contract-pin equality or dependency digests. New optional fields are
//! compatible; a removal, a type change, a new required field or an optional field that
//! became required is breaking. A breaking change needs a declared migration bound to the
//! exact old package digest, new package digest and new consumer contract digest, and the
//! migration receipt must have passed. [`admit_skill_upgrade`] is a pure function over those
//! inputs; the served operation and the admission record write call it.

use medulla_types::contract::{BoundedVec, Digest256, OpaqueId};
use serde::{Deserialize, Serialize};

use crate::consumer_contract::{compare_contracts, ConsumerContractSchema};

/// Most consumers one admission records as its observed cohort.
pub const MAX_ADMISSION_COHORT: usize = 256;

/// The closed verdict of one admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SkillAdmissionVerdict {
    /// Additive only: nothing a consumer relied on changed.
    Compatible,
    /// Breaking, and a declared migration bound to this exact change passed.
    BreakingWithMigration,
    /// Breaking, with no admissible migration.
    BreakingRefused,
}

/// Whether the migration's own run passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SkillMigrationOutcome {
    /// The receipt passed.
    Passed,
    /// The receipt records a failed run.
    Failed,
}

/// A migration the author declared for a breaking change, with its receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DeclaredSkillMigration {
    /// The migration id.
    pub migration_id: OpaqueId,
    /// Digest of the migration definition.
    pub migration_digest: Digest256,
    /// The package digest the migration starts from.
    pub previous_package_digest: Digest256,
    /// The package digest the migration produces.
    pub new_package_digest: Digest256,
    /// The consumer contract digest the migration was proven against.
    pub consumer_contract_digest: Digest256,
    /// Whether the receipt passed.
    pub outcome: SkillMigrationOutcome,
}

/// What the admission is asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillUpgradeAdmissionInput {
    /// Content digest of the package already admitted.
    pub previous_package_digest: Digest256,
    /// Content digest of the package being admitted.
    pub new_package_digest: Digest256,
    /// The consumer-declared contract the previous package was admitted against.
    pub previous_contract: ConsumerContractSchema,
    /// The consumer-declared contract now in force.
    pub new_contract: ConsumerContractSchema,
    /// The consumers observed using the skill.
    pub cohort: Vec<OpaqueId>,
    /// The declared migration, when the change is breaking.
    pub migration: Option<DeclaredSkillMigration>,
    /// The revision this admission takes.
    pub admission_revision: u64,
}

/// The durable admission record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillAdmission {
    /// Content digest of the package already admitted.
    pub previous_package_digest: Digest256,
    /// Content digest of the package admitted.
    pub new_package_digest: Digest256,
    /// Digest of the consumer-declared contract of the previous revision.
    pub previous_contract_digest: Digest256,
    /// Digest of the consumer-declared contract of this revision.
    pub new_contract_digest: Digest256,
    /// The consumers observed using the skill.
    pub cohort: BoundedVec<OpaqueId, MAX_ADMISSION_COHORT>,
    /// The verdict.
    pub verdict: SkillAdmissionVerdict,
    /// The migration id, when the verdict is `BreakingWithMigration`.
    pub migration_id: Option<OpaqueId>,
    /// The migration digest, when the verdict is `BreakingWithMigration`.
    pub migration_digest: Option<Digest256>,
    /// The revision of this admission.
    pub admission_revision: u64,
}

/// Why an admission was refused. Each variant has one registered wire code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillAdmissionRefusal {
    /// A breaking change names no declared migration.
    BreakingNoMigration,
    /// The consumer contract or package digest differs from the one admitted.
    ConsumerContractChanged,
    /// The migration is bound to another change, or its receipt did not pass.
    MigrationReceiptInvalid,
    /// An input is not well formed (contract, cohort size).
    InvalidInput,
}

impl SkillAdmissionRefusal {
    /// The wire code of this refusal.
    pub fn code(self) -> &'static str {
        match self {
            Self::BreakingNoMigration => "SKILL_BREAKING_NO_MIGRATION",
            Self::ConsumerContractChanged => "SKILL_CONSUMER_CONTRACT_CHANGED",
            Self::MigrationReceiptInvalid => "SKILL_MIGRATION_RECEIPT_INVALID",
            Self::InvalidInput => "INVALID_PARAMS",
        }
    }
}

/// Judge one upgrade. The verdict `BreakingRefused` is never returned from here: a breaking
/// change without an admissible migration is the typed refusal, so no admission record
/// exists for it.
pub fn admit_skill_upgrade(
    input: &SkillUpgradeAdmissionInput,
) -> Result<SkillAdmission, SkillAdmissionRefusal> {
    let invalid = |_| SkillAdmissionRefusal::InvalidInput;
    let previous_contract_digest = input.previous_contract.digest().map_err(invalid)?;
    let new_contract_digest = input.new_contract.digest().map_err(invalid)?;
    let cohort = BoundedVec::new(input.cohort.clone()).map_err(invalid)?;
    let diff = compare_contracts(&input.previous_contract, &input.new_contract);
    let (verdict, migration_id, migration_digest) = if diff.is_compatible() {
        (SkillAdmissionVerdict::Compatible, None, None)
    } else {
        let migration = input
            .migration
            .as_ref()
            .ok_or(SkillAdmissionRefusal::BreakingNoMigration)?;
        let bound = migration.previous_package_digest == input.previous_package_digest
            && migration.new_package_digest == input.new_package_digest
            && migration.consumer_contract_digest == new_contract_digest
            && migration.outcome == SkillMigrationOutcome::Passed;
        if !bound {
            return Err(SkillAdmissionRefusal::MigrationReceiptInvalid);
        }
        (
            SkillAdmissionVerdict::BreakingWithMigration,
            Some(migration.migration_id.clone()),
            Some(migration.migration_digest),
        )
    };
    Ok(SkillAdmission {
        previous_package_digest: input.previous_package_digest,
        new_package_digest: input.new_package_digest,
        previous_contract_digest,
        new_contract_digest,
        cohort,
        verdict,
        migration_id,
        migration_digest,
        admission_revision: input.admission_revision,
    })
}

impl SkillAdmission {
    /// An admission holds only while the package and the consumer contract in force are the
    /// ones it was taken against; any change invalidates it.
    pub fn still_holds(
        &self,
        package_digest: &Digest256,
        contract: &ConsumerContractSchema,
    ) -> Result<(), SkillAdmissionRefusal> {
        let contract_digest = contract
            .digest()
            .map_err(|_| SkillAdmissionRefusal::InvalidInput)?;
        if *package_digest != self.new_package_digest || contract_digest != self.new_contract_digest
        {
            return Err(SkillAdmissionRefusal::ConsumerContractChanged);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consumer_contract::ContractField;

    fn contract(fields: &[(&str, &str, bool)]) -> ConsumerContractSchema {
        ConsumerContractSchema::new(
            fields
                .iter()
                .map(|(name, type_name, required)| ContractField {
                    name: (*name).into(),
                    type_name: (*type_name).into(),
                    required: *required,
                })
                .collect(),
        )
        .unwrap()
    }

    fn digest(byte: u8) -> Digest256 {
        Digest256::from_bytes([byte; 32])
    }

    fn input(new: ConsumerContractSchema) -> SkillUpgradeAdmissionInput {
        SkillUpgradeAdmissionInput {
            previous_package_digest: digest(1),
            new_package_digest: digest(2),
            previous_contract: contract(&[("a", "int", true), ("b", "text", false)]),
            new_contract: new,
            cohort: vec![OpaqueId::new("consumer:one").unwrap()],
            migration: None,
            admission_revision: 1,
        }
    }

    fn migration(for_input: &SkillUpgradeAdmissionInput) -> DeclaredSkillMigration {
        DeclaredSkillMigration {
            migration_id: OpaqueId::new("migration:v2").unwrap(),
            migration_digest: digest(9),
            previous_package_digest: for_input.previous_package_digest,
            new_package_digest: for_input.new_package_digest,
            consumer_contract_digest: for_input.new_contract.digest().unwrap(),
            outcome: SkillMigrationOutcome::Passed,
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R100.101
    #[test]
    fn additive_change_is_compatible_and_breaking_needs_the_exact_migration() {
        let additive = input(contract(&[
            ("a", "int", true),
            ("b", "text", false),
            ("c", "int", false),
        ]));
        let admitted = admit_skill_upgrade(&additive).unwrap();
        assert_eq!(admitted.verdict, SkillAdmissionVerdict::Compatible);
        assert_eq!(admitted.migration_id, None);

        let mut breaking = input(contract(&[("a", "text", true), ("b", "text", false)]));
        assert_eq!(
            admit_skill_upgrade(&breaking),
            Err(SkillAdmissionRefusal::BreakingNoMigration)
        );
        breaking.migration = Some(migration(&breaking));
        let admitted = admit_skill_upgrade(&breaking).unwrap();
        assert_eq!(
            admitted.verdict,
            SkillAdmissionVerdict::BreakingWithMigration
        );
        assert_eq!(admitted.migration_digest, Some(digest(9)));

        let mut wrong_old = breaking.clone();
        wrong_old
            .migration
            .as_mut()
            .unwrap()
            .previous_package_digest = digest(7);
        let mut wrong_contract = breaking.clone();
        wrong_contract
            .migration
            .as_mut()
            .unwrap()
            .consumer_contract_digest = digest(7);
        let mut failed = breaking.clone();
        failed.migration.as_mut().unwrap().outcome = SkillMigrationOutcome::Failed;
        for refused in [wrong_old, wrong_contract, failed] {
            assert_eq!(
                admit_skill_upgrade(&refused),
                Err(SkillAdmissionRefusal::MigrationReceiptInvalid)
            );
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R100.101
    #[test]
    fn a_changed_contract_or_package_invalidates_the_admission() {
        let request = input(contract(&[("a", "int", true), ("b", "text", false)]));
        let admitted = admit_skill_upgrade(&request).unwrap();
        assert_eq!(
            admitted.still_holds(&digest(2), &request.new_contract),
            Ok(())
        );
        assert_eq!(
            admitted.still_holds(&digest(3), &request.new_contract),
            Err(SkillAdmissionRefusal::ConsumerContractChanged)
        );
        let changed = contract(&[("a", "int", true)]);
        assert_eq!(
            admitted.still_holds(&digest(2), &changed),
            Err(SkillAdmissionRefusal::ConsumerContractChanged)
        );
    }
}
