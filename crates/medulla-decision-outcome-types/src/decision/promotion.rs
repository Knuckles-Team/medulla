//! Typed revision lifecycle, held-out evidence, champion and rollback records
//! for every promotable subject (MEDULLA-DECISION-ENGINE-R805.7, R805.10,
//! R813.1).
//!
//! A revision moves through [`RevisionStage`] only by [`RevisionStage::apply`],
//! the one transition function of `RevisionMachine`; an illegal event is
//! refused with the typed [`TransitionRefused`] record.

use serde::{Deserialize, Serialize};

use medulla_decision_types::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use medulla_types::contract::OpaqueId;
use medulla_types::refusal_code::RefusalCode;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};

/// The schema version of [`HeldOutEvidence`].
pub const HELD_OUT_EVIDENCE_SCHEMA_VERSION: u16 = 1;

/// Where a revision stands between its draft and its retirement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RevisionStage {
    Draft,
    Evaluated,
    Published,
    Champion,
    Retired,
}

/// An event of `RevisionMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RevisionEvent {
    /// Held-out evidence was recorded for a draft.
    Evaluate,
    /// An evaluated revision was published.
    Publish,
    /// A published revision won the gate and became the champion.
    Promote,
    /// A revision left use: superseded, or withdrawn before it won.
    Retire,
    /// A rollback put a retired revision back as the champion.
    Reinstate,
}

impl MachineState for RevisionStage {
    fn stored(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Evaluated => "evaluated",
            Self::Published => "published",
            Self::Champion => "champion",
            Self::Retired => "retired",
        }
    }
}

impl MachineEvent for RevisionEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Evaluate => "Evaluate",
            Self::Publish => "Publish",
            Self::Promote => "Promote",
            Self::Retire => "Retire",
            Self::Reinstate => "Reinstate",
        }
    }
}

/// `RevisionMachine`: the closed edge table. Every other pair of state and
/// event is refused. It has no terminal state: a rollback reinstates a
/// retired revision.
pub static REVISION_MACHINE: MachineDef<RevisionStage, RevisionEvent, &'static str> = MachineDef {
    name: "RevisionMachine",
    states: &[
        RevisionStage::Draft,
        RevisionStage::Evaluated,
        RevisionStage::Published,
        RevisionStage::Champion,
        RevisionStage::Retired,
    ],
    initial: RevisionStage::Draft,
    terminals: &[],
    events: &[
        RevisionEvent::Evaluate,
        RevisionEvent::Publish,
        RevisionEvent::Promote,
        RevisionEvent::Retire,
        RevisionEvent::Reinstate,
    ],
    guards: &[],
    edges: &[
        Edge {
            id: "V01",
            from: RevisionStage::Draft,
            event: RevisionEvent::Evaluate,
            guard: None,
            to: RevisionStage::Evaluated,
        },
        Edge {
            id: "V02",
            from: RevisionStage::Evaluated,
            event: RevisionEvent::Publish,
            guard: None,
            to: RevisionStage::Published,
        },
        Edge {
            id: "V03",
            from: RevisionStage::Published,
            event: RevisionEvent::Promote,
            guard: None,
            to: RevisionStage::Champion,
        },
        Edge {
            id: "V04",
            from: RevisionStage::Champion,
            event: RevisionEvent::Retire,
            guard: None,
            to: RevisionStage::Retired,
        },
        Edge {
            id: "V05",
            from: RevisionStage::Published,
            event: RevisionEvent::Retire,
            guard: None,
            to: RevisionStage::Retired,
        },
        Edge {
            id: "V06",
            from: RevisionStage::Evaluated,
            event: RevisionEvent::Retire,
            guard: None,
            to: RevisionStage::Retired,
        },
        Edge {
            id: "V07",
            from: RevisionStage::Retired,
            event: RevisionEvent::Reinstate,
            guard: None,
            to: RevisionStage::Champion,
        },
    ],
};

