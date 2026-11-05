//! Business glossary term model (MEDULLA-TYPED-PACKS-R801.1).
//!
//! Pure data. Each field maps to one SKOS / Dublin Core predicate on a
//! `cat:GlossaryTerm` node; served wiring is `handle_glossary` (R801.3).

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
pub const CAT_GLOSSARY_TERM: &str = "http://knuckles.team/kg/catalog#GlossaryTerm";

/// Where a glossary term is in its review lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GlossaryTermStatus {
    Draft,
    InReview,
    Approved,
    Deprecated,
}

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
}

/// One `(predicate, object)` pair of a term's SKOS projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkosObject {
    Literal(String),
    Iri(String),
}

impl GlossaryTerm {
    /// The SKOS predicates this term projects onto its `cat:GlossaryTerm` node.
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
        out
    }
}
