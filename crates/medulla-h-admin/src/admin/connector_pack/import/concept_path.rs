//! Canonical hierarchical concept paths (MEDULLA-TYPED-PACKS-R071.100).
//!
//! Producers submit proposed paths as `concept_path` entries; this validator
//! owns canonical identity. Grammar: segments join with `/`; `%2F` escapes a
//! literal slash and `%25` a literal percent; each segment is NFC-normalized
//! and case-sensitive, has no leading or trailing whitespace and is not empty.
//! Existing published IRIs never change: a record only names one.

use std::collections::BTreeMap;

use medulla_catalog_types::connector_pack::concept_path::{
    ConceptPathRecord, CONCEPT_PATH_SCHEMA_VERSION, MAX_CONCEPT_PATH_ALIASES,
    MAX_CONCEPT_PATH_BYTES,
};
use medulla_catalog_types::connector_pack::{
    PackEntry, PackEntryKind, PackViolation, PackViolationCode,
};
use unicode_normalization::UnicodeNormalization;

use super::validation::{section_bytes, violation};
use super::vocabulary::PinnedVocabulary;

/// Parse a path into its decoded, NFC-normalized segments.
pub(super) fn parse_path(path: &str) -> Result<Vec<String>, &'static str> {
    if path.is_empty() || path.len() > MAX_CONCEPT_PATH_BYTES {
        return Err("path is empty or exceeds its byte bound");
    }
    path.split('/').map(decode_segment).collect()
}

fn decode_segment(raw: &str) -> Result<String, &'static str> {
    let mut decoded = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('%') {
        decoded.push_str(&rest[..at]);
        let escape = rest.get(at..at + 3).ok_or("dangling percent escape")?;
        match escape {
            "%2F" => decoded.push('/'),
            "%25" => decoded.push('%'),
            _ => return Err("only %2F and %25 are valid escapes"),
        }
        rest = &rest[at + 3..];
    }
    decoded.push_str(rest);
    let segment: String = decoded.nfc().collect();
    if segment.is_empty() {
        return Err("empty segment");
    }
    if segment.trim() != segment {
        return Err("segment has leading or trailing whitespace");
    }
    Ok(segment)
}

/// The canonical text of decoded segments: the identity key of a path.
pub(super) fn canonical(segments: &[String]) -> String {
    segments
        .iter()
        .map(|segment| segment.replace('%', "%25").replace('/', "%2F"))
        .collect::<Vec<_>>()
        .join("/")
}

struct Parsed<'a> {
    uri: &'a str,
    record: ConceptPathRecord,
    segments: Vec<String>,
    key: String,
    alias_keys: Vec<String>,
}

/// Validate every `concept_path` entry of the pack against the pinned
/// vocabulary revision. Every refusal names the conflicting ids.
pub(super) fn validate_concept_paths(
    archive: &[u8],
    all: &[&PackEntry],
    vocabulary: &PinnedVocabulary,
    violations: &mut Vec<PackViolation>,
) {
    let mut parsed = Vec::new();
    for entry in all.iter().filter(|e| e.kind == PackEntryKind::ConceptPath) {
        match parse_entry(archive, entry, vocabulary) {
            Ok(item) => parsed.push(item),
            Err((code, detail)) => violations.push(violation(code, Some(&entry.uri), &detail)),
        }
    }
    let mut owners: BTreeMap<&str, &str> = BTreeMap::new();
    let mut classes: BTreeMap<&str, &Parsed<'_>> = BTreeMap::new();
    for item in &parsed {
        for key in std::iter::once(&item.key).chain(item.alias_keys.iter()) {
            if let Some(first) = owners.insert(key.as_str(), item.uri) {
                if first != item.uri || key != &item.key {
                    violations.push(violation(
                        PackViolationCode::ConceptPathDuplicateAlias,
                        Some(item.uri),
                        &format!("path {key} is already claimed by {first}"),
                    ));
                }
            }
        }
        if let Some(first) = classes.insert(item.record.class_iri.as_str(), item) {
            violations.push(violation(
                PackViolationCode::ConceptPathIriConflict,
                Some(item.uri),
                &format!(
                    "class {} already has canonical path {} in {}",
                    item.record.class_iri, first.key, first.uri
                ),
            ));
        }
    }
    let by_key: BTreeMap<&str, &Parsed<'_>> = parsed
        .iter()
        .flat_map(|item| {
            std::iter::once(item.key.as_str())
                .chain(item.alias_keys.iter().map(String::as_str))
                .map(move |key| (key, item))
        })
        .collect();
    for item in parsed.iter().filter(|item| item.segments.len() > 1) {
        let parent_key = canonical(&item.segments[..item.segments.len() - 1]);
        let parent_ok = by_key.get(parent_key.as_str()).is_some_and(|parent| {
            vocabulary
                .parents
                .get(&item.record.class_iri)
                .is_some_and(|direct| direct.contains(&parent.record.class_iri))
        });
        if !parent_ok {
            violations.push(violation(
                PackViolationCode::ConceptPathParentConflict,
                Some(item.uri),
                &format!(
                    "parent path {parent_key} does not resolve to a direct rdfs:subClassOf parent of {}",
                    item.record.class_iri
                ),
            ));
        }
    }
}

