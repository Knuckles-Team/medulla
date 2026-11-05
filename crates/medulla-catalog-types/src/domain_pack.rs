//! A typed, versioned domain vocabulary pack (MEDULLA-TYPED-PACKS-R097.1).
//!
//! Human-resources, legal, medical and government domain vocabularies —
//! classes, enumerations, properties and their alignment to upper ontologies —
//! publish through this typed model instead of each agent or connector
//! carrying its own model classes. This is the producer-first slice of
//! `MEDULLA-TYPED-PACKS-R097` (split per `SPEC-SIZING-AND-DEPENDENCIES.md` §5): the
//! typed model plus validation and a round-trip test for one domain (HR).
//! Wiring this pack through the `connector_pack` import boundary, the
//! remaining three domains, and retiring Agent Utilities' `domains/*` and
//! `models/domains` modules are later child requirements.

use std::collections::BTreeSet;

mod crosswalk;

pub use crosswalk::{
    validate_crosswalks, CrosswalkContext, PinnedHierarchy, MAX_CROSSWALK_IRI_BYTES,
    MAX_CROSSWALK_MAPPINGS, MAX_CROSSWALK_SOURCE_CLASSES,
};

/// The four domains this requirement names. Closed: a fifth domain is a new
/// variant and a new child requirement, not a free-form string.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DomainPackKind {
    Hr,
    Legal,
    Medical,
    Government,
}

/// One `owl:Class` in a domain pack, optionally aligned to an upper-ontology
/// class (for example a BFO IRI) via `rdfs:subClassOf`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainClass {
    pub local_name: String,
    pub label: String,
    pub aligns_to_upper_ontology: Option<String>,
}

/// The closed set of relations a vocabulary crosswalk may declare.
///
/// These are references between independently governed vocabularies.  They do
/// not imply `owl:sameAs`, class equivalence or a new subclass assertion.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum VocabularyCrosswalkRelation {
    SeeAlso,
    SkosExactMatch,
    SkosCloseMatch,
    SkosBroadMatch,
    SkosNarrowMatch,
}

/// One approved class declaration in the target vocabulary revision.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedTargetDeclaration {
    pub class_iri: String,
    pub category: String,
}

/// A revision-pinned alignment between two vocabulary classes.
///
/// The target declarations are carried with the target vocabulary revision so
/// later validation can check a mapping against the approved target vocabulary
/// without fetching external definitions.  This model is deliberately separate
/// from [`DomainPack`]; import, activation and served inspection own the later
/// production-path checks described by MEDULLA-TYPED-PACKS-R071.1.3-.6.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct VocabularyCrosswalk {
    pub schema_version: u16,
    pub source_class_iri: String,
    pub target_class_iri: String,
    pub relation: VocabularyCrosswalkRelation,
    pub source_revision: String,
    pub target_vocabulary_revision: String,
    pub origin: String,
    pub content_digest: String,
    pub approved_target_declarations: Vec<ApprovedTargetDeclaration>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct VocabularyCrosswalkWire {
    schema_version: u16,
    source_class_iri: String,
    target_class_iri: String,
    relation: VocabularyCrosswalkRelation,
    source_revision: String,
    target_vocabulary_revision: String,
    origin: String,
    content_digest: String,
    approved_target_declarations: Vec<ApprovedTargetDeclaration>,
}

