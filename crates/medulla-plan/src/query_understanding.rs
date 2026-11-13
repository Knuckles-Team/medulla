//! Query understanding model (MEDULLA-FEDERATED-QUERY-R651.1) and the model-free
//! analysis behind it (MEDULLA-FEDERATED-QUERY-R651.2).
//!
//! The typed filters, relational intent and the one expansion port's request
//! and result shapes that served `Discover` mode `deep` hands to `understand`.
//! Stopping is `StopVerdict` (MEDULLA-FEDERATED-QUERY-R082); no stop type
//! lives here.
//!
//! `understand` splits a search query into structured filters (`key:value`
//! tokens with keys `type`, `source`, `tag`, `after`, `before`), exact quoted
//! phrases, free-text terms and an optional relational intent. No model is
//! called and nothing is written. Cost is linear in the query length (times
//! the fixed relationship vocabulary of the caller's graph schema).

use medulla_result_types::retrieval_verdict::TermFeedback;
use serde::{Deserialize, Serialize};

/// Largest query text an expansion request accepts, in bytes.
pub const MAX_QUERY_BYTES: usize = 2_048;
/// Smallest and largest number of perspectives one request may ask for.
pub const PERSPECTIVES_MIN: u8 = 1;
pub const PERSPECTIVES_MAX: u8 = 8;

/// Structured filters lifted out of the query text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryFilters {
    pub types: Vec<String>,
    pub sources: Vec<String>,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

/// A subject-relation-object reading of the query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationalIntent {
    pub subject: String,
    pub relation: String,
    pub object: String,
}

/// One relationship of the caller's graph schema: its stored name and, when
/// the ontology declares one, its `rdfs:label`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationshipName {
    pub name: String,
    pub label: Option<String>,
}

impl RelationshipName {
    pub fn new(name: impl Into<String>, label: Option<String>) -> Self {
        Self {
            name: name.into(),
            label,
        }
    }
}

/// The model-free analysis of one query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryAnalysis {
    pub filters: QueryFilters,
    /// Quoted phrases, verbatim (without the quotes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phrases: Vec<String>,
    /// Free-text terms left after filters and phrases, in query order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terms: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<RelationalIntent>,
}

/// Analyse `text` against the relationship vocabulary of the caller's graph.
pub fn understand(text: &str, relationships: &[RelationshipName]) -> QueryAnalysis {
    let mut analysis = QueryAnalysis::default();
    for piece in split_pieces(text) {
        match piece {
            Piece::Phrase(phrase) => {
                if !phrase.is_empty() {
                    analysis.phrases.push(phrase.to_string());
                }
            }
            Piece::Word(word) => {
                if !apply_filter(&mut analysis.filters, word) {
                    analysis.terms.push(word.to_string());
                }
            }
        }
    }
    analysis.intent = relational_intent(&analysis.terms, relationships);
    analysis
}

enum Piece<'a> {
    Word(&'a str),
    Phrase(&'a str),
}

/// One left-to-right pass: whitespace-separated words, and `"..."` phrases
/// (an unterminated quote runs to the end of the text).
fn split_pieces(text: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut rest = text;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return pieces;
        }
        if let Some(after_quote) = rest.strip_prefix('"') {
            let end = after_quote.find('"').unwrap_or(after_quote.len());
            pieces.push(Piece::Phrase(after_quote[..end].trim()));
            rest = after_quote.get(end + 1..).unwrap_or("");
            continue;
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '"')
            .unwrap_or(rest.len());
        pieces.push(Piece::Word(&rest[..end]));
        rest = &rest[end..];
    }
}

/// Record `word` as a filter when it is a well-formed `key:value` token.
fn apply_filter(filters: &mut QueryFilters, word: &str) -> bool {
    let Some((key, value)) = word.split_once(':') else {
        return false;
    };
    if value.is_empty() {
        return false;
    }
    match key {
        "type" => filters.types.push(value.to_string()),
        "source" => filters.sources.push(value.to_string()),
        "tag" => filters.tags.push(value.to_string()),
        "after" if is_date(value) => filters.after = Some(value.to_string()),
        "before" if is_date(value) => filters.before = Some(value.to_string()),
        _ => return false,
    }
    true
}

/// `YYYY-MM-DD` with a month 01..=12 and a day 01..=31.
fn is_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    if !(digits(0..4) && digits(5..7) && digits(8..10)) {
        return false;
    }
    let month: u8 = value[5..7].parse().unwrap_or(0);
    let day: u8 = value[8..10].parse().unwrap_or(0);
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

