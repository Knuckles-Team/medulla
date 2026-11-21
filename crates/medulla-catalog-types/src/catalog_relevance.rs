//! The ranked catalog relevance function (MEDULLA-TYPED-PACKS-R108.16).
//!
//! One function ranks every asset family. SPARQL reaches it as `cat:relevance(?asset, "text")`
//! and `cat:rankComponents(?asset, "text")`; SQL reaches it as `catalog_relevance(props, 'text')`.
//! Both read the same [`AssetDoc`] of an asset node and call the same scorer here, so the two
//! surfaces never rank differently. The score is the sum of the name, term-label, concept and
//! description matches, fused with the usage prior of [`fuse_with_usage_prior`] (capped by
//! `USAGE_PRIOR_CAP`), then scaled by the trust flag of the asset.

use serde_json::{Map, Value};

use crate::asset_usage::{fuse_with_usage_prior, HitComponents, UsageKnob};

/// Namespace of the catalog vocabulary.
pub const CAT_NS: &str = "https://knuckles-team.github.io/ontology/catalog#";
/// IRI of the SPARQL function `cat:relevance`.
pub const CAT_RELEVANCE_IRI: &str = "https://knuckles-team.github.io/ontology/catalog#relevance";
/// IRI of the SPARQL function `cat:rankComponents`.
pub const CAT_RANK_COMPONENTS_IRI: &str =
    "https://knuckles-team.github.io/ontology/catalog#rankComponents";

/// Refusal code: the search text is longer than `catalog_search.max_query_chars`.
pub const CATALOG_QUERY_TOO_LONG: &str = "CATALOG_QUERY_TOO_LONG";
/// Refusal code: the search text is blank after trimming.
pub const CATALOG_QUERY_EMPTY: &str = "CATALOG_QUERY_EMPTY";

const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const DCT: &str = "http://purl.org/dc/terms/";
/// The reserved cell that holds the second and later values of one predicate.
const MULTI_VALUE_KEY: &str = "__rdf_multivalue_literals";

/// `knob catalog_search.max_query_chars`: longest search text, in characters.
pub const CATALOG_SEARCH_MAX_QUERY_CHARS: UsageKnob = UsageKnob {
    name: "catalog_search.max_query_chars",
    default: 1000,
    range: (16, 8192),
    env: "MEDULLA_CATALOG_SEARCH_MAX_QUERY_CHARS",
};
/// `knob catalog.search_page_max`: most hits one search page returns.
pub const CATALOG_SEARCH_PAGE_MAX: UsageKnob = UsageKnob {
    name: "catalog.search_page_max",
    default: 100,
    range: (1, 1000),
    env: "MEDULLA_CATALOG_SEARCH_PAGE_MAX",
};

/// A ratio knob with inclusive bounds and an environment override.
#[derive(Clone, Copy, Debug)]
pub struct RatioKnob {
    /// The knob name.
    pub name: &'static str,
    /// The value when the environment sets none.
    pub default: f64,
    /// Inclusive bounds.
    pub range: (f64, f64),
    /// The environment override.
    pub env: &'static str,
}

impl RatioKnob {
    /// The effective value: the environment value inside the bounds, the default otherwise.
    pub fn value(&self) -> f64 {
        std::env::var(self.env)
            .ok()
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .filter(|n| n.is_finite() && *n >= self.range.0 && *n <= self.range.1)
            .unwrap_or(self.default)
    }
}

/// `knob catalog.endorse_boost`: the score ratio of an endorsed asset.
pub const CATALOG_ENDORSE_BOOST: RatioKnob = RatioKnob {
    name: "catalog.endorse_boost",
    default: 1.25,
    range: (1.0, 2.0),
    env: "MEDULLA_CATALOG_ENDORSE_BOOST",
};
/// `knob catalog.deprecate_penalty`: the score ratio of a deprecated asset.
pub const CATALOG_DEPRECATE_PENALTY: RatioKnob = RatioKnob {
    name: "catalog.deprecate_penalty",
    default: 0.5,
    range: (0.1, 1.0),
    env: "MEDULLA_CATALOG_DEPRECATE_PENALTY",
};

