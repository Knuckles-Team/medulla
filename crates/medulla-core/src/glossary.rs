//! Glossary rules (MEDULLA-TYPED-PACKS-R801.2).
//!
//! Pure functions over the records in `medulla_catalog_types::glossary`: binding a
//! term to an asset through `:definesAsset`, label search over preferred and
//! alternative labels, the deprecated-term replacement, the `broader` cycle check and
//! the `PutScheme` batch bound. No I/O. The served handler and the `catalog_relevance`
//! UDF import this module and never each other.

use std::collections::{BTreeMap, HashMap, HashSet};

use medulla_catalog_types::asset_usage::UsageKnob;
use medulla_catalog_types::glossary::{GlossaryTerm, GlossaryTermStatus};

/// `:definesAsset`: links a term to an asset that serves the class the term names.
pub const DEFINES_ASSET: &str = medulla_types::ontology_iri!("core#definesAsset");

/// `knob glossary.broader_depth_max`: hops the `broader` cycle check walks.
pub const GLOSSARY_BROADER_DEPTH_MAX: UsageKnob = UsageKnob {
    name: "glossary.broader_depth_max",
    default: 64,
    range: (4, 256),
    env: "MEDULLA_GLOSSARY_BROADER_DEPTH_MAX",
};
/// `knob glossary.scheme_batch_max`: terms one `PutScheme` carries.
pub const GLOSSARY_SCHEME_BATCH_MAX: UsageKnob = UsageKnob {
    name: "glossary.scheme_batch_max",
    default: 5000,
    range: (100, 50_000),
    env: "MEDULLA_GLOSSARY_SCHEME_BATCH_MAX",
};

/// Why a glossary operation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlossaryError {
    /// The term is deprecated; bind to its replacement instead.
    Deprecated { replaced_by: Option<String> },
    /// The `broader` links of this term IRI lead back to itself.
    BroaderCycle(String),
    /// A `broader` chain is longer than `glossary.broader_depth_max`.
    BroaderTooDeep { term: String, max: u64 },
    /// A scheme batch carries more terms than `glossary.scheme_batch_max`.
    SchemeBatchTooLarge { terms: usize, max: u64 },
    /// A term of the batch names another scheme.
    SchemeMismatch { term: String, scheme: String },
    /// Another approved term of the scheme already carries this preferred label.
    LabelTaken { label: String, term: String },
    /// The term moves to `approved` without an approved change request naming it.
    Unreviewed { term: String },
}

impl GlossaryError {
    /// The registered refusal code of this error, when it has one; the rest are
    /// `INVALID_ARGUMENT`.
    pub fn code(&self) -> Option<medulla_types::contract::ServerErrorCode> {
        use medulla_types::contract::ServerErrorCode;
        match self {
            Self::BroaderCycle(_) => Some(ServerErrorCode::GlossaryCycle),
            Self::LabelTaken { .. } => Some(ServerErrorCode::GlossaryLabelTaken),
            Self::Unreviewed { .. } => Some(ServerErrorCode::GlossaryUnreviewed),
            _ => None,
        }
    }
}

impl std::fmt::Display for GlossaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deprecated { replaced_by } => match replaced_by {
                Some(r) => write!(f, "term is deprecated; use {r}"),
                None => f.write_str("term is deprecated"),
            },
            Self::BroaderCycle(t) => write!(f, "broader links of {t} form a cycle"),
            Self::BroaderTooDeep { term, max } => {
                write!(f, "broader chain of {term} is deeper than {max} hops")
            }
            Self::SchemeBatchTooLarge { terms, max } => {
                write!(f, "scheme batch of {terms} terms exceeds {max}")
            }
            Self::SchemeMismatch { term, scheme } => {
                write!(f, "term {term} is not in scheme {scheme}")
            }
            Self::LabelTaken { label, term } => {
                write!(f, "the approved term {term} already carries the label '{label}'")
            }
            Self::Unreviewed { term } => {
                write!(f, "term {term} needs an approved change request to be approved")
            }
        }
    }
}

impl std::error::Error for GlossaryError {}

/// One `(term, :definesAsset, asset)` link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinesAssetLink {
    pub term: String,
    pub predicate: &'static str,
    pub asset: String,
}

