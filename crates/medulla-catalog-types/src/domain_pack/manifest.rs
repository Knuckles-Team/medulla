//! The domain pack manifest, its mapping language and the served operations
//! (MEDULLA-TYPED-PACKS-R097.700).
//!
//! A manifest is a JSON document that declares classes and the mappings from
//! source positions (a column, a front-matter key, a table cell, a heading, a
//! link, a JSON field) to a class property or relation, plus conformance
//! cases. The loader checks `integrity_sha256` before any use. Pure data and
//! bounds; evaluation lives in `medulla-core` and the handler in `medulla-h-graph`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The only manifest schema version this engine reads.
pub const DOMAIN_PACK_MANIFEST_VERSION: u32 = 1;
/// Most mappings one manifest declares.
pub const MAX_MANIFEST_MAPPINGS: usize = 1_024;
/// Most conformance cases one manifest carries.
pub const MAX_MANIFEST_CASES: usize = 256;
/// Most source rows one request evaluates. Memory is bounded by 64 MiB per
/// 1,000 rows, so this cap keeps one request under 640 MiB.
pub const MAX_PACK_ROWS: usize = 10_000;
/// Largest serialized source row, in bytes (64 MiB per 1,000 rows).
pub const MAX_PACK_ROW_BYTES: usize = 65_536;
/// Most markdown sources one request carries.
pub const MAX_MARKDOWN_SOURCES: usize = 256;

/// Where in a source a mapping reads its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SourceKind {
    Column,
    FrontMatter,
    Table,
    Heading,
    Link,
    JsonField,
}

/// One class a manifest declares, with the properties and relations a mapping
/// may target on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ManifestClass {
    pub name: String,
    #[serde(default)]
    pub properties: Vec<String>,
    #[serde(default)]
    pub relations: Vec<String>,
}

/// `{source_kind, selector, target_class, target_property_or_relation}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ManifestMapping {
    pub source_kind: SourceKind,
    /// The field key a row carries; for `json_field` a key or a JSON pointer
    /// (a selector starting with `/`).
    pub selector: String,
    pub target_class: String,
    pub target_property_or_relation: String,
}

/// One source row. `fields` holds the named values of its position kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SourceRow {
    pub row_id: String,
    pub source_kind: SourceKind,
    pub fields: BTreeMap<String, serde_json::Value>,
}

/// One fact a mapping proposes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "fact", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ProposedFact {
    Entity {
        entity_id: String,
        class: String,
    },
    Property {
        entity_id: String,
        property: String,
        value: serde_json::Value,
    },
    Relationship {
        from: String,
        relation: String,
        to: String,
    },
}

/// A conformance case: the rows and the facts they must evaluate to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ManifestCase {
    pub name: String,
    pub rows: Vec<SourceRow>,
    pub expected: Vec<ProposedFact>,
}

/// The manifest of one domain pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DomainPackManifest {
    pub schema_version: u32,
    pub pack_id: String,
    pub version: String,
    /// Lowercase hex SHA-256 over [`DomainPackManifest::compute_integrity`]'s body.
    pub integrity_sha256: String,
    pub classes: Vec<ManifestClass>,
    pub mappings: Vec<ManifestMapping>,
    #[serde(default)]
    pub cases: Vec<ManifestCase>,
}

/// The part of a manifest the integrity digest covers: everything but the
/// digest itself.
#[derive(Serialize)]
struct IntegrityBody<'a> {
    schema_version: u32,
    pack_id: &'a str,
    version: &'a str,
    classes: &'a [ManifestClass],
    mappings: &'a [ManifestMapping],
    cases: &'a [ManifestCase],
}

/// The class, property and relation terms of a graph's composed ontology (the
/// core plus every admitted pack), each held by full IRI and by local name so a
/// mapping may name a term either way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposedVocabulary {
    classes: BTreeSet<String>,
    properties: BTreeSet<String>,
    relations: BTreeSet<String>,
}

impl ComposedVocabulary {
    /// Collect the terms of a composed ontology: `(iri, kind)` pairs.
    pub fn from_terms<'a>(
        terms: impl IntoIterator<Item = (&'a str, crate::graph_schema::GraphSchemaTermKind)>,
    ) -> Self {
        use crate::graph_schema::{term_local_name, GraphSchemaTermKind as Kind};
        let mut vocabulary = Self::default();
        for (iri, kind) in terms {
            let set = match kind {
                Kind::Class => &mut vocabulary.classes,
                Kind::ObjectProperty => &mut vocabulary.relations,
                Kind::DatatypeProperty | Kind::AnnotationProperty | Kind::Property => {
                    &mut vocabulary.properties
                }
            };
            set.insert(iri.to_owned());
            set.insert(term_local_name(iri).to_owned());
        }
        vocabulary
    }

    fn has_class(&self, name: &str) -> bool {
        self.classes.contains(name)
    }

    fn has_property(&self, name: &str) -> bool {
        self.properties.contains(name)
    }

    fn has_relation(&self, name: &str) -> bool {
        self.relations.contains(name)
    }
}