/// Reads at which the usage of an asset reaches half of the usage prior.
pub const USAGE_HALF_SATURATION_READS: f64 = 50.0;

/// Weight of each text field in the text score; the weights sum to one.
const W_NAME: f64 = 0.4;
const W_TERM: f64 = 0.3;
const W_CONCEPT: f64 = 0.2;
const W_DESCRIPTION: f64 = 0.1;

/// The trust flag one principal set on an asset (MEDULLA-TYPED-PACKS-R802.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustFlag {
    /// The asset is endorsed: its score is boosted.
    Endorsement,
    /// The asset carries a warning: its score is unchanged.
    Warning,
    /// The asset is deprecated: its score is penalized.
    Deprecation,
}

impl TrustFlag {
    /// The flag a stored value names, if it names one.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "endorsement" | "endorsed" => Some(Self::Endorsement),
            "warning" => Some(Self::Warning),
            "deprecation" | "deprecated" => Some(Self::Deprecation),
            _ => None,
        }
    }

    /// The score ratio of the flag under the knobs.
    pub fn multiplier(self) -> f64 {
        match self {
            Self::Endorsement => CATALOG_ENDORSE_BOOST.value(),
            Self::Deprecation => CATALOG_DEPRECATE_PENALTY.value(),
            Self::Warning => 1.0,
        }
    }
}

/// The text and counters of one asset that ranking reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssetDoc {
    /// Names and labels of the asset.
    pub names: Vec<String>,
    /// Labels of the glossary terms that define the asset.
    pub term_labels: Vec<String>,
    /// Classes the defining terms name.
    pub concepts: Vec<String>,
    /// Descriptions and definitions.
    pub descriptions: Vec<String>,
    /// Reads counted on the asset from the usage series, filled by the caller with
    /// [`usage_of`]; the asset node stores no usage.
    pub usage_reads: u64,
    /// Trust flags set on the asset.
    pub flags: Vec<TrustFlag>,
    /// The review status of an article (`cat:articleStatus`); `None` for any other asset.
    pub article_status: Option<String>,
    /// For an article: the best score, for the search text, among the assets its
    /// `cat:aboutAsset` edges name, filled by the caller that can read the edges.
    pub described_best: f64,
}

/// Predicate keys, by IRI and by bare local name, that carry each text field.
fn name_keys() -> [String; 6] {
    [
        format!("{SKOS}prefLabel"),
        format!("{RDFS}label"),
        format!("{DCT}title"),
        "name".into(),
        "label".into(),
        "title".into(),
    ]
}
fn description_keys() -> [String; 5] {
    [
        format!("{DCT}description"),
        format!("{SKOS}definition"),
        format!("{RDFS}comment"),
        "description".into(),
        "comment".into(),
    ]
}
fn term_keys() -> [String; 3] {
    [format!("{CAT_NS}termLabel"), "termLabel".into(), "term_labels".into()]
}
fn concept_keys() -> [String; 3] {
    [format!("{CAT_NS}concept"), "concept".into(), "concepts".into()]
}
fn article_status_keys() -> [String; 2] {
    [format!("{CAT_NS}articleStatus"), "articleStatus".into()]
}
fn flag_keys() -> [String; 2] {
    [
        format!("{CAT_NS}flag"),
        "flag".into(),
    ]
}

/// The lexical form of one stored cell.
fn cell_text(cell: &Value) -> Option<String> {
    match cell {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Object(m) => m.get("value").and_then(Value::as_str).map(String::from),
        _ => None,
    }
}

/// Every value stored under `key`, scalar, array or reserved multi-value cell.
fn values_of(obj: &Map<String, Value>, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    match obj.get(key) {
        Some(Value::Array(items)) => out.extend(items.iter().filter_map(cell_text)),
        Some(cell) => out.extend(cell_text(cell)),
        None => {}
    }
    if let Some(Value::Array(extra)) = obj.get(MULTI_VALUE_KEY).and_then(|m| m.get(key)) {
        out.extend(extra.iter().filter_map(cell_text));
    }
    out
}

fn collect(obj: &Map<String, Value>, keys: &[String]) -> Vec<String> {
    keys.iter().flat_map(|key| values_of(obj, key)).collect()
}

