//! Field shielding model (MEDULLA-DECISION-ENGINE-R162.1).
//!
//! A step may read only the fields it declared. A shielded field is withheld
//! even when declared. Paths are dotted names of at most eight segments.

use serde::{Deserialize, Serialize};

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// The most segments a path may have.
pub const MAX_PATH_SEGMENTS: usize = 8;
/// The most paths a request may hold.
pub const MAX_SHIELDING_PATHS: usize = 1024;

/// The `shielding.*` knobs.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "shielding.max_depth",
    unit: "levels",
    default: KnobValue::Integer(8),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(32),
    safety: KnobSafety::Fixed,
}];

/// A dotted field path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
pub struct FieldPath(String);

impl FieldPath {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        let segments: Vec<&str> = value.split('.').collect();
        if segments.len() > MAX_PATH_SEGMENTS || segments.iter().any(|s| s.is_empty()) {
            return Err(format!(
                "a path holds 1 to {MAX_PATH_SEGMENTS} non-empty dotted segments"
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `self` is `other` or lies below it.
    pub fn is_within(&self, other: &FieldPath) -> bool {
        self.0 == other.0 || self.0.starts_with(&format!("{}.", other.0))
    }
}

impl TryFrom<String> for FieldPath {
    type Error = String;
    fn try_from(value: String) -> Result<Self, String> {
        Self::new(value)
    }
}

impl From<FieldPath> for String {
    fn from(path: FieldPath) -> String {
        path.0
    }
}

/// A field that is never readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ShieldedField {
    pub path: FieldPath,
}

/// One input a step declared; `depth` counts nested levels below it that stay
/// readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DeclaredInput {
    pub path: FieldPath,
    pub required: bool,
    pub depth: u8,
}

/// A step and the inputs it declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StepProjection {
    pub step_id: String,
    pub declared: Vec<DeclaredInput>,
}

/// What a step may read and what was withheld.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectionDecision {
    pub allowed: Vec<FieldPath>,
    pub withheld: Vec<FieldPath>,
}

/// The question: object paths, shielded paths, one step and its reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FieldShieldingRequest {
    pub object_paths: Vec<FieldPath>,
    pub shielded: Vec<ShieldedField>,
    pub step: StepProjection,
    pub requested_reads: Vec<FieldPath>,
}

impl FieldShieldingRequest {
    /// At most 1024 paths in all.
    pub fn checked(self) -> Result<Self, String> {
        let total = self.object_paths.len()
            + self.shielded.len()
            + self.step.declared.len()
            + self.requested_reads.len();
        if total > MAX_SHIELDING_PATHS {
            return Err(format!(
                "a request holds at most {MAX_SHIELDING_PATHS} paths"
            ));
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(value: &str) -> FieldPath {
        FieldPath::new(value).unwrap()
    }

    // spec: MEDULLA-DECISION-ENGINE-R162.1
    #[test]
    fn types_round_trip() {
        let request = FieldShieldingRequest {
            object_paths: vec![path("customer.name"), path("customer.ssn")],
            shielded: vec![ShieldedField {
                path: path("customer.ssn"),
            }],
            step: StepProjection {
                step_id: "step:1".into(),
                declared: vec![DeclaredInput {
                    path: path("customer"),
                    required: true,
                    depth: 1,
                }],
            },
            requested_reads: vec![path("customer.name")],
        }
        .checked()
        .unwrap();
        let text = serde_json::to_string(&request).unwrap();
        assert!(text.contains("\"customer.ssn\""));
        assert_eq!(
            serde_json::from_str::<FieldShieldingRequest>(&text).unwrap(),
            request
        );
        let decision = ProjectionDecision {
            allowed: vec![path("customer.name")],
            withheld: vec![path("customer.ssn")],
        };
        let text = serde_json::to_string(&decision).unwrap();
        assert_eq!(
            serde_json::from_str::<ProjectionDecision>(&text).unwrap(),
            decision
        );

        // Eight segments pass; nine and empty segments are refused.
        assert!(FieldPath::new("a.b.c.d.e.f.g.h").is_ok());
        assert!(FieldPath::new("a.b.c.d.e.f.g.h.i").is_err());
        assert!(FieldPath::new("a..b").is_err());
        assert!(serde_json::from_str::<FieldPath>("\"\"").is_err());
        assert!(path("customer.name").is_within(&path("customer")));
        assert!(!path("customers.name").is_within(&path("customer")));

        let mut big = request.clone();
        big.object_paths = vec![path("a"); MAX_SHIELDING_PATHS];
        assert!(big.checked().is_err());
        assert!(KNOBS[0].admits(&KNOBS[0].default));
    }
}