/// A malformed typed crosswalk is rejected before it reaches an import or
/// schema-activation boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VocabularyCrosswalkError {
    #[error("crosswalk source_class_iri is not an absolute IRI: {0:?}")]
    InvalidSourceClassIri(String),
    #[error("crosswalk target_class_iri is not an absolute IRI: {0:?}")]
    InvalidTargetClassIri(String),
    #[error("crosswalk approved target class_iri is not an absolute IRI: {0:?}")]
    InvalidApprovedTargetClassIri(String),
    #[error("crosswalk source_revision is empty")]
    EmptySourceRevision,
    #[error("crosswalk target_vocabulary_revision is empty")]
    EmptyTargetVocabularyRevision,
    #[error("crosswalk origin is empty")]
    EmptyOrigin,
    #[error("crosswalk content_digest is empty")]
    EmptyContentDigest,
    #[error("crosswalk approved target category is empty")]
    EmptyApprovedTargetCategory,
    #[error("crosswalk declares duplicate approved target class_iri {0:?}")]
    DuplicateApprovedTargetClassIri(String),
    #[error("crosswalk schema_version {0} is not supported (expected {VOCABULARY_CROSSWALK_SCHEMA_VERSION})")]
    UnsupportedSchemaVersion(u16),
    #[error(
        "crosswalk content_digest {declared:?} does not match the computed digest {computed:?}"
    )]
    ContentDigestMismatch { declared: String, computed: String },
    #[error("crosswalk source class {source_iri:?} does not resolve as a class in revision {revision:?} of scope {scope:?}")]
    SourceUnknown {
        source_iri: String,
        revision: String,
        scope: String,
    },
    #[error("crosswalk source class {source_iri:?} (revision {revision:?}) is outside the selected source scope {scope:?}")]
    SourceOutOfScope {
        source_iri: String,
        revision: String,
        scope: String,
    },
    #[error("crosswalk target class {0:?} is not an approved declaration of the target vocabulary revision")]
    TargetUnknown(String),
    #[error("crosswalk mapping {source_iri:?} -> {target_iri:?} conflicts with a declared mapping or the pinned hierarchy")]
    Conflict {
        source_iri: String,
        target_iri: String,
    },
    #[error("crosswalk target {target_iri:?} category {target_category:?} differs from source category {source_category:?}")]
    CategoryConflict {
        target_iri: String,
        source_category: String,
        target_category: String,
    },
    #[error("crosswalk input exceeds the fixed bound {bound} at {iri:?}")]
    LimitExceeded { bound: &'static str, iri: String },
}

/// The one record layout this build reads and writes (MEDULLA-TYPED-PACKS-R071.1.1).
pub const VOCABULARY_CROSSWALK_SCHEMA_VERSION: u16 = 1;

fn relation_tag(relation: VocabularyCrosswalkRelation) -> &'static str {
    match relation {
        VocabularyCrosswalkRelation::SeeAlso => "see_also",
        VocabularyCrosswalkRelation::SkosExactMatch => "skos_exact_match",
        VocabularyCrosswalkRelation::SkosCloseMatch => "skos_close_match",
        VocabularyCrosswalkRelation::SkosBroadMatch => "skos_broad_match",
        VocabularyCrosswalkRelation::SkosNarrowMatch => "skos_narrow_match",
    }
}

