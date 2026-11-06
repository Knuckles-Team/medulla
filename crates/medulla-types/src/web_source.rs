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
