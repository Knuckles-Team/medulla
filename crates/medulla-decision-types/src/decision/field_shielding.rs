//! Field shielding model (MEDULLA-DECISION-ENGINE-R162.1).
//!
//! A step may read only the fields it declared. A shielded field is withheld
//! even when declared. Paths are dotted names of at most eight segments.

use serde::{Deserialize, Serialize};

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::record::outcome::{DecisionRefusal, RefusalCode};
use crate::decision::DecisionPolicy;

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

/// The segments of a path, compared segment by segment (never by string prefix).
fn segments(path: &FieldPath) -> Vec<&str> {
    path.as_str().split('.').collect()
}

/// Whether `ancestor` is `path` or one of its ancestors, by segments.
fn is_segment_prefix(ancestor: &[&str], path: &[&str]) -> bool {
    ancestor.len() <= path.len() && ancestor.iter().zip(path).all(|(a, b)| a == b)
}

/// What a step may read (MEDULLA-DECISION-ENGINE-R162.2).
///
/// A path is allowed when a declared input equals it or is an ancestor of it
/// within the input's `depth` nested levels. A path is withheld when it is not
/// allowed, and also when it or any ancestor is shielded: shielding wins over
/// a declaration. Both lists keep the order of `object_paths`.
pub fn project_declared_inputs(
    object_paths: &[FieldPath],
    shielded: &[ShieldedField],
    declaration: &StepProjection,
) -> ProjectionDecision {
    let shielded: Vec<Vec<&str>> = shielded.iter().map(|s| segments(&s.path)).collect();
    let declared: Vec<(Vec<&str>, usize)> = declaration
        .declared
        .iter()
        .map(|d| (segments(&d.path), usize::from(d.depth)))
        .collect();
    let mut decision = ProjectionDecision {
        allowed: Vec::new(),
        withheld: Vec::new(),
    };
    for path in object_paths {
        let segs = segments(path);
        let is_shielded = shielded.iter().any(|s| is_segment_prefix(s, &segs));
        let is_declared = declared
            .iter()
            .any(|(d, depth)| is_segment_prefix(d, &segs) && segs.len() - d.len() <= *depth);
        if is_declared && !is_shielded {
            decision.allowed.push(path.clone());
        } else {
            decision.withheld.push(path.clone());
        }
    }
    decision
}

/// Refusal code: a requested read is withheld and no shield names it.
pub const FIELD_UNDECLARED: RefusalCode = RefusalCode::new("FIELD_UNDECLARED");
/// Refusal code: a requested read is a shielded path or lies below one.
pub const FIELD_SHIELDED: RefusalCode = RefusalCode::new("FIELD_SHIELDED");
/// Refusal code: a required declared input is absent from the object paths.
pub const FIELD_REQUIRED_MISSING: RefusalCode = RefusalCode::new("FIELD_REQUIRED_MISSING");
/// Refusal code: a path holds more segments than `shielding.max_depth`.
pub const FIELD_PATH_INVALID: RefusalCode = RefusalCode::new("FIELD_PATH_INVALID");

/// The `REFUSALS` table of the `field_shielding` family, registered in
/// `codes::FAMILY_TABLES` (MEDULLA-DECISION-ENGINE-R162.4).
pub const REFUSALS: &[RefusalCode] = &[
    FIELD_UNDECLARED,
    FIELD_SHIELDED,
    FIELD_REQUIRED_MISSING,
    FIELD_PATH_INVALID,
];

/// Why a field-shielding request is refused, naming the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShieldingRefusal {
    pub code: RefusalCode,
    pub path: String,
}

impl DecisionRefusal for ShieldingRefusal {
    fn code(&self) -> RefusalCode {
        self.code
    }

    fn detail(&self) -> String {
        let what = if self.code == FIELD_SHIELDED {
            "is shielded"
        } else if self.code == FIELD_REQUIRED_MISSING {
            "is a required input the object does not hold"
        } else if self.code == FIELD_PATH_INVALID {
            "has more segments than shielding.max_depth"
        } else {
            "is not declared by the step"
        };
        format!("{} {what}", self.path)
    }
}

/// The `shielding.max_depth` the policy pins, or its default.
pub fn max_depth(policy: &DecisionPolicy) -> usize {
    match policy.knob(&KNOBS[0]) {
        KnobValue::Integer(value) => usize::try_from(value).unwrap_or(MAX_PATH_SEGMENTS),
        _ => MAX_PATH_SEGMENTS,
    }
}

