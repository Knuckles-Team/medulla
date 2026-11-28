//! The server implementation of the one expansion port
//! (MEDULLA-FEDERATED-QUERY-R651.3, MEDULLA-FEDERATED-QUERY-R651.7).
//!
//! `HttpExpansionProvider` asks the model configured in the `nl_query` section
//! (`expansion_model`, default the planner model; `expansion_read_timeout_secs`,
//! default 30, range 1 to 600) for at most `perspectives` items, parses a JSON list
//! of strings and returns them as data. Its output is never a decision. It is
//! installed once at server start by [`install_expansion_provider`].
//!
//! Kinds: a request that carries term feedback (the feedback operator) yields
//! `perspective` items; a plain request yields one `hypothetical` passage first,
//! then `sub_query` items.

use std::collections::HashSet;
use std::sync::Arc;

use medulla_nlq::nl::UreqNlPlanner;
use medulla_plan::query_understanding::{
    set_expansion_provider, Expansion, ExpansionError, ExpansionKind, ExpansionProvider,
    ExpansionRequest,
};
use sha2::{Digest, Sha256};

/// Largest hypothetical passage, in bytes.
pub const MAX_HYPOTHETICAL_BYTES: usize = 512;

/// One chat completion: system and user text in, raw model text out. The seam the
/// provider calls through, so the parse is testable without a network.
pub trait ChatComplete: Send + Sync {
    fn complete(&self, system: &str, user: &str) -> Result<String, String>;
    /// The model this client asks.
    fn model(&self) -> String;
}

impl ChatComplete for UreqNlPlanner {
    fn complete(&self, system: &str, user: &str) -> Result<String, String> {
        UreqNlPlanner::complete(self, system, user)
    }

    fn model(&self) -> String {
        UreqNlPlanner::model(self).to_string()
    }
}

/// The HTTP-backed expansion provider.
pub struct HttpExpansionProvider {
    client: Box<dyn ChatComplete>,
}

impl HttpExpansionProvider {
    pub fn new(client: Box<dyn ChatComplete>) -> Self {
        Self { client }
    }
}

fn system_prompt() -> &'static str {
    "You expand a search query. Answer with ONLY a JSON list of strings, no prose and \
     no code fence."
}

fn user_prompt(req: &ExpansionRequest) -> String {
    let n = req.perspectives;
    if req.feedback.is_empty() {
        format!(
            "Query: {}\nReturn at most {n} items. The first item is a short hypothetical \
             passage (under {MAX_HYPOTHETICAL_BYTES} bytes) that would answer the query. \
             Every later item is a sub-query that covers one part of the query.",
            req.query
        )
    } else {
        let terms = req
            .feedback
            .iter()
            .map(|f| format!("{} ({:?})", f.term, f.tier))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Query: {}\nTerm feedback: {terms}\nReturn at most {n} items, each a distinct \
             rephrasing of the query from a different perspective.",
            req.query
        )
    }
}

/// SHA-256 hex of the expansion text: the digest the evidence lists.
pub fn expansion_digest(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Parse the model answer: a JSON list of strings. Empty items, duplicates by digest
/// and items past the request are dropped; an oversized hypothetical is cut at a
/// character boundary. Anything that is not a JSON list of strings is
/// `ProviderFailed`.
pub fn parse_expansions(
    raw: &str,
    req: &ExpansionRequest,
) -> Result<Vec<Expansion>, ExpansionError> {
    let body = strip_fence(raw);
    let items: Vec<String> =
        serde_json::from_str(body).map_err(|e| ExpansionError::ProviderFailed {
            message: format!("answer is not a JSON list of strings: {e}"),
        })?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let text = item.trim().to_string();
        if text.is_empty() {
            continue;
        }
        let kind = if !req.feedback.is_empty() {
            ExpansionKind::Perspective
        } else if out.is_empty() {
            ExpansionKind::Hypothetical
        } else {
            ExpansionKind::SubQuery
        };
        let text = if kind == ExpansionKind::Hypothetical {
            truncate_bytes(&text, MAX_HYPOTHETICAL_BYTES)
        } else {
            text
        };
        let digest = expansion_digest(&text);
        if !seen.insert(digest.clone()) {
            continue;
        }
        out.push(Expansion { text, kind, digest });
        if out.len() == usize::from(req.perspectives) {
            break;
        }
    }
    Ok(out)
}

