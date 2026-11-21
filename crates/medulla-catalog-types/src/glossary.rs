//! Business glossary term model (MEDULLA-TYPED-PACKS-R801.1).
//!
//! Pure data. Each field maps to one SKOS / Dublin Core predicate on a
//! `cat:CatalogGlossaryTerm` node; served wiring is `handle_glossary` (R801.3).

use serde::{Deserialize, Serialize};

/// `skos:prefLabel`.
pub const SKOS_PREF_LABEL: &str = "http://www.w3.org/2004/02/skos/core#prefLabel";
/// `skos:altLabel`.
pub const SKOS_ALT_LABEL: &str = "http://www.w3.org/2004/02/skos/core#altLabel";
/// `skos:definition`.
pub const SKOS_DEFINITION: &str = "http://www.w3.org/2004/02/skos/core#definition";
/// `skos:broader`.
pub const SKOS_BROADER: &str = "http://www.w3.org/2004/02/skos/core#broader";
/// `skos:related`.
pub const SKOS_RELATED: &str = "http://www.w3.org/2004/02/skos/core#related";
/// `skos:inScheme`.
pub const SKOS_IN_SCHEME: &str = "http://www.w3.org/2004/02/skos/core#inScheme";
/// `dct:isReplacedBy`, carried by a deprecated term.
pub const DCT_IS_REPLACED_BY: &str = "http://purl.org/dc/terms/isReplacedBy";
/// The class every term node carries.
pub const CAT_GLOSSARY_TERM: &str = medulla_types::ontology_iri!("catalog#CatalogGlossaryTerm");

pub use medulla_types::glossary_term_machine::GlossaryTermStatus;

/// One business glossary term.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GlossaryTerm {
    /// The concept scheme IRI (`skos:inScheme`).
    pub scheme: String,
    pub pref_label: String,
    #[serde(default)]
    pub alt_labels: Vec<String>,
    pub definition: String,
    #[serde(default)]
    pub broader: Vec<String>,
    #[serde(default)]
    pub related: Vec<String>,
    pub status: GlossaryTermStatus,
    /// `dct:isReplacedBy`: the term a deprecated term names as its replacement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<String>,
}

/// One `(predicate, object)` pair of a term's SKOS projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkosObject {
    Literal(String),
    Iri(String),
}

impl GlossaryTerm {
    /// The SKOS predicates this term projects onto its `cat:CatalogGlossaryTerm` node.
    pub fn skos_pairs(&self) -> Vec<(&'static str, SkosObject)> {
        let mut out = vec![
            (SKOS_IN_SCHEME, SkosObject::Iri(self.scheme.clone())),
            (
                SKOS_PREF_LABEL,
                SkosObject::Literal(self.pref_label.clone()),
            ),
            (
                SKOS_DEFINITION,
                SkosObject::Literal(self.definition.clone()),
            ),
        ];
        out.extend(
            self.alt_labels
                .iter()
                .map(|l| (SKOS_ALT_LABEL, SkosObject::Literal(l.clone()))),
        );
        out.extend(
            self.broader
                .iter()
                .map(|b| (SKOS_BROADER, SkosObject::Iri(b.clone()))),
        );
        out.extend(
            self.related
                .iter()
                .map(|r| (SKOS_RELATED, SkosObject::Iri(r.clone()))),
        );
        if let Some(replacement) = &self.replaced_by {
            out.push((DCT_IS_REPLACED_BY, SkosObject::Iri(replacement.clone())));
        }
        out
    }
}

/// One term of a scheme batch with the IRI that names its node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GlossaryTermEntry {
    /// The term IRI; the graph node id.
    pub iri: String,
    pub term: GlossaryTerm,
}

/// A status move of a term, one per event of `GlossaryTermMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GlossaryTransition {
    /// `draft` to `in_review`.
    Submit,
    /// `in_review` to `approved`.
    Approve,
    /// `approved` to `deprecated`.
    Deprecate,
}

impl GlossaryTransition {
    /// The machine event this move applies.
    pub fn event(self) -> medulla_types::glossary_term_machine::GlossaryTermEvent {
        use medulla_types::glossary_term_machine::GlossaryTermEvent;
        match self {
            Self::Submit => GlossaryTermEvent::Submit,
            Self::Approve => GlossaryTermEvent::Approve,
            Self::Deprecate => GlossaryTermEvent::Deprecate,
        }
    }
}

/// One glossary operation (`Method::Glossary`, MEDULLA-TYPED-PACKS-R801.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GlossaryOp {
    /// Import one concept scheme and its terms as one batch.
    PutScheme {
        scheme: String,
        terms: Vec<GlossaryTermEntry>,
    },
    /// Write one term (an upsert of its node).
    PutTerm { iri: String, term: GlossaryTerm },
    /// Move one term along `GlossaryTermMachine`. `change_request` and the
    /// verified principal are recorded on the term.
    Transition {
        iri: String,
        event: GlossaryTransition,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        change_request: Option<String>,
    },
    /// Link a term to an asset through `:definesAsset`.
    Bind { term: String, asset: String },
}

impl GlossaryOp {
    /// Every glossary operation writes.
    pub fn is_mutation(&self) -> bool {
        true
    }
}

/// The answer of a glossary operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GlossaryReceipt {
    /// The scheme or term IRI the operation addressed.
    pub iri: String,
    /// The term's status after the operation, for term operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<GlossaryTermStatus>,
    /// Nodes written.
    pub written: u64,
}
