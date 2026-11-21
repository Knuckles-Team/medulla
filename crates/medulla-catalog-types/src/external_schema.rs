//! External ontology term mappings and their coverage record
//! (MEDULLA-TYPED-PACKS-R097.2.7.1).
//!
//! A connector supplies candidate mappings from an external schema's terms to
//! platform terms; the platform validates them and records the coverage. The
//! records here are the typed model only: the function that builds a coverage
//! record from a pack import is `compute_external_coverage` (R097.2.7.2).

use serde::{Deserialize, Serialize};

use medulla_types::contract::Digest256;

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

    /// Check version and non-empty revisions.
    fn validate_fields(&self) -> Result<(), ExternalSchemaError> {
        version(self.schema_version)?;
        require(&self.source_revision, "source_revision")?;
        require(&self.mapping_revision, "mapping_revision")?;
        require(&self.pack_revision, "pack_revision")?;
        require(&self.schema_revision, "schema_revision")?;
        require(
            &self.platform_vocabulary_revision,
            "platform_vocabulary_revision",
        )
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

/// The external schema, its revisions and the terms and declared mappings one pack import
/// brings. The terms are in any order.
#[derive(Debug, Clone, Copy)]
pub struct ExternalCoverageInput<'a> {
    /// Digest of the external schema.
    pub source_schema_digest: Digest256,
    /// Revision of the external schema.
    pub source_revision: &'a str,
    /// Revision of the declared mappings.
    pub mapping_revision: &'a str,
    /// The pack revision being imported.
    pub pack_revision: &'a str,
    /// The graph schema revision.
    pub schema_revision: &'a str,
    /// The platform vocabulary revision.
    pub platform_vocabulary_revision: &'a str,
    /// Every term of the external schema with its kind.
    pub terms: &'a [(String, ExternalTermKind)],
    /// The mappings the connector declared.
    pub mappings: &'a [ExternalTermMapping],
}

/// The provenance digest of one adopted mapping: the original external IRI and kind, the
/// source schema and revisions, and the platform term.
fn provenance_digest(mapping: &ExternalTermMapping) -> Digest256 {
    let fields: [&[u8]; 7] = [
        mapping.external_iri.as_bytes(),
        kind_tag(mapping.external_kind).as_bytes(),
        mapping.source_schema_digest.as_bytes(),
        mapping.source_revision.as_bytes(),
        mapping.mapping_revision.as_bytes(),
        mapping.platform_iri.as_bytes(),
        kind_tag(mapping.platform_kind).as_bytes(),
    ];
    Digest256::framed(b"medulla.external_term_provenance", &fields)
        .expect("the framing domain is fixed and the field count is small")
}

/// Build the coverage record of one external schema for one pack revision
/// (MEDULLA-TYPED-PACKS-R097.2.7.2). Only a declared mapping of the same source schema and
/// revision, whose platform target has the kind of the external term, is adopted; every other
/// term is reported as unmapped and none is added as a platform term. When two adopted
/// mappings name one term, the smaller platform IRI wins, so the input order never changes the
/// result. The terms are sorted by `external_iri` before the digest is computed.
pub fn compute_external_coverage(
    input: &ExternalCoverageInput<'_>,
) -> Result<ExternalOntologyCoverage, ExternalSchemaError> {
    for mapping in input.mappings {
        mapping.validate()?;
    }
    let mut external: Vec<&(String, ExternalTermKind)> = input.terms.iter().collect();
    external.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut terms: Vec<ExternalTerm> = Vec::with_capacity(external.len());
    for (iri, kind) in external {
        if terms.last().is_some_and(|last| &last.external_iri == iri) {
            return Err(ExternalSchemaError::DuplicateIri(iri.clone()));
        }
        let adopted = input
            .mappings
            .iter()
            .filter(|m| {
                &m.external_iri == iri
                    && m.external_kind == *kind
                    && m.platform_kind == *kind
                    && m.source_schema_digest == input.source_schema_digest
                    && m.source_revision == input.source_revision
                    && m.mapping_revision == input.mapping_revision
            })
            .min_by(|a, b| a.platform_iri.cmp(&b.platform_iri));
        let result = match adopted {
            Some(mapping) => ExternalTermResult::Mapped {
                platform_iri: mapping.platform_iri.clone(),
                provenance_digest: provenance_digest(mapping),
            },
            None => ExternalTermResult::Unmapped {
                reason: ExternalUnmappedReason::NoDeclaredMapping,
            },
        };
        terms.push(ExternalTerm {
            external_iri: iri.clone(),
            kind: *kind,
            result,
        });
    }
    let mut coverage = ExternalOntologyCoverage {
        schema_version: EXTERNAL_SCHEMA_VERSION,
        source_schema_digest: input.source_schema_digest,
        source_revision: input.source_revision.to_string(),
        mapping_revision: input.mapping_revision.to_string(),
        pack_revision: input.pack_revision.to_string(),
        schema_revision: input.schema_revision.to_string(),
        platform_vocabulary_revision: input.platform_vocabulary_revision.to_string(),
        terms,
        coverage_digest: Digest256::sha256(b""),
    };
    coverage.validate_fields()?;
    coverage.coverage_digest = coverage.compute_digest();
    Ok(coverage)
}

