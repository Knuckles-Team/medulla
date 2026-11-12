//! Typed principal metadata admission (MEDULLA-IDENTITY-R023.8 to R023.11).
//!
//! `compile_methods` calls [`admit_all`] on the whole write set before it
//! compiles anything. A node write that carries one of the labels `Preference`,
//! `ClientMetadata` or `InteractionNetwork` is validated as [`TypedMetadata`],
//! its owner is replaced by the verified principal, and the version and
//! provenance keys are stamped. Any other write passes untouched. A refusal
//! happens before any durable write, so nothing is committed.

use std::fmt;

use medulla_identity_types::identity::{TypedMetadata, METADATA_SCHEMA_VERSION};
use medulla_protocol_types::protocol::Method;
use medulla_types::contract::ServerErrorCode;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// The three graph labels of typed principal metadata.
pub const METADATA_LABELS: [&str; 3] = ["Preference", "ClientMetadata", "InteractionNetwork"];

/// The refusal code for a caller that may not write the record.
const ACCESS_DENIED: &str = "ACCESS_DENIED";

/// The ontology the records are admitted under; its digest is stamped on each record.
const SCHEMA_TTL: &str = include_str!("../../../medulla-core/ontology/identity_metadata.ttl");

/// Node keys the caller may never choose: the admission step fills them.
const SERVER_KEYS: [&str; 6] = [
    "owner",
    "owner_tenant",
    "revision",
    "actor",
    "schema_digest",
    "parent_revision",
];

/// The verified caller of one write set.
#[derive(Debug, Clone, Copy)]
pub struct MetadataCaller<'a> {
    /// The verified principal; `None` for an engine-internal write.
    pub principal: Option<&'a str>,
    /// The verified tenant.
    pub tenant: &'a str,
}

/// A refused metadata write: a stable code and a message naming the field path only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataRefusal {
    /// The wire code.
    pub code: &'static str,
    /// The message; it holds a field path and no hidden identity.
    pub message: String,
}

impl fmt::Display for MetadataRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

fn refuse(code: ServerErrorCode, message: impl Into<String>) -> MetadataRefusal {
    MetadataRefusal {
        code: code.as_str(),
        message: message.into(),
    }
}

/// The digest of the schema the records are admitted under (SHA-256, lowercase hex).
pub fn schema_digest() -> String {
    hex::encode(Sha256::digest(SCHEMA_TTL.as_bytes()))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn mentions_label(blob: &[u8]) -> bool {
    METADATA_LABELS
        .iter()
        .any(|label| contains(blob, label.as_bytes()))
}

/// True when `text` names a metadata label as a whole word.
fn text_names_label(text: &str) -> bool {
    METADATA_LABELS.iter().any(|label| {
        text.match_indices(label).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + label.len()..].chars().next();
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            !word(before) && !word(after)
        })
    })
}

fn properties_of(method: &mut Method) -> Option<&mut Vec<u8>> {
    match method {
        Method::AddNode {
            properties_msgpack, ..
        }
        | Method::CreateNodeIfAbsent {
            properties_msgpack, ..
        }
        | Method::TxnAddNode {
            properties_msgpack, ..
        } => Some(properties_msgpack),
        _ => None,
    }
}

fn node_id_of(method: &Method) -> &str {
    match method {
        Method::AddNode { node_id, .. }
        | Method::CreateNodeIfAbsent { node_id, .. }
        | Method::TxnAddNode { node_id, .. } => node_id,
        _ => "",
    }
}

/// The text of a write route whose body is a statement or a document.
fn statement_text(method: &Method) -> Option<Vec<&str>> {
    match method {
        Method::ApplyMutation { query, .. }
        | Method::Sql { query, .. }
        | Method::CypherQuery { query, .. } => Some(vec![query.as_str()]),
        Method::AddTriples {
            turtle, ntriples, ..
        } => Some(vec![turtle.as_str(), ntriples.as_str()]),
        _ => None,
    }
}

