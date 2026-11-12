//! Independent readback of one ingest write (MEDULLA-REPO-INGEST-R002.100).
//!
//! `verify` compares what a write receipt promised (id, content digest,
//! revision per result) with what the caller principal reads back. The reader
//! is the caller's, never the writer's: an id the caller may not see is
//! `READBACK_DENIED`, not a silent match. `lower` turns a `Matched` receipt
//! into the one `:ReadbackReceipt` node the route commits.

use medulla_protocol_types::protocol::Method;
use medulla_types::contract::Digest256;
use medulla_types::ingest_readback::{
    ReadbackReceipt, ReadbackVerdict, VerifyIngestReadbackRequest, READBACK_DENIED,
    READBACK_MISMATCH, READBACK_PARTIAL, READBACK_STALE,
};
use serde_json::json;

use super::repository_index::{EnvelopeLabels, IndexWriteSet};

/// Node type of the readback record.
pub const READBACK_RECEIPT_TYPE: &str = "ReadbackReceipt";
const ENVELOPE_PREFIX: &str = "readback-receipt:";
/// Envelope labels of the readback commit.
pub const LABELS: EnvelopeLabels = EnvelopeLabels {
    version_type: "readback-receipt-sha256",
    policy_version: "readback-receipt",
};

/// What the caller principal reads for one id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeRead {
    /// No such node.
    Absent,
    /// The node exists but the caller may not read it.
    Denied,
    /// The node as the caller reads it.
    Visible {
        /// Digest of the stored properties.
        content_digest: Digest256,
        /// The node's `revision` property (0 when absent).
        revision: u64,
    },
}

/// The caller's view of the graph.
pub trait ReadbackView {
    /// Graph revision now.
    fn graph_revision(&self) -> u64;
    /// One id as the caller reads it.
    fn read(&self, id: &str) -> NodeRead;
}

/// Why a readback refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadbackRefusal {
    /// The caller may not read these ids.
    Denied(Vec<String>),
    /// These ids are absent from the caller's view (or none were named).
    Partial(Vec<String>),
    /// The graph has not reached the write's revision.
    Stale {
        /// The revision the graph has reached.
        have: u64,
        /// The revision the write needs.
        need: u64,
    },
    /// These ids differ in content digest or revision.
    Mismatch(Vec<String>),
}

impl ReadbackRefusal {
    /// The wire refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Denied(_) => READBACK_DENIED,
            Self::Partial(_) => READBACK_PARTIAL,
            Self::Stale { .. } => READBACK_STALE,
            Self::Mismatch(_) => READBACK_MISMATCH,
        }
    }
}

impl std::fmt::Display for ReadbackRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale { have, need } => write!(
                f,
                "{}: graph revision {have} is behind the write's revision {need}",
                self.code()
            ),
            Self::Denied(ids) | Self::Partial(ids) | Self::Mismatch(ids) => {
                write!(f, "{}: {}", self.code(), ids.join(","))
            }
        }
    }
}

/// Digest and revision of a stored node-properties blob.
///
/// The digest is `Digest256::framed` under domain `eg/ingest-readback/node/v1`
/// over the compact JSON of the decoded properties with keys sorted, so a
/// client reproduces it from the properties it reads. A blob that does not
/// decode digests its raw bytes.
pub fn observe(blob: &[u8]) -> (Digest256, u64) {
    let Ok(value) = medulla_types::msgpack::decode_property_value(blob) else {
        return (Digest256::sha256(blob), 0);
    };
    let canonical = serde_json::to_string(&value).unwrap_or_default();
    let digest = Digest256::framed(b"eg/ingest-readback/node/v1", &[canonical.as_bytes()])
        .unwrap_or_else(|_| Digest256::sha256(canonical.as_bytes()));
    let revision = value.get("revision").and_then(|v| v.as_u64()).unwrap_or(0);
    (digest, revision)
}

/// Compare the write receipt with the caller's reads.
///
/// Order: stale revision, then denied ids, then absent ids, then differing
/// ids. Only an all-match comparison yields a receipt.
pub fn verify(
    request: &VerifyIngestReadbackRequest,
    principal: &str,
    view: &dyn ReadbackView,
) -> Result<ReadbackReceipt, ReadbackRefusal> {
    let grant_revision = view.graph_revision();
    if grant_revision < request.expected_graph_revision {
        return Err(ReadbackRefusal::Stale {
            have: grant_revision,
            need: request.expected_graph_revision,
        });
    }
    let (mut denied, mut absent) = (Vec::new(), Vec::new());
    let (mut matched, mut mismatched) = (Vec::new(), Vec::new());
    for expected in &request.results {
        match view.read(&expected.id) {
            NodeRead::Denied => denied.push(expected.id.clone()),
            NodeRead::Absent => absent.push(expected.id.clone()),
            NodeRead::Visible {
                content_digest,
                revision,
            } => {
                if content_digest == expected.content_digest && revision == expected.revision {
                    matched.push(expected.id.clone());
                } else {
                    mismatched.push(expected.id.clone());
                }
            }
        }
    }
    if !denied.is_empty() {
        return Err(ReadbackRefusal::Denied(denied));
    }
    if request.results.is_empty() || !absent.is_empty() {
        return Err(ReadbackRefusal::Partial(absent));
    }
    if !mismatched.is_empty() {
        return Err(ReadbackRefusal::Mismatch(mismatched));
    }
    let verdict = ReadbackReceipt::verdict_for(&matched, &mismatched);
    debug_assert_eq!(verdict, ReadbackVerdict::Matched);
    Ok(ReadbackReceipt {
        write_receipt_digest: request.write_receipt_digest,
        principal: principal.to_string(),
        grant_revision,
        graph_revision: view.graph_revision(),
        matched_ids: matched,
        mismatched_ids: mismatched,
        verdict,
    })
}

