//! The temporal context model (MEDULLA-DECISION-ENGINE-R146.1).
//!
//! Three clocks describe one fact: when the engine stored it, when the event
//! happened and when the narrative says it happened. A temporal query carries
//! the window it asked for and the context of the node it read; which basis
//! explains an empty first pass is derived by the `temporal_basis` family and
//! never stored.

use serde::{Deserialize, Serialize};

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;
use crate::decision::record::outcome::{DecisionRefusal, RefusalCode};
use medulla_types::contract::Digest256;

const fn knob(
    name: &'static str,
    unit: &'static str,
    default: i64,
    min: i64,
    max: i64,
) -> KnobSpec {
    KnobSpec {
        name,
        unit,
        default: KnobValue::Integer(default),
        min: KnobValue::Integer(min),
        max: KnobValue::Integer(max),
        safety: KnobSafety::Tunable,
    }
}

/// Largest gap between two present dates that is not yet a confusion.
pub const CONFUSION_GAP: KnobSpec = knob("temporal.confusion_gap", "days", 1, 0, 365);
/// Fewest first-pass rows that count as sufficient.
pub const MIN_HITS: KnobSpec = knob("temporal.min_hits", "rows", 3, 1, 1000);
/// How far each fallback window widens each side.
pub const WIDEN: KnobSpec = knob("temporal.widen", "days", 7, 1, 365);
/// Most fallback windows the kernel returns.
pub const MAX_RETRIES: KnobSpec = knob("temporal.max_retries", "retries", 1, 0, 3);

/// The `temporal.*` knobs.
pub const KNOBS: &[KnobSpec] = &[CONFUSION_GAP, MIN_HITS, WIDEN, MAX_RETRIES];

/// The resolved `temporal.*` values one decision reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemporalKnobs {
    pub confusion_gap_days: u32,
    pub min_hits: u32,
    pub widen_days: u32,
    pub max_retries: u32,
}

impl TemporalKnobs {
    /// The values the policy pins, or the defaults.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let value = |spec: &KnobSpec| match policy.knob(spec) {
            KnobValue::Integer(value) => u32::try_from(value).unwrap_or(0),
            _ => 0,
        };
        Self {
            confusion_gap_days: value(&CONFUSION_GAP),
            min_hits: value(&MIN_HITS),
            widen_days: value(&WIDEN),
            max_retries: value(&MAX_RETRIES),
        }
    }
}

/// Refusal code: the window is inverted.
pub const TEMPORAL_WINDOW_INVALID: RefusalCode = RefusalCode::new("TEMPORAL_WINDOW_INVALID");
/// Refusal code: the request carries no usable temporal context.
pub const TEMPORAL_CONTEXT_MISSING: RefusalCode = RefusalCode::new("TEMPORAL_CONTEXT_MISSING");
/// Refusal code: the declared options are not the three time bases.
pub const TEMPORAL_BASIS_UNMAPPABLE: RefusalCode = RefusalCode::new("TEMPORAL_BASIS_UNMAPPABLE");

/// The `REFUSALS` table of the `temporal_basis` family, registered in
/// `codes::FAMILY_TABLES` (MEDULLA-DECISION-ENGINE-R146.4).
pub const REFUSALS: &[RefusalCode] = &[
    TEMPORAL_CONTEXT_MISSING,
    TEMPORAL_WINDOW_INVALID,
    TEMPORAL_BASIS_UNMAPPABLE,
];

/// A question the `temporal_basis` family abstains from: the context it needs
/// is not there, and it names the field that is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemporalRefusal {
    /// No `temporal_query` parameter, or a context with no usable date.
    ContextMissing { field: &'static str },
}

impl DecisionRefusal for TemporalRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::ContextMissing { .. } => TEMPORAL_CONTEXT_MISSING,
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::ContextMissing { field } => format!("{field} is missing"),
        }
    }
}

/// The three bases a `temporal_basis` question declares, in basis order.
pub const BASIS_OPTIONS: [&str; 3] = ["event", "narrative", "storage"];

impl TimeBasis {
    /// The declared option id of the basis.
    pub fn option_id(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Narrative => "narrative",
            Self::Storage => "storage",
        }
    }
}

impl TemporalContext {
    /// Whether the context carries a date a basis can be read from: an event
    /// or narrative date, or a storage stamp other than the unset epoch 0.
    pub fn has_usable_date(&self) -> bool {
        self.event_ms.is_some() || self.narrative_ms.is_some() || self.storage_ms != 0
    }

    /// Whether the date of `basis` is present. An unset storage stamp (0) is absent.
    pub fn has_date(&self, basis: TimeBasis) -> bool {
        match basis {
            TimeBasis::Event => self.event_ms.is_some(),
            TimeBasis::Narrative => self.narrative_ms.is_some(),
            TimeBasis::Storage => self.storage_ms != 0,
        }
    }
}

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
    pub fn checked(self) -> Result<Self, RefusalCode> {
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
            input(21, 20).checked().unwrap_err().as_str(),
            "TEMPORAL_WINDOW_INVALID"
        );
        assert!(input(20, 20).checked().is_ok());
        assert!(serde_json::from_str::<TemporalContext>(r#"{"storage_ms":1,"extra":2}"#).is_err());
    }
}
