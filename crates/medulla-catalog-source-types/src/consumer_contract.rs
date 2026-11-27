//! The consumer-declared contract of a skill, tool or prompt and the additive-only
//! comparison of two of them (MEDULLA-DECISION-ENGINE-R167.1, R167.2).
//!
//! A contract is the sorted list of fields a consumer relies on. It lives in the catalog
//! layer so the skill upgrade question, the decision family that answers it and the
//! skill admission record (MEDULLA-TYPED-PACKS-R100.101) share one definition and one
//! comparison rule.

use std::collections::BTreeMap;

use medulla_types::contract::{BoundedVec, Digest256};
use serde::{Deserialize, Serialize};

/// Most fields one consumer contract may declare.
pub const MAX_CONTRACT_FIELDS: usize = 256;
/// Domain of a consumer contract digest.
pub const CONSUMER_CONTRACT_DIGEST_DOMAIN: &[u8] = b"eg/consumer-contract/v1";

/// One field a consumer relies on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContractField {
    /// The field name.
    pub name: String,
    /// The field type name.
    pub type_name: String,
    /// Whether consumers must supply it.
    pub required: bool,
}

/// The fields a consumer relies on, sorted by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ConsumerContractSchema {
    /// The fields, sorted by name and unique.
    pub fields: BoundedVec<ContractField, MAX_CONTRACT_FIELDS>,
}

impl ConsumerContractSchema {
    /// Sort the fields by name and refuse an empty or duplicated name.
    pub fn new(mut fields: Vec<ContractField>) -> Result<Self, String> {
        fields.sort_by(|a, b| a.name.cmp(&b.name));
        let schema = Self {
            fields: BoundedVec::new(fields)?,
        };
        schema.validate()?;
        Ok(schema)
    }

    /// Fields are named, typed, unique and sorted by name.
    pub fn validate(&self) -> Result<(), String> {
        let fields = self.fields.as_slice();
        if fields
            .iter()
            .any(|f| f.name.trim().is_empty() || f.type_name.trim().is_empty())
        {
            return Err("every contract field has a name and a type".into());
        }
        if fields.windows(2).any(|pair| pair[0].name >= pair[1].name) {
            return Err("contract fields are sorted by name and unique".into());
        }
        Ok(())
    }

    /// SHA-256 over the canonical bytes: for each field in order its name, type
    /// name and `required` flag as framed fields.
    pub fn digest(&self) -> Result<Digest256, String> {
        self.validate()?;
        let flags: Vec<[u8; 1]> = self.fields.iter().map(|f| [u8::from(f.required)]).collect();
        let mut framed: Vec<&[u8]> = Vec::with_capacity(self.fields.len() * 3);
        for (field, flag) in self.fields.iter().zip(&flags) {
            framed.push(field.name.as_bytes());
            framed.push(field.type_name.as_bytes());
            framed.push(flag);
        }
        Digest256::framed(CONSUMER_CONTRACT_DIGEST_DOMAIN, &framed)
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
