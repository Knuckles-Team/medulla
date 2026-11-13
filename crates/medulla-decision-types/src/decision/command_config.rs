//! Command config records (MEDULLA-DECISION-ENGINE-R148.1).
//!
//! A command's settings are typed against a schema and grouped into numbered
//! modes. Equal settings always give equal digests, so a mode decision names
//! exactly the configuration it chose.

use std::collections::BTreeMap;

use medulla_query_types::solve_scalar::{digest_text, DIGEST_TEXT_PREFIX};
use serde::{Deserialize, Serialize};

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::record::outcome::{DecisionRefusal, RefusalCode};
use medulla_types::contract::Digest256;

/// Refusal code: no mode matches the request or the default ordinal.
pub const MODE_UNKNOWN: &str = "MODE_UNKNOWN";
/// Refusal code: a setting is missing, unknown or of the wrong type.
pub const CONFIG_INVALID: &str = "CONFIG_INVALID";

/// The refusal codes the `command_config` family declares.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(MODE_UNKNOWN),
    RefusalCode::new(CONFIG_INVALID),
];

/// The `mode.*` knobs. `mode.default_mode_id` is an integer, so it names a
/// mode ordinal.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "mode.default_mode_id",
    unit: "ordinal",
    default: KnobValue::Integer(0),
    min: KnobValue::Integer(0),
    max: KnobValue::Integer(1024),
    safety: KnobSafety::Tunable,
}];

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

/// Why a mode could not be selected or a config could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeRefusal {
    /// No mode matches (`MODE_UNKNOWN`).
    ModeUnknown { detail: String },
    /// A setting fails the schema (`CONFIG_INVALID`).
    ConfigInvalid { detail: String },
}

impl ModeRefusal {
    /// The registered refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ModeUnknown { .. } => MODE_UNKNOWN,
            Self::ConfigInvalid { .. } => CONFIG_INVALID,
        }
    }

    /// The human-readable detail.
    pub fn detail(&self) -> &str {
        match self {
            Self::ModeUnknown { detail } | Self::ConfigInvalid { detail } => detail,
        }
    }
}

impl DecisionRefusal for ModeRefusal {
    fn code(&self) -> RefusalCode {
        RefusalCode::new(ModeRefusal::code(self))
    }

    fn detail(&self) -> String {
        ModeRefusal::detail(self).to_string()
    }
}

/// Pick a mode: the one named by `requested`, or, without a request, the one
/// whose ordinal equals `default_ordinal` (the `mode.default_mode_id` knob).
pub fn select_mode<'a>(
    table: &'a [CommandMode],
    requested: Option<&str>,
    default_ordinal: u32,
) -> Result<&'a CommandMode, ModeRefusal> {
    let found = match requested {
        Some(id) => table.iter().find(|mode| mode.mode_id == id),
        None => table
            .iter()
            .find(|mode| mode.mode_ordinal == default_ordinal),
    };
    found.ok_or_else(|| ModeRefusal::ModeUnknown {
        detail: match requested {
            Some(id) => format!("no mode with id {id}"),
            None => format!("no mode with default ordinal {default_ordinal}"),
        },
    })
}

/// Build the immutable config of a mode, checking every setting against the
/// schema. Unknown and missing keys and wrong types refuse.
pub fn build_config(
    mode: &CommandMode,
    schema: &CommandConfigSchema,
) -> Result<CommandConfig, ModeRefusal> {
    if mode.schema_id != schema.schema_id {
        return Err(ModeRefusal::ConfigInvalid {
            detail: format!(
                "mode {} names schema {} but schema {} was given",
                mode.mode_id, mode.schema_id, schema.schema_id
            ),
        });
    }
    for (key, value) in &mode.settings {
        match schema.fields.get(key) {
            None => {
                return Err(ModeRefusal::ConfigInvalid {
                    detail: format!("unknown setting {key}"),
                })
            }
            Some(expected) if *expected != value.field_type() => {
                return Err(ModeRefusal::ConfigInvalid {
                    detail: format!(
                        "setting {key} is {:?}, the schema requires {expected:?}",
                        value.field_type()
                    ),
                })
            }
            Some(_) => {}
        }
    }
    if let Some(missing) = schema
        .fields
        .keys()
        .find(|key| !mode.settings.contains_key(*key))
    {
        return Err(ModeRefusal::ConfigInvalid {
            detail: format!("missing setting {missing}"),
        });
    }
    Ok(CommandConfig::new(
        mode.schema_id.clone(),
        mode.mode_id.clone(),
        mode.settings.clone(),
    ))
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

    fn schema() -> CommandConfigSchema {
        CommandConfigSchema {
            schema_version: COMMAND_CONFIG_SCHEMA_VERSION,
            schema_id: "schema:cmd".into(),
            revision: 1,
            fields: BTreeMap::from([
                ("retries".to_string(), FieldType::Int),
                ("label".to_string(), FieldType::Text),
                ("dry_run".to_string(), FieldType::Bool),
            ]),
        }
    }

    fn mode(id: &str, ordinal: u32, retries: i64) -> CommandMode {
        CommandMode {
            schema_version: COMMAND_CONFIG_SCHEMA_VERSION,
            mode_id: id.into(),
            mode_ordinal: ordinal,
            mode_revision: 1,
            schema_id: "schema:cmd".into(),
            settings: settings(retries),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R148.2
    #[test]
    fn replay_idempotence() {
        let m = mode("mode:fast", 1, 3);
        let first = build_config(&m, &schema()).unwrap();
        let second = build_config(&m, &schema()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.config_digest, second.config_digest);
        let changed = build_config(&mode("mode:fast", 1, 4), &schema()).unwrap();
        assert_ne!(first.config_digest, changed.config_digest);
        // Unknown, missing and mistyped settings refuse with CONFIG_INVALID.
        let mut extra = mode("mode:fast", 1, 3);
        extra
            .settings
            .insert("unknown".into(), ConfigScalar::Bool(true));
        let mut missing = mode("mode:fast", 1, 3);
        missing.settings.remove("label");
        let mut mistyped = mode("mode:fast", 1, 3);
        mistyped
            .settings
            .insert("retries".into(), ConfigScalar::Text("x".into()));
        for bad in [extra, missing, mistyped] {
            let refusal = build_config(&bad, &schema()).unwrap_err();
            assert_eq!(refusal.code(), CONFIG_INVALID);
            assert!(!refusal.detail().is_empty());
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R148.2
    #[test]
    fn default_ordinal_selects_a_mode() {
        let table = vec![mode("mode:slow", 0, 1), mode("mode:fast", 1, 3)];
        assert_eq!(
            select_mode(&table, None, 1).unwrap().mode_id,
            "mode:fast",
            "the default ordinal picks the mode"
        );
        assert_eq!(
            select_mode(&table, Some("mode:slow"), 1).unwrap().mode_id,
            "mode:slow",
            "a requested id wins over the default"
        );
        let unknown = select_mode(&table, Some("mode:none"), 1).unwrap_err();
        assert_eq!(unknown.code(), MODE_UNKNOWN);
        assert_eq!(
            select_mode(&table, None, 9).unwrap_err().code(),
            MODE_UNKNOWN
        );
        assert!(KNOBS[0].admits(&KNOBS[0].default));
        assert!(!KNOBS[0].admits(&KnobValue::Integer(1025)));
    }
}