impl AssetDoc {
    /// Read the asset text, counters and flags of one decoded node property object.
    pub fn from_props(obj: &Map<String, Value>) -> Self {
        let status = collect(obj, &article_status_keys())
            .into_iter()
            .next()
            .map(|raw| raw.trim().to_ascii_lowercase());
        let article_status = status.as_deref();
        Self {
            names: collect(obj, &name_keys()),
            term_labels: collect(obj, &term_keys()),
            concepts: collect(obj, &concept_keys()),
            descriptions: collect(obj, &description_keys()),
            usage_reads: 0,
            flags: collect(obj, &flag_keys())
                .iter()
                .filter_map(|raw| TrustFlag::parse(raw))
                .chain(
                    // A deprecated article takes the deprecation penalty.
                    (article_status == Some("deprecated")).then_some(TrustFlag::Deprecation),
                )
                .collect(),
            article_status: article_status.map(str::to_string),
            described_best: 0.0,
        }
    }
}

/// The labels of one glossary term node: preferred, alternative and plain labels in every
/// language tag.
pub fn term_labels(obj: &Map<String, Value>) -> Vec<String> {
    collect(
        obj,
        &[
            format!("{SKOS}prefLabel"),
            format!("{SKOS}altLabel"),
            format!("{RDFS}label"),
        ],
    )
}

/// Reads per asset name, summed over principals, from the `asset_usage.reads` series.
pub type UsageIndex = std::collections::HashMap<String, u64>;

/// Sum the `asset_usage.reads` series (`(series name, summed count)`) per asset name.
pub fn usage_index(series: &[(String, u64)]) -> UsageIndex {
    let prefix = format!("{}|principal=", crate::asset_usage::ASSET_USAGE_READS_SERIES);
    let mut out = UsageIndex::new();
    for (name, count) in series {
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        if let Some((_, asset)) = rest.split_once("|asset=") {
            let slot = out.entry(asset.to_string()).or_insert(0);
            *slot = slot.saturating_add(*count);
        }
    }
    out
}

/// The reads of one asset: the usage recorded under any of its names or under the last
/// segment of its node key.
pub fn usage_of(index: &UsageIndex, names: &[String], node_key: &str) -> u64 {
    let bare = node_key.trim_start_matches('<').trim_end_matches('>');
    let local = bare.rsplit(['#', '/', ':']).next().unwrap_or(bare);
    let mut keys: Vec<&str> = names.iter().map(String::as_str).collect();
    keys.push(local);
    keys.sort_unstable();
    keys.dedup();
    keys.iter()
        .filter_map(|key| index.get(*key))
        .fold(0u64, |sum, n| sum.saturating_add(*n))
}

/// Whether `text` (a SPARQL query or a SQL statement) calls one of the catalog ranking
/// functions, so a served read knows to load the usage counts.
pub fn ranks_assets(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("catalog_relevance")
        || lower.contains("relevance(")
        || lower.contains("rankcomponents(")
        || text.contains(CAT_RELEVANCE_IRI)
        || text.contains(CAT_RANK_COMPONENTS_IRI)
}

/// The SQL string literal (single-quoted, quotes doubled) that carries `index` into a
/// statement, with the entries sorted so one usage state is one literal.
pub fn usage_sql_literal(index: &UsageIndex) -> String {
    let sorted: std::collections::BTreeMap<&String, &u64> = index.iter().collect();
    let json = serde_json::to_string(&sorted).unwrap_or_else(|_| "{}".to_string());
    format!("'{}'", json.replace('\'', "''"))
}

/// Parse the usage argument `catalog_relevance` receives; an empty or malformed value is an
/// error, never an empty index.
pub fn parse_usage_argument(raw: &str) -> Result<UsageIndex, String> {
    serde_json::from_str::<UsageIndex>(raw)
        .map_err(|error| format!("CATALOG_USAGE_INVALID: the usage argument is not an index: {error}"))
}

/// Refusal code: a statement passed its own usage to `catalog_relevance`. Usage comes only
/// from the server's usage store, never from the caller.
pub const CATALOG_USAGE_CLAIMED: &str = "CATALOG_USAGE_CLAIMED";

