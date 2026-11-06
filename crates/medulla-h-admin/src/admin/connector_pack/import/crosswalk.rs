//! `crosswalk` pack entries: validated against the pinned vocabulary before
//! anything is written (MEDULLA-TYPED-PACKS-R071.1.3, R071.1.4).

use std::collections::BTreeMap;

use medulla_catalog_types::connector_pack::{
    PackEntry, PackEntryKind, PackViolation, PackViolationCode,
};
use medulla_catalog_types::domain_pack::{
    validate_crosswalks, CrosswalkContext, PinnedHierarchy, VocabularyCrosswalk,
    VocabularyCrosswalkError,
};

use super::validation::{section_bytes, violation};
use super::vocabulary::PinnedVocabulary;

/// The scope name refusal messages carry: the selected source set is the
/// engine-owned core catalog.
const SOURCE_SCOPE: &str = "core";

/// Validate every `crosswalk` entry of the pack as one set. A refusal is a
/// violation and nothing is written.
pub(super) fn validate_crosswalk_entries(
    archive: &[u8],
    all: &[&PackEntry],
    vocabulary: &PinnedVocabulary,
    violations: &mut Vec<PackViolation>,
) {
    let mut records: Vec<(&str, VocabularyCrosswalk)> = Vec::new();
    for entry in all.iter().filter(|e| e.kind == PackEntryKind::Crosswalk) {
        let parsed = section_bytes(archive, &entry.body)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<VocabularyCrosswalk>(bytes)
                    .map_err(|error| error.to_string())
            });
        match parsed {
            Ok(record) => records.push((entry.uri.as_str(), record)),
            Err(error) => violations.push(violation(
                PackViolationCode::MalformedBody,
                Some(&entry.uri),
                &format!("crosswalk record is invalid: {error}"),
            )),
        }
    }
    if records.is_empty() {
        return;
    }
    let set: Vec<VocabularyCrosswalk> = records.iter().map(|(_, record)| record.clone()).collect();
    let context = context_of(&set, vocabulary);
    if let Err(error) = validate_crosswalks(&set, &context) {
        let (code, iri) = classify(&error);
        let uri = iri.and_then(|iri| {
            records
                .iter()
                .find(|(_, r)| r.source_class_iri == iri || r.target_class_iri == iri)
                .map(|(uri, _)| *uri)
        });
        violations.push(violation(code, uri, &error.to_string()));
    }
}

/// The pinned context: revision, class categories, selected sources and the
/// hierarchy between every mapped pair, all read from the composed schema.
fn context_of(set: &[VocabularyCrosswalk], vocabulary: &PinnedVocabulary) -> CrosswalkContext {
    let mut hierarchy = BTreeMap::new();
    for record in set {
        let (source, target) = (&record.source_class_iri, &record.target_class_iri);
        if vocabulary.ancestors(source).contains(target) {
            hierarchy.insert(
                (source.clone(), target.clone()),
                PinnedHierarchy::TargetBroader,
            );
        } else if vocabulary.ancestors(target).contains(source) {
            hierarchy.insert(
                (source.clone(), target.clone()),
                PinnedHierarchy::TargetNarrower,
            );
        }
    }
    CrosswalkContext {
        source_revision: vocabulary.revision.clone(),
        scope: SOURCE_SCOPE.to_string(),
        source_classes: vocabulary.categories.clone(),
        selected_sources: vocabulary.core_classes.clone(),
        hierarchy,
    }
}

fn classify(error: &VocabularyCrosswalkError) -> (PackViolationCode, Option<&str>) {
    use VocabularyCrosswalkError as E;
    match error {
        E::SourceUnknown { source_iri, .. } => {
            (PackViolationCode::CrosswalkSourceUnknown, Some(source_iri))
        }
        E::SourceOutOfScope { source_iri, .. } => (
            PackViolationCode::CrosswalkSourceOutOfScope,
            Some(source_iri),
        ),
        E::TargetUnknown(iri) => (PackViolationCode::CrosswalkTargetUnknown, Some(iri)),
        E::Conflict { source_iri, .. } => (PackViolationCode::CrosswalkConflict, Some(source_iri)),
        E::CategoryConflict { target_iri, .. } => (
            PackViolationCode::CrosswalkCategoryConflict,
            Some(target_iri),
        ),
        E::LimitExceeded { iri, .. } => (PackViolationCode::CrosswalkLimitExceeded, Some(iri)),
        _ => (PackViolationCode::MalformedBody, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_refusal_has_its_registered_code() {
        let unknown = VocabularyCrosswalkError::SourceUnknown {
            source_iri: "urn:a".into(),
            revision: "r".into(),
            scope: "core".into(),
        };
        assert_eq!(
            classify(&unknown),
            (PackViolationCode::CrosswalkSourceUnknown, Some("urn:a"))
        );
        let limit = VocabularyCrosswalkError::LimitExceeded {
            bound: "mappings",
            iri: "urn:b".into(),
        };
        assert_eq!(
            classify(&limit).0,
            PackViolationCode::CrosswalkLimitExceeded
        );
        assert_eq!(
            classify(&VocabularyCrosswalkError::EmptyOrigin).0,
            PackViolationCode::MalformedBody
        );
    }
}
