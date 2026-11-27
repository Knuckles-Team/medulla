//! Mapping and provenance of one paper record (MEDULLA-MEDIA-VOCAB-R002.5.2).

use std::collections::{BTreeMap, BTreeSet};

use medulla_types::contract::Digest256;
use medulla_types::paper_ingest::PAPER_FIELD_PROVENANCE_MISSING;
use medulla_types::refusal_code::RefusalCode;
use serde_json::{Map, Value};

use super::{cites, paper_node_id, PaperMapping};

/// The paper fields whose populated value needs a non-empty provenance entry.
const CITED_FIELDS: [&str; 4] = ["title", "authors", "abstract", "publication_date"];

fn compact<T: serde::Serialize>(value: &T) -> String {
    // A serde_json value always serializes.
    serde_json::to_string(value).unwrap_or_default()
}

fn populated(payload: &Value, field: &str) -> bool {
    match payload.get(field) {
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Null) | None => false,
        Some(_) => true,
    }
}

/// `^10\.\d{4,9}/\S+$`.
fn is_doi(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("10.") else {
        return false;
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let tail = &rest[digits..];
    (4..=9).contains(&digits)
        && tail.len() > 1
        && tail.starts_with('/')
        && !tail[1..].chars().any(char::is_whitespace)
}

/// `^\d{4}\.\d{4,5}(v\d+)?$`.
fn is_arxiv_id(text: &str) -> bool {
    let Some((year, rest)) = text.split_once('.') else {
        return false;
    };
    let (number, version) = match rest.split_once('v') {
        Some((number, version)) => (number, Some(version)),
        None => (rest, None),
    };
    year.len() == 4
        && year.bytes().all(|byte| byte.is_ascii_digit())
        && (4..=5).contains(&number.len())
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && version.is_none_or(|v| !v.is_empty() && v.bytes().all(|byte| byte.is_ascii_digit()))
}

fn doi_of(references: &[Value]) -> Option<String> {
    references.iter().find_map(|reference| {
        let record_id = reference.get("record_id").and_then(Value::as_str);
        let original = reference
            .get("original_id")
            .and_then(Value::as_str)
            .map(|id| {
                ["https://doi.org/", "http://doi.org/", "doi:"]
                    .iter()
                    .find_map(|prefix| id.strip_prefix(prefix))
                    .unwrap_or(id)
            });
        [record_id, original]
            .into_iter()
            .flatten()
            .find(|candidate| is_doi(candidate))
            .map(str::to_lowercase)
    })
}

fn arxiv_of(references: &[Value]) -> Option<String> {
    let record_id = |reference: &Value| {
        reference
            .get("record_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    if let Some(id) = references
        .iter()
        .find(|reference| reference.get("provider_id").and_then(Value::as_str) == Some("arxiv"))
        .and_then(record_id)
    {
        return Some(id.strip_prefix("arXiv:").map(str::to_owned).unwrap_or(id));
    }
    references
        .iter()
        .filter_map(record_id)
        .find(|id| is_arxiv_id(id))
}

/// The compact JSON of the receipt with its keys sorted, digested.
fn receipt_digest(receipt: &Value) -> String {
    let sorted: BTreeMap<&String, &Value> = receipt
        .as_object()
        .map(|object| object.iter().collect())
        .unwrap_or_default();
    Digest256::sha256(compact(&sorted).as_bytes()).to_hex()
}

/// One `{field, provider_ref_or_document_digest, extraction_location}` per provenance element,
/// fields in name order, elements in declared order.
fn provenance(payload: &Value) -> Vec<Value> {
    let Some(fields) = payload.get("field_provenance").and_then(Value::as_object) else {
        return Vec::new();
    };
    let ordered: BTreeMap<&String, &Value> = fields.iter().collect();
    let text = |entry: &Value, key: &str| {
        entry
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    ordered
        .into_iter()
        .flat_map(|(field, entries)| {
            entries
                .as_array()
                .into_iter()
                .flatten()
                .map(move |entry| (field, entry))
        })
        .map(|(field, entry)| {
            let reference = match entry.get("document_sha256").and_then(Value::as_str) {
                Some(sha) => format!("sha256:{sha}"),
                None => format!(
                    "{}:{}",
                    text(entry, "provider_id"),
                    text(entry, "source_id")
                ),
            };
            serde_json::json!({
                "field": field,
                "provider_ref_or_document_digest": reference,
                "extraction_location": text(entry, "location"),
            })
        })
        .collect()
}

/// Map a validated paper payload to its node, citation edges and stubs. A populated `title`,
/// `authors`, `abstract` or `publication_date` with no non-empty provenance entry refuses with
/// `PAPER_FIELD_PROVENANCE_MISSING` before anything is built.
pub(crate) fn map_paper(
    payload: &Value,
    known: &BTreeSet<String>,
) -> Result<PaperMapping, RefusalCode> {
    let cited = payload.get("field_provenance").and_then(Value::as_object);
    for field in CITED_FIELDS {
        let has_entry = cited
            .and_then(|fields| fields.get(field))
            .and_then(Value::as_array)
            .is_some_and(|entries| !entries.is_empty());
        if populated(payload, field) && !has_entry {
            return Err(PAPER_FIELD_PROVENANCE_MISSING);
        }
    }
    let text = |key: &str| payload.get(key).and_then(Value::as_str).map(str::to_owned);
    let canonical_id = text("canonical_id").unwrap_or_default();
    let node_id = paper_node_id(&canonical_id);
    let references: Vec<Value> = payload
        .get("references")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut properties = Map::new();
    properties.insert("type".into(), "ResearchPaper".into());
    properties.insert("canonical_id".into(), canonical_id.into());
    for (key, value) in [
        ("revision", text("revision")),
        ("title", text("title")),
        ("abstract", text("abstract")),
        ("publishedAt", text("publication_date")),
        ("doi", doi_of(&references)),
        ("arxivId", arxiv_of(&references)),
    ] {
        if let Some(value) = value {
            properties.insert(key.into(), value.into());
        }
    }
    if let Some(authors) = payload.get("authors").filter(|authors| !authors.is_null()) {
        properties.insert("authors".into(), compact(authors).into());
    }
    if let Some(receipt) = payload
        .get("document_receipt")
        .filter(|value| !value.is_null())
    {
        properties.insert("receipt_digest".into(), receipt_digest(receipt).into());
        if let Some(sha) = receipt.get("sha256").and_then(Value::as_str) {
            properties.insert("document_digest".into(), sha.into());
        }
    }
    properties.insert("provenance".into(), compact(&provenance(payload)).into());
    let (stubs, edges) = cites::write_citations(&node_id, payload, known);
    Ok(PaperMapping {
        node_id,
        properties,
        stubs,
        edges,
    })
}
