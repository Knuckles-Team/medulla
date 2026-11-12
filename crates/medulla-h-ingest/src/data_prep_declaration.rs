//! Served data-prep declarations (MEDULLA-INGEST-WORKER-R001.1007 to R001.1009).
//!
//! A host declares which opaque fields a data-prep capability reads and the
//! digest of the policy it runs under. `declare_data_prep` validates the
//! payload before any write (`validate_declaration`), compares the content
//! digest with the stored one and lowers a changed declaration to ONE
//! digest-keyed write-set: the declaration node is upserted and its
//! `declares_field` edges are replaced (new edges added, absent ones removed)
//! in the same envelope, so a reader never sees a mixed set. An equal digest is
//! unchanged and writes nothing. A refusal changes no state.

use std::collections::BTreeSet;
use std::fmt;

use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::Method;
use medulla_types::contract::Digest256;
use medulla_types::data_prep_declaration::{
    DataPrepDeclaration, DataPrepDeclarationError, DataPrepDeclarationView,
    DATA_PREP_DECLARATION_INVALID, DATA_PREP_DECLARATION_TOO_LARGE, MAX_FIELD_REFS,
};
use serde_json::json;

use super::repository_index::{EnvelopeLabels, IndexWriteSet};

/// Node type of one stored declaration.
pub const DECLARATION_NODE_TYPE: &str = "DataPrepDeclaration";
/// Node type of one declared field reference.
pub const FIELD_NODE_TYPE: &str = "DataPrepField";
/// Relationship from a declaration to each field it declares.
pub const DECLARES_FIELD: &str = "declares_field";
const ENVELOPE_PREFIX: &str = "data-prep-declaration:";
const CONTENT_DOMAIN: &[u8] = b"eg/data-prep-declaration";
const TENANT_DOMAIN: &[u8] = b"eg/data-prep-tenant";

/// Envelope labels of the declaration commit.
pub const LABELS: EnvelopeLabels = EnvelopeLabels {
    version_type: "data-prep-declaration-sha256",
    policy_version: "data-prep-declaration",
};

/// One refusal of a served declaration call. Every refusal changes no state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataPrepRefusal {
    /// The stable wire code the client sees.
    pub code: &'static str,
    /// The detail carried with the code.
    pub detail: String,
}

impl fmt::Display for DataPrepRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for DataPrepRefusal {}

fn invalid(detail: impl Into<String>) -> DataPrepRefusal {
    DataPrepRefusal {
        code: DATA_PREP_DECLARATION_INVALID,
        detail: detail.into(),
    }
}

impl From<DataPrepDeclarationError> for DataPrepRefusal {
    fn from(error: DataPrepDeclarationError) -> Self {
        match error {
            DataPrepDeclarationError::TooManyFieldRefs => DataPrepRefusal {
                code: DATA_PREP_DECLARATION_TOO_LARGE,
                detail: error.to_string(),
            },
            other => invalid(other.to_string()),
        }
    }
}

/// Hex digest of the tenant under the declaration domain, used in node ids so
/// a tenant name never appears in one.
pub fn tenant_hex(tenant: &str) -> Result<String, DataPrepRefusal> {
    Digest256::framed(TENANT_DOMAIN, &[tenant.as_bytes()])
        .map(|digest| digest.to_hex())
        .map_err(invalid)
}

/// Id of the declaration node of one tenant and capability.
pub fn declaration_node_id(tenant: &str, capability: &str) -> Result<String, DataPrepRefusal> {
    Ok(format!("data_prep:{}:{capability}", tenant_hex(tenant)?))
}

fn field_node_prefix(tenant_hex: &str) -> String {
    format!("data_prep_field:{tenant_hex}:")
}

/// SHA-256 framed under `eg/data-prep-declaration` over the capability, the
/// sorted field references and the policy digest.
pub fn content_digest(declaration: &DataPrepDeclaration) -> Result<Digest256, DataPrepRefusal> {
    let count = (declaration.field_refs.len() as u64).to_be_bytes();
    let mut fields: Vec<&[u8]> = vec![declaration.capability.as_bytes(), &count];
    fields.extend(declaration.field_refs.iter().map(|field| field.as_bytes()));
    let policy = declaration.policy_digest.to_hex();
    fields.push(policy.as_bytes());
    Digest256::framed(CONTENT_DOMAIN, &fields).map_err(invalid)
}

