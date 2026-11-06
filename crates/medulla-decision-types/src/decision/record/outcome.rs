//! What a decision concluded: a proved assembly, or a typed abstention.
//!
//! There is no third arm. An engine that cannot prove an assembly says so and
//! names what it could not resolve; it never returns a guess wearing the shape
//! of an answer.

use serde::{Deserialize, Serialize};

use super::premise::Violation;
use crate::solve::Certificate;
use crate::solve::ObjectiveValue;
use medulla_agent_types::agent_component::ComponentDependency;
use medulla_types::contract::BoundedVec;
use medulla_types::solve_scalar::Scalar;

/// One slot of the assembled graph and the component bound to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SlotAssignment {
    pub slot: String,
    pub component: ComponentDependency,
}

/// Why the engine declined to answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum AbstainReason {
    /// A required capability no candidate covers.
    UncoveredCapability { iri: String },
    /// A declared capability IRI that is not a native term.
    UnresolvedCapabilityIri { iri: String },
    /// Free text no mapping resolved to a task.
    UnmappedTask { text_digest: String },
    /// The constraints named cannot be satisfied together.
    Infeasible { constraints: BoundedVec<String, 64> },
    /// The search budget ran out above the accepted gap.
    BudgetExhausted {
        #[serde(default)]
        incumbent: Option<String>,
        lower_bound: Scalar,
    },
    /// A fact the objective needs is absent on a candidate.
    UnknownFact { component_id: String, field: String },
    /// An external agent without its required observation.
    IneligibleExternalAgent { component_id: String },
    /// The record itself would exceed the durable record bound.
    RecordTooLarge { bytes: u64 },
    /// The request tried to widen the pinned policy.
    PolicyLoosening { field: String },
    /// Statistical: below the policy's support, coverage or risk floor. It
    /// names no invisible data, by construction.
    InsufficientConfidence,
    /// A refusal from one family's registered table, or a shared request code.
    Family {
        family: crate::decision::statistical::QuestionKind,
        code: RefusalCode,
        detail: String,
    },
    /// A refusal raised on a decision-plane method that is not a `Decide`
    /// question (`DecisionLog`, `DecisionEval`, `AgentAssemble`,
    /// `PolicyEvolution`, `DecisionCommit`), folded from its typed refusal enum.
    Surface {
        surface: RefusalSurface,
        code: RefusalCode,
        detail: String,
    },
}

/// The decision-plane method, other than `Decide`, that raised a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RefusalSurface {
    DecisionLog,
    DecisionEval,
    Assemble,
    PolicyEvolution,
    DecisionCommit,
}

/// Where a refusal was raised. The caller that raises it knows the origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalOrigin {
    /// The `Decide` path, with the request's question kind.
    Decide(crate::decision::statistical::QuestionKind),
    /// A decision-plane method other than `Decide`.
    Surface(RefusalSurface),
}

impl RefusalOrigin {
    pub const POLICY_EVOLUTION: Self = Self::Surface(RefusalSurface::PolicyEvolution);
    pub const ASSEMBLE: Self = Self::Surface(RefusalSurface::Assemble);
}

/// A typed refusal enum of the decision plane. Each variant maps to exactly one
/// registered code (`crate::decision::codes`), so no refusal reaches the wire as
/// a catch-all (MEDULLA-DECISION-ENGINE-R010.100.2).
pub trait DecisionRefusal {
    /// The registered code of this refusal.
    fn code(&self) -> RefusalCode;
    /// The human detail that accompanies the code.
    fn detail(&self) -> String;
}

/// A stable refusal code: one entry of a family `REFUSALS` table or of the
/// shared request codes in `crate::decision::codes`. On the wire it is the code
/// string; deserializing accepts only a registered code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RefusalCode(&'static str);

impl RefusalCode {
    /// Declare a code. Only tables and the fixed variants call this.
    pub const fn new(code: &'static str) -> Self {
        Self(code)
    }

    /// The stable wire string.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl Serialize for RefusalCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for RefusalCode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        crate::decision::codes::resolve(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("unregistered refusal code `{text}`")))
    }
}

/// Hand-written: the wire form is the code string, not the private field.
#[cfg(feature = "contract-schema")]
impl schemars::JsonSchema for RefusalCode {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("RefusalCode")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::Schema::try_from(serde_json::json!({
            "type": "string",
            "description": "A registered decision refusal code",
            "pattern": "^[A-Z][A-Z0-9_]*$",
        }))
        .expect("a JSON object is a valid schema")
    }
}

