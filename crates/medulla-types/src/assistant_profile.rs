//! The assistant profile and document-set model
//! (MEDULLA-DECISION-ENGINE-R656.1).
//!
//! A profile is a named set of instructions, document sets, a tool allowlist
//! and model hints, owned by one principal and shared by role. A share reuses
//! the grant vocabulary of [`crate::acl`] (`RbacAction`, `GrantEffect`); there
//! is no second ACL type. Every type refuses unknown fields on the wire.

use serde::{Deserialize, Serialize};

use crate::acl::{GrantEffect, RbacAction};

/// Version of the serialized profile types.
pub const ASSISTANT_PROFILE_SCHEMA_VERSION: u32 = 1;

/// The `knob profile.instructions_max_chars` default; the served handler
/// reads the live value and passes it to [`AssistantProfile::check`].
pub const DEFAULT_INSTRUCTIONS_MAX_CHARS: usize = 8000;

/// Preferences for the model that runs under a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ModelHints {
    pub preferred_model_profile_refs: Vec<String>,
    pub max_context_tokens: Option<u32>,
}

/// One share of a profile or document set: a role may or may not perform an
/// action on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProfileShare {
    pub role: String,
    pub action: RbacAction,
    pub effect: GrantEffect,
}

/// A named assistant profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AssistantProfile {
    pub profile_id: String,
    pub tenant: String,
    pub name: String,
    pub instructions: String,
    pub document_set_ids: Vec<String>,
    pub tool_allowlist: Vec<String>,
    pub model_hints: ModelHints,
    pub owner_principal: String,
    pub shares: Vec<ProfileShare>,
    pub revision: u64,
    pub schema_version: u32,
}

/// What a document set holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DocumentSetKind {
    /// Document or source IRIs.
    Members(Vec<String>),
    /// One read-only UQL text.
    SavedQuery(String),
}

/// A named set of documents a profile reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DocumentSet {
    pub set_id: String,
    pub tenant: String,
    pub name: String,
    pub kind: DocumentSetKind,
    pub owner_principal: String,
    pub shares: Vec<ProfileShare>,
    pub schema_version: u32,
}

impl AssistantProfile {
    /// Structural admission: the schema version, a non-empty id, tenant and
    /// name, and instructions within `max_instructions_chars` characters.
    pub fn check(&self, max_instructions_chars: usize) -> Result<(), String> {
        if self.schema_version != ASSISTANT_PROFILE_SCHEMA_VERSION {
            return Err(format!(
                "assistant profile schema_version {} is not served",
                self.schema_version
            ));
        }
        for (field, value) in [
            ("profile_id", &self.profile_id),
            ("tenant", &self.tenant),
            ("name", &self.name),
            ("owner_principal", &self.owner_principal),
        ] {
            if value.trim().is_empty() {
                return Err(format!("assistant profile {field} is empty"));
            }
        }
        let chars = self.instructions.chars().count();
        if chars > max_instructions_chars {
            return Err(format!(
                "instructions hold {chars} characters, over the limit of {max_instructions_chars}"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> AssistantProfile {
        AssistantProfile {
            profile_id: "p-1".into(),
            tenant: "t".into(),
            name: "Research helper".into(),
            instructions: "Answer from the cited documents only.".into(),
            document_set_ids: vec!["set-1".into()],
            tool_allowlist: vec!["web_search".into()],
            model_hints: ModelHints {
                preferred_model_profile_refs: vec!["large".into()],
                max_context_tokens: Some(32_000),
            },
            owner_principal: "alice".into(),
            shares: vec![ProfileShare {
                role: "analyst".into(),
                action: RbacAction::Read,
                effect: GrantEffect::Allow,
            }],
            revision: 3,
            schema_version: ASSISTANT_PROFILE_SCHEMA_VERSION,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R656.1
    #[test]
    fn profile_round_trips_and_rejects_unknown_fields() {
        let original = profile();
        let wire = serde_json::to_value(&original).unwrap();
        assert_eq!(
            serde_json::from_value::<AssistantProfile>(wire.clone()).unwrap(),
            original
        );
        let mut extra = wire;
        extra["surprise"] = serde_json::json!(true);
        assert!(serde_json::from_value::<AssistantProfile>(extra).is_err());

        let set = DocumentSet {
            set_id: "set-1".into(),
            tenant: "t".into(),
            name: "Policies".into(),
            kind: DocumentSetKind::SavedQuery("SELECT ?d WHERE { ?d a :Document }".into()),
            owner_principal: "alice".into(),
            shares: original.shares.clone(),
            schema_version: ASSISTANT_PROFILE_SCHEMA_VERSION,
        };
        let wire = serde_json::to_value(&set).unwrap();
        assert_eq!(
            serde_json::from_value::<DocumentSet>(wire.clone()).unwrap(),
            set
        );
        let members = DocumentSet {
            kind: DocumentSetKind::Members(vec!["urn:doc:1".into()]),
            ..set
        };
        let wire = serde_json::to_value(&members).unwrap();
        assert_eq!(
            serde_json::from_value::<DocumentSet>(wire).unwrap(),
            members
        );
        let mut bad = serde_json::to_value(&members).unwrap();
        bad["kind"] = serde_json::json!({"members": ["x"], "extra": 1});
        assert!(serde_json::from_value::<DocumentSet>(bad).is_err());

        // The instruction limit counts characters and refuses beyond it.
        assert!(original.check(DEFAULT_INSTRUCTIONS_MAX_CHARS).is_ok());
        assert!(original.check(10).is_err());
        let mut stale = original;
        stale.schema_version = 99;
        assert!(stale.check(DEFAULT_INSTRUCTIONS_MAX_CHARS).is_err());
    }
}
