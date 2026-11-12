//! Source-native identifier grounding (MEDULLA-FEDERATED-QUERY-R058.4.7.2).
//!
//! A natural-language plan may name more than graph labels: decision-record
//! columns, time-series names and channels, CEP event attributes, UDF ids and
//! RDF/SPARQL IRIs. Each family has a catalog the caller is authorized to see.
//! The served handler binds those catalogs onto the [`SchemaContext`]
//! (`declare_source_catalogs`); [`SchemaContext::ground`] then checks every name a
//! candidate plan uses and refuses an unknown one with
//! `NL_IDENTIFIER_UNKNOWN{family: ...}` before anything executes.
//!
//! A family whose catalog is not bound (`None`) is not checked: it is the state of
//! a context built outside the served path, and the served handler always binds
//! every family its build can execute.
//!
//! [`SchemaContext`]: super::context::SchemaContext

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::context::SchemaViolation;

/// The source family an unknown identifier belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierFamily {
    /// Columns of the caller's decision log.
    DecisionRecord,
    /// Series names and value channels of the caller's time-series scope.
    Timeseries,
    /// Event attribute names a CEP pattern matches on.
    Cep,
    /// UDF ids registered by the caller.
    Udf,
    /// IRIs and prefixes embedded in SPARQL and ontology operators.
    Rdf,
}

impl IdentifierFamily {
    /// The stable family name a refusal carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DecisionRecord => "decision_record",
            Self::Timeseries => "timeseries",
            Self::Cep => "cep",
            Self::Udf => "udf",
            Self::Rdf => "rdf",
        }
    }
}

/// The IRIs and namespaces of the composed graph schema.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdfVocabulary {
    /// Full IRIs of the composed schema's classes and properties.
    pub terms: BTreeSet<String>,
    /// Namespace IRIs (`...#` or `.../`) those terms live in.
    pub namespaces: BTreeSet<String>,
}

impl RdfVocabulary {
    /// A vocabulary over `terms`, deriving each term's namespace.
    pub fn from_terms(terms: impl IntoIterator<Item = String>) -> Self {
        let terms: BTreeSet<String> = terms.into_iter().collect();
        let namespaces = terms.iter().filter_map(|t| namespace_of(t)).collect();
        Self { terms, namespaces }
    }
}

/// The caller-authorized catalogs of every source family. `None` = not bound.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCatalogs {
    /// Columns of the `decisions` relation.
    pub decision_columns: Option<BTreeSet<String>>,
    /// Series name -> its value channels (`v0..`, declared field names).
    pub series: Option<BTreeMap<String, BTreeSet<String>>>,
    /// Event attribute names stored on the caller's event nodes.
    pub cep_attributes: Option<BTreeSet<String>>,
    /// UDF ids the caller registered.
    pub udfs: Option<BTreeSet<String>>,
    /// The composed graph schema's vocabulary.
    pub rdf: Option<RdfVocabulary>,
}

/// The columns of the caller's decision log (the `decisions` relation).
pub fn decision_log_columns() -> BTreeSet<String> {
    medulla_decision_types::decision::DECISION_LOG_COLUMNS
        .iter()
        .map(|c| c.to_string())
        .collect()
}

/// Attribute names every `MOTIF`/`DISCORD` event row carries (read by the CEP stage).
pub const SERIES_EVENT_ATTRIBUTES: &[&str] =
    &["distance", "start", "end", "neighbor", "approximate"];

const BUILTIN_NAMESPACES: &[&str] = &[
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
    "http://www.w3.org/2000/01/rdf-schema#",
    "http://www.w3.org/2002/07/owl#",
    "http://www.w3.org/2001/XMLSchema#",
    "http://www.w3.org/ns/shacl#",
    "http://www.w3.org/2004/02/skos/core#",
];

fn unknown(
    family: IdentifierFamily,
    identifier: &str,
    known: &BTreeSet<String>,
) -> SchemaViolation {
    SchemaViolation::UnknownIdentifier {
        family,
        identifier: identifier.to_string(),
        known: known
            .iter()
            .take(super::context::MAX_PROPERTIES_PER_LABEL * 2)
            .cloned()
            .collect(),
    }
}

fn check(
    family: IdentifierFamily,
    catalog: Option<&BTreeSet<String>>,
    names: &[String],
) -> Result<(), SchemaViolation> {
    let Some(catalog) = catalog else {
        return Ok(());
    };
    match names.iter().find(|name| !catalog.contains(name.as_str())) {
        Some(name) => Err(unknown(family, name, catalog)),
        None => Ok(()),
    }
}

