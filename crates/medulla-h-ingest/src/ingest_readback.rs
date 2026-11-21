//! Independent readback of one ingest write (MEDULLA-REPO-INGEST-R002.100).
//!
//! `verify` compares what a write receipt promised (id, content digest,
//! revision per result) with what the caller principal reads back. The reader
//! is the caller's, never the writer's: an id the caller may not see is
//! `READBACK_DENIED`, not a silent match. `lower` turns a `Matched` receipt
//! into the one `:ReadbackReceipt` node the route commits.

use medulla_ingestion_types::ingest_readback::{
    HydrationRun, HydrationRunClaim, ReadbackReceipt, ReadbackVerdict, VerifyIngestReadbackRequest, READBACK_DENIED,
    READBACK_MISMATCH, READBACK_PARTIAL, READBACK_STALE,
};
use medulla_protocol_types::protocol::Method;
use medulla_types::contract::Digest256;
use serde_json::json;

use super::repository_index::{EnvelopeLabels, IndexWriteSet};

/// Node type of the readback record.
pub const READBACK_RECEIPT_TYPE: &str = "ReadbackReceipt";
/// Node type of a bound hydration run (MEDULLA-REPO-INGEST-R002.101).
pub const HYDRATION_RUN_TYPE: &str = "HydrationRun";
/// Node type of a compiled skill plan a hydration run names.
const SKILL_COMPILATION_TYPE: &str = "SkillCompilation";
/// Refusal code: the claimed workflow does not match its stored compilation.
pub const HYDRATION_BINDING_MISMATCH: &str = "HYDRATION_BINDING_MISMATCH";
const HYDRATION_PREFIX: &str = "hydration-run:";
const BOUND_BY: &str = "boundBy";
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
    /// The decoded properties of a node the caller may read.
    fn properties(&self, id: &str) -> Option<serde_json::Value>;
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
    .unwrap_or(receipt.write_receipt_digest);
    format!("{ENVELOPE_PREFIX}{}", key.to_hex())
}

fn receipt_properties(receipt: &ReadbackReceipt) -> serde_json::Value {
    let mut properties = json!({
        "type": READBACK_RECEIPT_TYPE,
        "write_receipt_digest": receipt.write_receipt_digest.to_hex(),
        "principal": receipt.principal,
        "grant_revision": receipt.grant_revision,
        "graph_revision": receipt.graph_revision,
        "matched_ids": receipt.matched_ids,
        "mismatched_ids": receipt.mismatched_ids,
        "verdict": "matched",
    });
    // The receipt is the reading principal's evidence, so it is that
    // principal's row: an unowned row is hidden from every non-System reader,
    // the principal included.
    properties[medulla_graph_core::rls::RLS_OWNER_ID_KEY] = json!(receipt.principal);
    properties
}

fn receipt_blob(receipt: &ReadbackReceipt) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(&receipt_properties(receipt))
        .map_err(|error| format!("readback receipt encoding failed: {error}"))
}

fn mismatch(reason: &str) -> String {
    format!("{HYDRATION_BINDING_MISMATCH}: {reason}")
}