/// Bind the term `term_iri` to `asset`. A deprecated term is refused and names its
/// replacement.
pub fn bind(
    term_iri: &str,
    term: &GlossaryTerm,
    asset: &str,
) -> Result<DefinesAssetLink, GlossaryError> {
    if term.status == GlossaryTermStatus::Deprecated {
        return Err(GlossaryError::Deprecated {
            replaced_by: term.replaced_by.clone(),
        });
    }
    Ok(DefinesAssetLink {
        term: term_iri.to_string(),
        predicate: DEFINES_ASSET,
        asset: asset.to_string(),
    })
}

/// The replacement a deprecated term names; `None` for a term that is not deprecated.
pub fn replacement(term: &GlossaryTerm) -> Option<&str> {
    if term.status == GlossaryTermStatus::Deprecated {
        term.replaced_by.as_deref()
    } else {
        None
    }
}

/// The lowercase words of a label or a query: runs of alphanumeric characters.
fn label_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// A word index over the preferred and alternative labels of a term list
/// (MEDULLA-TYPED-PACKS-R801.6). Each word of each label posts the terms that carry it, in a
/// sorted map, so a query word's prefix is one range scan and a search verifies only the
/// terms of the rarest query word with [`label_matches`] instead of scanning every term.
/// Build once per term list; a search never reads a term that no query word can reach.
#[derive(Debug, Clone, Default)]
pub struct GlossaryLabelIndex {
    postings: BTreeMap<String, Vec<u32>>,
    terms: usize,
}

impl GlossaryLabelIndex {
    /// Index `terms`. A term past `u32::MAX` is not indexed (a scheme batch is far smaller).
    pub fn build(terms: &[GlossaryTerm]) -> Self {
        let mut postings: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        for (position, term) in terms.iter().enumerate() {
            let Ok(position) = u32::try_from(position) else {
                break;
            };
            for label in std::iter::once(&term.pref_label).chain(term.alt_labels.iter()) {
                for word in label_words(label) {
                    let posted = postings.entry(word).or_default();
                    if posted.last() != Some(&position) {
                        posted.push(position);
                    }
                }
            }
        }
        Self {
            postings,
            terms: terms.len(),
        }
    }

    /// The postings of every indexed word that starts with `prefix`.
    fn prefix_postings<'a>(&'a self, prefix: &str) -> impl Iterator<Item = &'a Vec<u32>> {
        let owned = prefix.to_string();
        self.postings
            .range(owned.clone()..)
            .take_while(move |(word, _)| word.starts_with(owned.as_str()))
            .map(|(_, posted)| posted)
    }

    /// The positions in `terms` whose labels match `query` exactly as [`label_matches`] does,
    /// in ascending order. `terms` is the list the index was built over.
    pub fn search(&self, terms: &[GlossaryTerm], query: &str) -> Vec<usize> {
        let wanted = label_words(query);
        if wanted.is_empty() || terms.len() != self.terms {
            return Vec::new();
        }
        // The query word that reaches the fewest postings bounds the candidates.
        let rarest = wanted
            .iter()
            .min_by_key(|word| {
                self.prefix_postings(word)
                    .map(Vec::len)
                    .fold(0usize, usize::saturating_add)
            })
            .expect("a non-empty query has a word");
        let mut candidates: Vec<u32> = self
            .prefix_postings(rarest)
            .flat_map(|posted| posted.iter().copied())
            .collect();
        candidates.sort_unstable();
        candidates.dedup();
        candidates
            .into_iter()
            .map(|position| position as usize)
            .filter(|position| {
                terms
                    .get(*position)
                    .is_some_and(|term| label_matches(term, query))
            })
            .collect()
    }
}

/// True when every word of `query` equals or starts a word of the preferred label or of an
/// alternative label. Case is ignored; a blank query matches nothing.
pub fn label_matches(term: &GlossaryTerm, query: &str) -> bool {
    let wanted = label_words(query);
    if wanted.is_empty() {
        return false;
    }
    std::iter::once(&term.pref_label)
        .chain(term.alt_labels.iter())
        .any(|label| {
            let have = label_words(label);
            wanted
                .iter()
                .all(|w| have.iter().any(|h| h.starts_with(w.as_str())))
        })
}