/// Decode and check a payload against the declaration grammar. An unknown key
/// or a value outside the grammar is `DATA_PREP_DECLARATION_INVALID`; more
/// than 256 field references is `DATA_PREP_DECLARATION_TOO_LARGE`.
pub fn validate_declaration(
    payload: &serde_json::Value,
) -> Result<DataPrepDeclaration, DataPrepRefusal> {
    if let Some(count) = payload
        .get("field_refs")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
    {
        if count > MAX_FIELD_REFS {
            return Err(DataPrepDeclarationError::TooManyFieldRefs.into());
        }
    }
    let declaration: DataPrepDeclaration = serde_json::from_value(payload.clone())
        .map_err(|error| invalid(format!("declaration payload is not valid: {error}")))?;
    declaration.validate()?;
    Ok(declaration)
}

/// What the graph holds for one declaration node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredDeclaration {
    pub capability: String,
    pub content_digest: String,
    pub policy_digest: String,
    /// Ids of the field nodes the declaration points at through `declares_field`.
    pub field_targets: BTreeSet<String>,
}

/// The reader of stored declarations.
pub trait DeclarationView {
    /// The declaration stored at `node_id`, as the caller reads it.
    fn stored(&self, node_id: &str) -> Option<StoredDeclaration>;
}

/// A [`DeclarationView`] over a graph core, restricted to the rows `visible`
/// admits (the caller's row authority).
pub struct CoreDeclarationView<'a> {
    pub core: &'a GraphCore,
    pub visible: &'a dyn Fn(&str) -> bool,
}

impl DeclarationView for CoreDeclarationView<'_> {
    fn stored(&self, node_id: &str) -> Option<StoredDeclaration> {
        if !(self.visible)(node_id) {
            return None;
        }
        let blob = self.core.get_node_properties(node_id)?;
        let value = medulla_types::msgpack::decode_property_value(&blob).ok()?;
        if value.get("type").and_then(|v| v.as_str()) != Some(DECLARATION_NODE_TYPE) {
            return None;
        }
        let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_string);
        let field_targets = self
            .core
            .get_successors(node_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|target| {
                self.core
                    .typed_edges_between(node_id, target)
                    .iter()
                    .any(|(relation, _)| relation == DECLARES_FIELD)
            })
            .collect();
        Some(StoredDeclaration {
            capability: text("capability")?,
            content_digest: text("content_digest")?,
            policy_digest: text("policy_digest")?,
            field_targets,
        })
    }
}

/// What one declaration call decided.
pub enum Declared {
    /// The digest equals the stored one: nothing is written.
    Unchanged { content_digest: Digest256 },
    /// One write-set replaces the declaration and its edges.
    Write {
        content_digest: Digest256,
        write_set: IndexWriteSet,
    },
}

fn encode(value: &serde_json::Value) -> Result<Vec<u8>, DataPrepRefusal> {
    rmp_serde::to_vec_named(value)
        .map_err(|error| invalid(format!("declaration encoding failed: {error}")))
}