/// Bind an executor's hydration claim to a `Matched` readback receipt.
///
/// The claimed workflow must be a visible, non-stale `:SkillCompilation` node
/// whose revision and plan digest equal the claim's. Any failure refuses with
/// `HYDRATION_BINDING_MISMATCH` and nothing is written.
pub fn bind_hydration_run(
    claim: &HydrationRunClaim,
    receipt: &ReadbackReceipt,
    view: &dyn ReadbackView,
) -> Result<HydrationRun, String> {
    let revision = match view.read(&claim.workflow_id) {
        NodeRead::Visible { revision, .. } => revision,
        NodeRead::Absent => return Err(mismatch("the workflow node is absent")),
        NodeRead::Denied => return Err(mismatch("the workflow node is not readable")),
    };
    let properties = view
        .properties(&claim.workflow_id)
        .ok_or_else(|| mismatch("the workflow node has no readable properties"))?;
    if properties.get("type").and_then(|v| v.as_str()) != Some(SKILL_COMPILATION_TYPE) {
        return Err(mismatch("the workflow node is not a skill compilation"));
    }
    // Staleness is computed on read: the document and type digests are the
    // node's own, so only capability drift refuses here.
    let own = |name: &str| {
        properties
            .get(name)
            .and_then(|v| v.as_str())
            .and_then(|hex| Digest256::parse(hex).ok())
    };
    let (Some(document), Some(types)) = (own("document_digest"), own("type_digest")) else {
        return Err(mismatch("the workflow compilation carries no document or type digest"));
    };
    let capability = Digest256::parse(medulla_capabilities::catalog::contract_catalog_digest())
        .map_err(|_| mismatch("the contract catalog digest is invalid"))?;
    if let Some(drifted) = medulla_ingestion_types::skill_plan::skill_binding_stale(
        &properties,
        &document,
        &capability,
        &types,
    ) {
        return Err(mismatch(&format!("the workflow compilation is stale: {drifted}")));
    }
    if revision < 1 || revision != claim.workflow_revision {
        return Err(mismatch("the workflow revision differs from the claim"));
    }
    if properties.get("plan_digest").and_then(|v| v.as_str())
        != Some(claim.plan_digest.to_hex().as_str())
    {
        return Err(mismatch("the plan digest differs from the claim"));
    }
    let blob = receipt_blob(receipt)?;
    Ok(HydrationRun {
        workflow_id: claim.workflow_id.clone(),
        workflow_revision: claim.workflow_revision,
        plan_digest: claim.plan_digest,
        run_output_digest: claim.run_output_digest,
        write_receipt_digest: receipt.write_receipt_digest,
        readback_receipt_digest: observe(&blob).0,
    })
}

/// Id of the `:HydrationRun` node of one bound run.
pub fn hydration_node_id(run: &HydrationRun) -> String {
    let revision = run.workflow_revision.to_string();
    let (plan, output) = (run.plan_digest.to_hex(), run.run_output_digest.to_hex());
    let (write, read) = (
        run.write_receipt_digest.to_hex(),
        run.readback_receipt_digest.to_hex(),
    );
    let key = Digest256::framed(
        b"eg/hydration-run/v1",
        &[
            run.workflow_id.as_bytes(),
            revision.as_bytes(),
            plan.as_bytes(),
            output.as_bytes(),
            write.as_bytes(),
            read.as_bytes(),
        ],
    )
    .unwrap_or(run.write_receipt_digest);
    format!("{HYDRATION_PREFIX}{}", key.to_hex())
}

