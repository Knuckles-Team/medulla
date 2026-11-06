//! `ContextFacts` wire types and the pure fact selection behind them
//! (MEDULLA-DECISION-ENGINE-R152.1, R152.2, R152.4, R152.5).
//!
//! A caller that ranked node ids with `Discover` passes them here and gets back
//! cited facts of those nodes inside a token budget. Every fact names its source
//! and its evidence class. The graph read lives in `medulla-h-graph`; this module
//! holds the shapes, the refusals and the deterministic, budgeted selection.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::contract::ServerErrorCode;
use crate::evidence_class::EvidenceClass;

/// Schema version carried by every `ContextFacts` type.
pub const CONTEXT_FACTS_SCHEMA_VERSION: u32 = 1;

/// Largest number of node ids one request may carry.
pub const MAX_CONTEXT_FACT_NODES: usize = 512;

/// Every refusal code of the `context_facts` family.
pub const REFUSALS: &[&str] = &[
    "CONTEXT_FACTS_BUDGET_ZERO",
    "CONTEXT_FACTS_TOO_MANY_NODES",
    "FACT_PROVENANCE_MISSING",
];

fn schema_version() -> u32 {
    CONTEXT_FACTS_SCHEMA_VERSION
}

/// Ask for the cited facts of ranked nodes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContextFactsRequest {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    /// Node ids in the caller's order.
    pub node_ids: Vec<String>,
    /// Token budget of the whole bundle; 0 is refused.
    pub token_budget: u64,
    /// Per-node fact cap; absent means knob `context_facts.max_facts_per_node`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_facts_per_node: Option<u32>,
}

/// One cited fact of a visible node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContextFact {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    /// SHA-256 over subject, predicate and object, 64 lowercase hex characters.
    pub fact_id: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    /// The source the fact was ingested from.
    pub source_key: String,
    /// The stored evidence class of the fact.
    pub authority: EvidenceClass,
    /// Fact byte length divided by knob `context_facts.bytes_per_token`, rounded up.
    pub token_estimate: u64,
}

/// The bundle of cited facts inside the budget.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContextFactsResult {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub facts: Vec<ContextFact>,
    /// Facts left out by the per-node cap or the budget.
    pub omitted_count: u64,
    pub token_estimate_total: u64,
}

/// A fact read from the graph before selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawFact {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    /// `None` when the visible property or edge names no source.
    pub source_key: Option<String>,
    pub authority: EvidenceClass,
}

/// A typed refusal of a `ContextFacts` request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextFactsRefusal {
    pub code: ServerErrorCode,
    pub detail: String,
}

impl std::fmt::Display for ContextFactsRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl std::error::Error for ContextFactsRefusal {}