fn label_of(props: &Map<String, Value>) -> Option<&str> {
    ["type", "node_type"]
        .iter()
        .filter_map(|key| props.get(*key).and_then(Value::as_str))
        .find(|label| METADATA_LABELS.contains(label))
}

fn is_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn resolves(principal_id: &str, caller: &MetadataCaller<'_>) -> bool {
    if caller.principal == Some(principal_id) {
        return true;
    }
    #[cfg(feature = "security")]
    {
        crate::identity_view::any_manages(principal_id)
    }
    #[cfg(not(feature = "security"))]
    {
        false
    }
}

/// Validate the body, stamp the server keys and return the stamped properties plus
/// the typed record.
fn admit_record(
    label: &str,
    mut props: Map<String, Value>,
    caller: &MetadataCaller<'_>,
) -> Result<(Map<String, Value>, TypedMetadata), MetadataRefusal> {
    let Some(principal) = caller.principal.filter(|p| !p.is_empty()) else {
        return Err(MetadataRefusal {
            code: ACCESS_DENIED,
            message: "metadata is written by a verified principal".into(),
        });
    };
    // A caller-supplied owner is ignored: the verified principal replaces it.
    let parent_revision = props
        .get("parent_revision")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    for key in SERVER_KEYS {
        props.remove(key);
    }
    let supersedes = props.get("supersedes").cloned();
    let source_digest = props.get("source_digest").cloned();
    let mapping_version = props.get("mapping_version").cloned();
    let mut body = props.clone();
    for key in [
        "type",
        "node_type",
        "_tenant",
        "supersedes",
        "source_digest",
        "mapping_version",
    ] {
        body.remove(key);
    }
    body.entry("schema_version")
        .or_insert(json!(METADATA_SCHEMA_VERSION));
    let mut tagged = Map::new();
    tagged.insert(label.to_owned(), Value::Object(body));
    let typed: TypedMetadata = serde_json::from_value(Value::Object(tagged)).map_err(|error| {
        refuse(
            ServerErrorCode::MetadataUntyped,
            format!("{label} body is not valid: {error}"),
        )
    })?;
    typed.validate().map_err(|error| {
        refuse(
            ServerErrorCode::MetadataSchemaInvalid,
            format!("{label}: {error}"),
        )
    })?;
    if let Some(supersedes) = &supersedes {
        if !supersedes.as_str().is_some_and(|id| !id.is_empty()) {
            return Err(refuse(
                ServerErrorCode::MetadataSchemaInvalid,
                "supersedes must name a node id",
            ));
        }
    } else if parent_revision > 0 {
        return Err(refuse(
            ServerErrorCode::MetadataSchemaInvalid,
            "parent_revision requires supersedes",
        ));
    }
    match (&source_digest, &mapping_version) {
        (None, None) => {}
        (Some(digest), Some(version))
            if digest.as_str().is_some_and(is_hex_digest)
                && version.as_str().is_some_and(|v| !v.is_empty()) => {}
        _ => {
            return Err(refuse(
                ServerErrorCode::MetadataSchemaInvalid,
                "source_digest (64 lowercase hex) and mapping_version come together",
            ))
        }
    }
    if let TypedMetadata::InteractionNetwork(network) = &typed {
        if network.subject.as_str() != principal {
            return Err(MetadataRefusal {
                code: ACCESS_DENIED,
                message: "InteractionNetwork.subject must be the writing principal".into(),
            });
        }
        if !resolves(network.peer.as_str(), caller) {
            return Err(refuse(
                ServerErrorCode::MetadataPrincipalUnknown,
                "InteractionNetwork.peer does not resolve",
            ));
        }
    }
    props.insert("type".into(), json!(label));
    props.insert("schema_version".into(), json!(METADATA_SCHEMA_VERSION));
    props.insert("owner".into(), json!(principal));
    props.insert("owner_tenant".into(), json!(caller.tenant));
    props.insert("actor".into(), json!(principal));
    props.insert("revision".into(), json!(parent_revision + 1));
    props.insert("parent_revision".into(), json!(parent_revision));
    props.insert("schema_digest".into(), json!(schema_digest()));
    Ok((props, typed))
}

