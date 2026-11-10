//! Typed principal metadata (MEDULLA-IDENTITY-R023.7).
//!
//! [`TypedMetadata`] is the closed union of the three typed metadata classes a
//! principal may carry: a [`Preference`] with an explicit share scope, a
//! [`ClientMetadata`] record, and a weighted [`InteractionNetwork`] relation
//! between two principals. Every type carries `schema_version` and refuses
//! unknown fields; metadata references exact canonical principal ids through
//! [`PrincipalRef`] and never infers identity from an alias or display name.
//! The graph labels are `Preference`, `ClientMetadata` and `InteractionNetwork`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use super::segment::ShareScope;
use super::text::validate_principal_id;

/// The wire schema version of the typed metadata.
pub const METADATA_SCHEMA_VERSION: u32 = 1;
/// Most characters one preference key holds.
pub const MAX_PREFERENCE_KEY_CHARS: usize = 64;
/// Most characters one preference value holds.
pub const MAX_PREFERENCE_VALUE_CHARS: usize = 1024;
/// Most characters one client id holds.
pub const MAX_CLIENT_ID_CHARS: usize = 128;
/// Most characters one client metadata version holds.
pub const MAX_CLIENT_VERSION_CHARS: usize = 64;
/// Most capability entries one client metadata record carries.
pub const MAX_CLIENT_CAPABILITIES: usize = 32;
/// The top of the `weight_milli` range of an interaction.
pub const MAX_WEIGHT_MILLI: u16 = 1000;

/// The closed client kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ClientKind {
    Cli,
    Web,
    Mobile,
    Service,
}

/// The closed interaction relation between two principals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum InteractionRelation {
    Collaborates,
    Delegates,
    Follows,
}

/// A canonical principal id, validated at construction.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PrincipalRef(String);

impl PrincipalRef {
    /// One canonical principal id: non-empty, bounded, no whitespace or control
    /// characters.
    pub fn new(principal_id: &str) -> Result<Self, InvalidMetadata> {
        validate_principal_id(principal_id).map_err(|_| InvalidMetadata::PrincipalRef)?;
        Ok(Self(principal_id.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PrincipalRef {
    type Error = InvalidMetadata;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<PrincipalRef> for String {
    fn from(value: PrincipalRef) -> String {
        value.0
    }
}

/// A typed metadata value outside its bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidMetadata {
    SchemaVersion,
    PreferenceKeyTooLong,
    PreferenceValueTooLong,
    ClientIdTooLong,
    ClientVersionTooLong,
    TooManyCapabilities,
    WeightOutOfRange,
    PrincipalRef,
}

impl std::fmt::Display for InvalidMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SchemaVersion => "typed metadata schema_version is not supported",
            Self::PreferenceKeyTooLong => "preference key exceeds 64 characters",
            Self::PreferenceValueTooLong => "preference value exceeds 1,024 characters",
            Self::ClientIdTooLong => "client id exceeds 128 characters",
            Self::ClientVersionTooLong => "client metadata version exceeds 64 characters",
            Self::TooManyCapabilities => "client metadata carries more than 32 capabilities",
            Self::WeightOutOfRange => "interaction weight_milli is outside 0..=1000",
            Self::PrincipalRef => "not a canonical principal id",
        })
    }
}

impl std::error::Error for InvalidMetadata {}

fn check_version(version: u32) -> Result<(), InvalidMetadata> {
    if version == METADATA_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(InvalidMetadata::SchemaVersion)
    }
}

fn chars(value: &str) -> usize {
    value.chars().count()
}

/// One principal-owned preference, shared at an explicit scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Preference {
    pub schema_version: u32,
    /// At most [`MAX_PREFERENCE_KEY_CHARS`] characters.
    pub key: String,
    /// At most [`MAX_PREFERENCE_VALUE_CHARS`] characters.
    pub value: String,
    pub share: ShareScope,
}

impl Preference {
    /// Validate the bounds.
    pub fn validate(&self) -> Result<(), InvalidMetadata> {
        check_version(self.schema_version)?;
        if chars(&self.key) > MAX_PREFERENCE_KEY_CHARS {
            return Err(InvalidMetadata::PreferenceKeyTooLong);
        }
        if chars(&self.value) > MAX_PREFERENCE_VALUE_CHARS {
            return Err(InvalidMetadata::PreferenceValueTooLong);
        }
        Ok(())
    }
}

/// One client's bounded metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClientMetadata {
    pub schema_version: u32,
    /// The opaque client id, at most [`MAX_CLIENT_ID_CHARS`] characters.
    pub client_id: String,
    pub kind: ClientKind,
    /// At most [`MAX_CLIENT_VERSION_CHARS`] characters.
    pub version: String,
    /// At most [`MAX_CLIENT_CAPABILITIES`] entries.
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
}

impl ClientMetadata {
    /// Validate the bounds.
    pub fn validate(&self) -> Result<(), InvalidMetadata> {
        check_version(self.schema_version)?;
        if chars(&self.client_id) > MAX_CLIENT_ID_CHARS {
            return Err(InvalidMetadata::ClientIdTooLong);
        }
        if chars(&self.version) > MAX_CLIENT_VERSION_CHARS {
            return Err(InvalidMetadata::ClientVersionTooLong);
        }
        if self.capabilities.len() > MAX_CLIENT_CAPABILITIES {
            return Err(InvalidMetadata::TooManyCapabilities);
        }
        Ok(())
    }
}