/// The stable id of a fact: SHA-256 over the length-prefixed subject, predicate
/// and object, as 64 lowercase hex characters.
pub fn fact_id(subject: &str, predicate: &str, object: &str) -> String {
    let mut hasher = Sha256::new();
    for part in [subject, predicate, object] {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Token estimate of one fact: byte length divided by `bytes_per_token`, rounded up.
pub fn token_estimate(subject: &str, predicate: &str, object: &str, bytes_per_token: u64) -> u64 {
    let bytes = (subject.len() + predicate.len() + object.len()) as u64;
    bytes.div_ceil(bytes_per_token.max(1))
}

/// Refuse a request with a zero budget or too many node ids.
pub fn validate_request(request: &ContextFactsRequest) -> Result<(), ContextFactsRefusal> {
    if request.token_budget == 0 {
        return Err(ContextFactsRefusal {
            code: ServerErrorCode::ContextFactsBudgetZero,
            detail: "token_budget must be at least 1".to_string(),
        });
    }
    if request.node_ids.len() > MAX_CONTEXT_FACT_NODES {
        return Err(ContextFactsRefusal {
            code: ServerErrorCode::ContextFactsTooManyNodes,
            detail: format!(
                "at most {MAX_CONTEXT_FACT_NODES} node ids, got {}",
                request.node_ids.len()
            ),
        });
    }
    Ok(())
}

/// Select facts inside the budget. `per_node` holds one fact list per node id in
/// the caller's order. Each node's facts sort by predicate, then object, keep at
/// most `max_facts_per_node`, and are then taken in order until the first fact
/// that would pass `token_budget`; that fact and the rest are counted in
/// `omitted_count`. A kept fact with no source refuses the whole request.
pub fn select(
    per_node: Vec<Vec<RawFact>>,
    token_budget: u64,
    max_facts_per_node: usize,
    bytes_per_token: u64,
) -> Result<ContextFactsResult, ContextFactsRefusal> {
    let mut omitted = 0u64;
    let mut kept: Vec<(RawFact, String)> = Vec::new();
    for mut facts in per_node {
        facts.sort_by(|a, b| {
            (&a.predicate, &a.object, &a.subject).cmp(&(&b.predicate, &b.object, &b.subject))
        });
        facts.dedup();
        if facts.len() > max_facts_per_node {
            omitted += (facts.len() - max_facts_per_node) as u64;
            facts.truncate(max_facts_per_node);
        }
        for fact in facts {
            let Some(source_key) = fact.source_key.clone().filter(|s| !s.is_empty()) else {
                return Err(ContextFactsRefusal {
                    code: ServerErrorCode::FactProvenanceMissing,
                    detail: format!(
                        "fact `{}` of `{}` names no source_key",
                        fact.predicate, fact.subject
                    ),
                });
            };
            kept.push((fact, source_key));
        }
    }

    let mut facts = Vec::new();
    let mut total = 0u64;
    let mut stopped = false;
    for (raw, source_key) in kept {
        if stopped {
            omitted += 1;
            continue;
        }
        let estimate = token_estimate(&raw.subject, &raw.predicate, &raw.object, bytes_per_token);
        if total.saturating_add(estimate) > token_budget {
            stopped = true;
            omitted += 1;
            continue;
        }
        total += estimate;
        facts.push(ContextFact {
            schema_version: CONTEXT_FACTS_SCHEMA_VERSION,
            fact_id: fact_id(&raw.subject, &raw.predicate, &raw.object),
            subject: raw.subject,
            predicate: raw.predicate,
            object: raw.object,
            source_key,
            authority: raw.authority,
            token_estimate: estimate,
        });
    }
    Ok(ContextFactsResult {
        schema_version: CONTEXT_FACTS_SCHEMA_VERSION,
        facts,
        omitted_count: omitted,
        token_estimate_total: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(subject: &str, predicate: &str, object: &str) -> RawFact {
        RawFact {
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: object.to_string(),
            source_key: Some("src-1".to_string()),
            authority: EvidenceClass::Observation,
        }
    }

    fn fixture() -> Vec<Vec<RawFact>> {
        vec![
            vec![
                raw("a", "name", "Alpha"),
                raw("a", "kind", "Tool"),
                raw("a", "name", "Aleph"),
            ],
            vec![raw("b", "kind", "Skill"), raw("b", "owner", "team-b")],
        ]
    }

    // spec: MEDULLA-DECISION-ENGINE-R152.1
    #[test]
    fn context_fact_round_trip() {
        let result = select(fixture(), 1_000, 16, 4).unwrap();
        let json = serde_json::to_string(&result).unwrap();
        let back: ContextFactsResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, result);
        let again = select(fixture(), 1_000, 16, 4).unwrap();
        for (a, b) in result.facts.iter().zip(again.facts.iter()) {
            assert_eq!(a.fact_id, b.fact_id);
            assert_eq!(a.fact_id.len(), 64);
            assert!(a
                .fact_id
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
            assert_eq!(a.fact_id, fact_id(&a.subject, &a.predicate, &a.object));
        }
        let request = ContextFactsRequest {
            schema_version: CONTEXT_FACTS_SCHEMA_VERSION,
            node_ids: vec!["a".into()],
            token_budget: 10,
            max_facts_per_node: Some(2),
        };
        let back: ContextFactsRequest =
            serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
        assert_eq!(back, request);
    }

    // spec: MEDULLA-DECISION-ENGINE-R152.2
    #[test]
    fn selection_is_deterministic_and_within_budget() {
        let first = select(fixture(), 6, 16, 4).unwrap();
        let second = select(fixture(), 6, 16, 4).unwrap();
        assert_eq!(first, second);
        assert!(first.token_estimate_total <= 6);
        assert_eq!(
            first.facts.len() as u64 + first.omitted_count,
            5,
            "every fact is returned or counted"
        );
        assert!(first.omitted_count > 0);
        // Sorted by predicate then object inside a node, in caller node order.
        let order: Vec<(String, String)> = select(fixture(), 1_000, 16, 4)
            .unwrap()
            .facts
            .into_iter()
            .map(|f| (f.subject, f.predicate))
            .collect();
        let expected: Vec<(String, String)> = [
            ("a", "kind"),
            ("a", "name"),
            ("a", "name"),
            ("b", "kind"),
            ("b", "owner"),
        ]
        .iter()
        .map(|(s, p)| (s.to_string(), p.to_string()))
        .collect();
        assert_eq!(order, expected);
        // The per-node cap counts what it drops.
        let capped = select(fixture(), 1_000, 1, 4).unwrap();
        assert_eq!(capped.facts.len(), 2);
        assert_eq!(capped.omitted_count, 3);
    }

    #[test]
    fn refusals_are_registered_codes() {
        for code in [
            ServerErrorCode::ContextFactsBudgetZero,
            ServerErrorCode::ContextFactsTooManyNodes,
            ServerErrorCode::FactProvenanceMissing,
        ] {
            assert!(REFUSALS.contains(&code.as_str()));
        }
    }
}