/// `DECISIONS WHERE` columns against the decision-log schema.
pub fn ground_decision_records(
    catalogs: &SourceCatalogs,
    columns: &[String],
) -> Result<(), SchemaViolation> {
    check(
        IdentifierFamily::DecisionRecord,
        catalogs.decision_columns.as_ref(),
        columns,
    )
}

/// `TsScan` series names against the caller's series catalog.
pub fn ground_timeseries(
    catalogs: &SourceCatalogs,
    series: &[String],
) -> Result<(), SchemaViolation> {
    let Some(catalog) = catalogs.series.as_ref() else {
        return Ok(());
    };
    let names: BTreeSet<String> = catalog.keys().cloned().collect();
    check(IdentifierFamily::Timeseries, Some(&names), series)
}

/// Value channels a plan reads (`EVENTS`) against the channels of the caller's series
/// plus the `extra` channels earlier stages of the same plan define (`DERIVE ... AS`).
pub fn ground_series_channels(
    catalogs: &SourceCatalogs,
    channels: &[String],
    extra: &BTreeSet<String>,
) -> Result<(), SchemaViolation> {
    let Some(catalog) = catalogs.series.as_ref() else {
        return Ok(());
    };
    let known: BTreeSet<String> = catalog
        .values()
        .flatten()
        .cloned()
        .chain(extra.iter().cloned())
        .collect();
    check(IdentifierFamily::Timeseries, Some(&known), channels)
}

/// CEP matcher attribute names against the event attributes the caller can match.
/// `extra` are the channels earlier stages of the plan define.
pub fn ground_cep(
    catalogs: &SourceCatalogs,
    attributes: &[String],
    extra: &BTreeSet<String>,
) -> Result<(), SchemaViolation> {
    let Some(cep) = catalogs.cep_attributes.as_ref() else {
        return Ok(());
    };
    let mut known: BTreeSet<String> = cep.clone();
    known.extend(extra.iter().cloned());
    known.extend(SERIES_EVENT_ATTRIBUTES.iter().map(|a| a.to_string()));
    if let Some(series) = catalogs.series.as_ref() {
        known.extend(series.values().flatten().cloned());
    }
    check(IdentifierFamily::Cep, Some(&known), attributes)
}

/// A UDF id against the caller's own registered UDFs.
pub fn ground_udf(catalogs: &SourceCatalogs, id: &str) -> Result<(), SchemaViolation> {
    check(
        IdentifierFamily::Udf,
        catalogs.udfs.as_ref(),
        &[id.to_string()],
    )
}

/// Every IRI and prefix a SPARQL text names against the composed schema.
pub fn ground_rdf(catalogs: &SourceCatalogs, sparql: &str) -> Result<(), SchemaViolation> {
    let Some(vocabulary) = catalogs.rdf.as_ref() else {
        return Ok(());
    };
    for iri in sparql_iris(sparql) {
        ground_rdf_iri(vocabulary, &iri)?;
    }
    Ok(())
}

/// A `Reason` target class against the composed schema. A bare local name (no scheme)
/// names a class the request's inline ontology or a later stage defines and is not
/// an IRI, so only an absolute IRI is checked.
pub fn ground_rdf_class(catalogs: &SourceCatalogs, class: &str) -> Result<(), SchemaViolation> {
    let Some(vocabulary) = catalogs.rdf.as_ref() else {
        return Ok(());
    };
    let iri = class.trim().trim_start_matches('<').trim_end_matches('>');
    if iri.contains("://") {
        ground_rdf_iri(vocabulary, iri)?;
    }
    Ok(())
}

/// One IRI against the composed schema.
pub fn ground_rdf_iri(vocabulary: &RdfVocabulary, iri: &str) -> Result<(), SchemaViolation> {
    let builtin = BUILTIN_NAMESPACES.iter().any(|ns| iri.starts_with(ns));
    if builtin || vocabulary.terms.contains(iri) || vocabulary.namespaces.contains(iri) {
        return Ok(());
    }
    let mut known = vocabulary.terms.clone();
    known.extend(vocabulary.namespaces.iter().cloned());
    Err(unknown(IdentifierFamily::Rdf, iri, &known))
}

/// The namespace of an IRI: everything up to and including the last `#` or `/`.
pub fn namespace_of(iri: &str) -> Option<String> {
    let end = iri.rfind(['#', '/'])?;
    Some(iri[..=end].to_string())
}