/// Append the usage literal as the third argument of every `catalog_relevance(props, text)`
/// call in `sql`. A call that already has a third argument is refused. A statement without
/// the function is returned unchanged.
pub fn inject_usage_argument(sql: &str, usage_literal: &str) -> Result<String, String> {
    const NAME: &str = "catalog_relevance";
    let bytes = sql.as_bytes();
    let lower = sql.to_ascii_lowercase();
    let mut out = String::with_capacity(sql.len() + usage_literal.len());
    let mut copied = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        if bytes.get(i + 1) == Some(&quote) {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            _ if lower.as_bytes()[i..].starts_with(NAME.as_bytes())
                && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_')) =>
            {
                let mut j = i + NAME.len();
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if bytes.get(j) != Some(&b'(') {
                    i += NAME.len();
                    continue;
                }
                let (close, commas) = call_extent(bytes, j)?;
                if commas != 1 {
                    return Err(format!(
                        "{CATALOG_USAGE_CLAIMED}: catalog_relevance takes the properties and the search text only"
                    ));
                }
                out.push_str(&sql[copied..close]);
                out.push_str(", ");
                out.push_str(usage_literal);
                copied = close;
                i = close + 1;
            }
            _ => i += 1,
        }
    }
    out.push_str(&sql[copied..]);
    Ok(out)
}

/// The index of the `)` that closes the call opened at `open`, and the number of commas
/// at its top level.
fn call_extent(bytes: &[u8], open: usize) -> Result<(usize, usize), String> {
    let (mut depth, mut commas, mut i) = (0usize, 0usize, open);
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        if bytes.get(i + 1) == Some(&quote) {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((i, commas));
                }
            }
            b',' if depth == 1 => commas += 1,
            _ => {}
        }
        i += 1;
    }
    Err("INVALID_ARGUMENT: unterminated catalog_relevance call".to_string())
}

/// Check the search text against the knobs: not blank, not over `catalog_search.max_query_chars`.
pub fn validate_query(text: &str) -> Result<&str, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(format!("{CATALOG_QUERY_EMPTY}: the search text is blank"));
    }
    let max = CATALOG_SEARCH_MAX_QUERY_CHARS.value() as usize;
    if trimmed.chars().count() > max {
        return Err(format!(
            "{CATALOG_QUERY_TOO_LONG}: the search text is over {max} characters"
        ));
    }
    Ok(trimmed)
}

/// Lowercase alphanumeric tokens of `text`.
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Share of the query tokens one field matches: a whole token counts one, a token that only
/// contains the query token counts one half. The best value of the field's entries wins.
fn field_match(entries: &[String], query: &[String]) -> f64 {
    if query.is_empty() {
        return 0.0;
    }
    entries
        .iter()
        .map(|entry| {
            let have = tokens(entry);
            let hit: f64 = query
                .iter()
                .map(|q| {
                    if have.iter().any(|t| t == q) {
                        1.0
                    } else if have.iter().any(|t| t.contains(q.as_str())) {
                        0.5
                    } else {
                        0.0
                    }
                })
                .sum();
            hit / query.len() as f64
        })
        .fold(0.0, f64::max)
}

/// The text score in `[0, 1]`: the weighted sum of the name, term-label, concept and
/// description matches.
pub fn text_score(doc: &AssetDoc, query: &str) -> f64 {
    let q = tokens(query);
    W_NAME * field_match(&doc.names, &q)
        + W_TERM * field_match(&doc.term_labels, &q)
        + W_CONCEPT * field_match(&doc.concepts, &q)
        + W_DESCRIPTION * field_match(&doc.descriptions, &q)
}

/// The usage of an asset normalized to `[0, 1]`.
pub fn usage_norm(reads: u64) -> f64 {
    let reads = reads as f64;
    reads / (reads + USAGE_HALF_SATURATION_READS)
}