fn parse_entry<'a>(
    archive: &[u8],
    entry: &'a PackEntry,
    vocabulary: &PinnedVocabulary,
) -> Result<Parsed<'a>, (PackViolationCode, String)> {
    let malformed = |detail: String| (PackViolationCode::ConceptPathMalformed, detail);
    let record: ConceptPathRecord = section_bytes(archive, &entry.body)
        .map_err(|error| (PackViolationCode::MalformedBody, error.to_string()))
        .and_then(|bytes| {
            serde_json::from_slice(bytes).map_err(|error| {
                (
                    PackViolationCode::MalformedBody,
                    format!("concept path record is invalid: {error}"),
                )
            })
        })?;
    if record.schema_version != CONCEPT_PATH_SCHEMA_VERSION {
        return Err(malformed(format!(
            "schema_version {} is not {CONCEPT_PATH_SCHEMA_VERSION}",
            record.schema_version
        )));
    }
    if record.aliases.len() > MAX_CONCEPT_PATH_ALIASES {
        return Err(malformed("too many aliases".to_string()));
    }
    let segments = parse_path(&record.path)
        .map_err(|why| malformed(format!("path {:?}: {why}", record.path)))?;
    let mut alias_keys = Vec::with_capacity(record.aliases.len());
    for alias in &record.aliases {
        let alias_segments =
            parse_path(alias).map_err(|why| malformed(format!("alias {alias:?}: {why}")))?;
        alias_keys.push(canonical(&alias_segments));
    }
    if record.vocabulary_revision != vocabulary.revision {
        return Err((
            PackViolationCode::ConceptPathStaleRevision,
            format!(
                "revision {} is not the pinned composed schema digest {}",
                record.vocabulary_revision, vocabulary.revision
            ),
        ));
    }
    if !vocabulary.categories.contains_key(&record.class_iri) {
        return Err((
            PackViolationCode::ConceptPathIriConflict,
            format!(
                "class {} is not a class of the pinned vocabulary",
                record.class_iri
            ),
        ));
    }
    Ok(Parsed {
        uri: &entry.uri,
        key: canonical(&segments),
        segments,
        alias_keys,
        record,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_decodes_escapes_and_refuses_malformed_segments() {
        assert_eq!(
            parse_path("a/b%2Fc/100%25").unwrap(),
            vec!["a", "b/c", "100%"]
        );
        assert_eq!(canonical(&parse_path("a/b%2Fc").unwrap()), "a/b%2Fc");
        for bad in [
            "", "a//b", "/a", "a/", "a/ b", "a /b", "a/%41", "a/%2", "a/b%",
        ] {
            assert!(parse_path(bad).is_err(), "{bad:?} must refuse");
        }
    }

    #[test]
    fn segments_are_nfc_and_case_sensitive() {
        let composed = parse_path("Zo\u{eb}").unwrap();
        let decomposed = parse_path("Zoe\u{308}").unwrap();
        assert_eq!(composed, decomposed);
        assert_ne!(parse_path("Person").unwrap(), parse_path("person").unwrap());
    }
}
