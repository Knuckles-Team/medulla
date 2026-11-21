//! Option features for capability ranking, computed by medulla from candidate
//! text (MEDULLA-DECISION-ENGINE-R829.2).
//!
//! For each candidate of a `RankCatalog` the module computes the five
//! features of the capability-ranking schema from the request's `query` and
//! the candidate's `rank.name`, `rank.description` and `rank.kind` texts:
//! `name_overlap`, `description_overlap`, `term_rarity_weighted_overlap`,
//! `name_coverage` and `semantic_similarity`. Terms come from the tokenizer of
//! the text index (`medulla_text::tokenize`), after stopword removal and
//! plural folding. A candidate with no term match scores zero.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::statistical::declared::{DeclaredNumber, DeclaredOption};

/// `Q32` one.
const Q32_ONE: f64 = 4_294_967_296.0;

/// Weight of a name match in the rarity-weighted overlap
/// (`decision.rank.name_weight`, starting value 2.0).
pub const NAME_WEIGHT: f64 = 2.0;
/// Weight of the semantic similarity (`decision.rank.semantic_weight`,
/// starting value 1.0); it is carried for the pinned head's schema and does
/// not enter the lexical features.
pub const SEMANTIC_WEIGHT: f64 = 1.0;

/// Key of the candidate's name text.
pub const NAME_KEY: &str = "rank.name";
/// Key of the candidate's description text.
pub const DESCRIPTION_KEY: &str = "rank.description";
/// Key of the candidate's kind text.
pub const KIND_KEY: &str = "rank.kind";

/// Feature keys, in the order of the capability-ranking schema.
pub const NAME_OVERLAP_KEY: &str = "rank.name_overlap";
pub const DESCRIPTION_OVERLAP_KEY: &str = "rank.description_overlap";
pub const TERM_RARITY_KEY: &str = "rank.term_rarity_weighted_overlap";
pub const NAME_COVERAGE_KEY: &str = "rank.name_coverage";
pub const SEMANTIC_KEY: &str = "rank.semantic_similarity";
/// The item-level score the rank family separates ties by; it is the
/// term-rarity-weighted overlap.
pub const ITEM_SCORE_KEY: &str = "rank.item_score";

/// Words that carry no ranking signal.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "into", "is", "it", "of",
    "on", "or", "that", "the", "this", "to", "with",
];

/// Embeds texts when the embedding model is ready: one vector per text in
/// order, or `None` while the model is not ready.
pub type Embed<'a> = &'a dyn Fn(&[String]) -> Option<Vec<Vec<f32>>>;

/// Fold a plural to its singular form.
fn fold_plural(term: &str) -> String {
    let len = term.chars().count();
    if len > 4 && term.ends_with("ies") {
        format!("{}y", &term[..term.len() - 3])
    } else if len > 4 && term.ends_with("sses") {
        term[..term.len() - 2].to_string()
    } else if len > 3 && term.ends_with('s') && !term.ends_with("ss") && !term.ends_with("us") {
        term[..term.len() - 1].to_string()
    } else {
        term.to_string()
    }
}

/// The folded, stopword-free terms of `text`.
fn terms(text: &str) -> BTreeSet<String> {
    medulla_text::tokenize(text)
        .into_iter()
        .filter(|token| !STOPWORDS.contains(&token.as_str()))
        .map(|token| fold_plural(&token))
        .collect()
}

fn text_of<'a>(option: &'a DeclaredOption, key: &str) -> &'a str {
    option
        .texts
        .iter()
        .find(|t| t.key == key)
        .map_or("", |t| t.text.as_str())
}

