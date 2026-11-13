//! The web metasearch capability and its result record (MEDULLA-UNIFIED-DATA-PLANE-R038.7.1).
//!
//! Registration derives a [`WebSearchCapability`] from the connector tool's declared schema
//! and stores it in the foreign-source catalog entry. A [`WebResult`] maps to `cv:SearchResult`
//! (MEDULLA-MEDIA-VOCAB-R004.4). There is no time-range field: the metasearch tool declares
//! `query`, `categories`, `engines`, `language` and `pageno` only.

use serde::{Deserialize, Serialize};

use crate::contract::ServerErrorCode;

/// What one bound web metasearch tool accepts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSearchCapability {
    /// Result categories the tool can be asked for.
    pub categories: Vec<String>,
    /// Engines the tool can be asked to use.
    pub engines: Vec<String>,
    /// Languages the tool can be asked for.
    pub languages: Vec<String>,
    /// The most results one call returns.
    pub max_results: u32,
}

/// One web search result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebResult {
    /// The result URL; never empty.
    pub url: String,
    /// The result title.
    pub title: String,
    /// The text snippet the engine returned.
    pub snippet: String,
    /// The engine that returned it.
    pub engine: String,
    /// The 1-based rank within the engine's results.
    pub rank: u32,
    /// Unix seconds at which the result was retrieved.
    pub retrieved_at: i64,
}

/// The fetch candidates one web query produced (MEDULLA-UNIFIED-DATA-PLANE-R038.7.7). The engine
/// stores none of them; the fetch selection of `MEDULLA-DECISION-ENGINE-R802` reads the set and the
/// consumer ingest path chooses which pages to fetch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebCandidateSet {
    /// The query text the results answer.
    pub query_text: String,
    /// SHA-256 of the pushed arguments, 64 lowercase hex characters.
    pub query_digest: String,
    /// The candidates in rank order; every one has a URL.
    pub candidates: Vec<WebResult>,
    /// Unix seconds of the newest retrieval among the candidates (0 when there are none).
    pub retrieved_at: i64,
}

impl WebCandidateSet {
    /// Build a set from `results`, refusing the whole set when any result has no URL.
    pub fn from_results(
        query_text: String,
        query_digest: String,
        results: Vec<WebResult>,
    ) -> Result<Self, WebResultRefusal> {
        for result in &results {
            result.validate()?;
        }
        let retrieved_at = results.iter().map(|r| r.retrieved_at).max().unwrap_or(0);
        Ok(Self {
            query_text,
            query_digest,
            candidates: results,
            retrieved_at,
        })
    }
}

/// The web provenance of a stored web Document node (MEDULLA-FEDERATED-QUERY-R653.7).
///
/// A node is a web Document when it carries a non-empty `url`. The other fields come from the
/// web result record ([`WebResult`], [`WebCandidateSet`]) the node was ingested from. A node
/// without a URL has no web provenance and every field is `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebProvenance {
    /// The source URL.
    pub url: Option<String>,
    /// Unix seconds at which the page was retrieved.
    pub retrieved_at: Option<i64>,
    /// The engine or fetch backend (`backend`, else `engine`).
    pub backend: Option<String>,
    /// SHA-256 of the originating query.
    pub query_digest: Option<String>,
}

impl WebProvenance {
    /// Read the provenance off a decoded node property object.
    pub fn from_properties(object: &serde_json::Map<String, serde_json::Value>) -> Self {
        let text = |key: &str| {
            object
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let Some(url) = text("url") else {
            return Self::default();
        };
        Self {
            url: Some(url),
            retrieved_at: object
                .get("retrieved_at")
                .and_then(serde_json::Value::as_i64),
            backend: text("backend").or_else(|| text("engine")),
            query_digest: text("query_digest"),
        }
    }
}

/// A refused web result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebResultRefusal {
    /// Always `WEB_RESULT_URL_MISSING`.
    pub code: ServerErrorCode,
}

impl std::fmt::Display for WebResultRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: a web result needs a URL", self.code)
    }
}

impl std::error::Error for WebResultRefusal {}

impl WebResult {
    /// A result with an empty `url` is refused with `WEB_RESULT_URL_MISSING`.
    pub fn validate(&self) -> Result<(), WebResultRefusal> {
        if self.url.trim().is_empty() {
            return Err(WebResultRefusal {
                code: ServerErrorCode::WebResultUrlMissing,
            });
        }
        Ok(())
    }

    /// Decode and validate one result from JSON.
    pub fn from_json(json: &str) -> Result<Self, WebResultRefusal> {
        let result: Self = serde_json::from_str(json).map_err(|_| WebResultRefusal {
            code: ServerErrorCode::WebResultUrlMissing,
        })?;
        result.validate()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies MEDULLA-FEDERATED-QUERY-R653.7: a node with a URL yields web provenance and one
    /// without yields none.
    // spec: MEDULLA-FEDERATED-QUERY-R653.7
    #[test]
    fn web_provenance_reads_only_web_documents() {
        let web = serde_json::json!({
            "url": "https://example.com/story",
            "retrieved_at": 1_700_000_000_i64,
            "engine": "duckduckgo",
            "query_digest": "ab",
        });
        let read = WebProvenance::from_properties(web.as_object().unwrap());
        assert_eq!(read.url.as_deref(), Some("https://example.com/story"));
        assert_eq!(read.retrieved_at, Some(1_700_000_000));
        assert_eq!(read.backend.as_deref(), Some("duckduckgo"));
        assert_eq!(read.query_digest.as_deref(), Some("ab"));
        let local = serde_json::json!({"retrieved_at": 5, "backend": "x"});
        assert_eq!(
            WebProvenance::from_properties(local.as_object().unwrap()),
            WebProvenance::default()
        );
    }

    fn result() -> WebResult {
        WebResult {
            url: "https://example.com/story".into(),
            title: "Story".into(),
            snippet: "A snippet".into(),
            engine: "duckduckgo".into(),
            rank: 2,
            retrieved_at: 1_700_000_000,
        }
    }

    /// Verifies MEDULLA-UNIFIED-DATA-PLANE-R038.7.1: Model: `WebSearchCapability` and `WebResult`
    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.7.1
    #[test]
    fn web_result_round_trips() {
        let original = result();
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(WebResult::from_json(&json).unwrap(), original);
        let capability = WebSearchCapability {
            categories: vec!["general".into()],
            engines: vec!["duckduckgo".into()],
            languages: vec!["en".into()],
            max_results: 20,
        };
        let json = serde_json::to_string(&capability).unwrap();
        assert_eq!(
            serde_json::from_str::<WebSearchCapability>(&json).unwrap(),
            capability
        );
        let no_url = WebResult {
            url: String::new(),
            ..original
        };
        let refused = WebResult::from_json(&serde_json::to_string(&no_url).unwrap()).unwrap_err();
        assert_eq!(refused.code, ServerErrorCode::WebResultUrlMissing);
    }
}