impl VocabularyCrosswalk {
    /// Validate the model-level invariants that are independent of a selected
    /// source/target vocabulary.  Unknown `relation` values are rejected by
    /// serde because the relation is a closed enum.
    pub fn validate(&self) -> Result<(), VocabularyCrosswalkError> {
        if !is_absolute_iri(&self.source_class_iri) {
            return Err(VocabularyCrosswalkError::InvalidSourceClassIri(
                self.source_class_iri.clone(),
            ));
        }
        if !is_absolute_iri(&self.target_class_iri) {
            return Err(VocabularyCrosswalkError::InvalidTargetClassIri(
                self.target_class_iri.clone(),
            ));
        }
        if self.source_revision.trim().is_empty() {
            return Err(VocabularyCrosswalkError::EmptySourceRevision);
        }
        if self.target_vocabulary_revision.trim().is_empty() {
            return Err(VocabularyCrosswalkError::EmptyTargetVocabularyRevision);
        }
        if self.origin.trim().is_empty() {
            return Err(VocabularyCrosswalkError::EmptyOrigin);
        }
        if self.schema_version != VOCABULARY_CROSSWALK_SCHEMA_VERSION {
            return Err(VocabularyCrosswalkError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }
        if self.content_digest.trim().is_empty() {
            return Err(VocabularyCrosswalkError::EmptyContentDigest);
        }

        let mut seen_targets = BTreeSet::new();
        for target in &self.approved_target_declarations {
            if !is_absolute_iri(&target.class_iri) {
                return Err(VocabularyCrosswalkError::InvalidApprovedTargetClassIri(
                    target.class_iri.clone(),
                ));
            }
            if target.category.trim().is_empty() {
                return Err(VocabularyCrosswalkError::EmptyApprovedTargetCategory);
            }
            if !seen_targets.insert(target.class_iri.as_str()) {
                return Err(VocabularyCrosswalkError::DuplicateApprovedTargetClassIri(
                    target.class_iri.clone(),
                ));
            }
        }

        let computed = self.compute_content_digest();
        if self.content_digest != computed {
            return Err(VocabularyCrosswalkError::ContentDigestMismatch {
                declared: self.content_digest.clone(),
                computed,
            });
        }

        Ok(())
    }

    /// SHA-256 (64 lowercase hex characters) over every field except the
    /// digest itself, `schema_version` first. Each field is length-prefixed so
    /// no two distinct records share an encoding.
    pub fn compute_content_digest(&self) -> String {
        use sha2::{Digest, Sha256};
        fn field(hasher: &mut Sha256, bytes: &[u8]) {
            hasher.update((bytes.len() as u64).to_be_bytes());
            hasher.update(bytes);
        }
        let mut hasher = Sha256::new();
        hasher.update(b"medulla.vocabulary_crosswalk");
        hasher.update(self.schema_version.to_be_bytes());
        field(&mut hasher, self.source_class_iri.as_bytes());
        field(&mut hasher, self.target_class_iri.as_bytes());
        field(&mut hasher, relation_tag(self.relation).as_bytes());
        field(&mut hasher, self.source_revision.as_bytes());
        field(&mut hasher, self.target_vocabulary_revision.as_bytes());
        field(&mut hasher, self.origin.as_bytes());
        hasher.update((self.approved_target_declarations.len() as u64).to_be_bytes());
        for target in &self.approved_target_declarations {
            field(&mut hasher, target.class_iri.as_bytes());
            field(&mut hasher, target.category.as_bytes());
        }
        hex::encode(hasher.finalize())
    }
}

impl VocabularyCrosswalk {
    fn from_wire(wire: VocabularyCrosswalkWire) -> Result<Self, VocabularyCrosswalkError> {
        let crosswalk = Self {
            schema_version: wire.schema_version,
            source_class_iri: wire.source_class_iri,
            target_class_iri: wire.target_class_iri,
            relation: wire.relation,
            source_revision: wire.source_revision,
            target_vocabulary_revision: wire.target_vocabulary_revision,
            origin: wire.origin,
            content_digest: wire.content_digest,
            approved_target_declarations: wire.approved_target_declarations,
        };
        crosswalk.validate()?;
        Ok(crosswalk)
    }
}

impl<'de> serde::Deserialize<'de> for VocabularyCrosswalk {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = VocabularyCrosswalkWire::deserialize(deserializer)?;
        Self::from_wire(wire).map_err(serde::de::Error::custom)
    }
}

/// The model only needs the absolute-IRI syntax boundary.  Vocabulary
/// membership, class kind and revision scope are selected-source checks owned
/// by the later import path, not by this reusable typed record.
fn is_absolute_iri(value: &str) -> bool {
    oxiri::Iri::parse(value).is_ok()
}

/// One closed enumeration of named members (for example EEO-1 job categories).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainEnumeration {
    pub local_name: String,
    pub label: String,
    pub members: Vec<String>,
}

/// One `owl:DatatypeProperty` or `owl:ObjectProperty` scoped to a declared
/// domain class.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainProperty {
    pub local_name: String,
    pub domain_class: String,
    pub range: String,
}

/// A rejection reason for an invalid domain pack. Carried through the caller's
/// error channel, never added as a field on a kernel row (consistent with the
/// federated-query spec's rejection-reason rule).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainPackError {
    #[error("domain pack {0:?} declares no classes")]
    NoClasses(DomainPackKind),
    #[error("domain pack {kind:?} has a duplicate class local_name {local_name:?}")]
    DuplicateClass {
        kind: DomainPackKind,
        local_name: String,
    },
    #[error("domain pack {kind:?} property {property:?} has an undeclared domain_class {domain_class:?}")]
    UndeclaredPropertyDomain {
        kind: DomainPackKind,
        property: String,
        domain_class: String,
    },
}

/// One typed, versioned domain vocabulary pack.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainPack {
    pub kind: DomainPackKind,
    pub version: u32,
    pub classes: Vec<DomainClass>,
    pub enumerations: Vec<DomainEnumeration>,
    pub properties: Vec<DomainProperty>,
}

