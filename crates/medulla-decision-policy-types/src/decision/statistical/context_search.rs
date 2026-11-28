//! MEDULLA-DECISION-ENGINE-R100.100.3: the typed request and result of a
//! context search over `Document` text. The offset kernel lives in
//! `medulla-decision-stats` (`decision::context_search`).
//!
//! Every offset is a half-open byte offset into the ORIGINAL UTF-8 text, and
//! each span names the digest of that text, so a reader can slice the exact
//! bytes it was given.

use medulla_agent_types::agent_component::DecisionPolicyRef;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Version of the context-search request and result shapes.
pub const CONTEXT_SEARCH_SCHEMA_VERSION: u16 = 1;

/// A context search: the n-grams of `query` are looked up in the stored text
/// of the `Document` nodes of `graph`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RetrievalContextSearchRequest {
    pub schema_version: u16,
    pub tenant_id: String,
    pub graph: String,
    pub query: String,
    pub ngram_n: u32,
    pub max_spans: u32,
    pub cursor: Option<String>,
    /// The policy whose `context_search.*` knobs the read uses: the engine
    /// default, or a pinned published policy of the caller's tenant
    /// (MEDULLA-DECISION-ENGINE-R100.100.4).
    #[serde(default = "default_policy_ref")]
    pub policy: DecisionPolicyRef,
}

fn default_policy_ref() -> DecisionPolicyRef {
    DecisionPolicyRef::Default
}

/// One match and the context around it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContextSpan {
    pub document_id: String,
    pub document_revision: u64,
    /// SHA-256 (64 lowercase hex characters) of the original text.
    pub text_digest: String,
    pub match_start: u64,
    pub match_end: u64,
    pub context_start: u64,
    pub context_end: u64,
    /// The original text between `context_start` and `context_end`.
    pub context_text: String,
}

/// The spans of one page and what they were computed against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RetrievalContextSearchResult {
    pub spans: Vec<ContextSpan>,
    pub query_revision: u64,
    pub index_digest: String,
    pub authorization_revision: u64,
    pub next_cursor: Option<String>,
}

/// SHA-256 of the concatenated `parts` as 64 lowercase hex characters.
pub fn sha256_hex(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The digest a span's `text_digest` names: SHA-256 of the original text.
pub fn text_digest(text: &str) -> String {
    sha256_hex(&[text.as_bytes()])
}
