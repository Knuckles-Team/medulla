//! Sub-call signatures and the admission check run before a sub-call is
//! dispatched (MEDULLA-DECISION-ENGINE-R137.15).
//!
//! A [`SubCallSignature`] is a tenant graph node (`sub-call-signature:<id>`,
//! engine-owned marker `_sub_call_signature`). A call is admitted only when the
//! stored signature matches the revision and digest the call names, the input
//! schema digest matches, every tool is admitted and the model matches. Every
//! mismatch is an abstention that names the expected and the actual value.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::record::outcome::{DecisionRefusal, RefusalCode};
use medulla_query_types::solve_scalar::digest_text;

/// The domain of a tool-set digest.
pub const SUB_CALL_TOOL_SET_DOMAIN: &str = "sub_call_tool_set";

/// The engine-owned marker property of a stored signature node.
pub const SUB_CALL_SIGNATURE_MARKER: &str = "_sub_call_signature";

/// The stored signature of one kind of sub-call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SubCallSignature {
    pub signature_id: String,
    pub revision: u64,
    pub digest: String,
    pub input_schema_digest: String,
    pub output_schema_digest: String,
    pub admitted_tool_ids: Vec<String>,
    pub model_id: String,
}

impl SubCallSignature {
    /// The tenant graph node id the signature is stored at.
    pub fn node_id(signature_id: &str) -> String {
        format!("sub-call-signature:{signature_id}")
    }

    /// Decode a stored node. The marker must be set; any other shape is not a
    /// signature.
    pub fn from_properties(
        props: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<SubCallSignature> {
        if props
            .get(SUB_CALL_SIGNATURE_MARKER)
            .and_then(|v| v.as_bool())
            != Some(true)
        {
            return None;
        }
        let mut record = props.clone();
        record.remove(SUB_CALL_SIGNATURE_MARKER);
        serde_json::from_value(serde_json::Value::Object(record)).ok()
    }

    /// Whether a recorded result node's output schema digest is the one the
    /// signature declares (MEDULLA-DECISION-ENGINE-R133.7.1).
    pub fn result_matches(&self, output_schema_digest: &str) -> bool {
        self.output_schema_digest == output_schema_digest
    }
}

/// One call about to be dispatched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SubCallRequest {
    pub parent_run_id: String,
    pub parent_span_id: String,
    pub call_id: String,
    pub signature_id: String,
    pub signature_revision: u64,
    pub signature_digest: String,
    pub input_digest: String,
    pub input_schema_digest: String,
    pub tool_ids: Vec<String>,
    pub model_id: String,
}

/// The digest of a tool set: order and repeats do not matter.
pub fn tool_set_digest(tool_ids: &[String]) -> String {
    let set: BTreeSet<&str> = tool_ids.iter().map(String::as_str).collect();
    digest_text(SUB_CALL_TOOL_SET_DOMAIN, &set)
}

/// What an admitted call is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubCallBinding {
    pub signature_digest: String,
    pub model_id: String,
    pub tool_set_digest: String,
    pub input_schema_digest: String,
    pub output_schema_digest: String,
}

/// Why a call is not admitted. Each names what was expected and what it got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubCallRefusal {
    /// The stored signature is absent or is not a signature.
    SignatureUnknown { signature_id: String },
    /// The stored signature differs in revision, digest or model.
    SignatureChanged {
        field: &'static str,
        expected: String,
        actual: String,
    },
    /// A tool of the call is not in the signature's admitted set.
    ToolNotAdmitted {
        tool_id: String,
        expected: String,
        actual: String,
    },
    /// The input schema digest differs from the signature's.
    InputSchemaMismatch { expected: String, actual: String },
    /// The parent run or span the call names does not exist.
    ParentMissing { parent_run_id: String },
}

/// Every refusal code of the sub-call admission family.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("SUBCALL_SIGNATURE_UNKNOWN"),
    RefusalCode::new("SUBCALL_SIGNATURE_CHANGED"),
    RefusalCode::new("SUBCALL_TOOL_NOT_ADMITTED"),
    RefusalCode::new("SUBCALL_INPUT_SCHEMA_MISMATCH"),
    RefusalCode::new("SUBCALL_PARENT_MISSING"),
];

