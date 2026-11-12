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

/// Refusal code: a value outside the declaration grammar, or an unknown key.
pub const DATA_PREP_DECLARATION_INVALID: &str = "DATA_PREP_DECLARATION_INVALID";
/// Refusal code: more than [`MAX_FIELD_REFS`] field references.
pub const DATA_PREP_DECLARATION_TOO_LARGE: &str = "DATA_PREP_DECLARATION_TOO_LARGE";

/// One data-prep declaration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DataPrepDeclaration {
    pub capability: String,
    pub field_refs: BTreeSet<String>,
    pub policy_digest: Digest256,
}

/// `Method::DataPrepDeclare`: one tenant's declaration payload.
///
/// The payload stays untyped on the wire so an unknown key or a value outside
/// the grammar is refused with `DATA_PREP_DECLARATION_INVALID` by the handler
/// (R001.1009), not by a generic decode failure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DataPrepDeclareRequest {
    pub tenant: String,
    pub payload: serde_json::Value,
}

/// What one served declaration did: exactly one of `written` or `unchanged` is 1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DataPrepDeclareReceipt {
    /// Content digest of the stored declaration.
    pub content_digest: Digest256,
    pub written: u64,
    pub unchanged: u64,
}

/// `Method::DataPrepDeclarationRead`: read one tenant's declaration of a capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DataPrepDeclarationReadRequest {
    pub tenant: String,
    pub capability: String,
}

/// The stored declaration of a capability, or none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DataPrepDeclarationView {
    pub declaration: Option<DataPrepDeclaration>,
    pub content_digest: Option<Digest256>,
}

/// Why a declaration is refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DataPrepDeclarationError {
    #[error("capability must match [a-z][a-z0-9_.-]{{0,127}}")]
    InvalidCapability,
    #[error("a declaration carries at most {MAX_FIELD_REFS} field references")]
    TooManyFieldRefs,
    #[error("a field reference must match [A-Za-z0-9_.-]{{1,{MAX_FIELD_REF_BYTES}}}")]
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
        if self.field_refs.iter().any(|field| !valid_field_ref(field)) {
            return Err(DataPrepDeclarationError::InvalidFieldRef);
        }
        Ok(())
    }
}

/// A field reference holds 1 to 256 of `[A-Za-z0-9_.-]`: a value with `:` or
/// `/` would be an import path or executable and is refused.
fn valid_field_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FIELD_REF_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
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
    /// Verifies MEDULLA-INGEST-WORKER-R001.1006: Model: data-prep declaration.
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
        for bad_ref in ["a:b", "a/b", "has space", ""] {
            let mut path_like = declaration();
            path_like.field_refs.insert(bad_ref.to_string());
            assert_eq!(
                path_like.validate(),
                Err(DataPrepDeclarationError::InvalidFieldRef)
            );
        }
        let mut many = declaration();
        many.field_refs = (0..257).map(|i| format!("f{i}")).collect();
        assert_eq!(
            many.validate(),
            Err(DataPrepDeclarationError::TooManyFieldRefs)
        );
    }

    // spec: MEDULLA-INGEST-WORKER-R001.1008
    /// Verifies MEDULLA-INGEST-WORKER-R001.1008: the wire shapes of both methods.
    #[test]
    fn method_payloads_round_trip_the_python_vectors() {
        let policy = Digest256::sha256(b"policy").to_hex();
        let request = serde_json::json!({
            "tenant": "tenant:test",
            "payload": {
                "capability": "text.normalize-v1",
                "field_refs": ["a.field", "b.field"],
                "policy_digest": policy,
            },
        });
        let decoded: DataPrepDeclareRequest =
            serde_json::from_value(request.clone()).expect("decode request");
        assert_eq!(serde_json::to_value(&decoded).expect("encode"), request);
        let inner: DataPrepDeclaration =
            serde_json::from_value(decoded.payload).expect("decode payload");
        inner.validate().expect("valid declaration");

        let receipt = serde_json::json!({
            "content_digest": policy,
            "written": 1,
            "unchanged": 0,
        });
        let decoded: DataPrepDeclareReceipt =
            serde_json::from_value(receipt.clone()).expect("decode receipt");
        assert_eq!(serde_json::to_value(&decoded).expect("encode"), receipt);

        let view = serde_json::json!({ "declaration": null, "content_digest": null });
        let decoded: DataPrepDeclarationView =
            serde_json::from_value(view.clone()).expect("decode view");
        assert_eq!(serde_json::to_value(&decoded).expect("encode"), view);

        let read =
            serde_json::json!({ "tenant": "tenant:test", "capability": "text.normalize-v1" });
        let decoded: DataPrepDeclarationReadRequest =
            serde_json::from_value(read.clone()).expect("decode read");
        assert_eq!(serde_json::to_value(&decoded).expect("encode"), read);
    }
}