impl RevisionStage {
    /// The one transition function of `RevisionMachine`: the next stage, or
    /// the typed refusal naming the machine, the stage and the event.
    pub fn apply(self, event: RevisionEvent) -> Result<Self, TransitionRefused> {
        transition(&REVISION_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

impl StateMachineApply for RevisionStage {
    type Event = RevisionEvent;
    type Facts = ();

    fn apply(&self, event: RevisionEvent, _facts: &()) -> Result<Self, TransitionRefused> {
        RevisionStage::apply(*self, event)
    }
}

/// What kind of definition a `Definition` subject names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DefinitionKind {
    Agent,
    Workflow,
    Team,
    HarnessComponent,
    HarnessBundle,
}

impl DefinitionKind {
    /// The snake_case text used in a pointer key.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Workflow => "workflow",
            Self::Team => "team",
            Self::HarnessComponent => "harness_component",
            Self::HarnessBundle => "harness_bundle",
        }
    }
}

/// The thing a revision revises; each subject has one champion pointer.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RevisionSubject {
    DecisionHead {
        component_id: String,
    },
    Program {
        program_ref: String,
    },
    KnobSet {
        policy_id: String,
    },
    ModelPolicy {
        family_ref: String,
    },
    Definition {
        definition_kind: DefinitionKind,
        definition_ref: String,
    },
    GovernedSet {
        scope_ref: String,
    },
    MeasuredFact {
        model_ref: String,
        property: String,
    },
    /// The rerank calibration curve of one scorer version (R822).
    RerankCalibration {
        scorer_version: String,
    },
}

impl RevisionSubject {
    /// The pointer key of this subject's champion: `champion:<kind>:<ref>`.
    pub fn pointer_key(&self) -> String {
        match self {
            Self::DecisionHead { component_id } => format!("champion:decision_head:{component_id}"),
            Self::Program { program_ref } => format!("champion:program:{program_ref}"),
            Self::KnobSet { policy_id } => format!("champion:knob_set:{policy_id}"),
            Self::ModelPolicy { family_ref } => format!("champion:model_policy:{family_ref}"),
            Self::Definition {
                definition_kind,
                definition_ref,
            } => {
                format!(
                    "champion:definition:{}:{definition_ref}",
                    definition_kind.as_str()
                )
            }
            Self::GovernedSet { scope_ref } => format!("champion:governed_set:{scope_ref}"),
            Self::MeasuredFact {
                model_ref,
                property,
            } => {
                format!("champion:measured_fact:{model_ref}:{property}")
            }
            Self::RerankCalibration { scorer_version } => {
                format!("champion:rerank_calibration:{scorer_version}")
            }
        }
    }
}

/// The kind of receipt a held-out evidence record was read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ReceiptKind {
    DecisionEval,
    ProgramEvaluation,
    PolicyEvaluation,
    RolloutSet,
    MeasurementRun,
    ReviewApproval,
}

/// Who graded a score. Only a model judge never decides a promotion (R813.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GraderKind {
    Annotation,
    Human,
    ModelJudge,
    TraceMetric,
    Api,
}

impl GraderKind {
    /// Whether a score of this grader may decide a promotion.
    pub const fn decides_promotion(self) -> bool {
        !matches!(self, Self::ModelJudge)
    }
}

/// When a measured fact's run and its champion's run were observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MeasurementWindow {
    pub observed_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub champion_observed_at_ms: Option<u64>,
}

/// An operator's review of a governed-set revision: two principal strings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReviewApproval {
    pub author: String,
    pub reviewer: String,
}