impl DecisionRefusal for SubCallRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::SignatureUnknown { .. } => REFUSALS[0],
            Self::SignatureChanged { .. } => REFUSALS[1],
            Self::ToolNotAdmitted { .. } => REFUSALS[2],
            Self::InputSchemaMismatch { .. } => REFUSALS[3],
            Self::ParentMissing { .. } => REFUSALS[4],
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::SignatureUnknown { signature_id } => {
                format!("no stored signature {signature_id}")
            }
            Self::SignatureChanged {
                field,
                expected,
                actual,
            } => format!("{field}: expected {expected}, got {actual}"),
            Self::ToolNotAdmitted {
                tool_id,
                expected,
                actual,
            } => format!("tool {tool_id} is not admitted: expected set {expected}, got {actual}"),
            Self::InputSchemaMismatch { expected, actual } => {
                format!("input schema: expected {expected}, got {actual}")
            }
            Self::ParentMissing { parent_run_id } => {
                format!("parent run {parent_run_id} or its span is not recorded")
            }
        }
    }
}

/// Run the four admission checks. `stored` is the signature node the request
/// names, `parent_known` whether its parent run node exists.
pub fn check_sub_call(
    request: &SubCallRequest,
    stored: Option<&SubCallSignature>,
    parent_known: bool,
) -> Result<SubCallBinding, SubCallRefusal> {
    if !parent_known || request.parent_span_id.trim().is_empty() {
        return Err(SubCallRefusal::ParentMissing {
            parent_run_id: request.parent_run_id.clone(),
        });
    }
    let Some(stored) = stored else {
        return Err(SubCallRefusal::SignatureUnknown {
            signature_id: request.signature_id.clone(),
        });
    };
    if stored.revision != request.signature_revision {
        return Err(SubCallRefusal::SignatureChanged {
            field: "revision",
            expected: stored.revision.to_string(),
            actual: request.signature_revision.to_string(),
        });
    }
    if stored.digest != request.signature_digest {
        return Err(SubCallRefusal::SignatureChanged {
            field: "digest",
            expected: stored.digest.clone(),
            actual: request.signature_digest.clone(),
        });
    }
    if stored.input_schema_digest != request.input_schema_digest {
        return Err(SubCallRefusal::InputSchemaMismatch {
            expected: stored.input_schema_digest.clone(),
            actual: request.input_schema_digest.clone(),
        });
    }
    let admitted: BTreeSet<&str> = stored
        .admitted_tool_ids
        .iter()
        .map(String::as_str)
        .collect();
    if let Some(tool) = request
        .tool_ids
        .iter()
        .find(|t| !admitted.contains(t.as_str()))
    {
        return Err(SubCallRefusal::ToolNotAdmitted {
            tool_id: tool.clone(),
            expected: tool_set_digest(&stored.admitted_tool_ids),
            actual: tool_set_digest(&request.tool_ids),
        });
    }
    if stored.model_id != request.model_id {
        return Err(SubCallRefusal::SignatureChanged {
            field: "model_id",
            expected: stored.model_id.clone(),
            actual: request.model_id.clone(),
        });
    }
    Ok(SubCallBinding {
        signature_digest: stored.digest.clone(),
        model_id: stored.model_id.clone(),
        tool_set_digest: tool_set_digest(&request.tool_ids),
        input_schema_digest: stored.input_schema_digest.clone(),
        output_schema_digest: stored.output_schema_digest.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored() -> SubCallSignature {
        SubCallSignature {
            signature_id: "sig".into(),
            revision: 2,
            digest: "d2".into(),
            input_schema_digest: "in".into(),
            output_schema_digest: "out".into(),
            admitted_tool_ids: vec!["a".into(), "b".into()],
            model_id: "m".into(),
        }
    }

    fn call() -> SubCallRequest {
        SubCallRequest {
            parent_run_id: "r".into(),
            parent_span_id: "s".into(),
            call_id: "c".into(),
            signature_id: "sig".into(),
            signature_revision: 2,
            signature_digest: "d2".into(),
            input_digest: "x".into(),
            input_schema_digest: "in".into(),
            tool_ids: vec!["b".into()],
            model_id: "m".into(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R137.15
    #[test]
    fn the_checks_run_in_order_and_the_tool_set_digest_ignores_order() {
        assert!(check_sub_call(&call(), Some(&stored()), true).is_ok());
        assert_eq!(
            tool_set_digest(&["a".into(), "b".into(), "a".into()]),
            tool_set_digest(&["b".into(), "a".into()])
        );
        let mut by_node = serde_json::to_value(stored()).unwrap();
        by_node[SUB_CALL_SIGNATURE_MARKER] = serde_json::json!(true);
        let props = by_node.as_object().unwrap().clone();
        assert_eq!(SubCallSignature::from_properties(&props), Some(stored()));
        let mut unmarked = props;
        unmarked.remove(SUB_CALL_SIGNATURE_MARKER);
        assert_eq!(SubCallSignature::from_properties(&unmarked), None);
        assert!(stored().result_matches("out") && !stored().result_matches("other"));
        for refusal in REFUSALS {
            assert!(refusal.as_str().starts_with("SUBCALL_"));
        }
    }
}
