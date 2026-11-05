//! Data-prep declaration model (MEDULLA-INGEST-WORKER-R001.1006).
//!
//! A declaration names a capability, the opaque field references it reads and
//! the digest of the policy it runs under. It holds no credential, import path
//! or executable: it is pure data decoded by `declare_data_prep`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::contract::Digest256;

/// Most field references one declaration may carry.
pub const MAX_FIELD_REFS: usize = 256;
/// Most bytes one field reference may hold.
pub const MAX_FIELD_REF_BYTES: usize = 256;
/// Most bytes a capability name may hold.
pub const MAX_CAPABILITY_BYTES: usize = 128;

/// One data-prep declaration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataPrepDeclaration {
    pub capability: String,
    pub field_refs: BTreeSet<String>,
    pub policy_digest: Digest256,
}

/// Why a declaration is refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DataPrepDeclarationError {
    #[error("capability must match [a-z][a-z0-9_.-]{{0,127}}")]
    InvalidCapability,
    #[error("a declaration carries at most {MAX_FIELD_REFS} field references")]
    TooManyFieldRefs,
    #[error("a field reference must hold 1 to {MAX_FIELD_REF_BYTES} bytes")]
    InvalidFieldRef,
}

impl DataPrepDeclaration {
    /// Check the capability pattern and the field-reference bounds.
    pub fn validate(&self) -> Result<(), DataPrepDeclarationError> {
        if !valid_capability(&self.capability) {
            return Err(DataPrepDeclarationError::InvalidCapability);
        }
        if self.field_refs.len() > MAX_FIELD_REFS {
            return Err(DataPrepDeclarationError::TooManyFieldRefs);
        }
        if self
            .field_refs
            .iter()
            .any(|field| field.is_empty() || field.len() > MAX_FIELD_REF_BYTES)
        {
            return Err(DataPrepDeclarationError::InvalidFieldRef);
        }
        Ok(())
    }
}

fn valid_capability(value: &str) -> bool {
    let bytes = value.as_bytes();
    match bytes.split_first() {
        Some((first, rest)) => {
            first.is_ascii_lowercase()
                && bytes.len() <= MAX_CAPABILITY_BYTES
                && rest.iter().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'.' | b'-')
                })
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declaration() -> DataPrepDeclaration {
        DataPrepDeclaration {
            capability: "text.normalize-v1".to_string(),
            field_refs: ["b.field", "a.field"]
                .iter()
                .map(|f| f.to_string())
                .collect(),
            policy_digest: Digest256::sha256(b"policy"),
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R001.1006
    #[test]
    fn declaration_round_trips_and_rejects_unknown_fields() {
        let value = declaration();
        value.validate().expect("valid declaration");
        let json = serde_json::to_value(&value).expect("encode");
        assert_eq!(
            json["field_refs"],
            serde_json::json!(["a.field", "b.field"])
        );
        let decoded: DataPrepDeclaration = serde_json::from_value(json.clone()).expect("decode");
        assert_eq!(decoded, value);

        let mut unknown = json;
        unknown["import_path"] = serde_json::json!("/usr/bin/sh");
        assert!(serde_json::from_value::<DataPrepDeclaration>(unknown).is_err());

        for bad in ["", "Upper", "9lead", "has space", &"a".repeat(129)] {
            let mut invalid = declaration();
            invalid.capability = bad.to_string();
            assert_eq!(
                invalid.validate(),
                Err(DataPrepDeclarationError::InvalidCapability)
            );
        }
        let mut long_ref = declaration();
        long_ref.field_refs.insert("x".repeat(257));
        assert_eq!(
            long_ref.validate(),
            Err(DataPrepDeclarationError::InvalidFieldRef)
        );
        let mut many = declaration();
        many.field_refs = (0..257).map(|i| format!("f{i}")).collect();
        assert_eq!(
            many.validate(),
            Err(DataPrepDeclarationError::TooManyFieldRefs)
        );
    }
}
