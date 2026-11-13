//! The operations of one served `Batch` (MEDULLA-DURABLE-KERNEL-R871): a closed set of the served
//! node, memory and claim methods that commit together or not at all.

use serde::{Deserialize, Serialize};

/// One operation of a `Batch`. Each variant carries exactly the fields of the served method of the
/// same name, so its access check and its effect are those of that method.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(tag = "op")]
pub enum BatchOperation {
    /// `CreateNodeIfAbsent`.
    CreateNodeIfAbsent {
        node_id: String,
        #[cfg_attr(feature = "contract-schema", schemars(with = "Vec<u8>"))]
        #[serde(with = "serde_bytes")]
        properties_msgpack: Vec<u8>,
    },
    /// `CompareAndSetNodeFields`.
    CompareAndSetNodeFields {
        node_id: String,
        #[cfg_attr(feature = "contract-schema", schemars(with = "Vec<u8>"))]
        #[serde(with = "serde_bytes")]
        conditions_msgpack: Vec<u8>,
        #[cfg_attr(feature = "contract-schema", schemars(with = "Vec<u8>"))]
        #[serde(with = "serde_bytes")]
        updates_msgpack: Vec<u8>,
    },
    /// `ClaimNext`.
    ClaimNext {
        label: String,
        #[cfg_attr(feature = "contract-schema", schemars(with = "Vec<u8>"))]
        #[serde(with = "serde_bytes")]
        updates_msgpack: Vec<u8>,
    },
    /// `Reinforce`.
    Reinforce {
        node_id: String,
        now_ms: u64,
        weight: f64,
    },
    /// `GetNodeProperties`, the one read: it sees the writes of the operations before it.
    GetNodeProperties { node_id: String },
}

/// The node a `ClaimNext` operation claimed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BatchClaimedNode {
    pub node_id: String,
    pub properties: serde_json::Value,
}

/// The outcome of one `BatchOperation`, in the order of the request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(tag = "op")]
pub enum BatchOpResult {
    CreateNodeIfAbsent {
        created: bool,
    },
    CompareAndSetNodeFields {
        applied: bool,
    },
    ClaimNext {
        claimed: Option<BatchClaimedNode>,
    },
    Reinforce {
        existed: bool,
    },
    GetNodeProperties {
        #[cfg_attr(feature = "contract-schema", schemars(with = "Option<Vec<u8>>"))]
        #[serde(with = "serde_bytes")]
        properties_msgpack: Option<Vec<u8>>,
    },
}

/// The results of a committed `Batch`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BatchResult {
    pub results: Vec<BatchOpResult>,
}
