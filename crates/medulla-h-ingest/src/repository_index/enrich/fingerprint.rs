//! Structural fingerprints of the committed index rows (MEDULLA-REPO-INGEST-R603.3).
//!
//! Parsing already ran in the index commit; this module decides which committed files
//! the enrichment run reads. Each file with committed `SYMBOL` rows has one
//! [`StructuralFingerprint`]: the content hash covers the symbol rows' identity and
//! byte spans, the structure hash covers their signatures. A body or whitespace edit
//! therefore changes the content hash and keeps the structure hash (`Cosmetic`), a
//! changed signature set is `Structural` and an identical file is `None`.
//!
//! The fingerprint of a file is stored as its own node, never on the symbol or file
//! rows: the index envelope is the only writer of those.

use std::collections::BTreeMap;

use medulla_enrich::fingerprint::{fingerprint, StructuralFingerprint};
use medulla_enrich::Id;
use medulla_protocol_types::protocol::Method;
use medulla_types::contract::Digest256;

use super::{add_node, Row};

/// The `type` property of a stored file fingerprint node.
pub const FINGERPRINT_NODE_TYPE: &str = "RepositoryFileFingerprint";

/// The digest domain of a fingerprint node id.
const FINGERPRINT_DOMAIN: &[u8] = b"eg/repository-fingerprint/v1";

/// The fingerprint of every file that has committed `SYMBOL` rows, in path order.
pub(super) fn file_fingerprints(rows: &[Row]) -> Vec<StructuralFingerprint> {
    let mut files: BTreeMap<&str, Vec<(&str, &BTreeMap<String, String>)>> = BTreeMap::new();
    for row in rows {
        let Row::Node { id, properties } = row else {
            continue;
        };
        if properties.get("type").map(String::as_str) != Some("SYMBOL") {
            continue;
        }
        let Some(path) = properties.get("file_path").filter(|path| !path.is_empty()) else {
            continue;
        };
        files
            .entry(path.as_str())
            .or_default()
            .push((id.as_str(), properties));
    }
    let mut fingerprints = Vec::with_capacity(files.len());
    for (path, symbols) in files {
        let Ok(path_id) = Id::new(path) else {
            continue;
        };
        fingerprints.push(symbol_fingerprint(path_id, symbols));
    }
    fingerprints
}

/// The fingerprint of one file from its `SYMBOL` rows `(id, properties)`: the
/// one definition of the content and structure hashes, shared by the committed
/// rows above and the changeset re-index (MEDULLA-SCM-R006).
pub(in crate::repository_index) fn symbol_fingerprint(
    path_id: Id,
    mut symbols: Vec<(&str, &BTreeMap<String, String>)>,
) -> StructuralFingerprint {
    let property = |properties: &BTreeMap<String, String>, key: &str| {
        properties.get(key).cloned().unwrap_or_default()
    };
    symbols.sort_by(|left, right| left.0.cmp(right.0));
    let mut content = String::new();
    let mut signatures = Vec::with_capacity(symbols.len());
    for (node_id, properties) in &symbols {
        content.push_str(&format!(
            "{node_id}\t{}\t{}\t{}\n",
            property(properties, "ast_hash"),
            property(properties, "start_byte"),
            property(properties, "end_byte"),
        ));
        signatures.push(format!(
            "{}\t{}\t{}",
            property(properties, "qualified_symbol"),
            property(properties, "symbol_type"),
            property(properties, "kind_detail"),
        ));
    }
    signatures.sort();
    fingerprint(path_id, content.as_bytes(), Ok(&signatures))
}

/// The node id of the stored fingerprint of one file of one repository.
pub fn fingerprint_node_id(repository_id: &str, path: &str) -> Result<String, String> {
    Ok(format!(
        "repository-fingerprint:{}",
        Digest256::framed(
            FINGERPRINT_DOMAIN,
            &[repository_id.as_bytes(), path.as_bytes()],
        )?
        .to_hex()
    ))
}