/// Validate `payload`, compare it with the stored declaration of the same
/// capability and lower the change. The grammar check runs before any read.
pub fn declare_data_prep(
    view: &dyn DeclarationView,
    tenant: &str,
    payload: &serde_json::Value,
) -> Result<Declared, DataPrepRefusal> {
    let declaration = validate_declaration(payload)?;
    let digest = content_digest(&declaration)?;
    let node_id = declaration_node_id(tenant, &declaration.capability)?;
    let stored = view.stored(&node_id);
    if stored
        .as_ref()
        .is_some_and(|stored| stored.content_digest == digest.to_hex())
    {
        return Ok(Declared::Unchanged {
            content_digest: digest,
        });
    }
    let prefix = field_node_prefix(&tenant_hex(tenant)?);
    let wanted: BTreeSet<String> = declaration
        .field_refs
        .iter()
        .map(|field| format!("{prefix}{field}"))
        .collect();
    let held = stored
        .map(|stored| stored.field_targets)
        .unwrap_or_default();

    let mut methods = vec![Method::AddNode {
        node_id: node_id.clone(),
        properties_msgpack: encode(&json!({
            "type": DECLARATION_NODE_TYPE,
            "capability": declaration.capability,
            "policy_digest": declaration.policy_digest.to_hex(),
            "content_digest": digest.to_hex(),
            "field_count": declaration.field_refs.len(),
        }))?,
    }];
    for target in wanted.difference(&held) {
        methods.push(Method::AddNode {
            node_id: target.clone(),
            properties_msgpack: encode(&json!({
                "type": FIELD_NODE_TYPE,
                "field_ref": &target[prefix.len()..],
            }))?,
        });
    }
    for target in held.difference(&wanted) {
        methods.push(Method::RemoveEdge {
            source_id: node_id.clone(),
            target_id: target.clone(),
        });
    }
    for target in wanted.difference(&held) {
        methods.push(Method::AddEdge {
            source_id: node_id.clone(),
            target_id: target.clone(),
            properties_msgpack: encode(&json!({ "relationship": DECLARES_FIELD }))?,
        });
    }
    let write_set =
        IndexWriteSet::from_methods(ENVELOPE_PREFIX, b"eg/data-prep-declaration-write", methods)
            .map_err(invalid)?;
    Ok(Declared::Write {
        content_digest: digest,
        write_set,
    })
}

