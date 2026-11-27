//! Admission of `skill_documents` batches (MEDULLA-REPO-INGEST-R002.1.6).
//!
//! A skill document is a typed item of its own batch kind, never a mapped record.
//! Each document compiles once, in batch order, through
//! `medulla_document::skill_plan::compile_skill` and is stored as one
//! `skill-compilation:<hex>` node, linked by `compiledFrom` to its
//! `skill-directory:<hex>` node (MEDULLA-REPO-INGEST-R002.1.3). A refused document
//! refuses the whole batch before any write.

use std::collections::{BTreeMap, BTreeSet};

use medulla_document::skill_plan::{compile_skill, CapabilityLookup, SkillDocument};
use medulla_ingestion_types::skill_plan::SkillCompilation;
use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_ingestion::batch_kind::{SkillDocumentItem, MAX_SKILL_DOCUMENTS};
use medulla_source_types::source_ingestion::SourceJson;
use medulla_types::contract::{Digest256, ServerErrorCode};

/// The digest domain of a skill compilation node id.
const SKILL_COMPILATION_DOMAIN: &[u8] = b"eg/skill-compilation/v1";
/// The digest domain of a skill directory node id.
const SKILL_DIRECTORY_DOMAIN: &[u8] = b"eg/skill-directory/v1";
/// The relation from a compilation node to its directory node.
const COMPILED_FROM: &str = "compiledFrom";

fn mismatch(detail: impl std::fmt::Display) -> String {
    format!(
        "{}: {detail}",
        ServerErrorCode::SourceBatchKindMismatch.as_str()
    )
}

/// The writes of one admitted `skill_documents` batch.
#[derive(Debug, Default)]
pub struct SkillAdmission {
    /// The compilation and directory nodes written: each takes one policy record.
    pub node_ids: Vec<String>,
    /// Per document, in batch order: the compilation node, the directory node and
    /// the `compiledFrom` edge between them.
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

/// The node id of the directory node a compilation is linked to.
pub fn directory_node_id(
    tenant_scope: &str,
    connector: &str,
    directory_ref: &str,
) -> Result<String, String> {
    Ok(format!(
        "skill-directory:{}",
        Digest256::framed(
            SKILL_DIRECTORY_DOMAIN,
            &[
                tenant_scope.as_bytes(),
                connector.as_bytes(),
                directory_ref.as_bytes(),
            ],
        )?
        .to_hex()
    ))
}

/// The compilation node ids a batch names, to read the held nodes by.
pub fn held_ids(
    tenant_scope: &str,
    connector: &str,
    documents: &[SkillDocumentItem],
) -> Result<BTreeSet<String>, String> {
    documents
        .iter()
        .map(|document| compilation_node_id(tenant_scope, connector, &document.directory_ref))
        .collect()
}

/// The revision a compilation is written under: 1 for a first compile; the held
/// revision plus one when any of the four digests differs from the held node's;
/// the held revision otherwise.
fn next_revision(held: Option<&serde_json::Value>, compilation: &SkillCompilation) -> u64 {
    let Some(held) = held else {
        return 1;
    };
    let held_revision = held
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1);
    let unchanged = [
        ("document_digest", compilation.document_digest),
        ("capability_digest", compilation.capability_digest),
        ("type_digest", compilation.type_digest),
        ("plan_digest", compilation.plan_digest),
    ]
    .into_iter()
    .all(|(name, digest)| {
        held.get(name).and_then(serde_json::Value::as_str) == Some(digest.to_hex().as_str())
    });
    if unchanged {
        held_revision
    } else {
        held_revision + 1
    }
}

