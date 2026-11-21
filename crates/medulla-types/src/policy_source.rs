//! Policy source binding records (MEDULLA-SRC-AUTH-R801).
//!
//! After a human review, the caller binds one reviewed source revision to the
//! active `DecisionPolicy` component. Medulla never writes the source: it only
//! records which reviewed revision and digest the active policy was activated
//! from. `PolicySourceBind` is the request; `PolicySourceBinding` is the stored
//! activation record, kept as the graph node `policy_source:<policy_iri>`.

use serde::{Deserialize, Serialize};

/// The graph label of the activation record node.
pub const POLICY_SOURCE_LABEL: &str = "PolicySourceBinding";

/// The graph node id prefix of the activation record; the node id is the prefix
/// followed by the policy component id.
pub const POLICY_SOURCE_NODE_PREFIX: &str = "policy_source:";

/// The activation record node id of one policy.
pub fn policy_source_node_id(policy_iri: &str) -> String {
    format!("{POLICY_SOURCE_NODE_PREFIX}{policy_iri}")
}

/// One `PolicySourceBind` call: a reviewed source revision to bind to the
/// active policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PolicySourceBind {
    /// The reviewed source reference (the part of the component `content_ref`
    /// before the last `@`).
    pub source_ref: String,
    /// The reviewed source commit id: 40 hex characters.
    pub source_revision: String,
    /// SHA-256 of the reviewed source: 64 lowercase hex characters.
    pub source_digest: String,
    /// The `component_id` of the published `DecisionPolicy` component.
    pub policy_iri: String,
    /// The head `version` of that component the reviewer saw.
    pub component_version: String,
    /// The human reviewer; never empty.
    pub approver: String,
}

/// The stored activation record of one policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PolicySourceBinding {
    pub source_ref: String,
    pub source_revision: String,
    pub source_digest: String,
    pub policy_iri: String,
    pub component_version: String,
    pub approver: String,
    /// The engine clock when the binding was written, in milliseconds.
    pub bound_at_ms: u64,
}

impl PolicySourceBind {
    /// Whether `source_revision` is 40 hex characters and `source_digest` is 64
    /// lowercase hex characters.
    pub fn shape_valid(&self) -> bool {
        self.source_revision.len() == 40
            && self.source_revision.bytes().all(|b| b.is_ascii_hexdigit())
            && self.source_digest.len() == 64
            && self
                .source_digest
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }
}

impl PolicySourceBinding {
    /// The record the engine writes for an accepted bind at `now_ms`.
    pub fn from_request(request: &PolicySourceBind, now_ms: u64) -> Self {
        Self {
            source_ref: request.source_ref.clone(),
            source_revision: request.source_revision.clone(),
            source_digest: request.source_digest.clone(),
            policy_iri: request.policy_iri.clone(),
            component_version: request.component_version.clone(),
            approver: request.approver.clone(),
            bound_at_ms: now_ms,
        }
    }

    /// Whether `other` binds the same revision, digest, version and approver,
    /// whatever the time of the write (a re-bind of the same review).
    pub fn same_review(&self, other: &Self) -> bool {
        Self {
            bound_at_ms: other.bound_at_ms,
            ..self.clone()
        } == *other
    }
}