/// Why a manifest is refused before use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// A bound, a reference or a name is wrong; the text says which.
    Invalid(String),
    /// A mapping has an empty selector or a target its class does not declare.
    MappingInvalid(String),
    /// A mapping names a class the pack does not declare.
    ClassUnknown(String),
    /// `integrity_sha256` is not the digest of the manifest body.
    IntegrityMismatch { declared: String, computed: String },
}

impl DomainPackManifest {
    /// Lowercase hex SHA-256 of the manifest body (every field but the digest).
    pub fn compute_integrity(&self) -> String {
        let body = IntegrityBody {
            schema_version: self.schema_version,
            pack_id: &self.pack_id,
            version: &self.version,
            classes: &self.classes,
            mappings: &self.mappings,
            cases: &self.cases,
        };
        let bytes = serde_json::to_vec(&body).unwrap_or_default();
        hex_lower(&Sha256::digest(bytes))
    }

    /// Check the declared digest against the body.
    pub fn verify_integrity(&self) -> Result<(), ManifestError> {
        let computed = self.compute_integrity();
        if computed == self.integrity_sha256 {
            Ok(())
        } else {
            Err(ManifestError::IntegrityMismatch {
                declared: self.integrity_sha256.clone(),
                computed,
            })
        }
    }

    /// Check bounds and references against the graph's composed ontology and
    /// return the manifest with every composed class its mappings target
    /// resolved into a class entry. A mapping targets either a class the
    /// manifest declares (its property or relation must be declared on it) or a
    /// class of the composed ontology (the core plus every admitted pack, which
    /// every pack rolls up to), whose property or relation must be a composed
    /// term. A class in neither is `ClassUnknown`.
    pub fn resolve(
        &self,
        composed: &ComposedVocabulary,
    ) -> Result<DomainPackManifest, ManifestError> {
        let bad = |detail: String| Err(ManifestError::Invalid(detail));
        if self.schema_version != DOMAIN_PACK_MANIFEST_VERSION {
            return bad(format!(
                "unsupported schema_version {}",
                self.schema_version
            ));
        }
        if self.pack_id.trim().is_empty() || self.version.trim().is_empty() {
            return bad("pack_id and version must not be empty".into());
        }
        if self.classes.is_empty() {
            return bad("a manifest declares at least one class".into());
        }
        if self.mappings.len() > MAX_MANIFEST_MAPPINGS || self.cases.len() > MAX_MANIFEST_CASES {
            return bad("too many mappings or cases".into());
        }
        let mut seen = BTreeSet::new();
        for class in &self.classes {
            if class.name.trim().is_empty() || !seen.insert(class.name.as_str()) {
                return bad(format!("class '{}' is empty or declared twice", class.name));
            }
        }
        let mut resolved = self.clone();
        for mapping in &self.mappings {
            let target = mapping.target_property_or_relation.as_str();
            if let Some(class) = self.classes.iter().find(|c| c.name == mapping.target_class) {
                if mapping.selector.trim().is_empty()
                    || !(class.properties.iter().any(|p| p == target)
                        || class.relations.iter().any(|r| r == target))
                {
                    return Err(ManifestError::MappingInvalid(format!(
                        "mapping target '{}' is not declared on class '{}'",
                        target, class.name
                    )));
                }
                continue;
            }
            if !composed.has_class(&mapping.target_class) {
                return Err(ManifestError::ClassUnknown(format!(
                    "mapping targets class '{}' absent from the graph's composed ontology",
                    mapping.target_class
                )));
            }
            let relation = composed.has_relation(target);
            if mapping.selector.trim().is_empty() || !(relation || composed.has_property(target)) {
                return Err(ManifestError::MappingInvalid(format!(
                    "mapping target '{}' is not a property or relation of the composed ontology",
                    target
                )));
            }
            let entry = match resolved
                .classes
                .iter()
                .position(|c| c.name == mapping.target_class)
            {
                Some(index) => &mut resolved.classes[index],
                None => {
                    resolved.classes.push(ManifestClass {
                        name: mapping.target_class.clone(),
                        properties: Vec::new(),
                        relations: Vec::new(),
                    });
                    resolved.classes.last_mut().expect("just pushed")
                }
            };
            let slot = if relation {
                &mut entry.relations
            } else {
                &mut entry.properties
            };
            if !slot.iter().any(|t| t == target) {
                slot.push(target.to_owned());
            }
        }
        Ok(resolved)
    }

