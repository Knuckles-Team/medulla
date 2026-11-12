//! Revision-bound structured code maps (MEDULLA-REPO-INGEST-R006.3).
//!
//! `Method::UnifiedQuery` carries an optional [`CodeMapQuery`]. The served
//! answer is a [`CodeMapResult`]: the entries (files and symbols) and edges
//! (contains, calls, depends-on) of one indexed repository revision, in pages.
//! A page binds the map generation, the query digest and the auth scope, so a
//! cursor never survives a reindex or a change of reader.

use serde::{Deserialize, Serialize};

/// Largest page a code-map request may ask for (`Fixed`).
pub const MAX_CODE_MAP_PAGE_LIMIT: u32 = 1000;

/// Stable refusal: the requested revision is not the repository's current one,
/// or a cursor was minted for another map generation.
pub const CODE_MAP_STALE: &str = "CODE_MAP_STALE";

/// Stable refusal: the request is malformed or its cursor was minted for
/// another query or another reader.
pub const CODE_MAP_INVALID: &str = "CODE_MAP_INVALID";

/// One relation family of the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CodeRelation {
    /// A file contains a symbol.
    Contains,
    /// A symbol calls a symbol.
    Calls,
    /// A file depends on a file.
    DependsOn,
}

impl CodeRelation {
    /// Every relation, in canonical order.
    pub const ALL: [Self; 3] = [Self::Contains, Self::Calls, Self::DependsOn];
}

/// The `code_map` field of `Method::UnifiedQuery`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CodeMapQuery {
    /// Repository identity the index was submitted under.
    pub repo: String,
    /// Immutable revision the caller read the map for.
    pub source_revision: String,
    /// Free text locating behavior; entries whose name, kind or path share a
    /// token rank first and the rest are dropped.
    #[serde(default)]
    pub behavior_text: Option<String>,
    /// Exact symbol name to focus on.
    #[serde(default)]
    pub symbol: Option<String>,
    /// Relations to return; empty means all three.
    #[serde(default)]
    pub relations: Vec<CodeRelation>,
    /// Entries per page, `1..=MAX_CODE_MAP_PAGE_LIMIT`.
    pub page_limit: u32,
    /// Cursor of the previous page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// One file or symbol of the map.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CodeMapEntry {
    /// Stable logical id: a symbol keeps it while its file content is unchanged.
    pub id: String,
    /// `file` or the symbol type.
    pub kind: String,
    pub name: String,
    pub path: String,
    /// First line of the span, 0 for a file.
    pub line: u32,
    /// Last line of the span, 0 for a file.
    pub end_line: u32,
}

/// One relation between two entries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CodeMapEdge {
    pub source: String,
    pub target: String,
    pub relation: CodeRelation,
}

/// A relation the index could not resolve to an entry of this map.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CodeMapIncomplete {
    pub source: String,
    pub relation: CodeRelation,
    /// The name the source refers to.
    pub name: String,
}

/// One page of a code map.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CodeMapResult {
    /// Generation of the map the page was cut from; changes with every reindex
    /// that moves a ref.
    pub map_generation: u64,
    /// `sha256:<hex>` over the revision's sorted `(path, blob digest)` list.
    pub inventory_digest: String,
    /// `sha256:<hex>` over the request without its cursor.
    pub query_digest: String,
    /// `sha256:<hex>` over the verified reader's principal, tenant and graph.
    pub auth_scope_digest: String,
    pub entries: Vec<CodeMapEntry>,
    pub edges: Vec<CodeMapEdge>,
    pub incomplete: Vec<CodeMapIncomplete>,
    pub next_cursor: Option<String>,
}

impl CodeMapQuery {
    /// Refuse a malformed request before anything is read.
    pub fn validate(&self) -> Result<(), String> {
        if self.repo.trim().is_empty() || self.source_revision.trim().is_empty() {
            return Err(format!(
                "{CODE_MAP_INVALID}: repo and source_revision are required"
            ));
        }
        if self.page_limit == 0 || self.page_limit > MAX_CODE_MAP_PAGE_LIMIT {
            return Err(format!(
                "{CODE_MAP_INVALID}: page_limit must be in 1..={MAX_CODE_MAP_PAGE_LIMIT}"
            ));
        }
        Ok(())
    }

    /// The relations to return, deduplicated in canonical order.
    pub fn wanted_relations(&self) -> Vec<CodeRelation> {
        CodeRelation::ALL
            .into_iter()
            .filter(|relation| self.relations.is_empty() || self.relations.contains(relation))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(page_limit: u32) -> CodeMapQuery {
        CodeMapQuery {
            repo: "r".into(),
            source_revision: "a".repeat(40),
            behavior_text: None,
            symbol: None,
            relations: vec![],
            page_limit,
            cursor: None,
        }
    }

    #[test]
    fn page_limit_is_bounded() {
        assert!(query(1).validate().is_ok());
        assert!(query(MAX_CODE_MAP_PAGE_LIMIT).validate().is_ok());
        assert!(query(0).validate().is_err());
        assert!(query(MAX_CODE_MAP_PAGE_LIMIT + 1).validate().is_err());
    }

    #[test]
    fn empty_relations_mean_all() {
        assert_eq!(query(1).wanted_relations().len(), 3);
        let mut only_calls = query(1);
        only_calls.relations = vec![CodeRelation::Calls, CodeRelation::Calls];
        assert_eq!(only_calls.wanted_relations(), vec![CodeRelation::Calls]);
    }
}
