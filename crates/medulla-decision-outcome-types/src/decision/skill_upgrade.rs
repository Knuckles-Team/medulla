//! Skill consumer-contract upgrade receipt (MEDULLA-DECISION-ENGINE-R167.1).

use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

use medulla_decision_types::decision::skill_upgrade_request::{
    ConsumerContractSchema, ContractField, SkillUpgradeRequest,
};
use medulla_types::{contract::Digest256, contract::OpaqueId};

/// Current wire schema version of [`SkillUpgradeReceipt`].
pub const SKILL_UPGRADE_RECEIPT_SCHEMA_VERSION: u16 = 1;

/// Records the old and new consumer contracts and any declared migration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillUpgradeReceipt {
    pub schema_version: u16,
    pub previous_consumer_contract_digest: Digest256,
    pub new_consumer_contract_digest: Digest256,
    pub declared_migration_id: Option<OpaqueId>,
}

impl SkillUpgradeReceipt {
    /// The receipt of one upgrade request: both contract digests and the
    /// migration the author declared.
    pub fn of_request(request: &SkillUpgradeRequest) -> Result<Self, String> {
        request.validate()?;
        Ok(Self {
            schema_version: SKILL_UPGRADE_RECEIPT_SCHEMA_VERSION,
            previous_consumer_contract_digest: request.previous.digest()?,
            new_consumer_contract_digest: request.new.digest()?,
            declared_migration_id: request.declared_migration_id.clone(),
        })
    }
}

/// What changed between two consumer contracts (MEDULLA-DECISION-ENGINE-R167.2).
/// Every list holds field names, sorted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractDiff {
    /// Only in the new contract, optional: compatible.
    pub added_optional: Vec<String>,
    /// Only in the new contract, required: breaking.
    pub added_required: Vec<String>,
    /// Only in the previous contract: breaking.
    pub removed: Vec<String>,
    /// In both, with a different `type_name`: breaking.
    pub type_changed: Vec<String>,
    /// In both, optional before and required now: breaking.
    pub became_required: Vec<String>,
}

impl ContractDiff {
    /// Whether the upgrade is additive only: nothing a consumer relied on
    /// was removed, retyped or made mandatory, and nothing mandatory was added.
    pub fn is_compatible(&self) -> bool {
        self.added_required.is_empty()
            && self.removed.is_empty()
            && self.type_changed.is_empty()
            && self.became_required.is_empty()
    }
}

/// Compare two consumer contracts under the additive-only rule. Identical
/// contracts (equal digests) have an empty diff and are compatible. A
/// required field that becomes optional stays compatible. A breaking diff
/// needs a declared migration id; that check belongs to the admission, not here.
pub fn compare_contracts(
    previous: &ConsumerContractSchema,
    new: &ConsumerContractSchema,
) -> ContractDiff {
    let old: BTreeMap<&str, &ContractField> = previous
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f))
        .collect();
    let current: BTreeMap<&str, &ContractField> =
        new.fields.iter().map(|f| (f.name.as_str(), f)).collect();
    let mut diff = ContractDiff::default();
    for (name, field) in &current {
        match old.get(name) {
            None if field.required => diff.added_required.push((*name).to_string()),
            None => diff.added_optional.push((*name).to_string()),
            Some(before) => {
                if before.type_name != field.type_name {
                    diff.type_changed.push((*name).to_string());
                }
                if !before.required && field.required {
                    diff.became_required.push((*name).to_string());
                }
            }
        }
    }
    diff.removed = old
        .keys()
        .filter(|name| !current.contains_key(*name))
        .map(|name| (*name).to_string())
        .collect();
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(migration: Option<&str>) -> SkillUpgradeReceipt {
        SkillUpgradeReceipt {
            schema_version: SKILL_UPGRADE_RECEIPT_SCHEMA_VERSION,
            previous_consumer_contract_digest: Digest256::from_bytes([1; 32]),
            new_consumer_contract_digest: Digest256::from_bytes([2; 32]),
            declared_migration_id: migration.map(|id| OpaqueId::new(id).unwrap()),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R167.1
    #[test]
    fn receipt_round_trips_with_and_without_migration() {
        for original in [receipt(Some("migration:skill-v2")), receipt(None)] {
            let bytes = rmp_serde::to_vec_named(&original).unwrap();
            let decoded = rmp_serde::from_slice::<SkillUpgradeReceipt>(&bytes).unwrap();
            assert_eq!(decoded, original);
            assert_ne!(
                decoded.previous_consumer_contract_digest,
                decoded.new_consumer_contract_digest
            );
        }
    }

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

    // spec: MEDULLA-DECISION-ENGINE-R167.2
    #[test]
    fn additive_only_diff() {
        let previous = contract(&[("a", "int", true), ("b", "text", false)]);

        // Equal contracts: equal digests, an empty diff, compatible.
        let same = contract(&[("b", "text", false), ("a", "int", true)]);
        assert_eq!(previous.digest().unwrap(), same.digest().unwrap());
        let diff = compare_contracts(&previous, &same);
        assert_eq!(diff, ContractDiff::default());
        assert!(diff.is_compatible());

        // Addition of an optional field is compatible; a required one is breaking.
        let added = compare_contracts(
            &previous,
            &contract(&[
                ("a", "int", true),
                ("b", "text", false),
                ("c", "int", false),
            ]),
        );
        assert_eq!(added.added_optional, vec!["c"]);
        assert!(added.is_compatible());
        let added_required = compare_contracts(
            &previous,
            &contract(&[("a", "int", true), ("b", "text", false), ("c", "int", true)]),
        );
        assert_eq!(added_required.added_required, vec!["c"]);
        assert!(!added_required.is_compatible());

        // Removal is breaking.
        let removed = compare_contracts(&previous, &contract(&[("a", "int", true)]));
        assert_eq!(removed.removed, vec!["b"]);
        assert!(!removed.is_compatible());

        // A type change is breaking.
        let retyped = compare_contracts(
            &previous,
            &contract(&[("a", "text", true), ("b", "text", false)]),
        );
        assert_eq!(retyped.type_changed, vec!["a"]);
        assert!(!retyped.is_compatible());

        // Optional to required is breaking; required to optional is compatible.
        let tightened = compare_contracts(
            &previous,
            &contract(&[("a", "int", true), ("b", "text", true)]),
        );
        assert_eq!(tightened.became_required, vec!["b"]);
        assert!(!tightened.is_compatible());
        let relaxed = compare_contracts(
            &previous,
            &contract(&[("a", "int", false), ("b", "text", false)]),
        );
        assert_eq!(relaxed, ContractDiff::default());
        assert!(relaxed.is_compatible());

        // Several changes are all reported.
        let mixed = compare_contracts(
            &previous,
            &contract(&[("a", "text", true), ("c", "int", true)]),
        );
        assert_eq!(
            (mixed.removed, mixed.type_changed, mixed.added_required),
            (
                vec!["b".to_string()],
                vec!["a".to_string()],
                vec!["c".to_string()]
            )
        );
    }
}
