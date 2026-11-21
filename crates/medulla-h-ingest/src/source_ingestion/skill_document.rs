//! Admission of `skill_documents` batches (MEDULLA-REPO-INGEST-R002.1.6).
//!
//! A skill document is a typed item of its own batch kind, never a mapped record.
//! Each document compiles once, in batch order, through
//! `medulla_document::skill_plan::compile_skill` and is stored as one
//! `skill-compilation:<hex>` node. A refused document refuses the whole batch
//! before any write.

use medulla_document::skill_plan::{compile_skill, CapabilityLookup, SkillDocument};
use medulla_ingestion_types::skill_plan::SkillCompilation;
use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_ingestion::batch_kind::{SkillDocumentItem, MAX_SKILL_DOCUMENTS};
use medulla_source_types::source_ingestion::SourceJson;
use medulla_types::contract::{Digest256, ServerErrorCode};

/// The digest domain of a skill compilation node id.
const SKILL_COMPILATION_DOMAIN: &[u8] = b"eg/skill-compilation/v1";

fn mismatch(detail: impl std::fmt::Display) -> String {
    format!(
        "{}: {detail}",
        ServerErrorCode::SourceBatchKindMismatch.as_str()
    )
}

/// The writes of one admitted `skill_documents` batch.
#[derive(Debug, Default)]
pub struct SkillAdmission {
    /// The compilation nodes written: each takes one policy record.
    pub node_ids: Vec<String>,
    /// One node write per document, in batch order.
    pub methods: Vec<Method>,
}

/// The capability and type registries a compilation resolves against: the served
/// method catalog and the graph's composed schema.
pub struct CatalogCapabilityLookup {
    /// Digest of the type registry, captured when the lookup is built.
    pub type_digest: Digest256,
}

impl CapabilityLookup for CatalogCapabilityLookup {
    fn knows(&self, capability_id: &str) -> bool {
        medulla_capabilities::catalog::method_schema(capability_id).is_some()
    }

    fn capability_digest(&self) -> Digest256 {
        Digest256::parse(medulla_capabilities::catalog::contract_catalog_digest())
            .expect("the contract catalog digest is a valid digest")
    }

    fn type_digest(&self) -> Digest256 {
        self.type_digest
    }
}

/// Decode the elements of a `skill_documents` extension. An item that does not
/// decode, an empty `directory_ref` or an empty `text` refuses the batch.
pub fn decode_skill_documents(items: &[SourceJson]) -> Result<Vec<SkillDocumentItem>, String> {
    if items.len() > MAX_SKILL_DOCUMENTS {
        return Err(mismatch(format!(
            "a skill_documents batch holds at most {MAX_SKILL_DOCUMENTS} documents"
        )));
    }
    items
        .iter()
        .map(|item| {
            let document: SkillDocumentItem = serde_json::from_value(item.value()?)
                .map_err(|error| mismatch(format!("skill document: {error}")))?;
            if document.directory_ref.trim().is_empty() {
                return Err(mismatch("directory_ref must not be empty"));
            }
            if document.text.trim().is_empty() {
                return Err(mismatch("text must not be empty"));
            }
            Ok(document)
        })
        .collect()
}

/// The node id of the compilation stored for one skill directory.
pub fn compilation_node_id(
    tenant_scope: &str,
    connector: &str,
    directory_ref: &str,
) -> Result<String, String> {
    Ok(format!(
        "skill-compilation:{}",
        Digest256::framed(
            SKILL_COMPILATION_DOMAIN,
            &[
                tenant_scope.as_bytes(),
                connector.as_bytes(),
                directory_ref.as_bytes(),
            ],
        )?
        .to_hex()
    ))
}

fn compilation_properties(compilation: &SkillCompilation) -> Result<serde_json::Value, String> {
    let mut properties = serde_json::json!({
        "type": "SkillCompilation",
        "directory_ref": compilation.directory_ref,
        "document_digest": compilation.document_digest.to_hex(),
        "grammar_revision": compilation.grammar_revision,
        "plan_digest": compilation.plan_digest.to_hex(),
        "capability_digest": compilation.capability_digest.to_hex(),
        "type_digest": compilation.type_digest.to_hex(),
    });
    properties["steps"] = serde_json::to_value(&compilation.steps)
        .map_err(|error| mismatch(format!("skill steps encoding failed: {error}")))?;
    Ok(properties)
}