/// Id of the readback node of one write receipt and principal.
pub fn receipt_node_id(receipt: &ReadbackReceipt) -> String {
    let key = Digest256::framed(
        b"eg/ingest-readback/id/v1",
        &[
            receipt.write_receipt_digest.as_bytes(),
            receipt.principal.as_bytes(),
        ],
    )
    .unwrap_or_else(|_| receipt.write_receipt_digest);
    format!("{ENVELOPE_PREFIX}{}", key.to_hex())
}

/// Lower a receipt to its one `:ReadbackReceipt` node write-set.
pub fn lower(receipt: &ReadbackReceipt) -> Result<IndexWriteSet, String> {
    let properties = json!({
        "type": READBACK_RECEIPT_TYPE,
        "write_receipt_digest": receipt.write_receipt_digest.to_hex(),
        "principal": receipt.principal,
        "grant_revision": receipt.grant_revision,
        "graph_revision": receipt.graph_revision,
        "matched_ids": receipt.matched_ids,
        "mismatched_ids": receipt.mismatched_ids,
        "verdict": "matched",
    });
    let methods = vec![Method::AddNode {
        node_id: receipt_node_id(receipt),
        properties_msgpack: rmp_serde::to_vec_named(&properties)
            .map_err(|error| format!("readback receipt encoding failed: {error}"))?,
    }];
    IndexWriteSet::from_methods(ENVELOPE_PREFIX, b"eg/ingest-readback", methods)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::ingest_readback::ReadbackExpectation;
    use std::collections::BTreeMap;

    struct View(u64, BTreeMap<String, NodeRead>);
    impl ReadbackView for View {
        fn graph_revision(&self) -> u64 {
            self.0
        }
        fn read(&self, id: &str) -> NodeRead {
            self.1.get(id).cloned().unwrap_or(NodeRead::Absent)
        }
    }

    fn digest(tag: &str) -> Digest256 {
        Digest256::sha256(tag.as_bytes())
    }

    fn visible(tag: &str, revision: u64) -> NodeRead {
        NodeRead::Visible {
            content_digest: digest(tag),
            revision,
        }
    }

    fn request(ids: &[&str]) -> VerifyIngestReadbackRequest {
        VerifyIngestReadbackRequest {
            write_receipt_digest: digest("receipt"),
            expected_graph_revision: 5,
            results: ids
                .iter()
                .map(|id| ReadbackExpectation {
                    id: id.to_string(),
                    content_digest: digest(id),
                    revision: 1,
                })
                .collect(),
        }
    }

    fn view(entries: Vec<(&str, NodeRead)>) -> View {
        View(
            7,
            entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    #[test]
    fn matched_then_each_refusal() {
        let ok = view(vec![("a", visible("a", 1)), ("b", visible("b", 1))]);
        let receipt = verify(&request(&["a", "b"]), "alice", &ok).unwrap();
        assert_eq!(receipt.verdict, ReadbackVerdict::Matched);
        assert_eq!(receipt.matched_ids, ["a", "b"]);
        assert_eq!(lower(&receipt).unwrap().methods.len(), 1);

        let stale = View(4, ok.1.clone());
        let refusal = verify(&request(&["a"]), "alice", &stale).unwrap_err();
        assert_eq!(refusal.code(), READBACK_STALE);

        let denied = view(vec![("a", visible("a", 1)), ("b", NodeRead::Denied)]);
        let refusal = verify(&request(&["a", "b"]), "alice", &denied).unwrap_err();
        assert_eq!(refusal, ReadbackRefusal::Denied(vec!["b".into()]));

        let refusal = verify(
            &request(&["a", "b"]),
            "alice",
            &view(vec![("a", visible("a", 1))]),
        )
        .unwrap_err();
        assert_eq!(refusal.code(), READBACK_PARTIAL);
        assert_eq!(
            verify(&request(&[]), "alice", &ok).unwrap_err().code(),
            READBACK_PARTIAL
        );

        let changed = view(vec![("a", visible("a", 1)), ("b", visible("other", 1))]);
        let refusal = verify(&request(&["a", "b"]), "alice", &changed).unwrap_err();
        assert_eq!(refusal, ReadbackRefusal::Mismatch(vec!["b".into()]));
        let old_revision = view(vec![("a", visible("a", 2))]);
        assert_eq!(
            verify(&request(&["a"]), "alice", &old_revision)
                .unwrap_err()
                .code(),
            READBACK_MISMATCH
        );
    }

    #[test]
    fn observe_reads_revision_and_digest() {
        let blob = rmp_serde::to_vec_named(&json!({"type": "Doc", "revision": 3})).unwrap();
        let (digest_a, revision) = observe(&blob);
        assert_eq!(revision, 3);
        let other = rmp_serde::to_vec_named(&json!({"type": "Doc", "revision": 4})).unwrap();
        assert_ne!(digest_a, observe(&other).0);
    }
}