fn strip_fence(raw: &str) -> &str {
    let t = raw.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

fn truncate_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

impl ExpansionProvider for HttpExpansionProvider {
    fn expand(&self, req: &ExpansionRequest) -> Result<Vec<Expansion>, ExpansionError> {
        req.validate()?;
        let raw = self
            .client
            .complete(system_prompt(), &user_prompt(req))
            .map_err(|message| ExpansionError::ProviderFailed { message })?;
        parse_expansions(&raw, req)
    }

    fn provider_id(&self) -> String {
        format!("http-chat:{}", self.client.model())
    }

    fn provider_revision(&self) -> String {
        expansion_digest(system_prompt())
    }
}

/// Install the provider from the `nl_query` config section, once, at server start.
/// No endpoint configured leaves the port unconfigured, which refuses typed. A
/// malformed section is logged and leaves it unconfigured too.
pub fn install_expansion_provider() {
    match crate::nl::build_expansion_client() {
        Ok(Some(client)) => {
            set_expansion_provider(Arc::new(HttpExpansionProvider::new(Box::new(client))))
        }
        Ok(None) => {}
        Err(e) => eprintln!("expansion provider not installed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(feedback: bool) -> ExpansionRequest {
        ExpansionRequest {
            query: "how do replicas recover".into(),
            perspectives: 3,
            feedback: if feedback {
                vec![medulla_result_types::retrieval_verdict::TermFeedback {
                    term: "replica".into(),
                    tier: medulla_result_types::retrieval_verdict::FeedbackTier::Reinforce,
                    weight: 0.5,
                    evidence_document_ids: vec![],
                }]
            } else {
                vec![]
            },
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R651.7
    #[test]
    fn parse_drops_duplicates_and_oversize() {
        let big = "x".repeat(900);
        let raw = format!(r#"["{big}", "  ", "a", "a", "b", "c"]"#);
        let out = parse_expansions(&raw, &req(false)).unwrap();
        assert_eq!(
            out.len(),
            3,
            "capped at perspectives, blanks and dups dropped"
        );
        assert_eq!(out[0].kind, ExpansionKind::Hypothetical);
        assert_eq!(out[0].text.len(), MAX_HYPOTHETICAL_BYTES);
        assert_eq!(out[1].kind, ExpansionKind::SubQuery);
        assert_eq!(out[1].text, "a");
        assert_eq!(out[2].text, "b");
        assert_eq!(out[1].digest, expansion_digest("a"));
        let fb = parse_expansions(r#"["p1","p2"]"#, &req(true)).unwrap();
        assert!(fb.iter().all(|e| e.kind == ExpansionKind::Perspective));
        let bad = parse_expansions("not json", &req(false)).unwrap_err();
        assert!(matches!(bad, ExpansionError::ProviderFailed { .. }));
        let fenced = parse_expansions("```json\n[\"q\"]\n```", &req(false)).unwrap();
        assert_eq!(fenced.len(), 1);
    }

    struct Scripted(Result<String, String>);
    impl ChatComplete for Scripted {
        fn complete(&self, _: &str, _: &str) -> Result<String, String> {
            self.0.clone()
        }
        fn model(&self) -> String {
            "scripted".into()
        }
    }

    #[test]
    fn provider_failure_is_typed() {
        let p = HttpExpansionProvider::new(Box::new(Scripted(Err("down".into()))));
        match p.expand(&req(false)) {
            Err(ExpansionError::ProviderFailed { message }) => assert_eq!(message, "down"),
            other => panic!("{other:?}"),
        }
        let ok = HttpExpansionProvider::new(Box::new(Scripted(Ok(r#"["h","s"]"#.into()))));
        assert_eq!(ok.expand(&req(false)).unwrap().len(), 2);
    }
}