/// The code of every fixed `AbstainReason` variant, in declaration order.
pub const FIXED_CODES: &[RefusalCode] = &[
    RefusalCode::new("UNCOVERED_CAPABILITY"),
    RefusalCode::new("UNRESOLVED_CAPABILITY_IRI"),
    RefusalCode::new("UNMAPPED_TASK"),
    RefusalCode::new("INFEASIBLE"),
    RefusalCode::new("BUDGET_EXHAUSTED"),
    RefusalCode::new("UNKNOWN_FACT"),
    RefusalCode::new("INELIGIBLE_EXTERNAL_AGENT"),
    RefusalCode::new("RECORD_TOO_LARGE"),
    RefusalCode::new("POLICY_LOOSENING"),
    RefusalCode::new("INSUFFICIENT_CONFIDENCE"),
];

impl AbstainReason {
    /// The stable code of this reason, for every variant.
    pub fn code(&self) -> RefusalCode {
        let index = match self {
            Self::UncoveredCapability { .. } => 0,
            Self::UnresolvedCapabilityIri { .. } => 1,
            Self::UnmappedTask { .. } => 2,
            Self::Infeasible { .. } => 3,
            Self::BudgetExhausted { .. } => 4,
            Self::UnknownFact { .. } => 5,
            Self::IneligibleExternalAgent { .. } => 6,
            Self::RecordTooLarge { .. } => 7,
            Self::PolicyLoosening { .. } => 8,
            Self::InsufficientConfidence => 9,
            Self::Family { code, .. } | Self::Surface { code, .. } => return *code,
        };
        FIXED_CODES[index]
    }

    /// Fold a typed refusal into the abstention that reports it, under the
    /// origin the raising caller passes.
    pub fn from_refusal(origin: RefusalOrigin, refusal: &impl DecisionRefusal) -> Self {
        let (code, detail) = (refusal.code(), refusal.detail());
        match origin {
            RefusalOrigin::Decide(family) => Self::Family {
                family,
                code,
                detail,
            },
            RefusalOrigin::Surface(surface) => Self::Surface {
                surface,
                code,
                detail,
            },
        }
    }
}

/// Why one candidate did not win its slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WhyNot {
    pub component_id: String,
    pub slot: String,
    /// The objective it would have forced, when it was feasible but worse.
    #[serde(default)]
    pub forced_objective: Option<ObjectiveValue>,
    /// The rule that removed it, when it was not feasible at all.
    #[serde(default)]
    pub violation: Option<Violation>,
}

