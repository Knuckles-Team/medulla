//! The served prompt render: request, result and receipt
//! (MEDULLA-TYPED-PACKS-R002.9).
//!
//! The engine renders one pinned prompt revision with the caller's variables
//! and answers the bytes together with a receipt. The receipt binds the framed
//! pins of the revision, the digest of the variables and the scope the render
//! was served under, so a later verification (R002.13) recomputes the same
//! digest from the same three inputs.

use std::collections::BTreeMap;

use medulla_types::contract::Digest256;
use serde::{Deserialize, Serialize};

use super::{canonical_blueprint_bytes, frame_pins, validate_text, PromptBlueprintIdentity};

/// Format identity (RF-ADR-006) of [`PromptRenderResult`].
pub const PROMPT_RENDER_SCHEMA_VERSION: u16 = 1;
/// The pins of the revision, or its stored body, do not resolve.
pub const PROMPT_PIN_UNRESOLVED: &str = "PROMPT_PIN_UNRESOLVED";
/// The render needs the engine body store and it is not serving.
pub const PROMPT_PRODUCER_UNAVAILABLE: &str = "PROMPT_PRODUCER_UNAVAILABLE";

/// Render one pinned prompt revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PromptRenderRequest {
    pub tenant_id: String,
    pub component_id: String,
    /// The exact revision; a render never reads a moving head.
    pub revision: u64,
    /// The schema revision the caller expects the blueprint to carry.
    pub schema_revision: u32,
    /// A value for every variable the blueprint declares, and no other.
    pub variables: BTreeMap<String, serde_json::Value>,
}

impl PromptRenderRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_text("tenant_id", &self.tenant_id)?;
        validate_text("component_id", &self.component_id)?;
        if self.revision == 0 {
            return Err(format!(
                "{PROMPT_PIN_UNRESOLVED}: a render names one exact revision"
            ));
        }
        Ok(())
    }
}

/// What a render was served under; verified again before a model dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PromptRenderReceipt {
    pub identity: PromptBlueprintIdentity,
    /// `sha256:` of the canonical JSON of the variables.
    pub variables_digest: String,
    /// The tenant scope the render was served to.
    pub scope: String,
    /// `sha256:` over the framed pins, the variables digest and the scope.
    pub receipt_digest: String,
}

/// The rendered bytes, their digest and the receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PromptRenderResult {
    pub schema_version: u16,
    #[serde(with = "serde_bytes")]
    #[cfg_attr(feature = "contract-schema", schemars(with = "Vec<u8>"))]
    pub bytes: Vec<u8>,
    /// `sha256:` of `bytes`.
    pub render_digest: String,
    pub receipt: PromptRenderReceipt,
}

/// `sha256:` of the canonical JSON of the variables, keys sorted at every depth.
pub fn variables_digest(variables: &BTreeMap<String, serde_json::Value>) -> Result<String, String> {
    let value = serde_json::to_value(variables).map_err(|error| error.to_string())?;
    let canonical = canonical_blueprint_bytes(&value).map_err(str::to_string)?;
    Ok(format!("sha256:{}", Digest256::sha256(&canonical).to_hex()))
}

/// The one receipt digest: the framed pins, then the variables digest and the
/// scope, each as a big-endian `u32` length and its bytes. Rendering issues it
/// and receipt verification recomputes it, so both share this function.
pub fn receipt_digest(
    identity: &PromptBlueprintIdentity,
    variables_digest: &str,
    scope: &str,
) -> String {
    let mut framed = frame_pins(identity);
    for field in [variables_digest, scope] {
        let len = u32::try_from(field.len()).unwrap_or(u32::MAX);
        framed.extend_from_slice(&len.to_be_bytes());
        framed.extend_from_slice(field.as_bytes());
    }
    format!("sha256:{}", Digest256::sha256(&framed).to_hex())
}

impl PromptRenderReceipt {
    /// Build the receipt for one render.
    pub fn issue(
        identity: &PromptBlueprintIdentity,
        variables: &BTreeMap<String, serde_json::Value>,
        scope: &str,
    ) -> Result<Self, String> {
        let variables_digest = variables_digest(variables)?;
        let receipt_digest = receipt_digest(identity, &variables_digest, scope);
        Ok(Self {
            identity: identity.clone(),
            variables_digest,
            scope: scope.to_string(),
            receipt_digest,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_component::{ComponentProvenance, PROMPT_SCHEMA_REVISION};

    fn identity(revision: u64) -> PromptBlueprintIdentity {
        PromptBlueprintIdentity {
            tenant_id: "tenant-a".into(),
            component_id: "prompt-a".into(),
            entry_revision: revision,
            schema_revision: PROMPT_SCHEMA_REVISION,
            body_sha256: "a".repeat(64),
            provenance: ComponentProvenance::Native,
        }
    }

    fn vars(value: &str) -> BTreeMap<String, serde_json::Value> {
        BTreeMap::from([("name".to_string(), serde_json::json!(value))])
    }

    // spec: MEDULLA-TYPED-PACKS-R002.9
    #[test]
    fn receipt_digest_binds_pins_variables_and_scope() {
        let base = PromptRenderReceipt::issue(&identity(1), &vars("x"), "tenant-a").unwrap();
        let again = PromptRenderReceipt::issue(&identity(1), &vars("x"), "tenant-a").unwrap();
        assert_eq!(base, again);
        for other in [
            PromptRenderReceipt::issue(&identity(2), &vars("x"), "tenant-a").unwrap(),
            PromptRenderReceipt::issue(&identity(1), &vars("y"), "tenant-a").unwrap(),
            PromptRenderReceipt::issue(&identity(1), &vars("x"), "tenant-b").unwrap(),
        ] {
            assert_ne!(base.receipt_digest, other.receipt_digest);
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R002.9
    #[test]
    fn render_request_names_one_exact_revision() {
        let mut request = PromptRenderRequest {
            tenant_id: "tenant-a".into(),
            component_id: "prompt-a".into(),
            revision: 1,
            schema_revision: PROMPT_SCHEMA_REVISION,
            variables: vars("x"),
        };
        request.validate().expect("an exact revision is valid");
        request.revision = 0;
        assert!(request
            .validate()
            .unwrap_err()
            .starts_with(PROMPT_PIN_UNRESOLVED));
    }
}
