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

use medulla_types::contract::ServerErrorCode;

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
            .and_then(parse_record);
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
    if let Err(refusal) = validate_set(&set, vocabulary) {
        let uri = refusal.iri.as_deref().and_then(|iri| {
            records
                .iter()
                .find(|(_, r)| r.source_class_iri == iri || r.target_class_iri == iri)
                .map(|(uri, _)| *uri)
        });
        violations.push(violation(refusal.pack, uri, &refusal.message));
    }
}

/// One crosswalk body: the JSON of a `VocabularyCrosswalk`.
pub(crate) fn parse_record(bytes: &[u8]) -> Result<VocabularyCrosswalk, String> {
    serde_json::from_slice::<VocabularyCrosswalk>(bytes).map_err(|error| error.to_string())
}

/// A refused crosswalk set: the pack violation code of the import, the
/// registered server code of an activation, the offending IRI and the text.
#[derive(Debug)]
pub(crate) struct CrosswalkRefusal {
    pub(crate) pack: PackViolationCode,
    pub(crate) server: ServerErrorCode,
    pub(crate) iri: Option<String>,
    pub(crate) message: String,
}

impl CrosswalkRefusal {
    /// The refusal as an activation error string: `<CODE>: <message>`.
    pub(crate) fn activation_error(&self) -> String {
        format!("{}: {}", self.server.as_str(), self.message)
    }
}

/// Validate a crosswalk set against the pinned vocabulary. Used by the import
/// and again by the pack activation, so both refuse the same records.
pub(crate) fn validate_set(
    set: &[VocabularyCrosswalk],
    vocabulary: &PinnedVocabulary,
) -> Result<(), CrosswalkRefusal> {
    let context = context_of(set, vocabulary);
    validate_crosswalks(set, &context).map_err(|error| {
        let (pack, server, iri) = classify(&error);
        CrosswalkRefusal {
            pack,
            server,
            iri: iri.map(str::to_string),
            message: error.to_string(),
        }
    })
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

fn classify(
    error: &VocabularyCrosswalkError,
) -> (PackViolationCode, ServerErrorCode, Option<&str>) {
    use VocabularyCrosswalkError as E;
    match error {
        E::SourceUnknown { source_iri, .. } => (
            PackViolationCode::CrosswalkSourceUnknown,
            ServerErrorCode::CrosswalkSourceUnknown,
            Some(source_iri),
        ),
        E::SourceOutOfScope { source_iri, .. } => (
            PackViolationCode::CrosswalkSourceOutOfScope,
            ServerErrorCode::CrosswalkSourceOutOfScope,
            Some(source_iri),
        ),
        E::TargetUnknown(iri) => (
            PackViolationCode::CrosswalkTargetUnknown,
            ServerErrorCode::CrosswalkTargetUnknown,
            Some(iri),
        ),
        E::Conflict { source_iri, .. } => (
            PackViolationCode::CrosswalkConflict,
            ServerErrorCode::CrosswalkConflict,
            Some(source_iri),
        ),
        E::CategoryConflict { target_iri, .. } => (
            PackViolationCode::CrosswalkCategoryConflict,
            ServerErrorCode::CrosswalkCategoryConflict,
            Some(target_iri),
        ),
        E::LimitExceeded { iri, .. } => (
            PackViolationCode::CrosswalkLimitExceeded,
            ServerErrorCode::CrosswalkLimitExceeded,
            Some(iri),
        ),
        _ => (
            PackViolationCode::MalformedBody,
            ServerErrorCode::MalformedBody,
            None,
        ),
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
            (
                PackViolationCode::CrosswalkSourceUnknown,
                ServerErrorCode::CrosswalkSourceUnknown,
                Some("urn:a")
            )
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
