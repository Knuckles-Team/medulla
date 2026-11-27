//! The registered catalog stored queries (MEDULLA-TYPED-PACKS-R802.8, R108.17).
//!
//! The definitions are written once here. The engine seeds them into a graph when the graph
//! first records a critical element (`Stewardship.MarkCritical`) or first takes a catalog write
//! (an `AddTriples` that carries a `cat:` asset), in the stored-query node shape of
//! `Method::StoredQueryPut`, shared with the tenant. A query is data, not code.

use std::collections::BTreeMap;

use medulla_identity_types::identity::ShareScope;

use medulla_catalog_types::catalog_read::{
    CATALOG_ASSETS_BY_KIND, CATALOG_ASSETS_SEARCH, CATALOG_ASSET_DETAIL, CATALOG_ASSET_KINDS,
};

use crate::stored_query::{RefreshPolicy, StoredParamType, StoredQuery, StoredQueryLanguage};

/// The stored query listing each critical element that lacks an owner, a steward or an
/// active quality rule.
pub const CATALOG_CRITICAL_GAPS: &str = "catalog.critical_gaps";

/// The node label of a critical element, written by `Stewardship.MarkCritical`.
pub const CRITICAL_ELEMENT_LABEL: &str = "CriticalElement";

/// `catalog.critical_gaps`: the `gaps` of an element is the comma list of what it misses
/// (`owner`, `steward`, `quality_rule`), empty when it misses nothing.
const CRITICAL_GAPS_TEXT: &str = "MATCH (:CriticalElement) WHERE gaps != '' \
|> ORDER BY element \
|> RETURN element, gaps, marked_by";

const CAT: &str = "https://knuckles-team.github.io/ontology/catalog#";
const CORE: &str = "https://knuckles-team.github.io/ontology/core#";

