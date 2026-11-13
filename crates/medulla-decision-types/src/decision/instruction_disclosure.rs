//! Instruction disclosure events (MEDULLA-DECISION-ENGINE-R143.1).
//!
//! The initial instruction set of a run is its `initial` event; every later
//! event is a runtime refinement released by a trigger the caller names. Each
//! event is immutable once sealed and carries the hash of the instructions it
//! disclosed. Pure data plus validation; no store.

use serde::{Deserialize, Serialize};

use super::context_package::is_sha256_hex;
use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;
use super::record::outcome::RefusalCode;
use medulla_query_types::solve_scalar::digest_text;
use medulla_types::contract::BoundedVec;

/// The domain of an instruction text hash.
pub const INSTRUCTION_DISCLOSURE_DOMAIN: &str = "instruction_disclosure";

/// The most events one set's disclosure history carries.
pub const MAX_DISCLOSURE_EVENTS: usize = 64;

/// The `disclosure.*` knobs (MEDULLA-DECISION-ENGINE-R143.2).
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "disclosure.timeout",
    unit: "seconds",
    default: KnobValue::Integer(300),
    min: KnobValue::Integer(30),
    max: KnobValue::Integer(86_400),
    safety: KnobSafety::Tunable,
}];

/// Refusal code: a non-initial trigger names no prior event.
pub const DISCLOSURE_PRIOR_MISSING: RefusalCode = RefusalCode::new("DISCLOSURE_PRIOR_MISSING");
/// Refusal code: the `initial` trigger came with a prior event.
pub const DISCLOSURE_PRIOR_UNEXPECTED: RefusalCode =
    RefusalCode::new("DISCLOSURE_PRIOR_UNEXPECTED");
/// Refusal code: the prior event belongs to another instruction set.
pub const DISCLOSURE_SET_MISMATCH: RefusalCode = RefusalCode::new("DISCLOSURE_SET_MISMATCH");
/// Refusal code: the new instructions hash equals the prior hash.
pub const DISCLOSURE_HASH_UNCHANGED: RefusalCode = RefusalCode::new("DISCLOSURE_HASH_UNCHANGED");
/// Refusal code: a `timeout` trigger before `disclosure.timeout` seconds elapsed.
pub const DISCLOSURE_TIMEOUT_NOT_ELAPSED: RefusalCode =
    RefusalCode::new("DISCLOSURE_TIMEOUT_NOT_ELAPSED");

/// The refusal codes the `instruction_disclosure` family declares: the event
/// computation's (MEDULLA-DECISION-ENGINE-R143.2) and the served question's
/// (R137.100). The table is registered in `codes::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[
    DISCLOSURE_PRIOR_MISSING,
    DISCLOSURE_PRIOR_UNEXPECTED,
    DISCLOSURE_SET_MISMATCH,
    DISCLOSURE_HASH_UNCHANGED,
    DISCLOSURE_TIMEOUT_NOT_ELAPSED,
    DISCLOSURE_VERSION_STALE,
    DISCLOSURE_UNAUTHORIZED,
];

/// The resolved `disclosure.timeout`, in seconds.
pub fn timeout_s(policy: &DecisionPolicy) -> u64 {
    match policy.knob(&KNOBS[0]) {
        KnobValue::Integer(v) => u64::try_from(v).unwrap_or(300),
        _ => 300,
    }
}

/// Why a disclosure event was released.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DisclosureTrigger {
    /// The first disclosure, before execution.
    Initial,
    /// Relevance of the task changed.
    RelevanceUpdate,
    /// A constraint of the run changed.
    ConstraintChange,
    /// The set's disclosure timeout elapsed.
    Timeout,
}

impl DisclosureTrigger {
    /// Whether this trigger is a runtime refinement.
    pub fn is_runtime_refinement(self) -> bool {
        !matches!(self, Self::Initial)
    }
}

/// One sealed disclosure of an instruction set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DisclosureEvent {
    pub set_id: String,
    pub version: u32,
    pub trigger: DisclosureTrigger,
    pub at_ms: u64,
    /// SHA-256 of the disclosed instructions, 64 lowercase hex characters.
    pub instructions_hash: String,
}

/// The ordered disclosure history of one instruction set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct InstructionSetDisclosure {
    pub set_id: String,
    pub events: BoundedVec<DisclosureEvent, MAX_DISCLOSURE_EVENTS>,
}

/// The hash of `instructions` as a disclosure event records it: 64 lowercase
/// hex characters.
pub fn instructions_hash(instructions: &str) -> String {
    let text = digest_text(INSTRUCTION_DISCLOSURE_DOMAIN, &instructions);
    text.trim_start_matches("sha256:").to_string()
}

impl DisclosureEvent {
    /// Check the version and the hash.
    pub fn validate(&self) -> Result<(), String> {
        if self.set_id.is_empty() {
            return Err("a disclosure event names its instruction set".to_string());
        }
        if self.version == 0 {
            return Err("a disclosure version starts at 1".to_string());
        }
        if !is_sha256_hex(&self.instructions_hash) {
            return Err("instructions_hash is not 64 lowercase hex characters".to_string());
        }
        Ok(())
    }
}

