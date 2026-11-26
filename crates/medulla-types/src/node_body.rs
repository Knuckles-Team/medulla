//! The node body read (MEDULLA-INGEST-WORKER-R003.21).
//!
//! `NodeBody` answers the stored body text of one node of the request graph at the revision a
//! caller names, with the digest of the node's content. It is a read of the caller's projected
//! graph: a node the caller cannot read is absent from the projection, so an invisible node and
//! an absent one answer alike. It writes nothing and keeps no store of its own.

use serde::{Deserialize, Serialize};

use crate::contract::Digest256;

/// The properties a node's body text is read from, in order: the first string wins.
pub const BODY_PROPERTIES: [&str; 3] = ["body", "content", "text"];

/// The refusal of a node the caller cannot read, an absent one, and one with no body text. The
/// three answer alike, so the read is no existence oracle.
pub const NODE_BODY_UNAVAILABLE: &str = "NODE_BODY_UNAVAILABLE";
/// The refusal of a readable node whose revision is not the one asked for.
pub const NODE_BODY_STALE: &str = "NODE_BODY_STALE";

/// A served `NodeBody` request. The graph is the request graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct NodeBodyRequest {
    /// The verified tenant; any other tenant refuses `ACCESS_DENIED`.
    pub tenant: String,
    pub node_id: String,
    /// The revision the caller read; a node at another revision refuses `NODE_BODY_STALE`.
    /// With none, the node's current revision is answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
}

impl NodeBodyRequest {
    /// Refuse a request outside its bounds.
    pub fn validate(&self) -> Result<(), String> {
        if self.tenant.is_empty() || self.node_id.is_empty() || self.node_id.len() > 1024 {
            return Err(
                "INVALID_ARGUMENT: a node body request names its tenant and a node id of 1 to 1024 bytes"
                    .into(),
            );
        }
        Ok(())
    }
}

/// The served `NodeBody` answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct NodeBodyAnswer {
    pub node_id: String,
    pub revision: u64,
    /// The digest `VerifyIngestReadback` computes over the node's properties, so a caller can
    /// bind the body to an ingest receipt it holds.
    pub content_digest: Digest256,
    pub body: String,
}

/// Digest and revision of a stored node-properties blob.
///
/// The digest is `Digest256::framed` under domain `eg/ingest-readback/node/v1` over the compact
/// JSON of the decoded properties with keys sorted, so a client reproduces it from the
/// properties it reads. A blob that does not decode digests its raw bytes. The one definition:
/// the ingest readback and the node body read both use it.
pub fn observe(blob: &[u8]) -> (Digest256, u64) {
    let Ok(value) = crate::msgpack::decode_property_value(blob) else {
        return (Digest256::sha256(blob), 0);
    };
    let canonical = serde_json::to_string(&value).unwrap_or_default();
    let digest = Digest256::framed(b"eg/ingest-readback/node/v1", &[canonical.as_bytes()])
        .unwrap_or_else(|_| Digest256::sha256(canonical.as_bytes()));
    let revision = value.get("revision").and_then(|v| v.as_u64()).unwrap_or(0);
    (digest, revision)
}

/// The answer for one node blob, or the refusal code. `revision` is the one asked for.
pub fn answer_for(
    node_id: &str,
    blob: &[u8],
    revision: Option<u64>,
) -> Result<NodeBodyAnswer, &'static str> {
    let Ok(value) = crate::msgpack::decode_property_value(blob) else {
        return Err(NODE_BODY_UNAVAILABLE);
    };
    let (content_digest, current) = observe(blob);
    if revision.is_some_and(|asked| asked != current) {
        return Err(NODE_BODY_STALE);
    }
    let body = BODY_PROPERTIES
        .iter()
        .find_map(|key| value.get(*key).and_then(serde_json::Value::as_str))
        .ok_or(NODE_BODY_UNAVAILABLE)?;
    Ok(NodeBodyAnswer {
        node_id: node_id.to_string(),
        revision: current,
        content_digest,
        body: body.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(value: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&value).unwrap()
    }

    // spec: MEDULLA-INGEST-WORKER-R003.21
    #[test]
    fn a_body_is_read_at_its_revision_and_a_stale_or_bodiless_node_is_refused() {
        let node = blob(serde_json::json!({"revision": 3, "content": "alpha", "text": "beta"}));
        let answer = answer_for("n1", &node, Some(3)).unwrap();
        assert_eq!((answer.revision, answer.body.as_str()), (3, "alpha"));
        assert_eq!(answer.content_digest, observe(&node).0);
        assert_eq!(answer_for("n1", &node, None).unwrap().revision, 3);
        assert_eq!(answer_for("n1", &node, Some(2)), Err(NODE_BODY_STALE));
        // `body` wins over `content` and `text`.
        let preferred = blob(serde_json::json!({"body": "first", "content": "second"}));
        assert_eq!(answer_for("n", &preferred, None).unwrap().body, "first");
        // A node with no body text answers like one that is not there.
        let bare = blob(serde_json::json!({"revision": 1, "label": "x"}));
        assert_eq!(answer_for("n", &bare, Some(1)), Err(NODE_BODY_UNAVAILABLE));
        assert_eq!(answer_for("n", b"\xc1", None), Err(NODE_BODY_UNAVAILABLE));
        let request = NodeBodyRequest {
            tenant: "t".into(),
            node_id: String::new(),
            revision: None,
        };
        assert!(request.validate().unwrap_err().starts_with("INVALID_ARGUMENT"));
    }
}
