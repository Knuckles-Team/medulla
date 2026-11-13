//! The freshness refresh model (MEDULLA-DECISION-ENGINE-R174.9.1): a request
//! to re-read fresh data through a search source, and one result it returns.
//!
//! Types only. They carry the policy references the later behaviour checks;
//! this module refuses nothing but a malformed shape.

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;

/// Format identity of both types.
pub const FRESHNESS_SCHEMA_VERSION: u16 = 1;
/// Most query texts one refresh may carry.
pub const MAX_REFRESH_QUERIES: usize = 32;

/// Why a refresh was asked for. Closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RefreshTrigger {
    DriftSignal,
    Staleness,
    JobDemand,
}

/// What a refresh may spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RefreshBudget {
    pub max_requests: u32,
    pub max_cost_units: u64,
}

/// A request to refresh one logical graph from a search source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FreshnessRefresh {
    pub schema_version: u16,
    pub logical_graph: String,
    /// `1..=32` query texts.
    pub queries: BoundedVec<String, MAX_REFRESH_QUERIES>,
    pub trigger: RefreshTrigger,
    /// Lower runs first.
    pub priority: u8,
    pub acl_policy_ref: String,
    pub privacy_policy_ref: String,
    pub budget: RefreshBudget,
}

/// One fresh result a refresh stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FreshResult {
    pub schema_version: u16,
    pub url: String,
    /// SHA-256 of the content: 64 lowercase hex characters.
    pub content_digest: String,
    pub source_connection_iri: String,
    /// Epoch milliseconds.
    pub retrieved_at: u64,
    pub query_text: String,
    pub query_digest: String,
    pub refresh_id: String,
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl FreshnessRefresh {
    /// The validating check: the served version and `1..=32` non-empty queries.
    pub fn check(&self) -> Result<(), String> {
        if self.schema_version != FRESHNESS_SCHEMA_VERSION {
            return Err(format!(
                "freshness refresh version {} is not served",
                self.schema_version
            ));
        }
        if self.queries.is_empty() || self.queries.iter().any(|q| q.trim().is_empty()) {
            return Err("a refresh carries 1 to 32 non-empty query texts".to_string());
        }
        Ok(())
    }
}

impl FreshResult {
    /// The validating check: the served version and a well-formed digest.
    pub fn check(&self) -> Result<(), String> {
        if self.schema_version != FRESHNESS_SCHEMA_VERSION {
            return Err(format!(
                "fresh result version {} is not served",
                self.schema_version
            ));
        }
        if !is_sha256_hex(&self.content_digest) {
            return Err("content_digest is 64 lowercase hex characters".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refresh_json(queries: usize, trigger: &str) -> String {
        let queries = vec!["\"q\""; queries].join(",");
        format!(
            r#"{{"schema_version":1,"logical_graph":"g","queries":[{queries}],
            "trigger":"{trigger}","priority":3,"acl_policy_ref":"acl","privacy_policy_ref":"p",
            "budget":{{"max_requests":4,"max_cost_units":9}}}}"#
        )
    }

    // spec: MEDULLA-DECISION-ENGINE-R174.9.1
    #[test]
    fn refresh_round_trips() {
        for trigger in ["drift_signal", "staleness", "job_demand"] {
            let refresh: FreshnessRefresh =
                serde_json::from_str(&refresh_json(32, trigger)).unwrap();
            refresh.check().unwrap();
            let again = serde_json::to_string(&refresh).unwrap();
            assert_eq!(
                serde_json::from_str::<FreshnessRefresh>(&again).unwrap(),
                refresh
            );
        }
        assert!(serde_json::from_str::<FreshnessRefresh>(&refresh_json(33, "staleness")).is_err());
        assert!(serde_json::from_str::<FreshnessRefresh>(&refresh_json(1, "whim")).is_err());
        let empty: FreshnessRefresh = serde_json::from_str(&refresh_json(0, "staleness")).unwrap();
        assert!(empty.check().is_err());

        let result = FreshResult {
            schema_version: FRESHNESS_SCHEMA_VERSION,
            url: "https://example.org/a".into(),
            content_digest: "a".repeat(64),
            source_connection_iri: "urn:conn".into(),
            retrieved_at: 7,
            query_text: "q".into(),
            query_digest: "b".repeat(64),
            refresh_id: "r1".into(),
        };
        result.check().unwrap();
        let again = serde_json::to_string(&result).unwrap();
        assert_eq!(serde_json::from_str::<FreshResult>(&again).unwrap(), result);
        let upper = FreshResult {
            content_digest: "A".repeat(64),
            ..result.clone()
        };
        assert!(upper.check().is_err());
        let short = FreshResult {
            content_digest: "a".repeat(63),
            ..result
        };
        assert!(short.check().is_err());
    }
}