/// One external term in a pack's external schema body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalSchemaTerm {
    /// The term's IRI in the external schema.
    pub external_iri: String,
    /// Its kind.
    pub kind: ExternalTermKind,
}

/// The body of one `external_schema` pack entry (MEDULLA-TYPED-PACKS-R097.2.7.3): the pinned
/// external schema, its terms and the mappings the connector declares. A report the body
/// carries must equal the one the importer computes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalSchemaBody {
    /// The record layout version.
    pub schema_version: u16,
    /// Digest of the external schema.
    pub source_schema_digest: Digest256,
    /// Revision of the external schema.
    pub source_revision: String,
    /// Revision of the declared mappings.
    pub mapping_revision: String,
    /// Every term of the external schema.
    pub terms: Vec<ExternalSchemaTerm>,
    /// The declared mappings.
    #[serde(default)]
    pub mappings: Vec<ExternalTermMapping>,
    /// A coverage report the producer computed; the importer recomputes and compares it.
    #[serde(default)]
    pub coverage: Option<ExternalOntologyCoverage>,
}

/// Why an external schema body was refused. Each variant but `Malformed` is a registered code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalSchemaRefusal {
    /// The body is not an `ExternalSchemaBody`.
    Malformed,
    /// A term or mapping names a kind outside the closed set.
    KindUnsupported,
    /// An external IRI is listed twice.
    TermDuplicate,
    /// A mapping's kinds disagree with the term's or with each other.
    KindMismatch,
    /// A mapping names another source schema or revision than the body pins.
    MappingStale,
    /// A term's IRI belongs to the platform vocabulary, not to an external schema.
    SourceOutOfScope,
    /// The body carries a coverage report that is not the computed one.
    ReportForged,
}

impl ExternalSchemaRefusal {
    /// The wire code a refusal is reported under; `MALFORMED_BODY` for a body that
    /// is not an external schema at all.
    pub fn code(self) -> &'static str {
        match self {
            Self::Malformed => "MALFORMED_BODY",
            Self::KindUnsupported => "EXTERNAL_KIND_UNSUPPORTED",
            Self::TermDuplicate => "EXTERNAL_TERM_DUPLICATE",
            Self::KindMismatch => "EXTERNAL_KIND_MISMATCH",
            Self::MappingStale => "EXTERNAL_MAPPING_STALE",
            Self::SourceOutOfScope => "EXTERNAL_SOURCE_OUT_OF_SCOPE",
            Self::ReportForged => "EXTERNAL_REPORT_FORGED",
        }
    }
}

const KIND_NAMES: [&str; 5] = [
    "class",
    "object_property",
    "datatype_property",
    "annotation_property",
    "named_individual",
];

fn unsupported_kind(value: &serde_json::Value) -> Option<String> {
    let mut kinds: Vec<&serde_json::Value> = Vec::new();
    for term in value.get("terms").and_then(|v| v.as_array()).into_iter().flatten() {
        kinds.extend(term.get("kind"));
    }
    for mapping in value.get("mappings").and_then(|v| v.as_array()).into_iter().flatten() {
        kinds.extend(mapping.get("external_kind"));
        kinds.extend(mapping.get("platform_kind"));
    }
    kinds
        .into_iter()
        .filter_map(|kind| kind.as_str())
        .find(|kind| !KIND_NAMES.contains(kind))
        .map(str::to_string)
}