/// Read the stored declaration of one capability, or none.
pub fn read_data_prep_declaration(
    view: &dyn DeclarationView,
    tenant: &str,
    capability: &str,
) -> Result<DataPrepDeclarationView, DataPrepRefusal> {
    let absent = DataPrepDeclarationView {
        declaration: None,
        content_digest: None,
    };
    let probe = DataPrepDeclaration {
        capability: capability.to_string(),
        field_refs: BTreeSet::new(),
        policy_digest: Digest256::sha256(b""),
    };
    probe.validate()?;
    let node_id = declaration_node_id(tenant, capability)?;
    let Some(stored) = view.stored(&node_id) else {
        return Ok(absent);
    };
    let prefix = field_node_prefix(&tenant_hex(tenant)?);
    let declaration = DataPrepDeclaration {
        capability: stored.capability,
        field_refs: stored
            .field_targets
            .iter()
            .filter_map(|target| target.strip_prefix(&prefix).map(str::to_string))
            .collect(),
        policy_digest: Digest256::parse(&stored.policy_digest).map_err(invalid)?,
    };
    Ok(DataPrepDeclarationView {
        content_digest: Some(Digest256::parse(&stored.content_digest).map_err(invalid)?),
        declaration: Some(declaration),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TENANT: &str = "tenant:a";

    fn payload(fields: &[&str]) -> serde_json::Value {
        json!({
            "capability": "text.normalize-v1",
            "field_refs": fields,
            "policy_digest": Digest256::sha256(b"policy").to_hex(),
        })
    }

    fn apply(core: &GraphCore, write_set: &IndexWriteSet) {
        for method in write_set.methods() {
            match method {
                Method::AddNode {
                    node_id,
                    properties_msgpack,
                } => core.add_node(node_id.clone(), properties_msgpack.clone()),
                Method::AddEdge {
                    source_id,
                    target_id,
                    properties_msgpack,
                } => core
                    .add_edge(
                        source_id.clone(),
                        target_id.clone(),
                        properties_msgpack.clone(),
                    )
                    .expect("edge endpoints exist"),
                Method::RemoveEdge {
                    source_id,
                    target_id,
                } => core.remove_edge(source_id.clone(), target_id.clone()),
                other => panic!("unexpected method {other:?}"),
            }
        }
    }

    fn edge_set(core: &GraphCore) -> BTreeSet<String> {
        let all = |_: &str| true;
        let view = CoreDeclarationView {
            core,
            visible: &all,
        };
        let node = declaration_node_id(TENANT, "text.normalize-v1").unwrap();
        view.stored(&node)
            .map(|stored| stored.field_targets)
            .unwrap_or_default()
    }

    // spec: MEDULLA-INGEST-WORKER-R001.1007
    /// Verifies MEDULLA-INGEST-WORKER-R001.1007: Behavior: data-prep declaration replace.
    #[test]
    fn replace_is_atomic_and_equal_digest_is_unchanged() {
        let core = GraphCore::new();
        let all = |_: &str| true;
        let view = || CoreDeclarationView {
            core: &core,
            visible: &all,
        };
        assert!(edge_set(&core).is_empty());

        let Declared::Write { write_set, .. } =
            declare_data_prep(&view(), TENANT, &payload(&["a.field", "b.field"])).unwrap()
        else {
            panic!("a first declaration writes");
        };
        apply(&core, &write_set);
        let before = edge_set(&core);
        assert_eq!(before.len(), 2);

        // An equal declaration, in any field order, writes nothing.
        let Declared::Unchanged { .. } =
            declare_data_prep(&view(), TENANT, &payload(&["b.field", "a.field"])).unwrap()
        else {
            panic!("an equal digest is unchanged");
        };
        assert_eq!(edge_set(&core), before);

        // A changed declaration is one write-set holding both the removal and the addition.
        let Declared::Write { write_set, .. } =
            declare_data_prep(&view(), TENANT, &payload(&["b.field", "c.field"])).unwrap()
        else {
            panic!("a changed declaration writes");
        };
        let removed = write_set
            .methods()
            .iter()
            .filter(|method| matches!(method, Method::RemoveEdge { .. }))
            .count();
        let added = write_set
            .methods()
            .iter()
            .filter(|method| matches!(method, Method::AddEdge { .. }))
            .count();
        assert_eq!((removed, added), (1, 1));
        assert_eq!(edge_set(&core), before, "nothing changes before the commit");
        apply(&core, &write_set);
        let after = edge_set(&core);
        let prefix = field_node_prefix(&tenant_hex(TENANT).unwrap());
        let refs: BTreeSet<String> = after
            .iter()
            .filter_map(|target| target.strip_prefix(&prefix).map(str::to_string))
            .collect();
        assert_eq!(
            refs,
            ["b.field", "c.field"]
                .iter()
                .map(|f| f.to_string())
                .collect()
        );

        let read = read_data_prep_declaration(&view(), TENANT, "text.normalize-v1").unwrap();
        assert_eq!(read.declaration.unwrap().field_refs, refs);
        let absent = read_data_prep_declaration(&view(), TENANT, "other.capability").unwrap();
        assert!(absent.declaration.is_none() && absent.content_digest.is_none());
    }

    // spec: MEDULLA-INGEST-WORKER-R001.1009
    /// Verifies MEDULLA-INGEST-WORKER-R001.1009: Refusal: data-prep declaration.
    #[test]
    fn refusals_name_their_code_and_validate_before_any_read() {
        struct Unreadable;
        impl DeclarationView for Unreadable {
            fn stored(&self, _: &str) -> Option<StoredDeclaration> {
                panic!("the grammar check runs before any read");
            }
        }
        let refuse = |value: serde_json::Value| match declare_data_prep(&Unreadable, TENANT, &value)
        {
            Err(refusal) => refusal.code,
            Ok(_) => panic!("must refuse {value}"),
        };
        for bad in ["a:b", "a/b", "usr/bin/sh", "has space", ""] {
            assert_eq!(refuse(payload(&[bad])), DATA_PREP_DECLARATION_INVALID);
        }
        let mut unknown = payload(&["a"]);
        unknown["command"] = json!("/bin/sh");
        assert_eq!(refuse(unknown), DATA_PREP_DECLARATION_INVALID);
        let mut capability = payload(&["a"]);
        capability["capability"] = json!("mod:fn");
        assert_eq!(refuse(capability), DATA_PREP_DECLARATION_INVALID);
        let many: Vec<String> = (0..257).map(|i| format!("f{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(refuse(payload(&many)), DATA_PREP_DECLARATION_TOO_LARGE);
    }
}
