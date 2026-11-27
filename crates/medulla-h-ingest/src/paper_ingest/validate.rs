//! Identity and receipt checks of one paper record (MEDULLA-MEDIA-VOCAB-R002.5.1).

use medulla_types::contract::Digest256;
use medulla_types::paper_ingest::{
    PAPER_ARTIFACT_UNAVAILABLE, PAPER_IDENTITY_INVALID, PAPER_RECEIPT_MISMATCH,
};
use medulla_types::refusal_code::RefusalCode;
use serde_json::Value;

fn blank(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_none_or(|text| text.trim().is_empty())
}

/// `id` is the lowercase hex SHA-256 of the compact JSON `[canonical_id, revision]`, `null` for an
/// absent revision, non-ASCII text unescaped.
fn identity_digest(canonical_id: &str, revision: Option<&str>) -> Result<String, RefusalCode> {
    let text = serde_json::to_string(&serde_json::json!([canonical_id, revision]))
        .map_err(|_| PAPER_IDENTITY_INVALID)?;
    Ok(Digest256::sha256(text.as_bytes()).to_hex())
}

fn check_identity(payload: &Value) -> Result<(), RefusalCode> {
    let canonical_id = payload
        .get("canonical_id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or(PAPER_IDENTITY_INVALID)?;
    let references = payload
        .get("references")
        .and_then(Value::as_array)
        .filter(|references| !references.is_empty())
        .ok_or(PAPER_IDENTITY_INVALID)?;
    if references
        .iter()
        .any(|reference| blank(reference.get("provider_id")) || blank(reference.get("record_id")))
    {
        return Err(PAPER_IDENTITY_INVALID);
    }
    let revision = match payload.get("revision") {
        None | Some(Value::Null) => None,
        Some(Value::String(revision)) => Some(revision.as_str()),
        Some(_) => return Err(PAPER_IDENTITY_INVALID),
    };
    match payload.get("id").and_then(Value::as_str) {
        Some(id) if id == identity_digest(canonical_id, revision)? => Ok(()),
        _ => Err(PAPER_IDENTITY_INVALID),
    }
}

fn lowercase_sha256(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn check_receipt(payload: &Value, receipt: &Value, tenant: &str) -> Result<(), RefusalCode> {
    let text = |key: &str| receipt.get(key).and_then(Value::as_str);
    let revision = |value: Option<&Value>| match value {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(revision)) => Some(Some(revision.clone())),
        Some(_) => None,
    };
    let matches = text("tenant") == Some(tenant)
        && text("paper_id") == payload.get("canonical_id").and_then(Value::as_str)
        && revision(receipt.get("revision")).is_some()
        && revision(receipt.get("revision")) == revision(payload.get("revision"))
        && text("sha256").is_some_and(lowercase_sha256)
        && receipt
            .get("byte_count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 1)
        && matches!(text("media_type"), Some("application/pdf" | "text/html"));
    if matches {
        Ok(())
    } else {
        Err(PAPER_RECEIPT_MISMATCH)
    }
}

/// A relative path: no leading `/`, and no empty, `.` or `..` segment.
fn relative_path(path: &str) -> bool {
    !path.starts_with('/')
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn check_artifact(payload: &Value, receipt: Option<&Value>) -> Result<(), RefusalCode> {
    if let Some(receipt) = receipt {
        match receipt.get("artifact_ref").and_then(Value::as_str) {
            Some(path) if relative_path(path) => {}
            _ => return Err(PAPER_ARTIFACT_UNAVAILABLE),
        }
    }
    let receipt_sha = receipt
        .and_then(|receipt| receipt.get("sha256"))
        .and_then(Value::as_str);
    let cited = payload
        .get("field_provenance")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|fields| fields.values())
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|entry| entry.get("document_sha256").and_then(Value::as_str));
    for document_sha in cited {
        if receipt_sha != Some(document_sha) {
            return Err(PAPER_ARTIFACT_UNAVAILABLE);
        }
    }
    Ok(())
}

/// Check one paper payload against the verified `tenant`, in order: identity, receipt, artifact.
/// The first failing check refuses the paper; nothing is written here.
pub(crate) fn validate_paper(payload: &Value, tenant: &str) -> Result<(), RefusalCode> {
    check_identity(payload)?;
    let receipt = payload
        .get("document_receipt")
        .filter(|value| !value.is_null());
    if let Some(receipt) = receipt {
        check_receipt(payload, receipt, tenant)?;
    }
    check_artifact(payload, receipt)
}
