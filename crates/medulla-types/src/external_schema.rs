//! External ontology term mappings and their coverage record
//! (MEDULLA-TYPED-PACKS-R097.2.7.1).
//!
//! A connector supplies candidate mappings from an external schema's terms to
//! platform terms; the platform validates them and records the coverage. The
//! records here are the typed model only: the function that builds a coverage
//! record from a pack import is `compute_external_coverage` (R097.2.7.2).

use serde::{Deserialize, Serialize};

use crate::contract::Digest256;

/// The one record layout this build reads and writes.
pub const EXTERNAL_SCHEMA_VERSION: u16 = 1;

/// The closed set of term kinds. An enumeration is its class plus its named
/// individuals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ExternalTermKind {
    Class,
    ObjectProperty,
    DatatypeProperty,
    AnnotationProperty,
    NamedIndividual,
}

/// Why an external term has no platform term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ExternalUnmappedReason {
    /// No declared mapping covers the term; none is inferred.
    NoDeclaredMapping,
}

/// One declared mapping from an external term to a platform term.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ExternalTermMapping {
    pub schema_version: u16,
    pub source_schema_digest: Digest256,
    pub source_revision: String,
    pub external_iri: String,
    pub external_kind: ExternalTermKind,
    pub platform_iri: String,
    pub platform_kind: ExternalTermKind,
    pub mapping_revision: String,
}

/// The coverage result of one external term.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ExternalTermResult {
    Mapped {
        platform_iri: String,
        provenance_digest: Digest256,
    },
    Unmapped {
        reason: ExternalUnmappedReason,
    },
}

/// One external term and its coverage result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ExternalTerm {
    pub external_iri: String,
    pub kind: ExternalTermKind,
    pub result: ExternalTermResult,
}

/// The coverage of one external schema by one pack revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ExternalOntologyCoverage {
    pub schema_version: u16,
    pub source_schema_digest: Digest256,
    pub source_revision: String,
    pub mapping_revision: String,
    pub pack_revision: String,
    pub schema_revision: String,
    pub platform_vocabulary_revision: String,
    /// Sorted by `external_iri`, one entry per IRI.
    pub terms: Vec<ExternalTerm>,
    pub coverage_digest: Digest256,
}

/// A malformed external mapping or coverage record.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExternalSchemaError {
    #[error("external schema record schema_version {0} is not supported (expected {EXTERNAL_SCHEMA_VERSION})")]
    UnsupportedSchemaVersion(u16),
    #[error("external schema field {0} is empty")]
    EmptyField(&'static str),
    #[error("external coverage lists the IRI {0:?} more than once")]
    DuplicateIri(String),
    #[error("external coverage terms are not sorted by IRI at {0:?}")]
    TermsNotSorted(String),
    #[error("external coverage digest {declared} does not match the computed digest {computed}")]
    CoverageDigestMismatch { declared: String, computed: String },
}

fn require(value: &str, field: &'static str) -> Result<(), ExternalSchemaError> {
    if value.is_empty() {
        return Err(ExternalSchemaError::EmptyField(field));
    }
    Ok(())
}

fn version(found: u16) -> Result<(), ExternalSchemaError> {
    if found != EXTERNAL_SCHEMA_VERSION {
        return Err(ExternalSchemaError::UnsupportedSchemaVersion(found));
    }
    Ok(())
}

fn kind_tag(kind: ExternalTermKind) -> &'static str {
    match kind {
        ExternalTermKind::Class => "class",
        ExternalTermKind::ObjectProperty => "object_property",
        ExternalTermKind::DatatypeProperty => "datatype_property",
        ExternalTermKind::AnnotationProperty => "annotation_property",
        ExternalTermKind::NamedIndividual => "named_individual",
    }
}

impl ExternalTermMapping {
    /// Check the layout version and that no identifying field is empty.
    pub fn validate(&self) -> Result<(), ExternalSchemaError> {
        version(self.schema_version)?;
        require(&self.source_revision, "source_revision")?;
        require(&self.external_iri, "external_iri")?;
        require(&self.platform_iri, "platform_iri")?;
        require(&self.mapping_revision, "mapping_revision")
    }
}

impl ExternalOntologyCoverage {
    /// The digest over every field except `coverage_digest`, length-framed.
    pub fn compute_digest(&self) -> Digest256 {
        let mut fields: Vec<Vec<u8>> = vec![
            self.schema_version.to_be_bytes().to_vec(),
            self.source_schema_digest.as_bytes().to_vec(),
            self.source_revision.as_bytes().to_vec(),
            self.mapping_revision.as_bytes().to_vec(),
            self.pack_revision.as_bytes().to_vec(),
            self.schema_revision.as_bytes().to_vec(),
            self.platform_vocabulary_revision.as_bytes().to_vec(),
        ];
        for term in &self.terms {
            fields.push(term.external_iri.as_bytes().to_vec());
            fields.push(kind_tag(term.kind).as_bytes().to_vec());
            match &term.result {
                ExternalTermResult::Mapped {
                    platform_iri,
                    provenance_digest,
                } => {
                    fields.push(b"mapped".to_vec());
                    fields.push(platform_iri.as_bytes().to_vec());
                    fields.push(provenance_digest.as_bytes().to_vec());
                }
                ExternalTermResult::Unmapped { .. } => {
                    fields.push(b"unmapped:no_declared_mapping".to_vec());
                }
            }
        }
        let refs: Vec<&[u8]> = fields.iter().map(Vec::as_slice).collect();
        Digest256::framed(b"medulla.external_ontology_coverage", &refs)
            .expect("the framing domain is fixed and the field count is bounded by memory")
    }

