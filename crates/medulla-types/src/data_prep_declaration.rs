//! Data-prep declaration record (MEDULLA-INGEST-WORKER-R001.1006).
//!
//! A declaration names a capability, the opaque field references it prepares
//! and the digest of the policy it ran under. It holds no credential, import
//! path or executable. Pure data; the served `DataPrepDeclare` operation is a
//! later child of the same family.

use crate::contract::Digest256;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeSet;

/// Most field references one declaration may carry.
pub const MAX_FIELD_REFS: usize = 256;
/// Longest field reference, in bytes.
pub const MAX_FIELD_REF_BYTES: usize = 256;
/// Longest capability name, in bytes.
pub const MAX_CAPABILITY_BYTES: usize = 128;

/// A declaration that failed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataPrepDeclarationError {
    /// The capability does not match `[a-z][a-z0-9_.-]{0,127}`.
    Capability,
    /// More than 256 references.
    TooManyFieldRefs,
    /// A reference is empty or longer than 256 bytes.
    FieldRef,
}

impl std::fmt::Display for DataPrepDeclarationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Capability => "capability must match [a-z][a-z0-9_.-]{0,127}",
            Self::TooManyFieldRefs => "at most 256 field references are allowed",
            Self::FieldRef => "a field reference must be 1 to 256 bytes",
        })
    }
}

impl std::error::Error for DataPrepDeclarationError {}

/// One data-prep declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DataPrepDeclaration {
    capability: String,
    field_refs: BTreeSet<String>,
    policy_digest: Digest256,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    capability: String,
    field_refs: BTreeSet<String>,
    policy_digest: Digest256,
}

impl DataPrepDeclaration {
    /// Validate and build a declaration. The reference set is sorted by type.
    pub fn new(
        capability: impl Into<String>,
        field_refs: impl IntoIterator<Item = String>,
        policy_digest: Digest256,
    ) -> Result<Self, DataPrepDeclarationError> {
        let capability = capability.into();
        let bytes = capability.as_bytes();
        let valid = !bytes.is_empty()
            && bytes.len() <= MAX_CAPABILITY_BYTES
            && bytes[0].is_ascii_lowercase()
            && bytes[1..]
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-'));
        if !valid {
            return Err(DataPrepDeclarationError::Capability);
        }
        let field_refs: BTreeSet<String> = field_refs.into_iter().collect();
        if field_refs.len() > MAX_FIELD_REFS {
            return Err(DataPrepDeclarationError::TooManyFieldRefs);
        }
        if field_refs
            .iter()
            .any(|r| r.is_empty() || r.len() > MAX_FIELD_REF_BYTES)
        {
            return Err(DataPrepDeclarationError::FieldRef);
        }
        Ok(Self {
            capability,
            field_refs,
            policy_digest,
        })
    }

    pub fn capability(&self) -> &str {
        &self.capability
    }

    pub fn field_refs(&self) -> &BTreeSet<String> {
        &self.field_refs
    }

    pub fn policy_digest(&self) -> Digest256 {
        self.policy_digest
    }
}

impl<'de> Deserialize<'de> for DataPrepDeclaration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Raw::deserialize(deserializer)?;
        Self::new(raw.capability, raw.field_refs, raw.policy_digest)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest() -> Digest256 {
        Digest256::sha256(b"policy")
    }

    // spec: MEDULLA-INGEST-WORKER-R001.1006
    #[test]
    fn declaration_round_trips_and_rejects_unknown_fields() {
        let declaration = DataPrepDeclaration::new(
            "normalize.v1",
            ["b".to_string(), "a".to_string(), "a".to_string()],
            digest(),
        )
        .unwrap();
        assert_eq!(declaration.field_refs().len(), 2);
        let json = serde_json::to_value(&declaration).unwrap();
        assert_eq!(json["field_refs"], serde_json::json!(["a", "b"]));
        let back: DataPrepDeclaration = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back, declaration);
        let packed: DataPrepDeclaration =
            rmp_serde::from_slice(&rmp_serde::to_vec_named(&declaration).unwrap()).unwrap();
        assert_eq!(packed, declaration);

        let mut extra = json.clone();
        extra["import_path"] = "os.system".into();
        assert!(serde_json::from_value::<DataPrepDeclaration>(extra).is_err());
        for bad in ["", "Upper", "1abc", "has space", &"a".repeat(129)] {
            assert_eq!(
                DataPrepDeclaration::new(bad, [], digest()).unwrap_err(),
                DataPrepDeclarationError::Capability
            );
        }
        let many = (0..257).map(|i| format!("f{i}"));
        assert_eq!(
            DataPrepDeclaration::new("ok", many, digest()).unwrap_err(),
            DataPrepDeclarationError::TooManyFieldRefs
        );
        assert_eq!(
            DataPrepDeclaration::new("ok", ["x".repeat(257)], digest()).unwrap_err(),
            DataPrepDeclarationError::FieldRef
        );
        let mut bad_cap = json;
        bad_cap["capability"] = "Bad".into();
        assert!(serde_json::from_value::<DataPrepDeclaration>(bad_cap).is_err());
    }
}