/// The node write that stores one current fingerprint, committed in the same
/// `repository-enrich:<digest>` write-set so the next run compares against it.
pub fn fingerprint_method(
    repository_id: &str,
    fingerprint: &StructuralFingerprint,
) -> Result<Method, String> {
    add_node(
        fingerprint_node_id(repository_id, fingerprint.path.as_str())?,
        &[
            ("type", FINGERPRINT_NODE_TYPE.to_string()),
            ("repository_id", repository_id.to_string()),
            ("path", fingerprint.path.as_str().to_string()),
            ("content_hash", fingerprint.content_hash.clone()),
            (
                "structure_hash",
                fingerprint.structure_hash.clone().unwrap_or_default(),
            ),
        ],
    )
}

/// The fingerprint a stored node holds, from its decoded properties.
pub fn stored_fingerprint(value: &serde_json::Value) -> Option<StructuralFingerprint> {
    if value.get("type").and_then(serde_json::Value::as_str) != Some(FINGERPRINT_NODE_TYPE) {
        return None;
    }
    let text = |key: &str| value.get(key).and_then(serde_json::Value::as_str);
    let structure = text("structure_hash").filter(|hash| !hash.is_empty());
    Some(StructuralFingerprint {
        path: Id::new(text("path")?).ok()?,
        content_hash: text("content_hash")?.to_string(),
        structure_hash: structure.map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(
        id: &str,
        path: &str,
        name: &str,
        ast_hash: &str,
        span: (u32, u32),
    ) -> Row {
        let properties = [
            ("type", "SYMBOL"),
            ("file_path", path),
            ("qualified_symbol", name),
            ("symbol_type", "Function"),
            ("ast_hash", ast_hash),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .chain([
            ("start_byte".to_string(), span.0.to_string()),
            ("end_byte".to_string(), span.1.to_string()),
        ])
        .collect();
        Row::Node {
            id: id.to_string(),
            properties,
        }
    }

    fn only(rows: &[Row]) -> StructuralFingerprint {
        let mut prints = file_fingerprints(rows);
        assert_eq!(prints.len(), 1);
        prints.remove(0)
    }

    // spec: MEDULLA-REPO-INGEST-R603.3
    #[test]
    fn a_body_edit_is_cosmetic_and_a_signature_edit_is_structural() {
        use medulla_enrich::fingerprint::{classify, ChangeLevel};

        let before = only(&[symbol("s1", "src/a.py", "a.run", "h1", (0, 40))]);
        // A whitespace edit inside the body moves the hash and the byte span only.
        let cosmetic = only(&[symbol("s1", "src/a.py", "a.run", "h2", (0, 44))]);
        assert_ne!(before.content_hash, cosmetic.content_hash);
        assert_eq!(before.structure_hash, cosmetic.structure_hash);
        assert_eq!(classify(Some(&before), &cosmetic).0, ChangeLevel::Cosmetic);
        // The same rows are an identical file.
        let same = only(&[symbol("s1", "src/a.py", "a.run", "h1", (0, 40))]);
        assert_eq!(classify(Some(&before), &same).0, ChangeLevel::None);
        // A new signature changes the structure hash.
        let structural = only(&[
            symbol("s1", "src/a.py", "a.run", "h1", (0, 40)),
            symbol("s2", "src/a.py", "a.stop", "h3", (41, 80)),
        ]);
        assert_ne!(before.structure_hash, structural.structure_hash);
        assert_eq!(
            classify(Some(&before), &structural).0,
            ChangeLevel::Structural
        );
        // A first sighting is structural, and a stored node reads back what was written.
        assert_eq!(classify(None, &before).0, ChangeLevel::Structural);
        assert_eq!(
            file_fingerprints(&[
                symbol("s1", "src/a.py", "a.run", "h1", (0, 40)),
                symbol("s9", "src/b.py", "b.run", "h9", (0, 10)),
            ])
            .len(),
            2
        );
        let method = fingerprint_method("repo-1", &before).unwrap();
        let Method::AddNode {
            node_id,
            properties_msgpack,
        } = method
        else {
            panic!("a fingerprint is one node write");
        };
        assert_eq!(node_id, fingerprint_node_id("repo-1", "src/a.py").unwrap());
        let value: serde_json::Value = rmp_serde::from_slice(&properties_msgpack).unwrap();
        assert_eq!(stored_fingerprint(&value), Some(before));
    }
}