/// Compile each document once, in batch order, and compile its node write. A
/// `SkillPlanRefusal` refuses the whole batch with its registered code before any
/// write.
pub fn admit_skill(
    tenant_scope: &str,
    connector: &str,
    documents: &[SkillDocumentItem],
    lookup: &dyn CapabilityLookup,
) -> Result<SkillAdmission, String> {
    let mut admission = SkillAdmission::default();
    for document in documents {
        let (_, compilation) = compile_skill(
            &SkillDocument {
                directory_ref: &document.directory_ref,
                text: &document.text,
            },
            lookup,
        )
        .map_err(|refusal| format!("{}: {refusal}", refusal.code().as_str()))?;
        let node_id = compilation_node_id(tenant_scope, connector, &document.directory_ref)?;
        admission.methods.push(Method::AddNode {
            node_id: node_id.clone(),
            properties_msgpack: rmp_serde::to_vec_named(&compilation_properties(&compilation)?)
                .map_err(|error| mismatch(format!("skill encoding failed: {error}")))?,
        });
        admission.node_ids.push(node_id);
    }
    Ok(admission)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lookup;

    impl CapabilityLookup for Lookup {
        fn knows(&self, capability_id: &str) -> bool {
            capability_id == "GetNodeProperties"
        }
        fn capability_digest(&self) -> Digest256 {
            Digest256::from_bytes([1; 32])
        }
        fn type_digest(&self) -> Digest256 {
            Digest256::from_bytes([2; 32])
        }
    }

    fn item(directory_ref: &str, text: &str) -> SkillDocumentItem {
        SkillDocumentItem {
            directory_ref: directory_ref.into(),
            text: text.into(),
        }
    }

    // spec: MEDULLA-REPO-INGEST-R002.1.6
    #[test]
    fn a_document_compiles_to_one_node_and_a_refusal_writes_nothing() {
        let good = item(
            "skills/inspect",
            "# Inspect\n\n## Steps\n- read: Read the node\n  capability: GetNodeProperties\n- report: Report it\n  after: read\n",
        );
        let admission = admit_skill("t1", "demo", std::slice::from_ref(&good), &Lookup).unwrap();
        assert_eq!(admission.methods.len(), 1);
        assert_eq!(admission.node_ids.len(), 1);
        assert_eq!(
            admission.node_ids[0],
            compilation_node_id("t1", "demo", "skills/inspect").unwrap()
        );
        match &admission.methods[0] {
            Method::AddNode {
                node_id,
                properties_msgpack,
            } => {
                assert_eq!(node_id, &admission.node_ids[0]);
                let properties: serde_json::Value =
                    rmp_serde::from_slice(properties_msgpack).unwrap();
                assert_eq!(properties["type"], "SkillCompilation");
                assert_eq!(properties["directory_ref"], "skills/inspect");
                assert_eq!(properties["steps"].as_array().unwrap().len(), 2);
            }
            other => panic!("expected AddNode, got {other:?}"),
        }
        // A refused document refuses the whole batch with its registered code.
        let broken = item("skills/broken", "no steps section at all");
        let refused = admit_skill("t1", "demo", &[good.clone(), broken], &Lookup).unwrap_err();
        assert!(refused.starts_with("SKILL_SPAN_MISSING"), "{refused}");
        let unknown = item(
            "skills/unknown",
            "## Steps\n- a: x\n  capability: nope.run\n",
        );
        let refused = admit_skill("t1", "demo", &[good, unknown], &Lookup).unwrap_err();
        assert!(refused.starts_with("SKILL_CAPABILITY_UNRESOLVED"), "{refused}");

        // Decoding refuses an empty directory_ref and an empty text.
        let empty_dir = SourceJson::new(serde_json::json!({"directory_ref": " ", "text": "x"}))
            .unwrap();
        let empty_text = SourceJson::new(serde_json::json!({"directory_ref": "d", "text": ""}))
            .unwrap();
        for bad in [empty_dir, empty_text] {
            let error = decode_skill_documents(&[bad]).unwrap_err();
            assert!(error.starts_with("SOURCE_BATCH_KIND_MISMATCH"), "{error}");
        }
    }
}