/// Check one request and return what the step may read
/// (MEDULLA-DECISION-ENGINE-R162.3, R162.4).
///
/// Every check runs on the request alone, so a refusal reads and writes
/// nothing. Order: path depth, required inputs, then each requested read
/// (shielded before undeclared).
pub fn release_projection(
    request: &FieldShieldingRequest,
    max_depth: usize,
) -> Result<ProjectionDecision, ShieldingRefusal> {
    let refuse = |code: RefusalCode, path: &FieldPath| ShieldingRefusal {
        code,
        path: path.as_str().to_string(),
    };
    let every_path = request
        .object_paths
        .iter()
        .chain(request.shielded.iter().map(|s| &s.path))
        .chain(request.step.declared.iter().map(|d| &d.path))
        .chain(request.requested_reads.iter());
    for path in every_path {
        if segments(path).len() > max_depth {
            return Err(refuse(FIELD_PATH_INVALID, path));
        }
    }
    for input in request.step.declared.iter().filter(|d| d.required) {
        let wanted = segments(&input.path);
        if !request
            .object_paths
            .iter()
            .any(|object| is_segment_prefix(&wanted, &segments(object)))
        {
            return Err(refuse(FIELD_REQUIRED_MISSING, &input.path));
        }
    }
    let decision = project_declared_inputs(&request.object_paths, &request.shielded, &request.step);
    let shielded: Vec<Vec<&str>> = request.shielded.iter().map(|s| segments(&s.path)).collect();
    for read in &request.requested_reads {
        let read_segments = segments(read);
        if shielded
            .iter()
            .any(|s| is_segment_prefix(s, &read_segments))
        {
            return Err(refuse(FIELD_SHIELDED, read));
        }
        if !decision.allowed.contains(read) {
            return Err(refuse(FIELD_UNDECLARED, read));
        }
    }
    Ok(decision)
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

    fn input(value: &str, depth: u8) -> DeclaredInput {
        DeclaredInput {
            path: path(value),
            required: false,
            depth,
        }
    }

    fn project(
        paths: &[&str],
        shielded: &[&str],
        declared: Vec<DeclaredInput>,
    ) -> ProjectionDecision {
        let paths: Vec<FieldPath> = paths.iter().map(|p| path(p)).collect();
        let shielded: Vec<ShieldedField> = shielded
            .iter()
            .map(|p| ShieldedField { path: path(p) })
            .collect();
        project_declared_inputs(
            &paths,
            &shielded,
            &StepProjection {
                step_id: "step:2".into(),
                declared,
            },
        )
    }

    fn names(paths: &[FieldPath]) -> Vec<&str> {
        paths.iter().map(FieldPath::as_str).collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R162.2
    #[test]
    fn shielding_removes_nested() {
        // Nested shielded path under a declared ancestor: shielding wins.
        let d = project(
            &[
                "customer",
                "customer.name",
                "customer.card.number",
                "customer.card.last4",
            ],
            &["customer.card.number"],
            vec![input("customer", 3)],
        );
        assert_eq!(
            names(&d.allowed),
            vec!["customer", "customer.name", "customer.card.last4"]
        );
        assert_eq!(names(&d.withheld), vec!["customer.card.number"]);

        // A shielded ancestor withholds every descendant, even a declared one.
        let d = project(
            &["customer.card", "customer.card.number"],
            &["customer.card"],
            vec![input("customer.card.number", 0)],
        );
        assert!(d.allowed.is_empty());
        assert_eq!(
            names(&d.withheld),
            vec!["customer.card", "customer.card.number"]
        );

        // Depth edges: depth 1 reads one level below the declared path.
        let d = project(
            &["order", "order.id", "order.lines.sku"],
            &[],
            vec![input("order", 1)],
        );
        assert_eq!(names(&d.allowed), vec!["order", "order.id"]);
        assert_eq!(names(&d.withheld), vec!["order.lines.sku"]);
        let d = project(&["order", "order.id"], &[], vec![input("order", 0)]);
        assert_eq!(names(&d.allowed), vec!["order"]);
        assert_eq!(names(&d.withheld), vec!["order.id"]);

        // An undeclared sibling is withheld, and a path sharing only a string
        // prefix with a declared one is not a descendant.
        let d = project(
            &["customer.name", "customers.name", "customer.email", "other"],
            &[],
            vec![input("customer.name", 2)],
        );
        assert_eq!(names(&d.allowed), vec!["customer.name"]);
        assert_eq!(
            names(&d.withheld),
            vec!["customers.name", "customer.email", "other"]
        );

        // A declared descendant does not open its ancestor.
        let d = project(
            &["customer", "customer.name"],
            &[],
            vec![input("customer.name", 0)],
        );
        assert_eq!(names(&d.allowed), vec!["customer.name"]);
        assert_eq!(names(&d.withheld), vec!["customer"]);

        // No declaration withholds everything.
        let d = project(&["a", "a.b"], &[], vec![]);
        assert!(d.allowed.is_empty());
        assert_eq!(names(&d.withheld), vec!["a", "a.b"]);
    }
}