/// The lowercase word sequence of a relationship name or label:
/// `assignedTo`, `ASSIGNED_TO` and `assigned to` all give `[assigned, to]`.
fn name_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            previous_lower = false;
            continue;
        }
        if c.is_uppercase() && previous_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        previous_lower = c.is_lowercase() || c.is_ascii_digit();
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// A relational intent exists when exactly one relationship occurrence is
/// found among the terms and it has a non-empty span on each side.
fn relational_intent(
    terms: &[String],
    relationships: &[RelationshipName],
) -> Option<RelationalIntent> {
    let lowered: Vec<String> = terms.iter().map(|term| term.to_lowercase()).collect();
    let mut found: Option<(usize, usize, &str)> = None;
    for relationship in relationships {
        let mut spellings = vec![name_words(&relationship.name)];
        if let Some(label) = &relationship.label {
            spellings.push(name_words(label));
        }
        spellings.retain(|words| !words.is_empty());
        spellings.dedup();
        for start in 0..lowered.len() {
            let hit = spellings
                .iter()
                .filter(|words| lowered[start..].starts_with(words.as_slice()))
                .map(Vec::len)
                .max();
            if let Some(len) = hit {
                if found.is_some() {
                    return None;
                }
                found = Some((start, len, relationship.name.as_str()));
            }
        }
    }
    let (start, len, relationship) = found?;
    let subject = terms[..start].join(" ");
    let object = terms[start + len..].join(" ");
    if subject.is_empty() || object.is_empty() {
        return None;
    }
    Some(RelationalIntent {
        subject,
        relation: relationship.to_string(),
        object,
    })
}

/// One request to the expansion port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpansionRequest {
    pub query: String,
    pub perspectives: u8,
    #[serde(default)]
    pub feedback: Vec<TermFeedback>,
}

impl ExpansionRequest {
    /// Refuse an empty or oversized query and a perspective count outside 1 to 8.
    pub fn validate(&self) -> Result<(), ExpansionError> {
        if self.query.trim().is_empty() {
            return Err(ExpansionError::EmptyQuery);
        }
        if self.query.len() > MAX_QUERY_BYTES {
            return Err(ExpansionError::QueryTooLong {
                bytes: self.query.len(),
            });
        }
        if !(PERSPECTIVES_MIN..=PERSPECTIVES_MAX).contains(&self.perspectives) {
            return Err(ExpansionError::PerspectivesOutOfRange {
                perspectives: self.perspectives,
            });
        }
        Ok(())
    }
}

/// The closed set of expansion kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpansionKind {
    Hypothetical,
    SubQuery,
    Perspective,
}

/// One expansion the port produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expansion {
    pub text: String,
    pub kind: ExpansionKind,
    pub digest: String,
}

/// Why an expansion request was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ExpansionError {
    EmptyQuery,
    QueryTooLong {
        bytes: usize,
    },
    PerspectivesOutOfRange {
        perspectives: u8,
    },
    /// The provider answered with more expansions than the request allowed.
    FanoutExceeded {
        requested: u8,
        returned: usize,
    },
    /// No provider is installed (`EXPANSION_PROVIDER_UNCONFIGURED`).
    ProviderUnconfigured,
    /// The provider failed; carries its error text (`EXPANSION_PROVIDER_FAILED`).
    ProviderFailed {
        message: String,
    },
}

impl std::fmt::Display for ExpansionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyQuery => write!(f, "expansion query is empty"),
            Self::QueryTooLong { bytes } => {
                write!(
                    f,
                    "expansion query is {bytes} bytes, over {MAX_QUERY_BYTES}"
                )
            }
            Self::PerspectivesOutOfRange { perspectives } => write!(
                f,
                "perspectives {perspectives} outside {PERSPECTIVES_MIN} to {PERSPECTIVES_MAX}"
            ),
            Self::FanoutExceeded {
                requested,
                returned,
            } => write!(
                f,
                "expansion provider returned {returned} items for a request of {requested}"
            ),
            Self::ProviderUnconfigured => write!(f, "no expansion provider is configured"),
            Self::ProviderFailed { message } => {
                write!(f, "expansion provider failed: {message}")
            }
        }
    }
}

impl std::error::Error for ExpansionError {}

/// The one expansion port (MEDULLA-FEDERATED-QUERY-R651.3). Its output is data and
/// never a decision. Callers: `plan_discover` and the feedback operator.
pub trait ExpansionProvider: Send + Sync {
    /// Produce at most `req.perspectives` expansions for `req.query`.
    fn expand(&self, req: &ExpansionRequest) -> Result<Vec<Expansion>, ExpansionError>;
}

static EXPANSION_PROVIDER: std::sync::OnceLock<std::sync::Arc<dyn ExpansionProvider>> =
    std::sync::OnceLock::new();

/// Install the process-wide expansion provider at server start. Set once; a second
/// call is ignored so the wiring is stable for the process lifetime.
pub fn set_expansion_provider(provider: std::sync::Arc<dyn ExpansionProvider>) {
    let _ = EXPANSION_PROVIDER.set(provider);
}

/// The installed provider, or `None` when the deployment configures none.
pub fn expansion_provider() -> Option<std::sync::Arc<dyn ExpansionProvider>> {
    EXPANSION_PROVIDER.get().cloned()
}