fn compilation_properties(
    compilation: &SkillCompilation,
    revision: u64,
) -> Result<serde_json::Value, String> {
    let mut properties = serde_json::json!({
        "type": "SkillCompilation",
        "revision": revision,
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

/// Compile each document once, in batch order, and compile its writes. `held` maps
/// each compilation node id [`held_ids`] named that the graph already holds to its
/// properties. A `SkillPlanRefusal` refuses the whole batch with its registered
/// code before any write.
pub fn admit_skill(
    tenant_scope: &str,
    connector: &str,
    documents: &[SkillDocumentItem],
    lookup: &dyn CapabilityLookup,
    held: &BTreeMap<String, serde_json::Value>,
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
        let directory_id = directory_node_id(tenant_scope, connector, &document.directory_ref)?;
        let revision = next_revision(held.get(&node_id), &compilation);
        let encode = |value: &serde_json::Value| {
            rmp_serde::to_vec_named(value)
                .map_err(|error| mismatch(format!("skill encoding failed: {error}")))
        };
        admission.methods.push(Method::AddNode {
            node_id: node_id.clone(),
            properties_msgpack: encode(&compilation_properties(&compilation, revision)?)?,
        });
        admission.methods.push(Method::AddNode {
            node_id: directory_id.clone(),
            properties_msgpack: encode(&serde_json::json!({
                "type": "SkillDirectory",
                "directory_ref": document.directory_ref,
            }))?,
        });
        admission.methods.push(Method::AddEdge {
            source_id: node_id.clone(),
            target_id: directory_id.clone(),
            properties_msgpack: encode(&serde_json::json!({ "relationship": COMPILED_FROM }))?,
        });
        admission.node_ids.push(node_id);
        admission.node_ids.push(directory_id);
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
        let held = BTreeMap::new();
        let admission =
            admit_skill("t1", "demo", std::slice::from_ref(&good), &Lookup, &held).unwrap();
        // The compilation node, its directory node and the edge between them.
        assert_eq!(admission.methods.len(), 3);
        assert_eq!(admission.node_ids.len(), 2);
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
        let refused =
            admit_skill("t1", "demo", &[good.clone(), broken], &Lookup, &held).unwrap_err();
        assert!(refused.starts_with("SKILL_SPAN_MISSING"), "{refused}");
        let unknown = item(
            "skills/unknown",
            "## Steps\n- a: x\n  capability: nope.run\n",
        );
        let refused = admit_skill("t1", "demo", &[good, unknown], &Lookup, &held).unwrap_err();
        assert!(
            refused.starts_with("SKILL_CAPABILITY_UNRESOLVED"),
            "{refused}"
        );

        // Decoding refuses an empty directory_ref and an empty text.
        let empty_dir =
            SourceJson::new(serde_json::json!({"directory_ref": " ", "text": "x"})).unwrap();
        let empty_text =
            SourceJson::new(serde_json::json!({"directory_ref": "d", "text": ""})).unwrap();
        for bad in [empty_dir, empty_text] {
            let error = decode_skill_documents(&[bad]).unwrap_err();
            assert!(error.starts_with("SOURCE_BATCH_KIND_MISMATCH"), "{error}");
        }
    }

    const SKILL: &str =
        "## Steps\n- read: Read the node\n  capability: GetNodeProperties\n- report: Report it\n  after: read\n";

    fn properties_of(method: &Method) -> serde_json::Value {
        match method {
            Method::AddNode {
                properties_msgpack, ..
            } => rmp_serde::from_slice(properties_msgpack).unwrap(),
            other => panic!("expected AddNode, got {other:?}"),
        }
    }

    // spec: MEDULLA-REPO-INGEST-R002.1.3
    #[test]
    fn a_compilation_links_to_its_directory_node() {
        let documents = [item("skills/inspect", SKILL)];
        let admission = admit_skill("t1", "demo", &documents, &Lookup, &BTreeMap::new()).unwrap();
        let compilation_id = compilation_node_id("t1", "demo", "skills/inspect").unwrap();
        let directory_id = directory_node_id("t1", "demo", "skills/inspect").unwrap();
        assert!(directory_id.starts_with("skill-directory:"));
        assert_eq!(
            admission.node_ids,
            [compilation_id.clone(), directory_id.clone()]
        );
        let kinds: Vec<&str> = admission
            .methods
            .iter()
            .map(|method| match method {
                Method::AddNode { .. } => "node",
                Method::AddEdge { .. } => "edge",
                _ => "other",
            })
            .collect();
        // The directory node is written before the edge.
        assert_eq!(kinds, ["node", "node", "edge"]);
        let directory = properties_of(&admission.methods[1]);
        assert_eq!(directory["type"], "SkillDirectory");
        assert_eq!(directory["directory_ref"], "skills/inspect");
        match &admission.methods[2] {
            Method::AddEdge {
                source_id,
                target_id,
                properties_msgpack,
            } => {
                assert_eq!(source_id, &compilation_id);
                assert_eq!(target_id, &directory_id);
                let edge: serde_json::Value = rmp_serde::from_slice(properties_msgpack).unwrap();
                assert_eq!(edge["relationship"], "compiledFrom");
            }
            other => panic!("expected AddEdge, got {other:?}"),
        }
        // The graph holds no document text.
        assert!(!properties_of(&admission.methods[0])
            .to_string()
            .contains("Read the node"));
    }

    // spec: MEDULLA-REPO-INGEST-R002.1.3
    #[test]
    fn revision_starts_at_one_and_moves_with_the_digests() {
        let id = compilation_node_id("t1", "demo", "skills/inspect").unwrap();
        let first = admit_skill(
            "t1",
            "demo",
            &[item("skills/inspect", SKILL)],
            &Lookup,
            &BTreeMap::new(),
        )
        .unwrap();
        let written = properties_of(&first.methods[0]);
        assert_eq!(written["revision"], 1);
        let mut held = BTreeMap::new();
        held.insert(id.clone(), written);

        // An identical re-send keeps the held revision.
        let again = admit_skill(
            "t1",
            "demo",
            &[item("skills/inspect", SKILL)],
            &Lookup,
            &held,
        )
        .unwrap();
        assert_eq!(properties_of(&again.methods[0])["revision"], 1);

        // A changed document moves it by one.
        let changed = SKILL.replace("Report it", "Report the result");
        let next = admit_skill(
            "t1",
            "demo",
            &[item("skills/inspect", &changed)],
            &Lookup,
            &held,
        )
        .unwrap();
        let next_properties = properties_of(&next.methods[0]);
        assert_eq!(next_properties["revision"], 2);
        held.insert(id, next_properties);
        let third = admit_skill(
            "t1",
            "demo",
            &[item(
                "skills/inspect",
                &changed.replace("Read the node", "Read it"),
            )],
            &Lookup,
            &held,
        )
        .unwrap();
        assert_eq!(properties_of(&third.methods[0])["revision"], 3);
    }
}