/// The measured evidence that qualifies a challenger against its champion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HeldOutEvidence {
    pub schema_version: u16,
    pub subject: RevisionSubject,
    pub challenger_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub champion_ref: Option<String>,
    pub receipt_kind: ReceiptKind,
    pub receipt_digest: String,
    pub evaluation_set_digest: String,
    pub challenger_score_ppm: i64,
    /// Absent when the subject has no champion to compare with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub champion_score_ppm: Option<i64>,
    pub delta_ppm: i64,
    pub case_count: u64,
    pub independent: bool,
    /// Who graded the score.
    pub grader_kind: GraderKind,
    /// Run times of a `MeasuredFact` challenger and champion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurement: Option<MeasurementWindow>,
    /// The review record of a `GovernedSet` challenger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewApproval>,
}

/// The active revision for one subject.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ChampionPointer {
    pub subject: RevisionSubject,
    pub revision_ref: OpaqueId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_ref: Option<OpaqueId>,
    pub promoted_by_decision: OpaqueId,
}

/// A decision to move a champion from one revision to another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RollbackDecision {
    pub pointer: ChampionPointer,
    pub from_ref: OpaqueId,
    pub to_ref: OpaqueId,
    pub reason: String,
    pub evidence_refs: Vec<OpaqueId>,
}

/// What a promotion decision concluded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PromotionVerdict {
    Promote { challenger_ref: String },
    Hold { reason: String },
    Rollback(RollbackDecision),
}

/// `promotion.min_cases`: fewest held-out cases a promotion needs.
pub const MIN_CASES: usize = 0;
/// `promotion.min_margin`: smallest score gain, as a fraction.
pub const MIN_MARGIN: usize = 1;
/// `measured_fact.min_cases`: fewest cases a measured-fact run needs.
pub const MEASURED_FACT_MIN_CASES: usize = 2;

/// The promotion knobs. The gate knobs are `Fixed`: a knob-set revision may
/// not loosen the bar a challenger must clear.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "promotion.min_cases",
        unit: "cases",
        default: KnobValue::Integer(50),
        min: KnobValue::Integer(30),
        max: KnobValue::Integer(100_000),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "promotion.min_margin",
        unit: "fraction",
        default: KnobValue::DecimalMicros(20_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(200_000),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "measured_fact.min_cases",
        unit: "cases",
        default: KnobValue::Integer(30),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(100_000),
        safety: KnobSafety::Tunable,
    },
];