impl InstructionSetDisclosure {
    /// The initial event, if the history has one.
    pub fn initial(&self) -> Option<&DisclosureEvent> {
        self.events
            .iter()
            .find(|event| event.trigger == DisclosureTrigger::Initial)
    }

    /// Every runtime refinement, in order.
    pub fn runtime_refinements(&self) -> impl Iterator<Item = &DisclosureEvent> {
        self.events
            .iter()
            .filter(|event| event.trigger.is_runtime_refinement())
    }

    /// Every event is valid and belongs to this set; versions strictly
    /// increase; the one initial event, when present, is first.
    pub fn validate(&self) -> Result<(), String> {
        let mut prior: Option<&DisclosureEvent> = None;
        for (index, event) in self.events.iter().enumerate() {
            event.validate()?;
            if event.set_id != self.set_id {
                return Err("an event belongs to another instruction set".to_string());
            }
            if event.trigger == DisclosureTrigger::Initial && index != 0 {
                return Err("only the first event is the initial disclosure".to_string());
            }
            if prior.is_some_and(|p| event.version <= p.version) {
                return Err("event versions must strictly increase".to_string());
            }
            prior = Some(event);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The stored instruction version and the refusals of the `instruction_disclosure`
// question family (MEDULLA-DECISION-ENGINE-R137.100).
// ---------------------------------------------------------------------------

/// Engine-owned marker property of a stored instruction version.
pub const INSTRUCTION_VERSION_MARKER: &str = "_instruction_version";

/// A pinned digest differs from the latest version of its set.
pub const DISCLOSURE_VERSION_STALE: RefusalCode = RefusalCode::new("DISCLOSURE_VERSION_STALE");
/// The named set has no version in the caller's tenant graph: a set that does
/// not exist and a set held by another tenant answer the same way.
pub const DISCLOSURE_UNAUTHORIZED: RefusalCode = RefusalCode::new("DISCLOSURE_UNAUTHORIZED");

/// When a stored version is disclosed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum InstructionPhase {
    /// Disclosed before execution starts.
    Initial,
    /// Withheld until its trigger occurs during the run.
    Runtime,
}

impl InstructionPhase {
    /// The wire word of the phase.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Runtime => "runtime",
        }
    }

    /// The phase named by a wire word.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "initial" => Some(Self::Initial),
            "runtime" => Some(Self::Runtime),
            _ => None,
        }
    }
}

/// One version of an instruction set, stored as a tenant graph node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct InstructionVersion {
    /// The tenant that holds the version.
    pub tenant_id: String,
    pub instruction_set_id: String,
    pub version: u64,
    /// Digest of the instructions this version discloses.
    pub digest: String,
    pub phase: InstructionPhase,
    /// The trigger that releases a runtime version; absent for an initial one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_iri: Option<String>,
}

/// The node id prefix every version of `set_id` is stored under.
pub fn instruction_version_prefix(set_id: &str) -> String {
    format!("instruction-version:{set_id}:")
}

impl InstructionVersion {
    /// The node id this version is stored under.
    pub fn node_id(&self) -> String {
        format!(
            "{}{}",
            instruction_version_prefix(&self.instruction_set_id),
            self.version
        )
    }

    /// A runtime version names its trigger; an initial one names none.
    pub fn validate(&self) -> Result<(), String> {
        let named = self
            .trigger_iri
            .as_deref()
            .is_some_and(|iri| !iri.trim().is_empty());
        match (self.phase, named) {
            (InstructionPhase::Runtime, false) => {
                Err("a runtime instruction version names its trigger_iri".to_string())
            }
            (InstructionPhase::Initial, _) if self.trigger_iri.is_some() => {
                Err("an initial instruction version names no trigger_iri".to_string())
            }
            _ => Ok(()),
        }
    }