/// Lower a receipt, and the hydration run bound to it, to one write-set: the
/// `:ReadbackReceipt` node, plus the `:HydrationRun` node and its `boundBy`
/// edge to the receipt when a run is bound.
pub fn lower(
    receipt: &ReadbackReceipt,
    hydration: Option<&HydrationRun>,
) -> Result<IndexWriteSet, String> {
    let receipt_id = receipt_node_id(receipt);
    let mut methods = vec![Method::AddNode {
        node_id: receipt_id.clone(),
        properties_msgpack: receipt_blob(receipt)?,
    }];
    if let Some(run) = hydration {
        let run_id = hydration_node_id(run);
        let mut properties = json!({
            "type": HYDRATION_RUN_TYPE,
            "workflow_id": run.workflow_id,
            "workflow_revision": run.workflow_revision,
            "plan_digest": run.plan_digest.to_hex(),
            "run_output_digest": run.run_output_digest.to_hex(),
            "write_receipt_digest": run.write_receipt_digest.to_hex(),
            "readback_receipt_digest": run.readback_receipt_digest.to_hex(),
        });
        properties[medulla_graph_core::rls::RLS_OWNER_ID_KEY] = json!(receipt.principal);
        let encode = |value: &serde_json::Value| {
            rmp_serde::to_vec_named(value)
                .map_err(|error| format!("hydration run encoding failed: {error}"))
        };
        methods.push(Method::AddNode {
            node_id: run_id.clone(),
            properties_msgpack: encode(&properties)?,
        });
        methods.push(Method::AddEdge {
            source_id: run_id,
            target_id: receipt_id,
            properties_msgpack: encode(&json!({ "relationship": BOUND_BY }))?,
        });
    }
    IndexWriteSet::from_methods(ENVELOPE_PREFIX, b"eg/ingest-readback", methods)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_ingestion_types::ingest_readback::ReadbackExpectation;
    use std::collections::BTreeMap;

    struct View(u64, BTreeMap<String, NodeRead>, BTreeMap<String, serde_json::Value>);
    impl ReadbackView for View {
        fn graph_revision(&self) -> u64 {
            self.0
        }
        fn read(&self, id: &str) -> NodeRead {
            self.1.get(id).cloned().unwrap_or(NodeRead::Absent)
        }
        fn properties(&self, id: &str) -> Option<serde_json::Value> {
            self.2.get(id).cloned()
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
            hydration_run: None,
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
            BTreeMap::new(),
        )
    }

    #[test]
    fn matched_then_each_refusal() {
        let ok = view(vec![("a", visible("a", 1)), ("b", visible("b", 1))]);
        let receipt = verify(&request(&["a", "b"]), "alice", &ok).unwrap();
        assert_eq!(receipt.verdict, ReadbackVerdict::Matched);
        assert_eq!(receipt.matched_ids, ["a", "b"]);
        let lowered = lower(&receipt, None).unwrap();
        assert_eq!(lowered.methods.len(), 1);
        let Method::AddNode {
            properties_msgpack, ..
        } = &lowered.methods[0]
        else {
            panic!("a readback receipt lowers to one AddNode");
        };
        let properties: serde_json::Value = rmp_serde::from_slice(properties_msgpack).unwrap();
        assert_eq!(
            properties[medulla_graph_core::rls::RLS_OWNER_ID_KEY],
            "alice",
            "the receipt is owned by the reading principal"
        );

        let stale = View(4, ok.1.clone(), BTreeMap::new());
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

    fn hydration_view(revision: u64, stale: bool) -> View {
        let capability = if stale {
            digest("drifted capability")
        } else {
            Digest256::parse(medulla_capabilities::catalog::contract_catalog_digest()).unwrap()
        };
        let properties = json!({
            "type": "SkillCompilation",
            "plan_digest": digest("plan").to_hex(),
            "document_digest": digest("document").to_hex(),
            "capability_digest": capability.to_hex(),
            "type_digest": digest("types").to_hex(),
        });
        let mut view = view(vec![
            ("a", visible("a", 1)),
            ("skill-compilation:1", visible("skill", revision)),
        ]);
        view.2.insert("skill-compilation:1".into(), properties);
        view
    }

    fn claim(revision: u64, plan: &str) -> HydrationRunClaim {
        HydrationRunClaim {
            workflow_id: "skill-compilation:1".into(),
            workflow_revision: revision,
            plan_digest: digest(plan),
            run_output_digest: digest("output"),
        }
    }

    // MEDULLA-REPO-INGEST-R002.101
    #[test]
    fn hydration_binding_refuses_stale_revision_and_plan_mismatch() {
        let good = hydration_view(2, false);
        let receipt = verify(&request(&["a"]), "alice", &good).unwrap();
        let run = bind_hydration_run(&claim(2, "plan"), &receipt, &good).unwrap();
        assert_eq!(run.write_receipt_digest, receipt.write_receipt_digest);
        let blob = receipt_blob(&receipt).unwrap();
        assert_eq!(run.readback_receipt_digest, observe(&blob).0);
        let lowered = lower(&receipt, Some(&run)).unwrap();
        assert_eq!(lowered.methods.len(), 3);
        assert!(matches!(lowered.methods[2], Method::AddEdge { .. }));

        for (view, claim) in [
            (hydration_view(3, false), claim(2, "plan")),
            (hydration_view(2, false), claim(2, "other")),
            (hydration_view(2, true), claim(2, "plan")),
            (view(vec![("a", visible("a", 1))]), claim(2, "plan")),
        ] {
            let refusal = bind_hydration_run(&claim, &receipt, &view).unwrap_err();
            assert!(refusal.starts_with(HYDRATION_BINDING_MISMATCH), "{refusal}");
        }
    }
}