    /// Whether `target` is a relation (not a property) of `class`.
    pub fn is_relation(&self, class: &str, target: &str) -> bool {
        self.classes
            .iter()
            .find(|c| c.name == class)
            .is_some_and(|c| c.relations.iter().any(|r| r == target))
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One markdown source whose headings become fragments of an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MarkdownSource {
    pub artifact_id: String,
    pub text: String,
}

/// What one request evaluates: the manifest, its source rows and markdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DomainPackInput {
    pub manifest: DomainPackManifest,
    #[serde(default)]
    pub rows: Vec<SourceRow>,
    #[serde(default)]
    pub markdown: Vec<MarkdownSource>,
}

impl DomainPackInput {
    /// Check the request's own bounds (row count and size, markdown count).
    pub fn check_bounds(&self) -> Result<(), ManifestError> {
        if self.rows.len() > MAX_PACK_ROWS || self.markdown.len() > MAX_MARKDOWN_SOURCES {
            return Err(ManifestError::Invalid(format!(
                "at most {MAX_PACK_ROWS} rows and {MAX_MARKDOWN_SOURCES} markdown sources"
            )));
        }
        for row in &self.rows {
            let size = serde_json::to_vec(row)
                .map(|b| b.len())
                .unwrap_or(usize::MAX);
            if row.row_id.trim().is_empty() || size > MAX_PACK_ROW_BYTES {
                return Err(ManifestError::Invalid(format!(
                    "row '{}' is unnamed or over {MAX_PACK_ROW_BYTES} bytes",
                    row.row_id
                )));
            }
        }
        Ok(())
    }
}

/// One domain pack operation (`Method::DomainPack`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DomainPackOp {
    /// Evaluate the mappings and return the proposed facts; writes nothing.
    Preview { input: DomainPackInput },
    /// Evaluate, then commit the facts as one durable batch.
    Run { input: DomainPackInput },
}

impl DomainPackOp {
    /// Whether this operation changes durable state.
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Run { .. })
    }
}

/// The answer of [`DomainPackOp::Preview`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DomainPackPreview {
    pub pack_id: String,
    pub version: String,
    /// How many of the manifest's own cases evaluated to their expected facts.
    pub cases_passed: u32,
    pub facts: Vec<ProposedFact>,
}

/// The answer of [`DomainPackOp::Run`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DomainPackRun {
    pub pack_id: String,
    pub version: String,
    pub entities_written: u64,
    pub relationships_written: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> DomainPackManifest {
        let mut manifest = DomainPackManifest {
            schema_version: 1,
            pack_id: "hr".into(),
            version: "1".into(),
            integrity_sha256: String::new(),
            classes: vec![ManifestClass {
                name: "Person".into(),
                properties: vec!["name".into()],
                relations: vec!["reportsTo".into()],
            }],
            mappings: vec![ManifestMapping {
                source_kind: SourceKind::Column,
                selector: "name".into(),
                target_class: "Person".into(),
                target_property_or_relation: "name".into(),
            }],
            cases: vec![],
        };
        manifest.integrity_sha256 = manifest.compute_integrity();
        manifest
    }

    // spec: MEDULLA-TYPED-PACKS-R097.700
    #[test]
    fn integrity_detects_a_changed_body_and_validate_checks_targets() {
        let mut manifest = sample();
        manifest.verify_integrity().unwrap();
        manifest.resolve(&ComposedVocabulary::default()).unwrap();
        manifest.version = "2".into();
        assert!(matches!(
            manifest.verify_integrity(),
            Err(ManifestError::IntegrityMismatch { .. })
        ));
        manifest.mappings[0].target_property_or_relation = "undeclared".into();
        assert!(matches!(
            manifest.resolve(&ComposedVocabulary::default()),
            Err(ManifestError::MappingInvalid(_))
        ));
        manifest.mappings[0].target_property_or_relation = "name".into();
        manifest.mappings[0].target_class = "Ghost".into();
        assert!(matches!(
            manifest.resolve(&ComposedVocabulary::default()),
            Err(ManifestError::ClassUnknown(_))
        ));
    }

    // spec: MEDULLA-TYPED-PACKS-R097.701
    #[test]
    fn a_mapping_resolves_against_the_composed_ontology_not_only_the_manifest() {
        use crate::graph_schema::GraphSchemaTermKind as Kind;
        let composed = ComposedVocabulary::from_terms([
            (
                "https://knuckles-team.github.io/ontology/core#Document",
                Kind::Class,
            ),
            (
                "https://knuckles-team.github.io/ontology/core#title",
                Kind::DatatypeProperty,
            ),
            (
                "https://knuckles-team.github.io/ontology/core#cites",
                Kind::ObjectProperty,
            ),
        ]);
        let mut manifest = sample();
        manifest.mappings[0].target_class = "Document".into();
        manifest.mappings[0].target_property_or_relation = "title".into();
        let resolved = manifest.resolve(&composed).unwrap();
        let document = resolved
            .classes
            .iter()
            .find(|c| c.name == "Document")
            .unwrap();
        assert_eq!(document.properties, ["title"]);
        assert!(!resolved.is_relation("Document", "title"));

        manifest.mappings[0].target_property_or_relation = "cites".into();
        let resolved = manifest.resolve(&composed).unwrap();
        assert!(resolved.is_relation("Document", "cites"));

        manifest.mappings[0].target_property_or_relation = "nothing".into();
        assert!(matches!(
            manifest.resolve(&composed),
            Err(ManifestError::MappingInvalid(_))
        ));
        manifest.mappings[0].target_class = "Ghost".into();
        assert!(matches!(
            manifest.resolve(&composed),
            Err(ManifestError::ClassUnknown(_))
        ));
    }
}