fn principal_node(principal_id: &str) -> String {
    format!("principal:{principal_id}")
}

fn encode(props: &Map<String, Value>) -> Result<Vec<u8>, MetadataRefusal> {
    rmp_serde::to_vec_named(&Value::Object(props.clone())).map_err(|error| {
        refuse(
            ServerErrorCode::MetadataSchemaInvalid,
            format!("record does not encode: {error}"),
        )
    })
}

/// Admit one method. A metadata node write comes back stamped, followed by the
/// edge writes of an interaction; every other method comes back unchanged.
pub fn admit(
    mut method: Method,
    caller: &MetadataCaller<'_>,
) -> Result<Vec<Method>, MetadataRefusal> {
    if let Some(texts) = statement_text(&method) {
        if texts.iter().any(|text| text_names_label(text)) {
            return Err(refuse(
                ServerErrorCode::MetadataUntyped,
                "typed metadata is written as a node write with a typed body, not as a statement",
            ));
        }
        return Ok(vec![method]);
    }
    let node_id = node_id_of(&method).to_owned();
    let Some(blob) = properties_of(&mut method) else {
        return Ok(vec![method]);
    };
    if !mentions_label(blob) {
        return Ok(vec![method]);
    }
    let Ok(props) = medulla_types::msgpack::decode_property_object(blob) else {
        return Ok(vec![method]);
    };
    let Some(label) = label_of(&props).map(str::to_owned) else {
        return Ok(vec![method]);
    };
    let (stamped, typed) = admit_record(&label, props, caller)?;
    *blob = encode(&stamped)?;
    let mut out = vec![method];
    if let TypedMetadata::InteractionNetwork(network) = &typed {
        let relation = stamped
            .get("relation")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        for principal_id in [network.subject.as_str(), network.peer.as_str()] {
            out.push(Method::CreateNodeIfAbsent {
                node_id: principal_node(principal_id),
                properties_msgpack: encode(
                    json!({ "type": "Person", "principal_id": principal_id })
                        .as_object()
                        .expect("object literal"),
                )?,
            });
        }
        out.push(Method::AddEdge {
            source_id: principal_node(network.subject.as_str()),
            target_id: principal_node(network.peer.as_str()),
            properties_msgpack: encode(
                json!({
                    "type": relation,
                    "weight_milli": network.weight_milli,
                    "record": node_id,
                })
                .as_object()
                .expect("object literal"),
            )?,
        });
    }
    Ok(out)
}

