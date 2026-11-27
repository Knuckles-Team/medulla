//! The credential use receipt (MEDULLA-IDENTITY-R753).
//!
//! One receipt is written per credentialed decision: who used which connector
//! resource for which operation, with what outcome, under which binding. It
//! names the binding by digest and carries no secret value. The record is a
//! graph node whose id derives from the receipt's own fields, so a retry of the
//! same decision writes the same node.

use serde::{Deserialize, Serialize};

use medulla_types::contract::Digest256;

/// Longest text field of a receipt, in bytes.
pub const MAX_RECEIPT_FIELD_BYTES: usize = 256;
/// The graph node type of a stored receipt.
pub const RECEIPT_NODE_TYPE: &str = "CredentialReceipt";

/// The op of the `CredentialReceipt` method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CredentialReceiptOp {
    /// Record one receipt.
    Record { receipt: CredentialReceipt },
}

/// One credentialed decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CredentialReceipt {
    pub tenant: String,
    pub client: String,
    pub connector: String,
    pub resource: String,
    pub operation: String,
    pub outcome: String,
    /// `sha256:` and 64 lowercase hex characters: the digest of the binding the decision used.
    pub binding_digest: String,
    pub at_ms: u64,
}

/// The answer of a recorded receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CredentialReceiptRecorded {
    /// The node id the receipt is stored under.
    pub receipt_id: String,
}

fn text_field(name: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_RECEIPT_FIELD_BYTES
        || value.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(format!(
            "INVALID_ARGUMENT: receipt field {name} is empty, too long or holds whitespace"
        ));
    }
    Ok(())
}

impl CredentialReceipt {
    /// Check every field is bounded and the binding digest is a `sha256:` digest.
    pub fn check(&self) -> Result<(), String> {
        for (name, value) in [
            ("tenant", &self.tenant),
            ("client", &self.client),
            ("connector", &self.connector),
            ("resource", &self.resource),
            ("operation", &self.operation),
            ("outcome", &self.outcome),
        ] {
            text_field(name, value)?;
        }
        let hex = self
            .binding_digest
            .strip_prefix("sha256:")
            .ok_or_else(|| "INVALID_ARGUMENT: binding_digest is not a sha256: digest".to_string())?;
        Digest256::parse(hex)
            .map(|_| ())
            .map_err(|_| "INVALID_ARGUMENT: binding_digest is not a sha256: digest".to_string())
    }

    /// The node id of this receipt: a digest over its own fields, so one decision is one node.
    pub fn node_id(&self) -> String {
        let at = self.at_ms.to_string();
        let digest = Digest256::framed(
            b"medulla/credential-receipt/v1",
            &[
                self.tenant.as_bytes(),
                self.client.as_bytes(),
                self.connector.as_bytes(),
                self.resource.as_bytes(),
                self.operation.as_bytes(),
                self.outcome.as_bytes(),
                self.binding_digest.as_bytes(),
                at.as_bytes(),
            ],
        )
        .expect("eight bounded fields frame");
        format!("credential-receipt:{}", digest.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt() -> CredentialReceipt {
        CredentialReceipt {
            tenant: "t1".into(),
            client: "agent-a".into(),
            connector: "github".into(),
            resource: "repo:x".into(),
            operation: "read".into(),
            outcome: "allowed".into(),
            binding_digest: format!("sha256:{}", "a".repeat(64)),
            at_ms: 5,
        }
    }

    // spec: MEDULLA-IDENTITY-R753
    #[test]
    fn a_receipt_is_checked_and_its_node_id_follows_its_fields() {
        assert_eq!(receipt().check(), Ok(()));
        assert_eq!(receipt().node_id(), receipt().node_id());
        let mut later = receipt();
        later.at_ms = 6;
        assert_ne!(receipt().node_id(), later.node_id());
        assert!(receipt().node_id().starts_with("credential-receipt:"));
    }

    // spec: MEDULLA-IDENTITY-R753
    #[test]
    fn a_blank_field_or_a_bare_digest_refuses() {
        let mut blank = receipt();
        blank.client = String::new();
        assert!(blank.check().is_err());
        let mut spaced = receipt();
        spaced.resource = "a b".into();
        assert!(spaced.check().is_err());
        let mut bare = receipt();
        bare.binding_digest = "a".repeat(64);
        assert!(bare.check().is_err());
    }
}
