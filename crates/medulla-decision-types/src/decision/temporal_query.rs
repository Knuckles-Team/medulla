//! The temporal context model (MEDULLA-DECISION-ENGINE-R146.1).
//!
//! Three clocks describe one fact: when the engine stored it, when the event
//! happened and when the narrative says it happened. A temporal query carries
//! the window it asked for and the context of the node it read; which basis
//! explains an empty first pass is derived by the `temporal_basis` family and
//! never stored.

use serde::{Deserialize, Serialize};

use medulla_types::contract::Digest256;

/// Refusal code: the window is inverted.
pub const TEMPORAL_WINDOW_INVALID: &str = "TEMPORAL_WINDOW_INVALID";

/// Format identity of [`TemporalQueryInput`].
pub const TEMPORAL_QUERY_SCHEMA_VERSION: u16 = 1;

/// The three clocks of one node, in milliseconds since the Unix epoch. The
/// caller reads event time from the node's valid-time fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TemporalContext {
    pub storage_ms: i64,
    #[serde(default)]
    pub event_ms: Option<i64>,
    #[serde(default)]
    pub narrative_ms: Option<i64>,
}

/// The clock a window is read against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TimeBasis {
    Event,
    Narrative,
    Storage,
}

/// A time-windowed query and the context its first pass read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TemporalQueryInput {
    pub schema_version: u16,
    pub query_digest: Digest256,
    pub window_from_ms: i64,
    pub window_to_ms: i64,
    pub first_pass_rows: u32,
    pub context: TemporalContext,
}

impl TemporalQueryInput {
    /// Refuses an inverted window with [`TEMPORAL_WINDOW_INVALID`].
    pub fn checked(self) -> Result<Self, &'static str> {
        if self.window_from_ms > self.window_to_ms {
            return Err(TEMPORAL_WINDOW_INVALID);
        }
        Ok(self)
    }
}

/// A retry of an earlier query on another basis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TemporalFallback {
    pub original_query_digest: Digest256,
    pub basis: TimeBasis,
    pub from_ms: i64,
    pub to_ms: i64,
    pub retry_index: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(from: i64, to: i64) -> TemporalQueryInput {
        TemporalQueryInput {
            schema_version: TEMPORAL_QUERY_SCHEMA_VERSION,
            query_digest: Digest256::sha256(b"q"),
            window_from_ms: from,
            window_to_ms: to,
            first_pass_rows: 0,
            context: TemporalContext {
                storage_ms: 1_700_000_000_000,
                event_ms: Some(1_600_000_000_000),
                narrative_ms: None,
            },
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R146.1
    #[test]
    fn temporal_context_round_trip() {
        let query = input(10, 20).checked().unwrap();
        let text = serde_json::to_string(&query).unwrap();
        assert_eq!(
            serde_json::from_str::<TemporalQueryInput>(&text).unwrap(),
            query
        );
        let fallback = TemporalFallback {
            original_query_digest: query.query_digest,
            basis: TimeBasis::Narrative,
            from_ms: 10,
            to_ms: 20,
            retry_index: 1,
        };
        let text = serde_json::to_string(&fallback).unwrap();
        assert!(text.contains("\"basis\":\"narrative\""));
        assert_eq!(
            serde_json::from_str::<TemporalFallback>(&text).unwrap(),
            fallback
        );
        for basis in [TimeBasis::Event, TimeBasis::Narrative, TimeBasis::Storage] {
            let text = serde_json::to_string(&basis).unwrap();
            assert_eq!(serde_json::from_str::<TimeBasis>(&text).unwrap(), basis);
        }
        assert_eq!(
            input(21, 20).checked().unwrap_err(),
            "TEMPORAL_WINDOW_INVALID"
        );
        assert!(input(20, 20).checked().is_ok());
        assert!(serde_json::from_str::<TemporalContext>(r#"{"storage_ms":1,"extra":2}"#).is_err());
    }
}