/// The absolute IRIs a SPARQL text names: `<iri>` references and `prefix:local` names
/// resolved through the text's own `PREFIX` declarations. Strings and comments are
/// skipped; a prefixed name with no declaration is reported as written, so grounding
/// refuses it as an unknown prefix.
pub fn sparql_iris(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut iris = Vec::new();
    let mut prefixes: BTreeMap<String, String> = BTreeMap::new();
    let mut names: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            q @ ('"' | '\'') => {
                i += 1;
                while i < chars.len() && chars[i] != q {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
            }
            '<' => match iri_ref_end(&chars, i) {
                Some(end) => {
                    iris.push(chars[i + 1..end].iter().collect());
                    i = end + 1;
                }
                None => i += 1,
            },
            c if c.is_ascii_alphabetic() || c == ':' || c == '_' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || "_-.:".contains(chars[i]))
                {
                    i += 1;
                }
                let mut word: String = chars[start..i].iter().collect();
                while word.ends_with('.') {
                    word.pop();
                    i -= 1;
                }
                if let Some((prefix, local)) = word.split_once(':') {
                    if prefix == "_" {
                        // A blank node label, not a prefixed name.
                        continue;
                    }
                    if local.is_empty() && word.ends_with(':') {
                        // `PREFIX p: <iri>` declaration: the next IRI is its namespace.
                        if let Some(open) = chars[i..].iter().position(|c| *c == '<') {
                            if let Some(end) = iri_ref_end(&chars, i + open) {
                                if chars[i..i + open].iter().all(|c| c.is_whitespace()) {
                                    let ns: String = chars[i + open + 1..end].iter().collect();
                                    prefixes.insert(prefix.to_string(), ns.clone());
                                    iris.push(ns);
                                    i = end + 1;
                                    continue;
                                }
                            }
                        }
                    }
                    names.push((prefix.to_string(), local.to_string()));
                }
            }
            _ => i += 1,
        }
    }
    for (prefix, local) in names {
        match prefixes.get(&prefix) {
            Some(ns) => iris.push(format!("{ns}{local}")),
            None => iris.push(format!("{prefix}:{local}")),
        }
    }
    iris
}

/// The index of the `>` closing an IRI reference opened at `open`, if `<...>` is one
/// (no whitespace or markup inside, and a scheme): `?a < ?b` is not.
fn iri_ref_end(chars: &[char], open: usize) -> Option<usize> {
    let close = chars[open + 1..].iter().position(|c| *c == '>')? + open + 1;
    let body = &chars[open + 1..close];
    let ok = !body.is_empty()
        && body
            .iter()
            .all(|c| !c.is_whitespace() && !"<>\"{}|^`\\".contains(*c))
        && body.contains(&':');
    ok.then_some(close)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocabulary() -> RdfVocabulary {
        RdfVocabulary::from_terms([
            "https://knuckles-team.github.io/ontology/core#Device".to_string()
        ])
    }

    // spec: MEDULLA-FEDERATED-QUERY-R058.4.7.2.5
    #[test]
    fn sparql_iris_resolve_prefixes_and_skip_strings_and_operators() {
        let text = "PREFIX c: <https://knuckles-team.github.io/ontology/core#>\n\
                    SELECT ?x WHERE { ?x a c:Device . FILTER(?x < 3 && ?y > 1) \
                    ?x rdfs:label \"<http://not/an/iri>\" } # <http://comment/iri>";
        let iris = sparql_iris(text);
        assert!(iris.contains(&"https://knuckles-team.github.io/ontology/core#".to_string()));
        assert!(iris.contains(&"https://knuckles-team.github.io/ontology/core#Device".to_string()));
        assert!(iris.contains(&"rdfs:label".to_string()));
        assert!(!iris
            .iter()
            .any(|i| i.contains("not/an/iri") || i.contains("comment")));
    }

    // spec: MEDULLA-FEDERATED-QUERY-R058.4.7.2.5
    #[test]
    fn rdf_grounding_accepts_schema_terms_and_refuses_invented_ones() {
        let catalogs = SourceCatalogs {
            rdf: Some(vocabulary()),
            ..Default::default()
        };
        let ok = "PREFIX c: <https://knuckles-team.github.io/ontology/core#> \
                  SELECT ?x WHERE { ?x a c:Device }";
        assert!(ground_rdf(&catalogs, ok).is_ok());
        let bad =
            "SELECT ?x WHERE { ?x a <https://knuckles-team.github.io/ontology/core#Invented> }";
        let err = ground_rdf(&catalogs, bad).unwrap_err();
        assert_eq!(err.code(), "NL_IDENTIFIER_UNKNOWN");
        assert!(err.to_string().contains("family: rdf"), "{err}");
        let undeclared = "SELECT ?x WHERE { ?x a zz:Device }";
        assert!(ground_rdf(&catalogs, undeclared).is_err());
    }

    // spec: MEDULLA-FEDERATED-QUERY-R058.4.7.2.1
    #[test]
    fn an_unbound_catalog_is_not_checked() {
        let catalogs = SourceCatalogs::default();
        assert!(ground_decision_records(&catalogs, &["invented".into()]).is_ok());
        assert!(ground_udf(&catalogs, "invented").is_ok());
    }
}
