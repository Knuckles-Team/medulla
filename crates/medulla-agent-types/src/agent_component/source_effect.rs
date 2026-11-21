//! The receipt of an approved repository-source effect, recorded on the component
//! revision it exported (MEDULLA-TYPED-PACKS-R094.8).
//!
//! Medulla never writes a repository file and never grants approval. The effect is
//! produced and receipted elsewhere; this is the evidence that revision N of a
//! component was applied to a source, keyed by the receipt's own digest.

use serde::{Deserialize, Serialize};

use super::validate_text;

/// Refusal: a digest is not SHA-256 as 64 lowercase hex characters.
pub const RECEIPT_DIGEST_INVALID: &str = "RECEIPT_DIGEST_INVALID";
/// Refusal: the tenant has no such revision of the component.
pub const RECEIPT_REVISION_UNKNOWN: &str = "RECEIPT_REVISION_UNKNOWN";
/// Refusal: the receipt digest is already recorded with other bytes.
pub const RECEIPT_DIGEST_REUSED: &str = "RECEIPT_DIGEST_REUSED";

/// One recorded source-effect receipt, as it is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SourceEffectReceipt {
    pub tenant_id: String,
    pub component_id: String,
    pub revision: u64,
    pub repository: String,
    pub path: String,
    /// The repository revision the effect actually produced.
    pub effect_revision: String,
    /// SHA-256 of the approved effect request, 64 lowercase hex characters.
    pub request_digest: String,
    /// SHA-256 of the receipt, 64 lowercase hex characters.
    pub receipt_digest: String,
}

/// The answer to a recording: `recorded` is false when the same receipt was
/// already held, so a replay writes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SourceEffectRecordResult {
    pub component_id: String,
    pub revision: u64,
    pub receipt_digest: String,
    pub recorded: bool,
}

fn is_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

impl SourceEffectReceipt {
    /// Refuse a malformed receipt before anything is read or written.
    pub fn validate(&self) -> Result<(), String> {
        validate_text("tenant_id", &self.tenant_id)?;
        validate_text("component_id", &self.component_id)?;
        validate_text("repository", &self.repository)?;
        validate_text("path", &self.path)?;
        validate_text("effect_revision", &self.effect_revision)?;
        for digest in [&self.request_digest, &self.receipt_digest] {
            if !is_hex_digest(digest) {
                return Err(format!(
                    "{RECEIPT_DIGEST_INVALID}: a digest is SHA-256 as 64 lowercase hex characters"
                ));
            }
        }
        Ok(())
    }

    /// The table key of this receipt: one row per component, revision and receipt digest.
    pub fn key(&self) -> String {
        format!(
            "source-effect:{}:{}:{}",
            self.component_id, self.revision, self.receipt_digest
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt() -> SourceEffectReceipt {
        SourceEffectReceipt {
            tenant_id: "tenant".into(),
            component_id: "component".into(),
            revision: 1,
            repository: "repo".into(),
            path: "prompts/a.json".into(),
            effect_revision: "abc123".into(),
            request_digest: "a".repeat(64),
            receipt_digest: "b".repeat(64),
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R094.8
    #[test]
    fn a_digest_must_be_sha256_lowercase_hex() {
        assert!(receipt().validate().is_ok());
        for bad in ["", "ABCDEF", &"A".repeat(64), &"b".repeat(63), &"g".repeat(64)] {
            let mut bad_receipt = receipt();
            bad_receipt.receipt_digest = bad.to_string();
            let error = bad_receipt.validate().unwrap_err();
            assert!(error.starts_with(RECEIPT_DIGEST_INVALID), "{error}");
        }
        assert_eq!(
            receipt().key(),
            format!("source-effect:component:1:{}", "b".repeat(64))
        );
    }
}
