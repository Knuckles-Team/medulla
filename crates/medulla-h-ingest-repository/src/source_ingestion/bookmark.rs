//! Bookmark ingest mapping (MEDULLA-MEDIA-VOCAB-R001.102.2).
//!
//! A record whose resolved mapping `ontology_class` is `Bookmark` becomes one `bookmark:<record_key>`
//! node carrying the keys of the connector's bookmark envelope, the entity nodes it names and the
//! edges to them. The mapping is called from `admit_record`; there is no second ingest path.

use medulla_protocol_types::protocol::Method;

/// The ontology class a bookmark record resolves to.
pub const BOOKMARK_CLASS: &str = "Bookmark";

/// The envelope keys stored as compact JSON arrays of strings.
const ARRAY_KEYS: [(&str, &str, &str, &str); 3] = [
    ("hashtags", "Hashtag", "hashtag", "taggedWith"),
    ("mentions", "Account", "account", "mentionsAccount"),
    ("tools", "Tool", "tool", "usesTool"),
];

/// What one bookmark record writes: its node, the entity nodes it names and the edges to them.
pub struct BookmarkMapping {
    pub node_id: String,
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// Entity node id with its `AddNode`; the caller writes each id once per batch.
    pub entities: Vec<(String, Method)>,
    pub edges: Vec<Method>,
}

fn text(payload: &serde_json::Value, key: &str) -> Result<String, String> {
    payload
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("BOOKMARK_RECORD_INVALID: `{key}` must be a string"))
}

fn strings(payload: &serde_json::Value, key: &str) -> Result<Vec<String>, String> {
    payload
        .get(key)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("BOOKMARK_RECORD_INVALID: `{key}` must be an array"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("BOOKMARK_RECORD_INVALID: `{key}` holds a non-string"))
        })
        .collect()
}

fn encode(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value).map_err(|error| format!("bookmark encoding failed: {error}"))
}

fn json_text(value: &impl serde::Serialize) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| format!("bookmark encoding failed: {error}"))
}

fn entity(
    entities: &mut Vec<(String, Method)>,
    edges: &mut Vec<Method>,
    bookmark_id: &str,
    entity: (&str, &str, &str, Option<&str>),
) -> Result<(), String> {
    let (class, prefix, value, label) = entity;
    let entity_id = format!("{prefix}:{}", value.to_lowercase());
    let mut properties = serde_json::json!({ "type": class });
    if let (Some(label), Some(object)) = (label, properties.as_object_mut()) {
        object.insert("label".into(), label.into());
    }
    if !entities.iter().any(|(id, _)| *id == entity_id) {
        entities.push((
            entity_id.clone(),
            Method::AddNode {
                node_id: entity_id.clone(),
                properties_msgpack: encode(&properties)?,
            },
        ));
    }
    let relationship = match class {
        "Hashtag" => "taggedWith",
        "Account" => "mentionsAccount",
        "Tool" => "usesTool",
        _ => "linksToDomain",
    };
    edges.push(Method::AddEdge {
        source_id: bookmark_id.into(),
        target_id: entity_id,
        properties_msgpack: encode(&serde_json::json!({ "relationship": relationship }))?,
    });
    Ok(())
}

/// Map one bookmark record. `source_id` is the connector stream id of the record; `receipt_id` and
/// `receipt_digest` are the envelope id and digest of the `SourceIngest` commit that writes the node.
pub fn map_bookmark(
    payload: &serde_json::Value,
    source_id: &str,
    receipt_id: &str,
    receipt_digest: &str,
) -> Result<BookmarkMapping, String> {
    let record_key = text(payload, "record_key")?;
    let node_id = format!("bookmark:{record_key}");
    let file_sha256 = text(payload, "file_sha256")?;
    let offset = payload
        .get("source_offset")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "BOOKMARK_RECORD_INVALID: `source_offset` must be an integer".to_string())?;
    let mut properties = serde_json::Map::new();
    for (key, value) in [
        ("type", BOOKMARK_CLASS.to_string()),
        ("bookmark_id", text(payload, "bookmark_id")?),
        ("record_key", record_key),
        ("text", text(payload, "text")?),
        ("source_offset", offset.to_string()),
        ("file_sha256", file_sha256.clone()),
        ("export_revision", text(payload, "export_revision")?),
        ("vocabulary_revision", text(payload, "vocabulary_revision")?),
        ("vocabulary_sha256", text(payload, "vocabulary_sha256")?),
        ("source_id", source_id.to_string()),
        ("ingestion_receipt_id", receipt_id.to_string()),
        ("ingestion_receipt_digest", receipt_digest.to_string()),
        ("source_revision", file_sha256),
    ] {
        properties.insert(key.into(), value.into());
    }
    let mut entities = Vec::new();
    let mut edges = Vec::new();
    for (key, class, prefix, _) in ARRAY_KEYS {
        let values = strings(payload, key)?;
        properties.insert(key.into(), json_text(&values)?.into());
        for value in &values {
            entity(
                &mut entities,
                &mut edges,
                &node_id,
                (class, prefix, value, None),
            )?;
        }
    }
    let urls = strings(payload, "urls")?;
    properties.insert("urls".into(), json_text(&urls)?.into());
    let domains: Vec<(String, Option<String>)> = serde_json::from_value(
        payload
            .get("domains")
            .cloned()
            .ok_or_else(|| "BOOKMARK_RECORD_INVALID: `domains` is missing".to_string())?,
    )
    .map_err(|error| format!("BOOKMARK_RECORD_INVALID: `domains`: {error}"))?;
    properties.insert("domains".into(), json_text(&domains)?.into());
    for (domain, label) in &domains {
        entity(
            &mut entities,
            &mut edges,
            &node_id,
            ("Domain", "domain", domain, label.as_deref()),
        )?;
    }
    Ok(BookmarkMapping {
        node_id,
        properties,
        entities,
        edges,
    })
}