fn q32(value: f64) -> i64 {
    (value * Q32_ONE).round() as i64
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| f64::from(*x) * f64::from(*y)).sum();
    let norm = |v: &[f32]| v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
    let (na, nb) = (norm(a), norm(b));
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// The sorted feature numbers of each option of `options`, in option order,
/// for the free text `query`. `embed` is the embedding model when it is
/// ready; with `None`, or when it answers `None`, `semantic_similarity` is
/// absent from every option and the lexical features are unchanged.
pub fn capability_features(
    query: &str,
    options: &[DeclaredOption],
    embed: Option<Embed<'_>>,
) -> Vec<Vec<DeclaredNumber>> {
    let query_terms = terms(query);
    let per_option: Vec<(BTreeSet<String>, BTreeSet<String>, BTreeSet<String>)> = options
        .iter()
        .map(|option| {
            (
                terms(text_of(option, NAME_KEY)),
                terms(text_of(option, DESCRIPTION_KEY)),
                terms(text_of(option, KIND_KEY)),
            )
        })
        .collect();
    let n = options.len() as f64;
    let mut df: BTreeMap<&str, f64> = BTreeMap::new();
    for term in &query_terms {
        let count = per_option
            .iter()
            .filter(|(name, description, kind)| {
                name.contains(term) || description.contains(term) || kind.contains(term)
            })
            .count();
        df.insert(term.as_str(), count as f64);
    }
    let semantic = embed.and_then(|embed| {
        let mut texts = vec![query.to_string()];
        texts.extend(options.iter().map(|option| {
            format!(
                "{} {} {}",
                text_of(option, NAME_KEY),
                text_of(option, DESCRIPTION_KEY),
                text_of(option, KIND_KEY)
            )
        }));
        let vectors = embed(&texts)?;
        (vectors.len() == texts.len()).then_some(vectors)
    });
    per_option
        .iter()
        .enumerate()
        .map(|(index, (name, description, kind))| {
            let mut name_overlap = 0.0;
            let mut description_overlap = 0.0;
            let mut weighted = 0.0;
            for term in &query_terms {
                let idf = (1.0 + (n + 1.0) / (df[term.as_str()] + 1.0)).ln();
                if name.contains(term) {
                    name_overlap += 1.0;
                    weighted += NAME_WEIGHT * idf;
                } else if description.contains(term) || kind.contains(term) {
                    weighted += idf;
                }
                if description.contains(term) {
                    description_overlap += 1.0;
                }
            }
            let coverage = if name.is_empty() {
                0.0
            } else {
                name_overlap / name.len() as f64
            };
            let mut numbers = vec![
                (NAME_OVERLAP_KEY, q32(name_overlap)),
                (DESCRIPTION_OVERLAP_KEY, q32(description_overlap)),
                (TERM_RARITY_KEY, q32(weighted)),
                (ITEM_SCORE_KEY, q32(weighted)),
                (NAME_COVERAGE_KEY, q32(coverage)),
            ];
            if let Some(vectors) = &semantic {
                let similarity = cosine(&vectors[0], &vectors[index + 1]).max(0.0);
                numbers.push((SEMANTIC_KEY, q32(similarity * SEMANTIC_WEIGHT)));
            }
            numbers.sort_by(|a, b| a.0.cmp(b.0));
            numbers
                .into_iter()
                .map(|(key, q32)| DeclaredNumber {
                    key: key.to_string(),
                    q32,
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::statistical::declared::DeclaredText;
    use medulla_types::contract::BoundedVec;

    fn option(id: &str, name: &str, description: &str) -> DeclaredOption {
        DeclaredOption {
            option_id: id.to_string(),
            classification: BoundedVec::default(),
            numbers: BoundedVec::default(),
            texts: BoundedVec::new(vec![
                DeclaredText {
                    key: DESCRIPTION_KEY.to_string(),
                    text: description.to_string(),
                },
                DeclaredText {
                    key: NAME_KEY.to_string(),
                    text: name.to_string(),
                },
            ])
            .unwrap(),
        }
    }

    fn value(numbers: &[DeclaredNumber], key: &str) -> Option<i64> {
        numbers.iter().find(|n| n.key == key).map(|n| n.q32)
    }

    // spec: MEDULLA-DECISION-ENGINE-R829.2
    #[test]
    fn name_match_counts_double_and_rare_terms_weigh_more() {
        let options = vec![
            option("a", "seerr_list_requests", "lists media requests"),
            option("b", "other_tool", "lists pending media requests"),
            option("c", "third_tool", "unrelated text"),
            option("d", "fourth_tool", "media things"),
        ];
        let out = capability_features("list requests in seerr", &options, None);
        let weighted = |i: usize| value(&out[i], TERM_RARITY_KEY).unwrap();
        // A name match outweighs the same match in the description.
        assert!(weighted(0) > weighted(1));
        // No term match scores zero.
        assert_eq!(weighted(2), 0);
        assert_eq!(value(&out[2], NAME_OVERLAP_KEY), Some(0));
        // Plural folding: "requests" matches the query term "requests"/"request".
        assert_eq!(value(&out[0], NAME_OVERLAP_KEY), Some(3 << 32));
        // The rare term "seerr" (df 1) weighs more than "request" (df 2).
        let seerr_only = capability_features("seerr", &options, None);
        let request_only = capability_features("requests", &options, None);
        assert!(
            value(&seerr_only[0], TERM_RARITY_KEY).unwrap()
                > value(&request_only[0], TERM_RARITY_KEY).unwrap()
        );
        // Coverage is the share of the name's terms matched.
        assert_eq!(value(&out[0], NAME_COVERAGE_KEY), Some(q32(1.0)));
        // The item score is the weighted overlap.
        assert_eq!(value(&out[0], ITEM_SCORE_KEY), Some(weighted(0)));
        // Keys are sorted, as a declared option requires.
        assert!(out[0].windows(2).all(|w| w[0].key < w[1].key));
    }

    // spec: MEDULLA-DECISION-ENGINE-R829.2
    #[test]
    fn semantic_absent_when_embedding_model_not_ready() {
        let options = vec![option("a", "seerr_list_requests", "lists requests")];
        let not_ready = |_: &[String]| -> Option<Vec<Vec<f32>>> { None };
        let absent = capability_features("list requests", &options, Some(&not_ready as Embed<'_>));
        assert_eq!(value(&absent[0], SEMANTIC_KEY), None);
        let without = capability_features("list requests", &options, None);
        assert_eq!(absent, without);
        let ready = |texts: &[String]| -> Option<Vec<Vec<f32>>> {
            Some(texts.iter().map(|_| vec![1.0_f32, 0.0]).collect())
        };
        let present = capability_features("list requests", &options, Some(&ready as Embed<'_>));
        assert_eq!(value(&present[0], SEMANTIC_KEY), Some(q32(1.0)));
        // The lexical order is unchanged by the semantic feature.
        assert_eq!(
            value(&present[0], TERM_RARITY_KEY),
            value(&absent[0], TERM_RARITY_KEY)
        );
    }
}