/// Admit a whole write set in order.
pub fn admit_all(
    methods: Vec<Method>,
    caller: &MetadataCaller<'_>,
) -> Result<Vec<Method>, MetadataRefusal> {
    let mut out = Vec::with_capacity(methods.len());
    for method in methods {
        out.extend(admit(method, caller)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_node(id: &str, props: Value) -> Method {
        Method::AddNode {
            node_id: id.into(),
            properties_msgpack: rmp_serde::to_vec_named(&props).unwrap(),
        }
    }

    fn caller() -> MetadataCaller<'static> {
        MetadataCaller {
            principal: Some("usr:alice"),
            tenant: "acme",
        }
    }

    fn props_of_first(out: &[Method]) -> Map<String, Value> {
        let Method::AddNode {
            properties_msgpack, ..
        } = &out[0]
        else {
            panic!("first method is the node write");
        };
        medulla_types::msgpack::decode_property_object(properties_msgpack).unwrap()
    }

    // spec: MEDULLA-IDENTITY-R023.9
    #[test]
    fn untyped_and_foreign_writes_are_refused_before_compilation() {
        // A label without a valid body.
        let untyped = admit(add_node("p1", json!({"type": "Preference"})), &caller());
        assert_eq!(untyped.unwrap_err().code, "METADATA_UNTYPED");
        // A statement route cannot write the label.
        let statement = Method::ApplyMutation {
            event_type: "x".into(),
            query: "CREATE (:Preference {key: 'a'})".into(),
        };
        assert_eq!(
            admit(statement, &caller()).unwrap_err().code,
            "METADATA_UNTYPED"
        );
        // Other writes pass untouched.
        let plain = add_node("d1", json!({"type": "Doc"}));
        let out = admit(plain, &caller()).unwrap();
        assert!(matches!(out.as_slice(), [Method::AddNode { .. }]));
    }

    // spec: MEDULLA-IDENTITY-R023.8
    #[test]
    fn accepted_record_is_stamped_with_version_and_owner() {
        let out = admit(
            add_node(
                "p1",
                json!({"type": "Preference", "key": "theme", "value": "dark",
                       "share": "tenant", "owner": "usr:mallory"}),
            ),
            &caller(),
        )
        .unwrap();
        let props = props_of_first(&out);
        assert_eq!(props["owner"], "usr:alice");
        assert_eq!(props["owner_tenant"], "acme");
        assert_eq!(props["revision"], 1);
        assert_eq!(props["schema_version"], 1);
        assert_eq!(props["schema_digest"], schema_digest());
    }

    // spec: MEDULLA-IDENTITY-R023.10
    #[test]
    fn invalid_records_are_refused_with_their_code() {
        let long_key = "k".repeat(65);
        let over = admit(
            add_node(
                "p1",
                json!({"type": "Preference", "key": long_key, "value": "v", "share": "private"}),
            ),
            &caller(),
        );
        assert_eq!(over.unwrap_err().code, "METADATA_SCHEMA_INVALID");
        let foreign = admit(
            add_node(
                "n1",
                json!({"type": "InteractionNetwork", "subject": "usr:bob",
                       "peer": "usr:alice", "relation": "follows", "weight_milli": 5}),
            ),
            &caller(),
        );
        assert_eq!(foreign.unwrap_err().code, "ACCESS_DENIED");
        let unknown = admit(
            add_node(
                "n2",
                json!({"type": "InteractionNetwork", "subject": "usr:alice",
                       "peer": "usr:nobody", "relation": "follows", "weight_milli": 5}),
            ),
            &caller(),
        );
        assert_eq!(unknown.unwrap_err().code, "METADATA_PRINCIPAL_UNKNOWN");
        let anonymous = admit(
            add_node(
                "p2",
                json!({"type": "Preference", "key": "a", "value": "b", "share": "private"}),
            ),
            &MetadataCaller {
                principal: None,
                tenant: "acme",
            },
        );
        assert_eq!(anonymous.unwrap_err().code, "ACCESS_DENIED");
    }

    // spec: MEDULLA-IDENTITY-R023.11
    #[test]
    fn provenance_keys_are_filled_and_validated() {
        let digest = "a".repeat(64);
        let out = admit(
            add_node(
                "c1",
                json!({"type": "ClientMetadata", "client_id": "cli:1", "kind": "cli",
                       "version": "1.0", "capabilities": ["read"],
                       "source_digest": digest, "mapping_version": "m1",
                       "supersedes": "c0", "parent_revision": 1}),
            ),
            &caller(),
        )
        .unwrap();
        let props = props_of_first(&out);
        assert_eq!(props["actor"], "usr:alice");
        assert_eq!(props["revision"], 2);
        assert_eq!(props["parent_revision"], 1);
        assert_eq!(props["source_digest"], digest.as_str());
        let bad = admit(
            add_node(
                "c2",
                json!({"type": "ClientMetadata", "client_id": "cli:1", "kind": "cli",
                       "version": "1.0", "capabilities": [], "source_digest": "XYZ",
                       "mapping_version": "m1"}),
            ),
            &caller(),
        );
        assert_eq!(bad.unwrap_err().code, "METADATA_SCHEMA_INVALID");
    }
}