/// Run the port: validate the request, call the installed provider and refuse an
/// answer larger than the request. No provider is the typed `ProviderUnconfigured`.
pub fn run_expansion(req: &ExpansionRequest) -> Result<Vec<Expansion>, ExpansionError> {
    req.validate()?;
    let provider = expansion_provider().ok_or(ExpansionError::ProviderUnconfigured)?;
    let out = provider.expand(req)?;
    if out.len() > usize::from(req.perspectives) {
        return Err(ExpansionError::FanoutExceeded {
            requested: req.perspectives,
            returned: out.len(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_result_types::retrieval_verdict::FeedbackTier;

    fn round_trip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, value);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R651.1
    #[test]
    fn types_round_trip_and_reject_unknown_kind() {
        round_trip(&QueryFilters {
            types: vec!["Document".into()],
            sources: vec!["wiki".into()],
            tags: vec!["ops".into()],
            after: Some("2026-01-01".into()),
            before: None,
        });
        round_trip(&RelationalIntent {
            subject: "a".into(),
            relation: "founded".into(),
            object: "b".into(),
        });
        let request = ExpansionRequest {
            query: "who founded b".into(),
            perspectives: 3,
            feedback: vec![TermFeedback {
                term: "founder".into(),
                tier: FeedbackTier::Reinforce,
                weight: 0.5,
                evidence_document_ids: vec!["d1".into()],
            }],
        };
        round_trip(&request);
        assert_eq!(request.validate(), Ok(()));
        for kind in [
            ExpansionKind::Hypothetical,
            ExpansionKind::SubQuery,
            ExpansionKind::Perspective,
        ] {
            round_trip(&Expansion {
                text: "x".into(),
                kind,
                digest: "0".repeat(64),
            });
        }
        assert_eq!(
            serde_json::to_value(ExpansionKind::SubQuery).unwrap(),
            serde_json::json!("sub_query")
        );
        round_trip(&ExpansionError::QueryTooLong { bytes: 3_000 });

        let unknown = r#"{"text":"x","kind":"paraphrase","digest":"d"}"#;
        assert!(serde_json::from_str::<Expansion>(unknown).is_err());

        for bad in [0, 9] {
            let r = ExpansionRequest {
                perspectives: bad,
                ..request.clone()
            };
            assert_eq!(
                r.validate(),
                Err(ExpansionError::PerspectivesOutOfRange { perspectives: bad })
            );
        }
        let long = ExpansionRequest {
            query: "q".repeat(MAX_QUERY_BYTES + 1),
            ..request.clone()
        };
        assert_eq!(
            long.validate(),
            Err(ExpansionError::QueryTooLong {
                bytes: MAX_QUERY_BYTES + 1
            })
        );
        let edge = ExpansionRequest {
            query: "q".repeat(MAX_QUERY_BYTES),
            perspectives: 8,
            ..request
        };
        assert_eq!(edge.validate(), Ok(()));
    }

    fn schema() -> Vec<RelationshipName> {
        vec![
            RelationshipName::new("assignedTo", Some("assigned to".into())),
            RelationshipName::new("WORKS_AT", None),
        ]
    }

    // spec: MEDULLA-FEDERATED-QUERY-R651.2
    #[test]
    fn filters_phrases_and_single_relation_intent() {
        let analysis = understand(
            r#"type:Ticket source:jira tag:p1 after:2026-01-02 before:2026-13-01 "login page" alice assigned to bob"#,
            &schema(),
        );
        assert_eq!(analysis.filters.types, vec!["Ticket"]);
        assert_eq!(analysis.filters.sources, vec!["jira"]);
        assert_eq!(analysis.filters.tags, vec!["p1"]);
        assert_eq!(analysis.filters.after.as_deref(), Some("2026-01-02"));
        // An invalid date is not a filter; it stays a free-text term.
        assert_eq!(analysis.filters.before, None);
        assert_eq!(analysis.phrases, vec!["login page"]);
        assert_eq!(
            analysis.intent,
            Some(RelationalIntent {
                subject: "before:2026-13-01 alice".into(),
                relation: "assignedTo".into(),
                object: "bob".into(),
            })
        );

        // The stored name matches too (camel case and screaming snake case).
        let by_name = understand("Carol works at Acme Corp", &schema());
        assert_eq!(
            by_name.intent,
            Some(RelationalIntent {
                subject: "Carol".into(),
                relation: "WORKS_AT".into(),
                object: "Acme Corp".into(),
            })
        );

        // Boundaries: no span before, no span after, two relationships, no
        // relationship, a relationship only inside a quoted phrase.
        for query in [
            "assigned to bob",
            "alice assigned to",
            "alice assigned to bob works at acme",
            "alice and bob",
            r#""alice assigned to bob""#,
            "",
        ] {
            assert_eq!(understand(query, &schema()).intent, None, "{query}");
        }

        // Unknown keys and empty values are terms; an unterminated quote runs
        // to the end.
        let rest = understand(r#"owner:x type: "open phrase"#, &[]);
        assert_eq!(rest.terms, vec!["owner:x", "type:"]);
        assert_eq!(rest.phrases, vec!["open phrase"]);
        assert_eq!(rest.filters, QueryFilters::default());
    }
}