    /// The node properties of the stored version: the record plus the marker.
    pub fn to_node_properties(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).unwrap_or_default();
        if let serde_json::Value::Object(map) = &mut value {
            map.insert(INSTRUCTION_VERSION_MARKER.to_string(), true.into());
        }
        value
    }

    /// Strictly decode a stored version; `None` for a node that is not one or
    /// does not hold the record's shape.
    pub fn from_node_properties(
        props: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<Self> {
        if props
            .get(INSTRUCTION_VERSION_MARKER)
            .and_then(|marker| marker.as_bool())
            != Some(true)
        {
            return None;
        }
        let mut record = props.clone();
        record.remove(INSTRUCTION_VERSION_MARKER);
        let version: Self = serde_json::from_value(serde_json::Value::Object(record)).ok()?;
        version.validate().ok()?;
        Some(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(version: u32, trigger: DisclosureTrigger) -> DisclosureEvent {
        DisclosureEvent {
            set_id: "set-a".to_string(),
            version,
            trigger,
            at_ms: 1_000 * u64::from(version),
            instructions_hash: instructions_hash(&format!("instructions v{version}")),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R143.1
    #[test]
    fn disclosure_events_round_trip() {
        for trigger in [
            DisclosureTrigger::Initial,
            DisclosureTrigger::RelevanceUpdate,
            DisclosureTrigger::ConstraintChange,
            DisclosureTrigger::Timeout,
        ] {
            let sealed = event(1, trigger);
            let json = serde_json::to_string(&sealed).unwrap();
            assert_eq!(
                serde_json::from_str::<DisclosureEvent>(&json).unwrap(),
                sealed
            );
            let with_extra = json.replacen('{', "{\"extra\":1,", 1);
            assert!(serde_json::from_str::<DisclosureEvent>(&with_extra).is_err());
        }
        assert_eq!(
            serde_json::to_string(&DisclosureTrigger::RelevanceUpdate).unwrap(),
            "\"relevance_update\""
        );
        assert_eq!(
            serde_json::to_string(&DisclosureTrigger::ConstraintChange).unwrap(),
            "\"constraint_change\""
        );

        let history = InstructionSetDisclosure {
            set_id: "set-a".to_string(),
            events: BoundedVec::new(vec![
                event(1, DisclosureTrigger::Initial),
                event(2, DisclosureTrigger::RelevanceUpdate),
                event(3, DisclosureTrigger::Timeout),
            ])
            .unwrap(),
        };
        assert_eq!(history.validate(), Ok(()));
        assert_eq!(history.initial().unwrap().version, 1);
        assert_eq!(history.runtime_refinements().count(), 2);
        let json = serde_json::to_string(&history).unwrap();
        assert_eq!(
            serde_json::from_str::<InstructionSetDisclosure>(&json).unwrap(),
            history
        );
        assert!(
            serde_json::from_str::<InstructionSetDisclosure>(&json.replacen(
                '{',
                "{\"extra\":1,",
                1
            ))
            .is_err()
        );

        // The hash is 64 lowercase hex and depends on the text.
        assert!(is_sha256_hex(&instructions_hash("a")));
        assert_ne!(instructions_hash("a"), instructions_hash("b"));

        // Malformed histories are refused.
        let mut bad_hash = event(1, DisclosureTrigger::Initial);
        bad_hash.instructions_hash = "sha256:x".to_string();
        assert!(bad_hash.validate().is_err());
        assert!(event(0, DisclosureTrigger::Initial).validate().is_err());
        let late_initial = InstructionSetDisclosure {
            set_id: "set-a".to_string(),
            events: BoundedVec::new(vec![
                event(1, DisclosureTrigger::RelevanceUpdate),
                event(2, DisclosureTrigger::Initial),
            ])
            .unwrap(),
        };
        assert!(late_initial.validate().is_err());
        let repeated = InstructionSetDisclosure {
            set_id: "set-a".to_string(),
            events: BoundedVec::new(vec![
                event(1, DisclosureTrigger::Initial),
                event(1, DisclosureTrigger::Timeout),
            ])
            .unwrap(),
        };
        assert!(repeated.validate().is_err());
        let foreign = InstructionSetDisclosure {
            set_id: "set-b".to_string(),
            events: BoundedVec::new(vec![event(1, DisclosureTrigger::Initial)]).unwrap(),
        };
        assert!(foreign.validate().is_err());
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;
    use crate::decision::statistical::StatisticalErrorCode;

    fn version(phase: InstructionPhase, trigger: Option<&str>) -> InstructionVersion {
        InstructionVersion {
            tenant_id: "tenant-a".to_string(),
            instruction_set_id: "set-a".to_string(),
            version: 3,
            digest: instructions_hash("do the thing"),
            phase,
            trigger_iri: trigger.map(str::to_string),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R137.100
    #[test]
    fn declared_refusals_are_registered_statistical_codes() {
        for code in REFUSALS {
            assert_eq!(super::super::codes::resolve(code.as_str()), Some(*code));
            assert!(StatisticalErrorCode::ALL
                .iter()
                .any(|known| known.as_str() == code.as_str()));
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R137.100
    #[test]
    fn stored_version_round_trips_and_refuses_a_malformed_shape() {
        let runtime = version(InstructionPhase::Runtime, Some("trigger:relevance"));
        assert_eq!(runtime.node_id(), "instruction-version:set-a:3");
        let value = runtime.to_node_properties();
        let props = value.as_object().unwrap();
        assert_eq!(
            InstructionVersion::from_node_properties(props),
            Some(runtime)
        );

        // A runtime version without a trigger and an initial one with a trigger
        // are not versions.
        assert!(version(InstructionPhase::Runtime, None).validate().is_err());
        assert!(version(InstructionPhase::Initial, Some("t"))
            .validate()
            .is_err());
        // A node without the marker, or with an unknown field, is not one.
        let mut unmarked = props.clone();
        unmarked.remove(INSTRUCTION_VERSION_MARKER);
        assert_eq!(InstructionVersion::from_node_properties(&unmarked), None);
        let mut extra = props.clone();
        extra.insert("surplus".to_string(), 1.into());
        assert_eq!(InstructionVersion::from_node_properties(&extra), None);
    }
}
