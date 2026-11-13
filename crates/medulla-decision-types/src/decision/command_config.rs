//! Command config records (MEDULLA-DECISION-ENGINE-R148.1).
//!
//! A command's settings are typed against a schema and grouped into numbered
//! modes. Equal settings always give equal digests, so a mode decision names
//! exactly the configuration it chose.

use std::collections::BTreeMap;

use medulla_query_types::solve_scalar::{digest_text, DIGEST_TEXT_PREFIX};
use serde::{Deserialize, Serialize};

use medulla_types::contract::Digest256;

/// The domain of a config digest.
pub const COMMAND_CONFIG_DIGEST_DOMAIN: &str = "eg/command-config";
/// Format identity of every record here.
pub const COMMAND_CONFIG_SCHEMA_VERSION: u16 = 1;

/// One setting value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ConfigScalar {
    Int(i64),
    Text(String),
    Bool(bool),
}

/// The type of one setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum FieldType {
    Int,
    Text,
    Bool,
}

impl ConfigScalar {
    /// The field type this value satisfies.
    pub fn field_type(&self) -> FieldType {
        match self {
            Self::Int(_) => FieldType::Int,
            Self::Text(_) => FieldType::Text,
            Self::Bool(_) => FieldType::Bool,
        }
    }
}

/// The typed shape of a command's settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CommandConfigSchema {
    pub schema_version: u16,
    pub schema_id: String,
    pub revision: u64,
    pub fields: BTreeMap<String, FieldType>,
}

/// One numbered mode of a command: a named set of settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CommandMode {
    pub schema_version: u16,
    pub mode_id: String,
    pub mode_ordinal: u32,
    pub mode_revision: u64,
    pub schema_id: String,
    pub settings: BTreeMap<String, ConfigScalar>,
}

/// A configuration chosen for a command, with the digest of its settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CommandConfig {
    pub schema_version: u16,
    pub schema_id: String,
    pub mode_id: String,
    pub config_digest: Digest256,
    pub settings: BTreeMap<String, ConfigScalar>,
}

/// The decision: which mode, at which revision, with which configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ModeDecision {
    pub schema_version: u16,
    pub mode_id: String,
    pub mode_revision: u64,
    pub config: CommandConfig,
    pub schema_digest: Digest256,
}

/// The digest of a settings map: `digest_text("eg/command-config", settings)`.
pub fn settings_digest(settings: &BTreeMap<String, ConfigScalar>) -> Digest256 {
    let text = digest_text(COMMAND_CONFIG_DIGEST_DOMAIN, settings);
    let hex = text.strip_prefix(DIGEST_TEXT_PREFIX).unwrap_or(&text);
    Digest256::parse(hex).unwrap_or_else(|_| Digest256::sha256(text.as_bytes()))
}

impl CommandConfig {
    /// Build a config whose digest is computed from its settings.
    pub fn new(
        schema_id: impl Into<String>,
        mode_id: impl Into<String>,
        settings: BTreeMap<String, ConfigScalar>,
    ) -> Self {
        Self {
            schema_version: COMMAND_CONFIG_SCHEMA_VERSION,
            schema_id: schema_id.into(),
            mode_id: mode_id.into(),
            config_digest: settings_digest(&settings),
            settings,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(retries: i64) -> BTreeMap<String, ConfigScalar> {
        BTreeMap::from([
            ("retries".to_string(), ConfigScalar::Int(retries)),
            ("label".to_string(), ConfigScalar::Text("fast".into())),
            ("dry_run".to_string(), ConfigScalar::Bool(false)),
        ])
    }

    // spec: MEDULLA-DECISION-ENGINE-R148.1
    #[test]
    fn config_round_trip() {
        let a = CommandConfig::new("schema:cmd", "mode:fast", settings(3));
        let b = CommandConfig::new("schema:cmd", "mode:fast", settings(3));
        let c = CommandConfig::new("schema:cmd", "mode:fast", settings(4));
        assert_eq!(
            a.config_digest, b.config_digest,
            "equal settings, equal digest"
        );
        assert_ne!(a.config_digest, c.config_digest);
        assert_eq!(a.config_digest.to_hex().len(), 64);
        let text = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<CommandConfig>(&text).unwrap(), a);

        let schema = CommandConfigSchema {
            schema_version: COMMAND_CONFIG_SCHEMA_VERSION,
            schema_id: "schema:cmd".into(),
            revision: 2,
            fields: BTreeMap::from([("retries".to_string(), FieldType::Int)]),
        };
        let mode = CommandMode {
            schema_version: COMMAND_CONFIG_SCHEMA_VERSION,
            mode_id: "mode:fast".into(),
            mode_ordinal: 1,
            mode_revision: 7,
            schema_id: "schema:cmd".into(),
            settings: settings(3),
        };
        let decision = ModeDecision {
            schema_version: COMMAND_CONFIG_SCHEMA_VERSION,
            mode_id: mode.mode_id.clone(),
            mode_revision: mode.mode_revision,
            config: a.clone(),
            schema_digest: Digest256::sha256(&serde_json::to_vec(&schema).unwrap()),
        };
        let wire = serde_json::to_string(&schema).unwrap();
        assert_eq!(
            serde_json::from_str::<CommandConfigSchema>(&wire).unwrap(),
            schema
        );
        let wire = serde_json::to_string(&mode).unwrap();
        assert_eq!(serde_json::from_str::<CommandMode>(&wire).unwrap(), mode);
        let wire = serde_json::to_string(&decision).unwrap();
        assert_eq!(
            serde_json::from_str::<ModeDecision>(&wire).unwrap(),
            decision
        );
        assert_eq!(ConfigScalar::Int(1).field_type(), FieldType::Int);

        // An unknown field is rejected.
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<CommandConfig>(value).is_err());
    }
}