/// Refuse a `broader` cycle among `terms` (`(term IRI, term)`). The walk is iterative and
/// follows at most `max_depth` hops from each term.
pub fn check_broader_acyclic(
    terms: &[(String, GlossaryTerm)],
    max_depth: u64,
) -> Result<(), GlossaryError> {
    let graph: HashMap<&str, &[String]> = terms
        .iter()
        .map(|(iri, t)| (iri.as_str(), t.broader.as_slice()))
        .collect();
    let mut done: HashSet<&str> = HashSet::new();
    for (start, _) in terms {
        if done.contains(start.as_str()) {
            continue;
        }
        let mut on_path: HashSet<&str> = HashSet::new();
        on_path.insert(start.as_str());
        // (node, next broader index)
        let mut stack: Vec<(&str, usize)> = vec![(start.as_str(), 0)];
        while let Some((node, idx)) = stack.pop() {
            let parents = graph.get(node).copied().unwrap_or(&[]);
            if idx >= parents.len() {
                on_path.remove(node);
                done.insert(node);
                continue;
            }
            stack.push((node, idx + 1));
            let parent = parents[idx].as_str();
            if on_path.contains(parent) {
                return Err(GlossaryError::BroaderCycle(parent.to_string()));
            }
            if done.contains(parent) || !graph.contains_key(parent) {
                continue;
            }
            if stack.len() as u64 > max_depth {
                return Err(GlossaryError::BroaderTooDeep {
                    term: start.clone(),
                    max: max_depth,
                });
            }
            on_path.insert(parent);
            stack.push((parent, 0));
        }
    }
    Ok(())
}

/// Refuse a `broader` cycle among the `stored` terms of a scheme with `incoming` written over
/// them (an incoming term replaces the stored term of the same IRI).
pub fn check_broader_acyclic_over(
    stored: Vec<(String, GlossaryTerm)>,
    incoming: &[(String, GlossaryTerm)],
) -> Result<(), GlossaryError> {
    let mut all: Vec<(String, GlossaryTerm)> = stored
        .into_iter()
        .filter(|(iri, _)| !incoming.iter().any(|(other, _)| other == iri))
        .collect();
    all.extend(incoming.iter().cloned());
    check_broader_acyclic(&all, GLOSSARY_BROADER_DEPTH_MAX.value())
}

/// Refuse approving `term` (stored as `iri`) while another approved term of the scheme carries
/// the same preferred label, compared without case and surrounding space.
pub fn check_label_free(
    stored: &[(String, GlossaryTerm)],
    iri: &str,
    term: &GlossaryTerm,
) -> Result<(), GlossaryError> {
    let label = term.pref_label.trim().to_lowercase();
    match stored.iter().find(|(other, t)| {
        other != iri
            && t.scheme == term.scheme
            && t.status == GlossaryTermStatus::Approved
            && t.pref_label.trim().to_lowercase() == label
    }) {
        Some((other, _)) => Err(GlossaryError::LabelTaken {
            label: term.pref_label.clone(),
            term: other.clone(),
        }),
        None => Ok(()),
    }
}

/// Refuse approving the term `iri` unless `request` is an approved or applied change request
/// whose target is that term.
pub fn require_review(
    iri: &str,
    request: Option<&medulla_catalog_types::stewardship::ChangeRequest>,
) -> Result<(), GlossaryError> {
    use medulla_catalog_types::stewardship::ChangeRequestState;
    match request {
        Some(request)
            if request.target == iri
                && matches!(
                    request.state,
                    ChangeRequestState::Approved | ChangeRequestState::Applied
                ) =>
        {
            Ok(())
        }
        _ => Err(GlossaryError::Unreviewed {
            term: iri.to_string(),
        }),
    }
}

/// The `PutScheme` payload: one concept scheme and its terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutScheme {
    pub scheme: String,
    /// `(term IRI, term)`.
    pub terms: Vec<(String, GlossaryTerm)>,
}

impl PutScheme {
    /// Check the batch bound, that every term is in the scheme, and that `broader` is acyclic.
    pub fn validate(&self) -> Result<(), GlossaryError> {
        let max = GLOSSARY_SCHEME_BATCH_MAX.value();
        if self.terms.len() as u64 > max {
            return Err(GlossaryError::SchemeBatchTooLarge {
                terms: self.terms.len(),
                max,
            });
        }
        if let Some((iri, _)) = self.terms.iter().find(|(_, t)| t.scheme != self.scheme) {
            return Err(GlossaryError::SchemeMismatch {
                term: iri.clone(),
                scheme: self.scheme.clone(),
            });
        }
        check_broader_acyclic(&self.terms, GLOSSARY_BROADER_DEPTH_MAX.value())
    }
}