/// The closed list of asset kinds as a SPARQL `IN` list.
fn kinds_in_list() -> String {
    CATALOG_ASSET_KINDS
        .iter()
        .map(|kind| format!("\"{kind}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `catalog.assets_by_kind` (R108.17): params `kind` and `owner` (empty for any), `limit` and
/// `offset`. One result carries the kind counts (the rows with `count` bound) and the page of
/// asset rows (the rows with `asset` bound).
fn assets_by_kind_text() -> String {
    let kinds = kinds_in_list();
    format!(
        "PREFIX cat: <{CAT}>\n\
PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\n\
SELECT ?asset ?kind ?name ?owner ?count WHERE {{\n\
  {{ SELECT ?kind (COUNT(DISTINCT ?a) AS ?count) WHERE {{\n\
      ?a a ?t .\n\
      BIND(STRAFTER(STR(?t), \"{CAT}\") AS ?kind)\n\
      FILTER(?kind IN ({kinds}))\n\
      OPTIONAL {{ ?a cat:hasOwner ?o }}\n\
      FILTER($owner = \"\" || STR(?o) = $owner)\n\
    }} GROUP BY ?kind }}\n\
  UNION\n\
  {{ SELECT ?asset ?kind ?name ?owner WHERE {{\n\
      ?asset a ?t .\n\
      BIND(STRAFTER(STR(?t), \"{CAT}\") AS ?kind)\n\
      FILTER(?kind IN ({kinds}))\n\
      FILTER($kind = \"\" || ?kind = $kind)\n\
      OPTIONAL {{ ?asset skos:prefLabel ?name }}\n\
      OPTIONAL {{ ?asset cat:hasOwner ?owner }}\n\
      FILTER($owner = \"\" || STR(?owner) = $owner)\n\
    }} ORDER BY ?name ?asset LIMIT $limit OFFSET $offset }}\n\
}}"
    )
}

/// `catalog.assets_search` (R108.17): params `text`, `kinds` (space-separated, empty for every
/// kind), `limit` and `offset`; ranked by `cat:relevance` of R108.16.
fn assets_search_text() -> String {
    let kinds = kinds_in_list();
    format!(
        "PREFIX cat: <{CAT}>\n\
PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\n\
SELECT ?asset ?kind ?name ?score WHERE {{\n\
  ?asset a ?t .\n\
  BIND(STRAFTER(STR(?t), \"{CAT}\") AS ?kind)\n\
  FILTER(?kind IN ({kinds}))\n\
  FILTER($kinds = \"\" || CONTAINS(CONCAT(\" \", $kinds, \" \"), CONCAT(\" \", ?kind, \" \")))\n\
  OPTIONAL {{ ?asset skos:prefLabel ?name }}\n\
  BIND(cat:relevance(?asset, $text) AS ?score)\n\
  FILTER(?score > 0)\n\
}} ORDER BY DESC(?score) ?asset LIMIT $limit OFFSET $offset"
    )
}

/// `catalog.asset_detail` (R108.17): param `asset_id`. One row per fact, named by `facet`
/// (`kind`, `name`, `owner`, `term`, `upstream`, `downstream`, `rating`, `comment`).
fn asset_detail_text() -> String {
    format!(
        "PREFIX cat: <{CAT}>\n\
PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\n\
SELECT ?facet ?value ?extra ?at ?who WHERE {{\n\
  ?asset a ?t .\n\
  FILTER(STR(?asset) = $asset_id)\n\
  FILTER(STRSTARTS(STR(?t), \"{CAT}\"))\n\
  {{ BIND(\"kind\" AS ?facet) BIND(STRAFTER(STR(?t), \"{CAT}\") AS ?value) }}\n\
  UNION {{ ?asset cat:hasOwner ?o . BIND(\"owner\" AS ?facet) BIND(STR(?o) AS ?value) }}\n\
  UNION {{ ?asset skos:prefLabel ?n . BIND(\"name\" AS ?facet) BIND(STR(?n) AS ?value) }}\n\
  UNION {{ ?term <{CORE}definesAsset> ?asset . OPTIONAL {{ ?term skos:prefLabel ?tl }}\n\
    BIND(\"term\" AS ?facet) BIND(COALESCE(STR(?tl), STR(?term)) AS ?value) }}\n\
  UNION {{ ?up cat:lineageTo ?asset . BIND(\"upstream\" AS ?facet) BIND(STR(?up) AS ?value) }}\n\
  UNION {{ ?asset cat:lineageTo ?down . BIND(\"downstream\" AS ?facet) BIND(STR(?down) AS ?value) }}\n\
  UNION {{ ?ann cat:annotates ?asset ; cat:annotationKind \"rating\" ; cat:rating ?r .\n\
    BIND(\"rating\" AS ?facet) BIND(STR(?r) AS ?value) }}\n\
  UNION {{ ?ann cat:annotates ?asset ; cat:annotationKind \"comment\" ; cat:annotationText ?c .\n\
    OPTIONAL {{ ?ann cat:annotationAuthor ?who }} OPTIONAL {{ ?ann cat:annotatedAtMs ?at }}\n\
    BIND(\"comment\" AS ?facet) BIND(STR(?c) AS ?value) BIND(STR(?ann) AS ?extra) }}\n\
}}"
    )
}

fn sparql_query(name: &str, text: String, params: &[(&str, StoredParamType)]) -> StoredQuery {
    StoredQuery {
        name: name.to_string(),
        version: 1,
        language: StoredQueryLanguage::Sparql,
        text,
        params: params
            .iter()
            .map(|(param, kind)| (param.to_string(), *kind))
            .collect(),
        materialize: false,
        refresh: RefreshPolicy::OnChange,
        author: String::new(),
        share: ShareScope::Tenant,
    }
}

/// The registered catalog queries, in the order they are put.
pub fn catalog_queries() -> [StoredQuery; 4] {
    [
        StoredQuery {
            name: CATALOG_CRITICAL_GAPS.to_string(),
            version: 1,
            language: StoredQueryLanguage::Uql,
            text: CRITICAL_GAPS_TEXT.to_string(),
            params: BTreeMap::new(),
            materialize: false,
            refresh: RefreshPolicy::OnChange,
            // The engine stamps `author` on put and ignores whatever a request carries.
            author: String::new(),
            share: ShareScope::Tenant,
        },
        sparql_query(
            CATALOG_ASSETS_BY_KIND,
            assets_by_kind_text(),
            &[
                ("kind", StoredParamType::String),
                ("owner", StoredParamType::String),
                ("limit", StoredParamType::Int),
                ("offset", StoredParamType::Int),
            ],
        ),
        sparql_query(
            CATALOG_ASSETS_SEARCH,
            assets_search_text(),
            &[
                ("text", StoredParamType::String),
                ("kinds", StoredParamType::String),
                ("limit", StoredParamType::Int),
                ("offset", StoredParamType::Int),
            ],
        ),
        sparql_query(
            CATALOG_ASSET_DETAIL,
            asset_detail_text(),
            &[("asset_id", StoredParamType::String)],
        ),
    ]
}