impl DomainPack {
    /// Validate the pack's internal consistency: at least one class, no
    /// duplicate class names, and every property's `domain_class` names a
    /// class the pack actually declares.
    pub fn validate(&self) -> Result<(), DomainPackError> {
        if self.classes.is_empty() {
            return Err(DomainPackError::NoClasses(self.kind));
        }

        let mut seen = BTreeSet::new();
        for class in &self.classes {
            if !seen.insert(class.local_name.as_str()) {
                return Err(DomainPackError::DuplicateClass {
                    kind: self.kind,
                    local_name: class.local_name.clone(),
                });
            }
        }

        for prop in &self.properties {
            if !seen.contains(prop.domain_class.as_str()) {
                return Err(DomainPackError::UndeclaredPropertyDomain {
                    kind: self.kind,
                    property: prop.local_name.clone(),
                    domain_class: prop.domain_class.clone(),
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small, real fixture drawn from `crates/medulla-core/ontology/hr-v1.ttl`:
    /// `EEOCategory` aligned to the upper-ontology class `bfo:0000031`, plus
    /// `W2Employee`, and one property scoped to `EEOCategory`.
    fn hr_fixture() -> DomainPack {
        DomainPack {
            kind: DomainPackKind::Hr,
            version: 1,
            classes: vec![
                DomainClass {
                    local_name: "EEOCategory".to_string(),
                    label: "EEO Category".to_string(),
                    aligns_to_upper_ontology: Some("bfo:0000031".to_string()),
                },
                DomainClass {
                    local_name: "W2Employee".to_string(),
                    label: "W-2 Employee".to_string(),
                    aligns_to_upper_ontology: None,
                },
            ],
            enumerations: vec![DomainEnumeration {
                local_name: "EEOJobCategory".to_string(),
                label: "EEO-1 job category".to_string(),
                members: vec![
                    "OfficialsAndManagers".to_string(),
                    "Professionals".to_string(),
                ],
            }],
            properties: vec![DomainProperty {
                local_name: "accommodationStatus".to_string(),
                domain_class: "EEOCategory".to_string(),
                range: "xsd:string".to_string(),
            }],
        }
    }

    fn crosswalk_fixture() -> VocabularyCrosswalk {
        sealed(VocabularyCrosswalk {
            schema_version: VOCABULARY_CROSSWALK_SCHEMA_VERSION,
            source_class_iri: "http://knuckles.team/kg#Person".to_string(),
            target_class_iri: "http://schema.org/Person".to_string(),
            relation: VocabularyCrosswalkRelation::SeeAlso,
            source_revision: "core-crosswalk-fixture-v1".to_string(),
            target_vocabulary_revision: "schemaorg-crosswalk-fixture-v1".to_string(),
            origin: "core-foundation-v1".to_string(),
            content_digest: String::new(),
            approved_target_declarations: vec![ApprovedTargetDeclaration {
                class_iri: "http://schema.org/Person".to_string(),
                category: "continuant".to_string(),
            }],
        })
    }

    fn sealed(mut crosswalk: VocabularyCrosswalk) -> VocabularyCrosswalk {
        crosswalk.content_digest = crosswalk.compute_content_digest();
        crosswalk
    }

    /// MEDULLA-TYPED-PACKS-R097.1: a valid HR domain pack passes validation.
    // spec: MEDULLA-TYPED-PACKS-R097.1
    #[test]
    fn hr_fixture_validates() {
        hr_fixture().validate().expect("fixture must be valid");
    }

    /// Round-trip test: serialize then deserialize the pack, and confirm the
    /// published classes and enumerations match the source declarations.
    #[test]
    fn round_trip_matches_source_declarations() {
        let original = hr_fixture();
        let encoded = serde_json::to_vec(&original).expect("serialize");
        let decoded: DomainPack = serde_json::from_slice(&encoded).expect("deserialize");
        assert_eq!(decoded.classes, original.classes);
        assert_eq!(decoded.enumerations, original.enumerations);
        assert_eq!(decoded, original);
    }

    /// A pack with zero classes is refused rather than silently accepted.
    #[test]
    fn empty_classes_is_refused() {
        let mut pack = hr_fixture();
        pack.classes.clear();
        assert_eq!(
            pack.validate(),
            Err(DomainPackError::NoClasses(DomainPackKind::Hr))
        );
    }

    /// A duplicate class local_name is refused.
    #[test]
    fn duplicate_class_is_refused() {
        let mut pack = hr_fixture();
        let dup = pack.classes[0].clone();
        pack.classes.push(dup);
        assert!(matches!(
            pack.validate(),
            Err(DomainPackError::DuplicateClass { .. })
        ));
    }

    /// A property naming an undeclared domain_class is refused.
    #[test]
    fn property_with_undeclared_domain_class_is_refused() {
        let mut pack = hr_fixture();
        pack.properties.push(DomainProperty {
            local_name: "stray".to_string(),
            domain_class: "NotDeclared".to_string(),
            range: "xsd:string".to_string(),
        });
        assert!(matches!(
            pack.validate(),
            Err(DomainPackError::UndeclaredPropertyDomain { .. })
        ));
    }

    /// MEDULLA-TYPED-PACKS-R071.1.1: every crosswalk field, including
    /// `schema_version`, the approved target declaration and category, survives
    /// the wire round trip, and every field (schema version included) is folded
    /// into the 64-hex SHA-256 content digest.
    // spec: MEDULLA-TYPED-PACKS-R071.1.1
    #[test]
    fn crosswalk_round_trips_every_field() {
        let original = crosswalk_fixture();
        original.validate().expect("fixture must validate");
        assert_eq!(original.schema_version, 1);
        assert_eq!(original.content_digest.len(), 64);
        assert!(original
            .content_digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        let encoded = serde_json::to_vec(&original).expect("serialize");
        let decoded: VocabularyCrosswalk = serde_json::from_slice(&encoded).expect("deserialize");
        assert_eq!(decoded, original);

        // Each field changes the digest.
        let base = original.compute_content_digest();
        let mut mutations: Vec<VocabularyCrosswalk> = Vec::new();
        let mut m = original.clone();
        m.schema_version = 2;
        mutations.push(m);
        let mut m = original.clone();
        m.source_class_iri.push('X');
        mutations.push(m);
        let mut m = original.clone();
        m.target_class_iri.push('X');
        mutations.push(m);
        let mut m = original.clone();
        m.relation = VocabularyCrosswalkRelation::SkosCloseMatch;
        mutations.push(m);
        let mut m = original.clone();
        m.source_revision.push('X');
        mutations.push(m);
        let mut m = original.clone();
        m.target_vocabulary_revision.push('X');
        mutations.push(m);
        let mut m = original.clone();
        m.origin.push('X');
        mutations.push(m);
        let mut m = original.clone();
        m.approved_target_declarations[0].category.push('X');
        mutations.push(m);
        for m in mutations {
            assert_ne!(m.compute_content_digest(), base);
        }

        // An unsupported schema version and a stale digest refuse.
        let mut wrong_version = original.clone();
        wrong_version.schema_version = 2;
        let wrong_version = sealed(wrong_version);
        assert!(matches!(
            wrong_version.validate(),
            Err(VocabularyCrosswalkError::UnsupportedSchemaVersion(2))
        ));
        let mut stale = original.clone();
        stale.origin = "other-origin".to_string();
        assert!(matches!(
            stale.validate(),
            Err(VocabularyCrosswalkError::ContentDigestMismatch { .. })
        ));
        let mut value = serde_json::to_value(&original).expect("serialize");
        value
            .as_object_mut()
            .expect("crosswalk object")
            .remove("schema_version");
        assert!(serde_json::from_value::<VocabularyCrosswalk>(value).is_err());
    }

    /// The relation is closed; a caller cannot smuggle in an unapproved
    /// relation token through the JSON contract.
    #[test]
    fn crosswalk_rejects_unknown_relation() {
        for relation in [
            VocabularyCrosswalkRelation::SeeAlso,
            VocabularyCrosswalkRelation::SkosExactMatch,
            VocabularyCrosswalkRelation::SkosCloseMatch,
            VocabularyCrosswalkRelation::SkosBroadMatch,
            VocabularyCrosswalkRelation::SkosNarrowMatch,
        ] {
            let mut fixture = crosswalk_fixture();
            fixture.relation = relation;
            let fixture = sealed(fixture);
            let encoded = serde_json::to_vec(&fixture).expect("serialize");
            let decoded: VocabularyCrosswalk =
                serde_json::from_slice(&encoded).expect("known relation must decode");
            assert_eq!(decoded.relation, relation);
        }

        let mut value = serde_json::to_value(crosswalk_fixture()).expect("serialize");
        value["relation"] = serde_json::Value::String("owl_same_as".to_string());
        assert!(serde_json::from_value::<VocabularyCrosswalk>(value).is_err());
    }

    /// Source and target class identifiers, including approved declarations,
    /// must be absolute IRIs before a crosswalk can enter a later boundary.
    #[test]
    fn crosswalk_rejects_malformed_iris() {
        let mut value = serde_json::to_value(crosswalk_fixture()).expect("serialize");
        value["source_class_iri"] = serde_json::Value::String("Person".to_string());
        assert!(serde_json::from_value::<VocabularyCrosswalk>(value).is_err());

        let mut crosswalk = crosswalk_fixture();
        crosswalk.source_class_iri = "Person".to_string();
        assert!(matches!(
            crosswalk.validate(),
            Err(VocabularyCrosswalkError::InvalidSourceClassIri(_))
        ));

        let mut crosswalk = crosswalk_fixture();
        crosswalk.target_class_iri = "<http://schema.org/Person>".to_string();
        assert!(matches!(
            crosswalk.validate(),
            Err(VocabularyCrosswalkError::InvalidTargetClassIri(_))
        ));

        let mut crosswalk = crosswalk_fixture();
        crosswalk.approved_target_declarations[0].class_iri = "schema Person".to_string();
        assert!(matches!(
            crosswalk.validate(),
            Err(VocabularyCrosswalkError::InvalidApprovedTargetClassIri(_))
        ));

        for malformed in [
            "http://schema.org/Person\\Subclass",
            "http://schema.org/Person%GG",
            "http://schema.org/Person%2",
        ] {
            let mut crosswalk = crosswalk_fixture();
            crosswalk.source_class_iri = malformed.to_string();
            assert!(matches!(
                crosswalk.validate(),
                Err(VocabularyCrosswalkError::InvalidSourceClassIri(_))
            ));

            let mut crosswalk = crosswalk_fixture();
            crosswalk.target_class_iri = malformed.to_string();
            assert!(matches!(
                crosswalk.validate(),
                Err(VocabularyCrosswalkError::InvalidTargetClassIri(_))
            ));

            let mut crosswalk = crosswalk_fixture();
            crosswalk.approved_target_declarations[0].class_iri = malformed.to_string();
            assert!(matches!(
                crosswalk.validate(),
                Err(VocabularyCrosswalkError::InvalidApprovedTargetClassIri(_))
            ));
        }
    }

    /// Revision fields are required on the wire and cannot be replaced by an
    /// empty value; provenance must also identify a non-empty origin.
    #[test]
    fn crosswalk_rejects_absent_revisions_and_empty_origin() {
        let value = serde_json::to_value(crosswalk_fixture()).expect("serialize");
        for field in ["source_revision", "target_vocabulary_revision"] {
            let mut missing = value.clone();
            missing
                .as_object_mut()
                .expect("crosswalk object")
                .remove(field);
            assert!(serde_json::from_value::<VocabularyCrosswalk>(missing).is_err());
        }

        let mut empty_origin = value.clone();
        empty_origin["origin"] = serde_json::Value::String(String::new());
        assert!(serde_json::from_value::<VocabularyCrosswalk>(empty_origin).is_err());

        let mut crosswalk = crosswalk_fixture();
        crosswalk.source_revision.clear();
        assert!(matches!(
            crosswalk.validate(),
            Err(VocabularyCrosswalkError::EmptySourceRevision)
        ));

        let mut crosswalk = crosswalk_fixture();
        crosswalk.target_vocabulary_revision = "  ".to_string();
        assert!(matches!(
            crosswalk.validate(),
            Err(VocabularyCrosswalkError::EmptyTargetVocabularyRevision)
        ));

        let mut crosswalk = crosswalk_fixture();
        crosswalk.origin = "".to_string();
        assert!(matches!(
            crosswalk.validate(),
            Err(VocabularyCrosswalkError::EmptyOrigin)
        ));
    }
}