    /// Check version, non-empty revisions, sorted unique terms and the digest.
    pub fn validate(&self) -> Result<(), ExternalSchemaError> {
        version(self.schema_version)?;
        require(&self.source_revision, "source_revision")?;
        require(&self.mapping_revision, "mapping_revision")?;
        require(&self.pack_revision, "pack_revision")?;
        require(&self.schema_revision, "schema_revision")?;
        require(
            &self.platform_vocabulary_revision,
            "platform_vocabulary_revision",
        )?;
        for pair in self.terms.windows(2) {
            let (before, after) = (&pair[0].external_iri, &pair[1].external_iri);
            if before == after {
                return Err(ExternalSchemaError::DuplicateIri(after.clone()));
            }
            if before > after {
                return Err(ExternalSchemaError::TermsNotSorted(after.clone()));
            }
        }
        let computed = self.compute_digest();
        if computed != self.coverage_digest {
            return Err(ExternalSchemaError::CoverageDigestMismatch {
                declared: self.coverage_digest.to_hex(),
                computed: computed.to_hex(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EX: &str = "http://example.org/ext#";

    fn mapping() -> ExternalTermMapping {
        ExternalTermMapping {
            schema_version: EXTERNAL_SCHEMA_VERSION,
            source_schema_digest: Digest256::sha256(b"external schema"),
            source_revision: "ext-2026-01".to_string(),
            external_iri: format!("{EX}Patient"),
            external_kind: ExternalTermKind::Class,
            platform_iri: "http://knuckles.team/kg#Person".to_string(),
            platform_kind: ExternalTermKind::Class,
            mapping_revision: "map-1".to_string(),
        }
    }

    fn coverage() -> ExternalOntologyCoverage {
        let mut coverage = ExternalOntologyCoverage {
            schema_version: EXTERNAL_SCHEMA_VERSION,
            source_schema_digest: Digest256::sha256(b"external schema"),
            source_revision: "ext-2026-01".to_string(),
            mapping_revision: "map-1".to_string(),
            pack_revision: "pack-3".to_string(),
            schema_revision: "schema-9".to_string(),
            platform_vocabulary_revision: "kg-2026".to_string(),
            terms: vec![
                ExternalTerm {
                    external_iri: format!("{EX}Patient"),
                    kind: ExternalTermKind::Class,
                    result: ExternalTermResult::Mapped {
                        platform_iri: "http://knuckles.team/kg#Person".to_string(),
                        provenance_digest: Digest256::sha256(b"provenance"),
                    },
                },
                ExternalTerm {
                    external_iri: format!("{EX}status"),
                    kind: ExternalTermKind::DatatypeProperty,
                    result: ExternalTermResult::Unmapped {
                        reason: ExternalUnmappedReason::NoDeclaredMapping,
                    },
                },
            ],
            coverage_digest: Digest256::sha256(b""),
        };
        coverage.coverage_digest = coverage.compute_digest();
        coverage
    }

    // spec: MEDULLA-TYPED-PACKS-R097.2.7.1
    #[test]
    fn mapping_and_coverage_round_trip() {
        let mapping = mapping();
        mapping.validate().expect("valid mapping");
        let encoded = serde_json::to_vec(&mapping).expect("serialize");
        assert_eq!(
            serde_json::from_slice::<ExternalTermMapping>(&encoded).expect("deserialize"),
            mapping
        );

        let coverage = coverage();
        coverage.validate().expect("valid coverage");
        let encoded = serde_json::to_vec(&coverage).expect("serialize");
        let decoded: ExternalOntologyCoverage =
            serde_json::from_slice(&encoded).expect("deserialize");
        assert_eq!(decoded, coverage);
        decoded.validate().expect("digest survives the round trip");

        // Unknown kinds and unknown fields refuse.
        let mut value = serde_json::to_value(&mapping).unwrap();
        value["external_kind"] = serde_json::json!("blank_node");
        assert!(serde_json::from_value::<ExternalTermMapping>(value).is_err());
        let mut value = serde_json::to_value(&mapping).unwrap();
        value["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<ExternalTermMapping>(value).is_err());

        // Duplicate IRIs, unsorted terms and a stale digest refuse.
        let mut duplicate = coverage.clone();
        duplicate.terms[1].external_iri = duplicate.terms[0].external_iri.clone();
        duplicate.coverage_digest = duplicate.compute_digest();
        assert!(matches!(
            duplicate.validate(),
            Err(ExternalSchemaError::DuplicateIri(_))
        ));
        let mut unsorted = coverage.clone();
        unsorted.terms.reverse();
        unsorted.coverage_digest = unsorted.compute_digest();
        assert!(matches!(
            unsorted.validate(),
            Err(ExternalSchemaError::TermsNotSorted(_))
        ));
        let mut stale = coverage;
        stale.pack_revision = "pack-4".to_string();
        assert!(matches!(
            stale.validate(),
            Err(ExternalSchemaError::CoverageDigestMismatch { .. })
        ));
    }
}