/// One weighted relation between two principals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct InteractionNetwork {
    pub schema_version: u32,
    pub subject: PrincipalRef,
    pub peer: PrincipalRef,
    pub relation: InteractionRelation,
    /// Milli-weight, 0 to [`MAX_WEIGHT_MILLI`].
    pub weight_milli: u16,
}

impl InteractionNetwork {
    /// Validate the bounds.
    pub fn validate(&self) -> Result<(), InvalidMetadata> {
        check_version(self.schema_version)?;
        if self.weight_milli > MAX_WEIGHT_MILLI {
            return Err(InvalidMetadata::WeightOutOfRange);
        }
        Ok(())
    }
}

/// The closed union of typed principal metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TypedMetadata {
    Preference(Preference),
    ClientMetadata(ClientMetadata),
    InteractionNetwork(InteractionNetwork),
}

impl TypedMetadata {
    /// The graph label of the variant.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Preference(_) => "Preference",
            Self::ClientMetadata(_) => "ClientMetadata",
            Self::InteractionNetwork(_) => "InteractionNetwork",
        }
    }

    /// Validate the schema version and the field bounds of the variant.
    pub fn validate(&self) -> Result<(), InvalidMetadata> {
        match self {
            Self::Preference(p) => p.validate(),
            Self::ClientMetadata(c) => c.validate(),
            Self::InteractionNetwork(i) => i.validate(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-IDENTITY-R023.7
    #[test]
    fn typed_metadata_variants_round_trip() {
        let metadata = vec![
            TypedMetadata::Preference(Preference {
                schema_version: METADATA_SCHEMA_VERSION,
                key: "theme".into(),
                value: "dark".into(),
                share: ShareScope::Tenant,
            }),
            TypedMetadata::ClientMetadata(ClientMetadata {
                schema_version: METADATA_SCHEMA_VERSION,
                client_id: "cli:0001".into(),
                kind: ClientKind::Cli,
                version: "1.0.0".into(),
                capabilities: BTreeSet::from(["read".to_string(), "write".to_string()]),
            }),
            TypedMetadata::InteractionNetwork(InteractionNetwork {
                schema_version: METADATA_SCHEMA_VERSION,
                subject: PrincipalRef::new("usr:alice").expect("canonical principal id"),
                peer: PrincipalRef::new("usr:bob").expect("canonical principal id"),
                relation: InteractionRelation::Delegates,
                weight_milli: 750,
            }),
        ];

        for (meta, expected) in
            metadata
                .iter()
                .zip(["Preference", "ClientMetadata", "InteractionNetwork"])
        {
            assert_eq!(meta.label(), expected);
            meta.validate().expect("in-bounds metadata validates");
            let encoded = serde_json::to_vec(meta).expect("metadata serializes");
            let decoded: TypedMetadata =
                serde_json::from_slice(&encoded).expect("metadata deserializes");
            assert_eq!(decoded, *meta);
        }

        // An over-bound field is refused by validation...
        let mut long_key = serde_json::to_value(&metadata[0]).expect("preference is JSON");
        long_key["Preference"]["key"] =
            serde_json::Value::String("k".repeat(MAX_PREFERENCE_KEY_CHARS + 1));
        let over_key: TypedMetadata =
            serde_json::from_value(long_key).expect("over-bound key still deserializes");
        assert_eq!(
            over_key.validate(),
            Err(InvalidMetadata::PreferenceKeyTooLong)
        );

        let over_weight = InteractionNetwork {
            schema_version: METADATA_SCHEMA_VERSION,
            subject: PrincipalRef::new("usr:alice").expect("canonical principal id"),
            peer: PrincipalRef::new("usr:bob").expect("canonical principal id"),
            relation: InteractionRelation::Follows,
            weight_milli: MAX_WEIGHT_MILLI + 1,
        };
        assert_eq!(
            over_weight.validate(),
            Err(InvalidMetadata::WeightOutOfRange)
        );

        // ...an unknown field is refused at the wire boundary.
        let mut unknown = serde_json::to_value(&metadata[0]).expect("preference is JSON");
        unknown["Preference"]["surprise"] = serde_json::Value::String("x".into());
        assert!(serde_json::from_value::<TypedMetadata>(unknown).is_err());

        // A closed enum refuses a value outside its set...
        let mut bad_kind = serde_json::to_value(&metadata[1]).expect("client metadata is JSON");
        bad_kind["ClientMetadata"]["kind"] = serde_json::Value::String("tv".into());
        assert!(serde_json::from_value::<TypedMetadata>(bad_kind).is_err());

        // ...and a principal id stops being canonical the moment it is not one.
        let mut aliased = serde_json::to_value(&metadata[2]).expect("network is JSON");
        aliased["InteractionNetwork"]["peer"] = serde_json::Value::String("usr: bob".into());
        assert!(serde_json::from_value::<TypedMetadata>(aliased).is_err());
        assert!(PrincipalRef::new("").is_err());
        assert!(PrincipalRef::new("has whitespace").is_err());
    }
}