/// The score ratio of the asset's trust flags: a deprecation wins over a warning, which wins
/// over an endorsement.
pub fn trust_multiplier(flags: &[TrustFlag]) -> f64 {
    if flags.contains(&TrustFlag::Deprecation) {
        TrustFlag::Deprecation.multiplier()
    } else if flags.contains(&TrustFlag::Endorsement) {
        TrustFlag::Endorsement.multiplier()
    } else {
        1.0
    }
}

/// The score components of one asset for one search text. An asset the text does not match
/// has no components: usage alone never makes a hit.
pub fn rank_components(doc: &AssetDoc, query: &str) -> HitComponents {
    let text = text_score(doc, query);
    if text <= 0.0 {
        return HitComponents {
            text: 0.0,
            vector: 0.0,
            usage: 0.0,
        };
    }
    // The scalar function holds no embedding model: its lexical score stands for both legs.
    fuse_with_usage_prior(text, text, usage_norm(doc.usage_reads))
}

/// The share of the best described-asset score an article gains as its `described_asset`
/// component. The constant is part of MEDULLA-TYPED-PACKS-R108.16, not a knob.
pub const DESCRIBED_ASSET_FACTOR: f64 = 0.5;

/// Whether the asset is an article no search returns: one still in `draft` or `in_review`.
pub fn hidden_from_search(doc: &AssetDoc) -> bool {
    matches!(doc.article_status.as_deref(), Some("draft" | "in_review"))
}

/// The `described_asset` component of an article: half the best score among the assets it
/// describes. Zero for a hidden article or an asset that is no article.
pub fn described_asset_component(doc: &AssetDoc) -> f64 {
    if doc.article_status.is_none() || hidden_from_search(doc) {
        return 0.0;
    }
    DESCRIBED_ASSET_FACTOR * doc.described_best
}

