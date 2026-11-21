//! The registered catalog stored queries (MEDULLA-TYPED-PACKS-R802.8).
//!
//! The definitions are written once here and put with `Method::StoredQueryPut` at tenant
//! bootstrap, like the owner digest of `business_digest`. A query is data, not code.

use std::collections::BTreeMap;

use medulla_identity_types::identity::ShareScope;

use crate::stored_query::{RefreshPolicy, StoredQuery, StoredQueryLanguage};

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

/// The registered catalog queries, in the order they are put.
pub fn catalog_queries() -> [StoredQuery; 1] {
    [StoredQuery {
        name: CATALOG_CRITICAL_GAPS.to_string(),
        version: 1,
        language: StoredQueryLanguage::Uql,
        text: CRITICAL_GAPS_TEXT.to_string(),
        params: BTreeMap::new(),
        materialize: false,
        refresh: RefreshPolicy::OnChange,
        // The engine stamps `author` on put and ignores whatever a request carries.
        author: String::new(),
        share: ShareScope::Private,
    }]
}
