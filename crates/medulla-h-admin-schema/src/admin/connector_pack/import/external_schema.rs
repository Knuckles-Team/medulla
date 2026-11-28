//! `external_schema` pack entries (MEDULLA-TYPED-PACKS-R097.2.7.3).
//!
//! Each entry pins one external schema with its terms and the mappings the connector
//! declares. The import admits the body, builds its coverage with
//! `compute_external_coverage` and reports the unmapped terms as warnings; a refusal is a
//! violation and the pack head and active schema stay as they were. Unmapped terms are
//! never added as platform terms.

use medulla_catalog_types::connector_pack::{
    ConnectorPackImportRequest, PackEntry, PackEntryKind, PackViolation, PackViolationCode,
    PackWarning, PackWarningCode,
};
use {
    medulla_catalog_source_types::external_schema::admit_external_schema,
    medulla_catalog_source_types::external_schema::ExternalSchemaRefusal,
    medulla_catalog_source_types::external_schema::ExternalTermResult,
};

use super::vocabulary::PinnedVocabulary;

use super::validation::{section_bytes, violation};

/// The most unmapped IRIs one warning names.
const WARNED_IRIS: usize = 20;

/// The pack violation a refusal is reported as.
fn violation_code(refusal: ExternalSchemaRefusal) -> PackViolationCode {
    match refusal {
        ExternalSchemaRefusal::Malformed => PackViolationCode::MalformedBody,
        ExternalSchemaRefusal::KindUnsupported => PackViolationCode::ExternalKindUnsupported,
        ExternalSchemaRefusal::TermDuplicate => PackViolationCode::ExternalTermDuplicate,
        ExternalSchemaRefusal::KindMismatch => PackViolationCode::ExternalKindMismatch,
        ExternalSchemaRefusal::MappingStale => PackViolationCode::ExternalMappingStale,
        ExternalSchemaRefusal::SourceOutOfScope => PackViolationCode::ExternalSourceOutOfScope,
        ExternalSchemaRefusal::ReportForged => PackViolationCode::ExternalReportForged,
    }
}

/// Admit every `external_schema` entry of the pack and report its unmapped terms.
///
/// The importer stamps the three pins and never reads them from the body:
/// `pack_revision` is the pack digest, `schema_revision` the composed digest of core
/// plus this pack's ontology and shapes, and `platform_vocabulary_revision` the core
/// composed digest the crosswalks pin.
pub fn validate_external_schemas(
    request: &ConnectorPackImportRequest,
    archive: &[u8],
    all: &[&PackEntry],
    vocabulary: &PinnedVocabulary,
    violations: &mut Vec<PackViolation>,
    warnings: &mut Vec<PackWarning>,
) {
    let pack_revision = request.index.pack_digest.to_hex();
    let schema_revision = vocabulary.composed_revision.as_str();
    let vocabulary_revision = vocabulary.revision.as_str();
    for entry in all
        .iter()
        .filter(|entry| entry.kind == PackEntryKind::ExternalSchema)
    {
        let value = section_bytes(archive, &entry.body).and_then(|bytes| {
            serde_json::from_slice::<serde_json::Value>(bytes).map_err(|e| e.to_string())
        });
        let value = match value {
            Ok(value) => value,
            Err(error) => {
                violations.push(violation(
                    PackViolationCode::MalformedBody,
                    Some(&entry.uri),
                    &format!("external schema body is invalid: {error}"),
                ));
                continue;
            }
        };
        match admit_external_schema(&value, &pack_revision, schema_revision, vocabulary_revision) {
            Ok((_, coverage)) => {
                let unmapped: Vec<&str> = coverage
                    .terms
                    .iter()
                    .filter(|term| matches!(term.result, ExternalTermResult::Unmapped { .. }))
                    .map(|term| term.external_iri.as_str())
                    .collect();
                if !unmapped.is_empty() {
                    let shown: Vec<&str> = unmapped.iter().copied().take(WARNED_IRIS).collect();
                    warnings.push(PackWarning {
                        code: PackWarningCode::ExternalTermUnmapped,
                        uri: Some(entry.uri.clone()),
                        detail: format!(
                            "{} external terms have no declared mapping: {}",
                            unmapped.len(),
                            shown.join(", ")
                        ),
                    });
                }
            }
            Err((refusal, detail)) => {
                violations.push(violation(
                    violation_code(refusal),
                    Some(&entry.uri),
                    &detail,
                ));
            }
        }
    }
}