/// The final score of one asset: the fused components plus the `described_asset` component,
/// scaled by the trust flags. A `draft` or `in_review` article scores zero for every caller.
pub fn relevance(doc: &AssetDoc, query: &str) -> f64 {
    if hidden_from_search(doc) {
        return 0.0;
    }
    (rank_components(doc, query).total() + described_asset_component(doc))
        * trust_multiplier(&doc.flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(name: &str) -> AssetDoc {
        AssetDoc {
            names: vec![name.into()],
            ..AssetDoc::default()
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R108.16
    #[test]
    fn name_outranks_description_and_unmatched_is_zero() {
        let named = doc("orders table");
        let described = AssetDoc {
            descriptions: vec!["all orders".into()],
            ..AssetDoc::default()
        };
        assert!(relevance(&named, "orders") > relevance(&described, "orders"));
        assert_eq!(relevance(&doc("customers"), "orders"), 0.0);
    }

    // spec: MEDULLA-TYPED-PACKS-R108.16
    #[test]
    fn usage_is_capped_and_never_creates_a_hit() {
        let mut busy = doc("orders");
        busy.usage_reads = 1_000_000;
        let c = rank_components(&busy, "orders");
        assert!(c.usage <= crate::asset_usage::USAGE_PRIOR_CAP * c.total() + 1e-9);
        let mut unrelated = doc("customers");
        unrelated.usage_reads = 1_000_000;
        assert_eq!(relevance(&unrelated, "orders"), 0.0);
        assert!(relevance(&busy, "orders") > relevance(&doc("orders"), "orders"));
    }

    // spec: MEDULLA-TYPED-PACKS-R108.16
    #[test]
    fn draft_articles_are_hidden_and_approved_ones_gain_the_described_asset_component() {
        let article = |status: &str, described: f64| AssetDoc {
            names: vec!["unrelated guide".into()],
            article_status: Some(status.into()),
            described_best: described,
            ..AssetDoc::default()
        };
        for hidden in ["draft", "in_review"] {
            assert_eq!(relevance(&article(hidden, 0.9), "orders"), 0.0);
        }
        let approved = article("approved", 0.8);
        assert_eq!(described_asset_component(&approved), 0.4);
        assert_eq!(relevance(&approved, "orders"), 0.4);
        assert_eq!(described_asset_component(&doc("orders")), 0.0);
        let plain = relevance(&article("approved", 0.0), "guide");
        let props: Map<String, Value> = serde_json::from_value(serde_json::json!({
            "https://knuckles-team.github.io/ontology/catalog#articleStatus": "deprecated",
            "name": "guide"
        }))
        .unwrap();
        assert!(relevance(&AssetDoc::from_props(&props), "guide") < plain);
    }

    // spec: MEDULLA-TYPED-PACKS-R108.16
    #[test]
    fn endorsement_boosts_and_deprecation_penalizes() {
        let plain = relevance(&doc("orders"), "orders");
        let mut endorsed = doc("orders");
        endorsed.flags = vec![TrustFlag::Endorsement];
        let mut deprecated = doc("orders");
        deprecated.flags = vec![TrustFlag::Endorsement, TrustFlag::Deprecation];
        assert!(relevance(&endorsed, "orders") > plain);
        assert!(relevance(&deprecated, "orders") < plain);
    }

    // spec: MEDULLA-TYPED-PACKS-R108.16
    #[test]
    fn props_are_read_by_iri_and_by_local_name() {
        let props: Map<String, Value> = serde_json::from_value(serde_json::json!({
            "http://www.w3.org/2004/02/skos/core#prefLabel": "Orders",
            "description": "sales orders",
            "__rdf_multivalue_literals": {"termLabel": ["Order Book"]},
            "flag": "deprecated"
        }))
        .unwrap();
        let doc = AssetDoc::from_props(&props);
        assert_eq!(doc.names, vec!["Orders"]);
        assert_eq!(doc.term_labels, vec!["Order Book"]);
        assert_eq!(doc.flags, vec![TrustFlag::Deprecation]);
    }

    // spec: MEDULLA-TYPED-PACKS-R108.16
    #[test]
    fn usage_series_sum_per_asset_across_principals() {
        let series = vec![
            ("asset_usage.reads|principal=a|asset=orders".to_string(), 4),
            ("asset_usage.reads|principal=b|asset=orders".to_string(), 6),
            ("asset_usage.join|principal=a|asset=orders|with=x".to_string(), 9),
        ];
        let index = usage_index(&series);
        assert_eq!(usage_of(&index, &["Orders".into(), "orders".into()], "<urn:x:y>"), 10);
        assert_eq!(usage_of(&index, &[], "<urn:medulla:asset:tool:orders>"), 10);
        assert_eq!(usage_of(&index, &["customers".into()], "<urn:x:y>"), 0);
    }

    // spec: MEDULLA-TYPED-PACKS-R108.16
    #[test]
    fn usage_is_injected_as_the_third_argument_and_never_claimed() {
        let literal = usage_sql_literal(&UsageIndex::from([("orders".to_string(), 7)]));
        assert_eq!(literal, "'{\"orders\":7}'");
        let sql = "SELECT id, CATALOG_RELEVANCE(props, 'a,(b') AS s FROM nodes WHERE catalog_relevance (props, lower('x')) > 0";
        let rewritten = inject_usage_argument(sql, &literal).unwrap();
        assert_eq!(
            rewritten,
            format!("SELECT id, CATALOG_RELEVANCE(props, 'a,(b', {literal}) AS s FROM nodes WHERE catalog_relevance (props, lower('x'), {literal}) > 0")
        );
        assert_eq!(inject_usage_argument("SELECT 1", &literal).unwrap(), "SELECT 1");
        assert!(inject_usage_argument("SELECT catalog_relevance(props, 'x', '{}')", &literal)
            .unwrap_err()
            .starts_with(CATALOG_USAGE_CLAIMED));
        assert_eq!(
            parse_usage_argument("{\"orders\":7}").unwrap().get("orders"),
            Some(&7)
        );
        assert!(parse_usage_argument("").is_err());
    }

    // spec: MEDULLA-TYPED-PACKS-R108.16
    #[test]
    fn long_and_blank_queries_are_refused_with_their_codes() {
        assert!(validate_query("   ").unwrap_err().starts_with(CATALOG_QUERY_EMPTY));
        let long = "a".repeat(1001);
        assert!(validate_query(&long)
            .unwrap_err()
            .starts_with(CATALOG_QUERY_TOO_LONG));
        assert_eq!(validate_query(" orders ").unwrap(), "orders");
    }
}
