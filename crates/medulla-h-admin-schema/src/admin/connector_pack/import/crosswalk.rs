//! `crosswalk` pack entries: validated against the pinned vocabulary before
//! anything is written (MEDULLA-TYPED-PACKS-R071.1.3, R071.1.4).

use std::collections::BTreeMap;

use medulla_catalog_types::connector_pack::{
    PackEntry, PackEntryKind, PackViolation, PackViolationCode,
};
use medulla_catalog_types::domain_pack::{
    compute_coverage_of, CrosswalkContext, CrosswalkCoverage, CrosswalkParseError, PinnedHierarchy,
    VocabularyCrosswalk, VocabularyCrosswalkError,
};

use medulla_types::contract::ServerErrorCode;

use super::validation::{section_bytes, violation};
use super::vocabulary::PinnedVocabulary;

/// The scope name refusal messages carry: the selected source set is the
/// engine-owned core catalog.
const SOURCE_SCOPE: &str = "core";

/// Validate every `crosswalk` entry of the pack as one set. A refusal is a
/// violation and nothing is written.
pub fn validate_crosswalk_entries(
    archive: &[u8],
    all: &[&PackEntry],
    vocabulary: &PinnedVocabulary,
    violations: &mut Vec<PackViolation>,
) {
    let mut records: Vec<(&str, VocabularyCrosswalk)> = Vec::new();
    for entry in all.iter().filter(|e| e.kind == PackEntryKind::Crosswalk) {
        let parsed = match section_bytes(archive, &entry.body) {
            Ok(bytes) => parse_record(bytes),
            Err(error) => Err(CrosswalkParseError::Json(error.to_string())),
        };
        match parsed {
            Ok(record) => records.push((entry.uri.as_str(), record)),
            Err(error) => violations.push(violation(
                parse_violation_code(&error),
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
pub fn parse_record(bytes: &[u8]) -> Result<VocabularyCrosswalk, CrosswalkParseError> {
    VocabularyCrosswalk::from_json(bytes)
}

/// The import code of a body that did not parse: a forged record (its declared digest is not
/// the computed one) is `CrosswalkEvidenceInvalid`, every other fault `MalformedBody`
/// (MEDULLA-TYPED-PACKS-R097.2.4).
pub fn parse_violation_code(error: &CrosswalkParseError) -> PackViolationCode {
    match error {
        CrosswalkParseError::Invalid(VocabularyCrosswalkError::ContentDigestMismatch {
            ..
        }) => PackViolationCode::CrosswalkEvidenceInvalid,
        _ => PackViolationCode::MalformedBody,
    }
}

/// The registered server code of a body that did not parse, as the activation path answers it.
pub fn parse_server_code(error: &CrosswalkParseError) -> ServerErrorCode {
    match error {
        CrosswalkParseError::Invalid(VocabularyCrosswalkError::ContentDigestMismatch {
            ..
        }) => ServerErrorCode::CrosswalkEvidenceInvalid,
        _ => ServerErrorCode::MalformedBody,
    }
}

/// A refused crosswalk set: the pack violation code of the import, the
/// registered server code of an activation, the offending IRI and the text.
#[derive(Debug)]
pub struct CrosswalkRefusal {
    pub pack: PackViolationCode,
    pub server: ServerErrorCode,
    pub iri: Option<String>,
    pub message: String,
}

impl CrosswalkRefusal {
    /// The refusal as an activation error string: `<CODE>: <message>`.
    pub fn activation_error(&self) -> String {
        format!("{}: {}", self.server.as_str(), self.message)
    }
}

/// Validate a crosswalk set against the pinned vocabulary and compute the
/// coverage of the pinned revision (MEDULLA-TYPED-PACKS-R097.2.3). Used by the
/// import before the pack head commits and again by the pack activation, so both
/// refuse the same records and report the same coverage.
pub fn coverage_of_set(
    set: &[VocabularyCrosswalk],
    vocabulary: &PinnedVocabulary,
) -> Result<CrosswalkCoverage, CrosswalkRefusal> {
    let context = context_of(set, vocabulary);
    compute_coverage_of(set, &context).map_err(|error| {
        let (pack, server, iri) = classify(&error);
        CrosswalkRefusal {
            pack,
            server,
            iri: iri.map(str::to_string),
            message: error.to_string(),
        }
    })
}

/// Validate a crosswalk set against the pinned vocabulary; the coverage it
/// computes on the way is [`coverage_of_set`]'s.
pub fn validate_set(
    set: &[VocabularyCrosswalk],
    vocabulary: &PinnedVocabulary,
) -> Result<(), CrosswalkRefusal> {
    coverage_of_set(set, vocabulary).map(drop)
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
        E::RevisionMismatch { source_iri, .. } => (
            PackViolationCode::CrosswalkRevisionMismatch,
            ServerErrorCode::CrosswalkRevisionMismatch,
            Some(source_iri),
        ),
        E::ContentDigestMismatch { .. } => (
            PackViolationCode::CrosswalkEvidenceInvalid,
            ServerErrorCode::CrosswalkEvidenceInvalid,
            None,
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