/// The promotion and calibration refusal codes.
pub const REFUSALS: &[RefusalCode] = medulla_decision_types::decision::codes::PROMOTION;

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(namespace: &str, value: &str) -> OpaqueId {
        OpaqueId::new(format!("{namespace}:{value}")).expect("valid opaque id")
    }

    fn head() -> RevisionSubject {
        RevisionSubject::DecisionHead {
            component_id: "head-a".to_owned(),
        }
    }

    fn pointer(subject: RevisionSubject) -> ChampionPointer {
        ChampionPointer {
            subject,
            revision_ref: reference("revision", "revision-2"),
            parent_ref: Some(reference("revision", "revision-1")),
            promoted_by_decision: reference("decision", "decision-4"),
        }
    }

    fn round_trip<T>(value: &T)
    where
        T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let bytes = rmp_serde::to_vec_named(value).unwrap();
        assert_eq!(&rmp_serde::from_slice::<T>(&bytes).unwrap(), value);
        let json = serde_json::to_string(value).unwrap();
        assert_eq!(&serde_json::from_str::<T>(&json).unwrap(), value);
    }

    // spec: MEDULLA-DECISION-ENGINE-R805.1
    #[test]
    fn records_round_trip() {
        let pointer = pointer(head());
        let rollback = RollbackDecision {
            pointer: pointer.clone(),
            from_ref: pointer.revision_ref.clone(),
            to_ref: pointer.parent_ref.clone().expect("parent revision"),
            reason: "held_out_regression".to_owned(),
            evidence_refs: vec![reference("evidence", "evaluation-7")],
        };
        round_trip(&pointer);
        round_trip(&rollback);
    }

    fn evidence() -> HeldOutEvidence {
        HeldOutEvidence {
            schema_version: HELD_OUT_EVIDENCE_SCHEMA_VERSION,
            subject: head(),
            challenger_ref: "revision-2".to_owned(),
            champion_ref: Some("revision-1".to_owned()),
            receipt_kind: ReceiptKind::DecisionEval,
            receipt_digest: "sha256:aa".to_owned(),
            evaluation_set_digest: "sha256:bb".to_owned(),
            challenger_score_ppm: 910_000,
            champion_score_ppm: Some(880_000),
            delta_ppm: 30_000,
            case_count: 120,
            independent: true,
            grader_kind: GraderKind::Annotation,
            measurement: None,
            review: None,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R805.7
    #[test]
    fn lifecycle_types_round_trip() {
        for stage in REVISION_MACHINE.states {
            round_trip(stage);
        }
        for subject in [
            head(),
            RevisionSubject::Program {
                program_ref: "program-a".to_owned(),
            },
            RevisionSubject::KnobSet {
                policy_id: "policy-a".to_owned(),
            },
            RevisionSubject::ModelPolicy {
                family_ref: "family-a".to_owned(),
            },
        ] {
            round_trip(&subject);
            round_trip(&pointer(subject));
        }
        assert_eq!(head().pointer_key(), "champion:decision_head:head-a");
        assert_eq!(
            RevisionSubject::Program {
                program_ref: "p".to_owned()
            }
            .pointer_key(),
            "champion:program:p"
        );
        assert_eq!(
            RevisionSubject::KnobSet {
                policy_id: "k".to_owned()
            }
            .pointer_key(),
            "champion:knob_set:k"
        );
        assert_eq!(
            RevisionSubject::ModelPolicy {
                family_ref: "m".to_owned()
            }
            .pointer_key(),
            "champion:model_policy:m"
        );
        for kind in [
            ReceiptKind::DecisionEval,
            ReceiptKind::ProgramEvaluation,
            ReceiptKind::PolicyEvaluation,
            ReceiptKind::RolloutSet,
        ] {
            round_trip(&kind);
        }
        for grader in [
            GraderKind::Annotation,
            GraderKind::Human,
            GraderKind::ModelJudge,
            GraderKind::TraceMetric,
            GraderKind::Api,
        ] {
            round_trip(&grader);
            assert_eq!(grader.decides_promotion(), grader != GraderKind::ModelJudge);
        }
        let held_out = evidence();
        round_trip(&held_out);
        let no_baseline = HeldOutEvidence {
            champion_ref: None,
            champion_score_ppm: None,
            ..held_out
        };
        round_trip(&no_baseline);
        round_trip(&PromotionVerdict::Promote {
            challenger_ref: "revision-2".to_owned(),
        });
        round_trip(&PromotionVerdict::Hold {
            reason: "margin_not_met".to_owned(),
        });
        let rollback = RollbackDecision {
            pointer: pointer(head()),
            from_ref: reference("revision", "revision-2"),
            to_ref: reference("revision", "revision-1"),
            reason: "held_out_regression".to_owned(),
            evidence_refs: vec![],
        };
        round_trip(&PromotionVerdict::Rollback(rollback));
    }

    // spec: MEDULLA-DECISION-ENGINE-R805.7
    #[test]
    fn revision_machine_allows_every_legal_edge_and_refuses_the_rest() {
        use RevisionEvent::*;
        use RevisionStage::*;
        let legal = [
            (Draft, Evaluate, Evaluated),
            (Evaluated, Publish, Published),
            (Published, Promote, Champion),
            (Champion, Retire, Retired),
            (Published, Retire, Retired),
            (Evaluated, Retire, Retired),
            (Retired, Reinstate, Champion),
        ];
        let events = REVISION_MACHINE.events;
        for &from in REVISION_MACHINE.states {
            for &event in events {
                let expected = legal.iter().find(|(f, e, _)| *f == from && *e == event);
                match (expected, from.apply(event)) {
                    (Some((_, _, to)), Ok(next)) => assert_eq!(next, *to),
                    (None, Err(refused)) => {
                        assert_eq!(refused.machine, "RevisionMachine");
                        assert_eq!(refused.from, from.stored());
                        assert_eq!(refused.event, event.name());
                    }
                    (expected, got) => panic!("{from:?} on {event:?}: {expected:?} vs {got:?}"),
                }
            }
        }
        assert_eq!(REVISION_MACHINE.edges.len(), legal.len());
    }

    // spec: MEDULLA-DECISION-ENGINE-R805.7
    #[test]
    fn promotion_knobs_lie_within_bounds_and_gate_knobs_are_fixed() {
        for spec in KNOBS {
            assert!(
                spec.admits(&spec.default),
                "{} default out of range",
                spec.name
            );
            let gate = spec.name.starts_with("promotion.");
            assert_eq!(spec.safety == KnobSafety::Fixed, gate, "{}", spec.name);
        }
        assert_eq!(KNOBS[MIN_CASES].name, "promotion.min_cases");
        assert_eq!(KNOBS[MIN_MARGIN].name, "promotion.min_margin");
        assert_eq!(
            KNOBS[MEASURED_FACT_MIN_CASES].name,
            "measured_fact.min_cases"
        );
        assert!(REFUSALS
            .iter()
            .any(|c| c.as_str() == "PROMOTION_NO_BASELINE"));
    }

    // spec: MEDULLA-DECISION-ENGINE-R805.10
    #[test]
    fn added_subject_kinds_round_trip_with_pointer_keys() {
        let cases = [
            (
                RevisionSubject::Definition {
                    definition_kind: DefinitionKind::Agent,
                    definition_ref: "agent-a".to_owned(),
                },
                "champion:definition:agent:agent-a",
            ),
            (
                RevisionSubject::Definition {
                    definition_kind: DefinitionKind::Workflow,
                    definition_ref: "wf".to_owned(),
                },
                "champion:definition:workflow:wf",
            ),
            (
                RevisionSubject::Definition {
                    definition_kind: DefinitionKind::Team,
                    definition_ref: "t".to_owned(),
                },
                "champion:definition:team:t",
            ),
            (
                RevisionSubject::Definition {
                    definition_kind: DefinitionKind::HarnessComponent,
                    definition_ref: "c".to_owned(),
                },
                "champion:definition:harness_component:c",
            ),
            (
                RevisionSubject::Definition {
                    definition_kind: DefinitionKind::HarnessBundle,
                    definition_ref: "b".to_owned(),
                },
                "champion:definition:harness_bundle:b",
            ),
            (
                RevisionSubject::GovernedSet {
                    scope_ref: "policy-rules".to_owned(),
                },
                "champion:governed_set:policy-rules",
            ),
            (
                RevisionSubject::MeasuredFact {
                    model_ref: "model-a".to_owned(),
                    property: "context_window".to_owned(),
                },
                "champion:measured_fact:model-a:context_window",
            ),
            (
                RevisionSubject::RerankCalibration {
                    scorer_version: "v3".to_owned(),
                },
                "champion:rerank_calibration:v3",
            ),
        ];
        for (subject, key) in cases {
            assert_eq!(subject.pointer_key(), key);
            round_trip(&subject);
            round_trip(&pointer(subject));
        }
        round_trip(&ReceiptKind::MeasurementRun);
        round_trip(&ReceiptKind::ReviewApproval);
        let evidence = HeldOutEvidence {
            receipt_kind: ReceiptKind::ReviewApproval,
            measurement: Some(MeasurementWindow {
                observed_at_ms: 2_000,
                champion_observed_at_ms: Some(1_000),
            }),
            review: Some(ReviewApproval {
                author: "a".to_owned(),
                reviewer: "b".to_owned(),
            }),
            ..evidence()
        };
        round_trip(&evidence);
    }
}
