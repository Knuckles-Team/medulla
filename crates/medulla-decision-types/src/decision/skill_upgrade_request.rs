//! The skill upgrade question: the request model (MEDULLA-DECISION-ENGINE-R167.1).
//!
//! Arbora asks one compatibility question for any definition kind: does the new
//! consumer contract still serve what the previous one promised? The request
//! carries both contracts as typed field lists; their digests are what the
//! sealed receipt records. Nothing here reads a clock or a store.

use medulla_types::contract::{BoundedVec, Digest256, OpaqueId};
use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};

/// A breaking upgrade names no migration (MEDULLA-DECISION-ENGINE-R167.4).
pub const SKILL_UPGRADE_BREAKING_WITHOUT_MIGRATION: RefusalCode =
    RefusalCode::new("SKILL_UPGRADE_BREAKING_WITHOUT_MIGRATION");
/// A claimed contract digest differs from the recomputed one
/// (MEDULLA-DECISION-ENGINE-R167.4).
pub const SKILL_UPGRADE_DIGEST_MISMATCH: RefusalCode =
    RefusalCode::new("SKILL_UPGRADE_DIGEST_MISMATCH");

/// The `skill_upgrade` family refusal table (MEDULLA-DECISION-ENGINE-R167.4),
/// collected by `codes_are_unique_and_stable`.
pub const REFUSALS: &[RefusalCode] = &[
    SKILL_UPGRADE_BREAKING_WITHOUT_MIGRATION,
    SKILL_UPGRADE_DIGEST_MISMATCH,
];

/// Current wire schema version of [`SkillUpgradeReceipt`].
pub const SKILL_UPGRADE_RECEIPT_SCHEMA_VERSION: u16 = 1;

/// Records the old and new consumer contracts and any declared migration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
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

/// Current wire schema version of [`SkillUpgradeRequest`].
pub const SKILL_UPGRADE_SCHEMA_VERSION: u16 = 1;
/// Most fields one consumer contract may declare.
pub const MAX_CONTRACT_FIELDS: usize = 256;
/// Domain of a consumer contract digest.
pub const CONSUMER_CONTRACT_DIGEST_DOMAIN: &[u8] = b"eg/consumer-contract/v1";

/// The kind of definition being upgraded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum UpgradeSubjectKind {
    Skill,
    Tool,
    Prompt,
}

/// One field a consumer relies on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContractField {
    pub name: String,
    pub type_name: String,
    pub required: bool,
}

/// The fields a consumer relies on, sorted by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ConsumerContractSchema {
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

/// One compatibility question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillUpgradeRequest {
    pub schema_version: u16,
    pub subject_kind: UpgradeSubjectKind,
    pub subject_id: OpaqueId,
    pub previous: ConsumerContractSchema,
    pub new: ConsumerContractSchema,
    /// The migration the author declared for a breaking change, if any.
    #[serde(default)]
    pub declared_migration_id: Option<OpaqueId>,
    /// The digest of the previous contract the caller computed, if it claims one.
    #[serde(default)]
    pub claimed_previous_digest: Option<Digest256>,
    /// The digest of the new contract the caller computed, if it claims one.
    #[serde(default)]
    pub claimed_new_digest: Option<Digest256>,
}

impl SkillUpgradeRequest {
    /// Check the version and both contracts.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SKILL_UPGRADE_SCHEMA_VERSION {
            return Err("unsupported skill upgrade schema_version".into());
        }
        self.previous.validate()?;
        self.new.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, type_name: &str, required: bool) -> ContractField {
        ContractField {
            name: name.into(),
            type_name: type_name.into(),
            required,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R167.1
    #[test]
    fn contract_digest_is_order_independent_and_field_sensitive() {
        let a =
            ConsumerContractSchema::new(vec![field("b", "text", true), field("a", "int", false)])
                .unwrap();
        let b =
            ConsumerContractSchema::new(vec![field("a", "int", false), field("b", "text", true)])
                .unwrap();
        assert_eq!(a, b);
        assert_eq!(a.fields.as_slice()[0].name, "a");
        assert_eq!(a.digest().unwrap(), b.digest().unwrap());
        let required_flipped =
            ConsumerContractSchema::new(vec![field("a", "int", true), field("b", "text", true)])
                .unwrap();
        assert_ne!(a.digest().unwrap(), required_flipped.digest().unwrap());
        assert!(ConsumerContractSchema::new(vec![
            field("a", "int", true),
            field("a", "int", true)
        ])
        .is_err());
        assert!(ConsumerContractSchema::new(vec![field("", "int", true)]).is_err());

        let request = SkillUpgradeRequest {
            schema_version: SKILL_UPGRADE_SCHEMA_VERSION,
            subject_kind: UpgradeSubjectKind::Prompt,
            subject_id: OpaqueId::new("skill:summarize").unwrap(),
            previous: a,
            new: required_flipped,
            declared_migration_id: None,
            claimed_previous_digest: None,
            claimed_new_digest: None,
        };
        request.validate().unwrap();
        let wire = serde_json::to_value(&request).unwrap();
        assert_eq!(wire["subject_kind"], "prompt");
        assert_eq!(
            serde_json::from_value::<SkillUpgradeRequest>(wire).unwrap(),
            request
        );
        let packed = rmp_serde::to_vec_named(&request).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<SkillUpgradeRequest>(&packed).unwrap(),
            request
        );
        assert_eq!(
            serde_json::to_string(&crate::decision::statistical::QuestionKind::SkillUpgrade)
                .unwrap(),
            "\"skill_upgrade\""
        );
    }
}
