//! The wire shapes of one retrieval feedback event (MEDULLA-FEDERATED-QUERY-R705.1,
//! R705.3). They live here so the protocol crate can carry them; `medulla-retrieval`
//! re-exports them from `feedback_boost`.

use serde::{Deserialize, Serialize};

/// The kind of a feedback event. Closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FeedbackKind {
    Upvote,
    Downvote,
    Cited,
    Ignored,
}

impl FeedbackKind {
    /// The snake_case wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upvote => "upvote",
            Self::Downvote => "downvote",
            Self::Cited => "cited",
            Self::Ignored => "ignored",
        }
    }

    /// `true` for the two kinds a caller may record; `cited` and `ignored` are derived.
    pub fn is_explicit(self) -> bool {
        matches!(self, Self::Upvote | Self::Downvote)
    }
}

/// One feedback event (MEDULLA-FEDERATED-QUERY-R705.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
// The contract already names a different `RetrievalFeedback` (the verdict feedback).
#[cfg_attr(
    feature = "contract-schema",
    schemars(rename = "RetrievalFeedbackEvent")
)]
#[serde(deny_unknown_fields)]
pub struct RetrievalFeedback {
    pub tenant: String,
    pub principal: String,
    pub document_ref: String,
    pub kind: FeedbackKind,
    pub weight: f64,
    /// Unix seconds.
    pub at: u64,
    /// The request envelope's idempotency key. A retry of one request names one node, and
    /// two distinct requests never share a node. Empty for implicit events, which are
    /// derived and never stored.
    pub idempotency_key: String,
}

/// The engine-stamped identity and time of a recorded event (MEDULLA-FEDERATED-QUERY-R705.3).
/// A wire value is replaced by the verified tenant and principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FeedbackStamp {
    pub tenant: String,
    pub principal: String,
    /// Unix seconds.
    pub at: u64,
    /// The request envelope's idempotency key.
    pub idempotency_key: String,
}

/// The node label of a stored explicit event.
pub const RETRIEVAL_FEEDBACK_LABEL: &str = "RetrievalFeedback";

impl RetrievalFeedback {
    /// Build the event a stamped `RetrievalFeedbackRecord` records.
    pub fn from_record(
        document_ref: &str,
        kind: FeedbackKind,
        weight: f64,
        stamp: &FeedbackStamp,
    ) -> Self {
        Self {
            tenant: stamp.tenant.clone(),
            principal: stamp.principal.clone(),
            document_ref: document_ref.to_string(),
            kind,
            weight,
            at: stamp.at,
            idempotency_key: stamp.idempotency_key.clone(),
        }
    }

    /// The node id of this event: `retrieval-feedback:` plus a framed digest of the
    /// tenant, principal, document, kind, time and idempotency key, so the serving write and
    /// the durable replay name the same node. The weight is not part of the id: two distinct
    /// feedbacks of equal weight are two nodes.
    pub fn node_id(&self) -> Result<String, String> {
        let at = self.at.to_be_bytes();
        let digest = medulla_types::contract::Digest256::framed(
            b"eg/retrieval-feedback/v1",
            &[
                self.tenant.as_bytes(),
                self.principal.as_bytes(),
                self.document_ref.as_bytes(),
                self.kind.as_str().as_bytes(),
                &at,
                self.idempotency_key.as_bytes(),
            ],
        )?;
        Ok(format!("retrieval-feedback:{}", digest.to_hex()))
    }

    /// The stored node properties: `type` plus the seven event fields.
    pub fn node_properties(&self) -> serde_json::Value {
        serde_json::json!({
            "type": RETRIEVAL_FEEDBACK_LABEL,
            "tenant": self.tenant,
            "principal": self.principal,
            "document_ref": self.document_ref,
            "kind": self.kind.as_str(),
            "weight": self.weight,
            "at": self.at,
            "idempotency_key": self.idempotency_key,
        })
    }

    /// Read an event back from its stored node properties.
    pub fn from_node_properties(
        props: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<Self> {
        if props.get("type")?.as_str()? != RETRIEVAL_FEEDBACK_LABEL {
            return None;
        }
        Some(Self {
            tenant: props.get("tenant")?.as_str()?.to_string(),
            principal: props.get("principal")?.as_str()?.to_string(),
            document_ref: props.get("document_ref")?.as_str()?.to_string(),
            kind: serde_json::from_value(props.get("kind")?.clone()).ok()?,
            weight: props.get("weight")?.as_f64()?,
            at: props.get("at")?.as_u64()?,
            idempotency_key: props.get("idempotency_key")?.as_str()?.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(key: &str) -> RetrievalFeedback {
        RetrievalFeedback {
            tenant: "t1".into(),
            principal: "alice".into(),
            document_ref: "doc".into(),
            kind: FeedbackKind::Upvote,
            weight: 0.25,
            at: 100,
            idempotency_key: key.into(),
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R705.3
    #[test]
    fn equal_weight_distinct_keys_are_distinct_nodes() {
        let a = event("key-a");
        let b = event("key-b");
        assert_eq!(a.weight, b.weight);
        assert_ne!(a.node_id().unwrap(), b.node_id().unwrap());
        // A replay of one key names the same node.
        assert_eq!(a.node_id().unwrap(), event("key-a").node_id().unwrap());
        // The id does not depend on the weight.
        let mut heavier = event("key-a");
        heavier.weight = -0.5;
        assert_eq!(a.node_id().unwrap(), heavier.node_id().unwrap());
        // The key survives the stored node.
        let props = a.node_properties();
        let map = props.as_object().unwrap();
        assert_eq!(RetrievalFeedback::from_node_properties(map), Some(a));
    }
}