/// The conclusion of one assembly decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DecisionOutcome {
    /// An assembly, with the certificate that proves it optimal. Boxed: the
    /// certificate's own proof tree dwarfs every other field this enum
    /// carries (320 bytes vs. `Abstained`'s 24), so every `DecisionOutcome`
    /// value would otherwise pay `Solved`'s size even when it holds the far
    /// smaller `Abstained` variant.
    Solved {
        graph_digest: String,
        slots: BoundedVec<SlotAssignment, 64>,
        certificate: Box<Certificate>,
        /// The decided widths, rounds, stop rule, leases and allowances, when
        /// the request asked a topology question.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topology: Option<Box<medulla_agent_types::agent_topology::TopologyPlan>>,
    },
    /// No assembly, and exactly why.
    Abstained {
        reasons: BoundedVec<AbstainReason, 64>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::codes;
    use crate::decision::errors::DecisionErrorCode;

    // spec: MEDULLA-DECISION-ENGINE-R010.100.1
    #[test]
    fn codes_are_unique_and_stable() {
        let mut all: Vec<&str> = FIXED_CODES.iter().map(|code| code.as_str()).collect();
        all.extend(codes::SHARED.iter().map(|code| code.as_str()));
        all.extend(codes::FOLDED.iter().map(|code| code.as_str()));
        for (_, table) in codes::FAMILY_TABLES {
            all.extend(table.iter().map(|code| code.as_str()));
        }
        let mut sorted = all.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len(), "a refusal code is declared twice");
        for code in &all {
            assert!(
                code.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
                "{code} is not an upper-snake code"
            );
        }
        // Stable: the codes shared with the request vocabulary keep its strings.
        assert_eq!(
            AbstainReason::RecordTooLarge { bytes: 1 }.code().as_str(),
            DecisionErrorCode::RecordTooLarge.as_str()
        );
        assert_eq!(
            AbstainReason::PolicyLoosening { field: "f".into() }
                .code()
                .as_str(),
            DecisionErrorCode::PolicyLoosening.as_str()
        );
        assert_eq!(
            AbstainReason::InsufficientConfidence.code().as_str(),
            "INSUFFICIENT_CONFIDENCE"
        );
        let family = AbstainReason::Family {
            family: crate::decision::statistical::QuestionKind::Route,
            code: codes::PARAMETER_INVALID,
            detail: "column.name".into(),
        };
        assert_eq!(family.code(), codes::PARAMETER_INVALID);
        let wire = serde_json::to_string(&family).expect("encodes");
        assert_eq!(
            wire,
            r#"{"reason":"family","family":"route","code":"PARAMETER_INVALID","detail":"column.name"}"#
        );
        assert_eq!(
            serde_json::from_str::<AbstainReason>(&wire).expect("decodes"),
            family
        );
        let unregistered = wire.replace("PARAMETER_INVALID", "NOT_A_CODE");
        assert!(serde_json::from_str::<AbstainReason>(&unregistered).is_err());
    }

    fn registered(code: &str) -> bool {
        use medulla_types::contract::{EngineErrorCode, ServerErrorCode};
        ServerErrorCode::ALL.iter().any(|c| c.as_str() == code)
            || EngineErrorCode::ALL.iter().any(|c| c.as_str() == code)
            || DecisionErrorCode::ALL.iter().any(|c| c.as_str() == code)
            || crate::decision::harness_contrast::HarnessContrastErrorCode::ALL
                .iter()
                .any(|c| c.as_str() == code)
    }

    // spec: MEDULLA-DECISION-ENGINE-R010.100.2
    #[test]
    fn every_decision_refusal_enum_maps_to_a_registered_code() {
        use crate::decision::harness_contrast::{
            HarnessContrastAbstainReason as Reason, HarnessContrastRefusal as Contrast,
        };
        use crate::decision::rapid_r015::HeadKindRefusal;
        use crate::decision::statistical::tool_subset::ToolSubsetRefusal;
        use crate::decision::statistical::QuestionKind;

        // Every folded code is a registered wire code, and resolves back.
        for code in codes::FOLDED {
            assert!(
                registered(code.as_str()),
                "{} is not registered",
                code.as_str()
            );
            assert_eq!(codes::resolve(code.as_str()), Some(*code));
        }
        let head = HeadKindRefusal::NotAStatisticalHead { name: "llm".into() };
        let refusals: Vec<(&str, RefusalCode)> = vec![
            ("head", DecisionRefusal::code(&head)),
            (
                "subset/uncoverable",
                DecisionRefusal::code(&ToolSubsetRefusal::CapabilityUncoverable),
            ),
            (
                "subset/budget",
                DecisionRefusal::code(&ToolSubsetRefusal::BudgetExceeded),
            ),
            (
                "contrast/min",
                DecisionRefusal::code(&Contrast::MinCandidates),
            ),
            (
                "contrast/many",
                DecisionRefusal::code(&Contrast::TooManyCandidates),
            ),
            (
                "contrast/unknown",
                DecisionRefusal::code(&Contrast::VersionUnknown),
            ),
            (
                "contrast/trace",
                DecisionRefusal::code(&Contrast::TraceMismatch),
            ),
            (
                "abstain/none",
                DecisionRefusal::code(&Reason::NoImprovement),
            ),
            (
                "abstain/trace",
                DecisionRefusal::code(&Reason::TraceMismatch),
            ),
            (
                "abstain/evidence",
                DecisionRefusal::code(&Reason::InsufficientEvidence),
            ),
        ];
        let mut seen = std::collections::BTreeSet::new();
        for (name, code) in &refusals {
            assert!(
                registered(code.as_str()),
                "{name}: {} is not registered",
                code.as_str()
            );
            assert!(
                codes::resolve(code.as_str()).is_some(),
                "{name} does not resolve"
            );
            // Typed variants map to distinct codes: no catch-all.
            assert!(seen.insert(code.as_str()), "{name} shares a code");
        }

        // The folded abstention carries the origin the caller passed.
        let on_decide = AbstainReason::from_refusal(
            RefusalOrigin::Decide(QuestionKind::ToolSubset),
            &ToolSubsetRefusal::BudgetExceeded,
        );
        assert_eq!(on_decide.code(), codes::TOOL_SUBSET_BUDGET_EXCEEDED);
        assert!(matches!(
            on_decide,
            AbstainReason::Family {
                family: QuestionKind::ToolSubset,
                ..
            }
        ));
        let on_assemble = AbstainReason::from_refusal(RefusalOrigin::ASSEMBLE, &head);
        assert_eq!(on_assemble.code(), codes::HEAD_KIND_NOT_STATISTICAL);
        let wire = serde_json::to_string(&on_assemble).expect("encodes");
        assert_eq!(
            serde_json::from_str::<AbstainReason>(&wire).expect("decodes"),
            on_assemble
        );
        let unregistered = wire.replace("HEAD_KIND_NOT_STATISTICAL", "NOT_A_CODE");
        assert!(serde_json::from_str::<AbstainReason>(&unregistered).is_err());
    }
}