/// Admit one external schema body: parse it, check every refusal rule and build its coverage.
/// `pack_revision`, `schema_revision` and `platform_vocabulary_revision` are the importer's
/// own revisions and are never read from the body.
pub fn admit_external_schema(
    value: &serde_json::Value,
    pack_revision: &str,
    schema_revision: &str,
    platform_vocabulary_revision: &str,
) -> Result<(ExternalSchemaBody, ExternalOntologyCoverage), (ExternalSchemaRefusal, String)> {
    use ExternalSchemaRefusal as R;
    if let Some(kind) = unsupported_kind(value) {
        return Err((R::KindUnsupported, format!("kind {kind:?} is not supported")));
    }
    let body: ExternalSchemaBody = serde_json::from_value(value.clone())
        .map_err(|error| (R::Malformed, format!("external schema body is invalid: {error}")))?;
    version(body.schema_version).map_err(|error| (R::Malformed, error.to_string()))?;
    let platform = medulla_types::ontology_base!();
    let mut seen = std::collections::BTreeSet::new();
    for term in &body.terms {
        if !seen.insert(term.external_iri.as_str()) {
            return Err((R::TermDuplicate, format!("{} is listed twice", term.external_iri)));
        }
        if term.external_iri.starts_with(platform) {
            return Err((
                R::SourceOutOfScope,
                format!("{} belongs to the platform vocabulary", term.external_iri),
            ));
        }
    }
    for mapping in &body.mappings {
        mapping.validate().map_err(|error| (R::Malformed, error.to_string()))?;
        if mapping.source_schema_digest != body.source_schema_digest
            || mapping.source_revision != body.source_revision
            || mapping.mapping_revision != body.mapping_revision
        {
            return Err((
                R::MappingStale,
                format!("the mapping of {} is for another schema or revision", mapping.external_iri),
            ));
        }
        let term = body.terms.iter().find(|term| term.external_iri == mapping.external_iri);
        // A mapping whose external IRI names no term of the body is outside the
        // pinned source set, as a crosswalk source outside it is.
        if term.is_none() {
            return Err((
                R::SourceOutOfScope,
                format!("the mapping names {}, which is no term of the body", mapping.external_iri),
            ));
        }
        let mismatched = term.is_some_and(|term| term.kind != mapping.external_kind)
            || mapping.platform_kind != mapping.external_kind;
        if mismatched {
            return Err((
                R::KindMismatch,
                format!("the mapping of {} does not match its kinds", mapping.external_iri),
            ));
        }
    }
    let terms: Vec<(String, ExternalTermKind)> = body
        .terms
        .iter()
        .map(|term| (term.external_iri.clone(), term.kind))
        .collect();
    let coverage = compute_external_coverage(&ExternalCoverageInput {
        source_schema_digest: body.source_schema_digest,
        source_revision: &body.source_revision,
        mapping_revision: &body.mapping_revision,
        pack_revision,
        schema_revision,
        platform_vocabulary_revision,
        terms: &terms,
        mappings: &body.mappings,
    })
    .map_err(|error| (R::Malformed, error.to_string()))?;
    if let Some(declared) = &body.coverage {
        if *declared != coverage {
            return Err((
                R::ReportForged,
                "the carried coverage report is not the computed one".to_string(),
            ));
        }
    }
    Ok((body, coverage))
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
            platform_iri: "https://knuckles-team.github.io/ontology/core#Person".to_string(),
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
                        platform_iri: "https://knuckles-team.github.io/ontology/core#Person"
                            .to_string(),
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

    // spec: MEDULLA-TYPED-PACKS-R097.2.7.2
    #[test]
    fn reordered_inputs_give_identical_digest() {
        let digest = Digest256::sha256(b"external schema");
        let adopted = mapping();
        let mut wrong_kind = mapping();
        wrong_kind.external_iri = format!("{EX}status");
        wrong_kind.external_kind = ExternalTermKind::DatatypeProperty;
        wrong_kind.platform_kind = ExternalTermKind::ObjectProperty;
        let terms = vec![
            (format!("{EX}status"), ExternalTermKind::DatatypeProperty),
            (format!("{EX}Patient"), ExternalTermKind::Class),
            (format!("{EX}Ward"), ExternalTermKind::Class),
        ];
        let mappings = vec![adopted.clone(), wrong_kind.clone()];
        let build = |terms: &[(String, ExternalTermKind)], mappings: &[ExternalTermMapping]| {
            compute_external_coverage(&ExternalCoverageInput {
                source_schema_digest: digest,
                source_revision: "ext-2026-01",
                mapping_revision: "map-1",
                pack_revision: "pack-3",
                schema_revision: "schema-9",
                platform_vocabulary_revision: "kg-2026",
                terms,
                mappings,
            })
            .expect("coverage")
        };
        let first = build(&terms, &mappings);
        first.validate().expect("the built coverage validates");
        let hex = first.coverage_digest.to_hex();
        assert_eq!(hex.len(), 64);
        assert!(hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));

        let mut reversed_terms = terms.clone();
        reversed_terms.reverse();
        let mut reversed_mappings = mappings.clone();
        reversed_mappings.reverse();
        let second = build(&reversed_terms, &reversed_mappings);
        assert_eq!(first, second);
        assert_eq!(first.coverage_digest, second.coverage_digest);

        // Sorted by IRI; the compatible mapping is adopted, the kind-incompatible and the
        // undeclared terms are reported and never become platform terms.
        let iris: Vec<&str> = first.terms.iter().map(|t| t.external_iri.as_str()).collect();
        assert_eq!(
            iris,
            vec![
                format!("{EX}Patient"),
                format!("{EX}Ward"),
                format!("{EX}status")
            ]
        );
        assert!(matches!(first.terms[0].result, ExternalTermResult::Mapped { .. }));
        assert!(matches!(first.terms[1].result, ExternalTermResult::Unmapped { .. }));
        assert!(matches!(first.terms[2].result, ExternalTermResult::Unmapped { .. }));

        // A term listed twice refuses.
        let mut duplicated = terms.clone();
        duplicated.push((format!("{EX}Ward"), ExternalTermKind::Class));
        assert!(matches!(
            compute_external_coverage(&ExternalCoverageInput {
                source_schema_digest: digest,
                source_revision: "ext-2026-01",
                mapping_revision: "map-1",
                pack_revision: "pack-3",
                schema_revision: "schema-9",
                platform_vocabulary_revision: "kg-2026",
                terms: &duplicated,
                mappings: &mappings,
            }),
            Err(ExternalSchemaError::DuplicateIri(_))
        ));
    }

    // spec: MEDULLA-TYPED-PACKS-R097.2.7.3
    #[test]
    fn admission_refuses_each_case_and_builds_the_coverage() {
        use serde_json::json;
        let digest = Digest256::sha256(b"external schema");
        let good = json!({
            "schema_version": 1,
            "source_schema_digest": digest,
            "source_revision": "ext-2026-01",
            "mapping_revision": "map-1",
            "terms": [
                {"external_iri": format!("{EX}Patient"), "kind": "class"},
                {"external_iri": format!("{EX}status"), "kind": "datatype_property"}
            ],
            "mappings": [{
                "schema_version": 1,
                "source_schema_digest": digest,
                "source_revision": "ext-2026-01",
                "external_iri": format!("{EX}Patient"),
                "external_kind": "class",
                "platform_iri": "https://knuckles-team.github.io/ontology/core#Person",
                "platform_kind": "class",
                "mapping_revision": "map-1"
            }]
        });
        let admit = |value: &serde_json::Value| {
            admit_external_schema(value, "pack-3", "schema-9", "kg-2026")
        };
        let (_, coverage) = admit(&good).expect("admitted");
        assert!(matches!(coverage.terms[0].result, ExternalTermResult::Mapped { .. }));
        assert!(matches!(coverage.terms[1].result, ExternalTermResult::Unmapped { .. }));

        let refused = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut value = good.clone();
            edit(&mut value);
            admit(&value).unwrap_err().0
        };
        use ExternalSchemaRefusal as R;
        assert_eq!(refused(&|v| v["terms"][0]["kind"] = json!("blank_node")), R::KindUnsupported);
        assert_eq!(
            refused(&|v| v["terms"][1]["external_iri"] = json!(format!("{EX}Patient"))),
            R::TermDuplicate
        );
        assert_eq!(refused(&|v| v["terms"][0]["kind"] = json!("object_property")), R::KindMismatch);
        assert_eq!(refused(&|v| v["mappings"][0]["mapping_revision"] = json!("map-0")), R::MappingStale);
        assert_eq!(
            refused(&|v| v["terms"][1]["external_iri"] =
                json!("https://knuckles-team.github.io/ontology/core#Thing")),
            R::SourceOutOfScope
        );
        let mut forged = coverage;
        forged.pack_revision = "pack-9".to_string();
        assert_eq!(
            refused(&|v| v["coverage"] = serde_json::to_value(&forged).unwrap()),
            R::ReportForged
        );
        assert_eq!(refused(&|v| v["surprise"] = json!(1)), R::Malformed);
    }
}
