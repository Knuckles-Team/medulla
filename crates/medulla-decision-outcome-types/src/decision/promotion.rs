//! Typed revision lifecycle, held-out evidence, champion and rollback records
//! for every promotable subject (MEDULLA-DECISION-ENGINE-R805.7, R805.10,
//! R813.1).
//!
//! A revision moves through [`RevisionStage`] only by [`RevisionStage::apply`],
//! the one transition function of `RevisionMachine`; an illegal event is
//! refused with the typed [`TransitionRefused`] record.

use serde::{Deserialize, Serialize};

use medulla_decision_types::decision::held_out::{
    DefinitionKind, GraderKind, HeldOutEvidence, MeasurementWindow, ReceiptKind, ReviewApproval,
    RevisionSubject, HELD_OUT_EVIDENCE_SCHEMA_VERSION,
};
use medulla_decision_types::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use medulla_types::contract::OpaqueId;
use medulla_types::refusal_code::RefusalCode;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};

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

impl TryFrom<&crate::policy_evolution::PolicyEvaluation> for HeldOutEvidence {
    type Error = RefusalCode;

    /// The held-out evidence a policy evaluation carries
    /// (MEDULLA-DECISION-ENGINE-R811.8). A failed safety outcome is not
    /// evidence: it refuses `PROMOTION_UNEVALUATED`.
    fn try_from(evaluation: &crate::policy_evolution::PolicyEvaluation) -> Result<Self, RefusalCode> {
        use crate::policy_evolution::SafetyOutcome;
        if evaluation.safety == SafetyOutcome::Failed {
            return Err(PROMOTION_UNEVALUATED);
        }
        let record = serde_json::to_value(evaluation).map_err(|_| PROMOTION_UNEVALUATED)?;
        let compact = serde_json::to_string(&record).map_err(|_| PROMOTION_UNEVALUATED)?;
        let receipt_digest =
            medulla_types::contract::Digest256::sha256(compact.as_bytes()).to_hex();
        let challenger = i64::from(evaluation.metrics.task_success_ppm);
        let champion = evaluation
            .champion_revision_ref
            .as_ref()
            .map(|_| i64::from(evaluation.metrics.baseline_task_success_ppm));
        Ok(Self {
            schema_version: HELD_OUT_EVIDENCE_SCHEMA_VERSION,
            subject: evaluation.subject.clone(),
            challenger_ref: evaluation.revision_ref.as_str().to_string(),
            champion_ref: evaluation
                .champion_revision_ref
                .as_ref()
                .map(|champion| champion.as_str().to_string()),
            receipt_kind: ReceiptKind::PolicyEvaluation,
            receipt_digest,
            evaluation_set_digest: evaluation.evaluation_set_digest.to_hex(),
            challenger_score_ppm: challenger,
            champion_score_ppm: champion,
            delta_ppm: champion.map_or(0, |champion| challenger - champion),
            case_count: evaluation.held_out_trajectory_ids.len() as u64,
            independent: evaluation.evaluator_kind.decides_promotion(),
            grader_kind: evaluation.evaluator_kind,
            measurement: None,
            review: None,
        })
    }
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
    /// The canary split of a skill champion, when a challenger takes a share of its runs
    /// (MEDULLA-DECISION-ENGINE-R712.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canary: Option<medulla_decision_types::skill_eval::SkillCanary>,
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
/// `promotion.regression_window`: cases a later run needs before it can roll
/// a promotion back (MEDULLA-DECISION-ENGINE-R805.2).
pub const REGRESSION_WINDOW: usize = 3;
/// `promotion.regression_margin`: score drop, as a fraction, that rolls a
/// promotion back (MEDULLA-DECISION-ENGINE-R805.2).
pub const REGRESSION_MARGIN: usize = 4;
/// `promotion.bootstrap_resamples`: paired-bootstrap resamples of a rollout
/// set (MEDULLA-DECISION-ENGINE-R654.2).
pub const BOOTSTRAP_RESAMPLES: usize = 5;
/// `promotion.bootstrap_level`: confidence level of the paired-bootstrap
/// lower bound (MEDULLA-DECISION-ENGINE-R654.2).
pub const BOOTSTRAP_LEVEL: usize = 6;

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
    KnobSpec {
        name: "promotion.regression_window",
        unit: "cases",
        default: KnobValue::Integer(50),
        min: KnobValue::Integer(20),
        max: KnobValue::Integer(10_000),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "promotion.regression_margin",
        unit: "fraction",
        default: KnobValue::DecimalMicros(20_000),
        min: KnobValue::DecimalMicros(5_000),
        max: KnobValue::DecimalMicros(200_000),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "promotion.bootstrap_resamples",
        unit: "resamples",
        default: KnobValue::Integer(1_000),
        min: KnobValue::Integer(200),
        max: KnobValue::Integer(10_000),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "promotion.bootstrap_level",
        unit: "fraction",
        default: KnobValue::DecimalMicros(950_000),
        min: KnobValue::DecimalMicros(800_000),
        max: KnobValue::DecimalMicros(990_000),
        safety: KnobSafety::Fixed,
    },
];

use medulla_decision_types::decision::codes::PROMOTION_UNEVALUATED;

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
            canary: None,
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

    // spec: MEDULLA-DECISION-ENGINE-R805.13
    #[test]
    fn pointer_key_parses_back_to_its_subject() {
        for subject in [
            head(),
            RevisionSubject::Program {
                program_ref: "eg:program:a".to_owned(),
            },
            RevisionSubject::KnobSet {
                policy_id: "policy-a".to_owned(),
            },
            RevisionSubject::ModelPolicy {
                family_ref: "family-a".to_owned(),
            },
            RevisionSubject::Definition {
                definition_kind: DefinitionKind::HarnessBundle,
                definition_ref: "agent:deployer:eg:taskclass/deploy".to_owned(),
            },
            RevisionSubject::GovernedSet {
                scope_ref: "scope-a".to_owned(),
            },
            RevisionSubject::MeasuredFact {
                model_ref: "model:x".to_owned(),
                property: "latency".to_owned(),
            },
            RevisionSubject::RerankCalibration {
                scorer_version: "v1".to_owned(),
            },
            RevisionSubject::RetrievalStrategy {
                strategy_ref: "strategy-a".to_owned(),
            },
            RevisionSubject::Skill {
                skill_ref: "skill:triage@3".to_owned(),
                tier: None,
            },
            RevisionSubject::KnowledgeEntry {
                entry_ref: "term:orders.amount".to_owned(),
            },
        ] {
            assert_eq!(
                RevisionSubject::from_pointer_key(&subject.pointer_key()),
                Some(subject)
            );
        }
        assert_eq!(RevisionSubject::from_pointer_key("champion:nothing:x"), None);
        assert_eq!(RevisionSubject::from_pointer_key("pointer:x"), None);
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

    // spec: MEDULLA-DECISION-ENGINE-R811.8
    #[test]
    fn policy_evaluation_maps_to_held_out_evidence() {
        use crate::policy_evolution::{
            EvaluationMetrics, EvaluationVerdict, PolicyEvaluation, SafetyOutcome,
        };
        use medulla_types::contract::{BoundedVec, Digest256};
        let evaluation = PolicyEvaluation {
            subject: RevisionSubject::ModelPolicy {
                family_ref: "family:policy".to_owned(),
            },
            revision_ref: reference("polver", "v2"),
            champion_revision_ref: Some(reference("polver", "v1")),
            evaluation_set_digest: Digest256::from_bytes([7; 32]),
            evaluator_digest: Digest256::from_bytes([8; 32]),
            evaluator_kind: GraderKind::Human,
            held_out_trajectory_ids: BoundedVec::new(vec![
                reference("poltraj", "a"),
                reference("poltraj", "b"),
            ])
            .expect("within bound"),
            metrics: EvaluationMetrics {
                task_success_ppm: 800_000,
                baseline_task_success_ppm: 600_000,
                ..EvaluationMetrics::default()
            },
            safety: SafetyOutcome::Passed,
            verdict: EvaluationVerdict::Accepted,
            report: None,
        };
        let held = HeldOutEvidence::try_from(&evaluation).expect("maps");
        assert_eq!(held.schema_version, 1);
        assert_eq!(held.challenger_ref, "polver:v2");
        assert_eq!(held.champion_ref.as_deref(), Some("polver:v1"));
        assert_eq!(held.receipt_kind, ReceiptKind::PolicyEvaluation);
        assert_eq!(held.evaluation_set_digest, Digest256::from_bytes([7; 32]).to_hex());
        assert_eq!(held.challenger_score_ppm, 800_000);
        assert_eq!(held.champion_score_ppm, Some(600_000));
        assert_eq!(held.delta_ppm, 200_000);
        assert_eq!(held.case_count, 2);
        assert!(held.independent);
        assert_eq!(held.grader_kind, GraderKind::Human);
        assert_eq!(held.receipt_digest.len(), 64);
        let judged = PolicyEvaluation {
            evaluator_kind: GraderKind::ModelJudge,
            champion_revision_ref: None,
            ..evaluation.clone()
        };
        let held = HeldOutEvidence::try_from(&judged).expect("maps");
        assert!(!held.independent);
        assert_eq!((held.champion_score_ppm, held.delta_ppm), (None, 0));
        let failed = PolicyEvaluation {
            safety: SafetyOutcome::Failed,
            verdict: EvaluationVerdict::Rejected,
            ..evaluation
        };
        assert_eq!(HeldOutEvidence::try_from(&failed), Err(PROMOTION_UNEVALUATED));
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

    // spec: MEDULLA-DECISION-ENGINE-R831.3
    #[test]
    fn every_legal_transition_applies() {
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
        assert_eq!(legal.len(), REVISION_MACHINE.edges.len());
        for (from, event, to) in legal {
            assert_eq!(from.apply(event), Ok(to));
            let mut row = revision_row(from);
            assert_eq!(row.transition(event), Ok(()));
            assert_eq!(row.stage, to);
        }
    }

    fn revision_row(stage: RevisionStage) -> super::super::revision_list::RevisionRow {
        super::super::revision_list::RevisionRow {
            id: "r1".into(),
            subject_kind: "harness".into(),
            subject_ref: "h1".into(),
            stage,
            digest: "d".into(),
            evaluation_receipt_id: None,
            created_ms: 1,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R831.3
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        for &from in REVISION_MACHINE.states {
            for &event in REVISION_MACHINE.events {
                if REVISION_MACHINE
                    .edges
                    .iter()
                    .any(|e| e.from == from && e.event == event)
                {
                    continue;
                }
                let refused = from.apply(event).unwrap_err();
                assert_eq!(refused.machine, "RevisionMachine");
                assert_eq!(refused.from, from.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
                // The holder keeps its stage when the transition is refused.
                let mut row = revision_row(from);
                assert_eq!(row.transition(event), Err(refused));
                assert_eq!(row.stage, from);
            }
        }
    }

    /// Lines that assign a `stage` field outside the transition function.
    // spec: MEDULLA-DECISION-ENGINE-R831.3
    #[test]
    fn no_writer_sets_stage_outside_transition() {
        for source in [
            include_str!("../../../medulla-h-decide/src/decide/stat_proposals.rs"),
            include_str!("../../../medulla-h-decide/src/decide/stat_pointer.rs"),
            include_str!("../../../medulla-h-decide/src/decide/stat_harness_adoption.rs"),
            include_str!("../../../medulla-program/src/optimizer.rs"),
            include_str!("revision_list.rs"),
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//") && l.contains(".stage = "))
                .collect();
            if source.contains("pub fn transition(&mut self, event: RevisionEvent)") {
                assert_eq!(writes, vec!["self.stage = self.stage.apply(event)?;"]);
            } else {
                assert!(writes.is_empty(), "{writes:?}");
            }
        }
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
            (
                RevisionSubject::RetrievalStrategy {
                    strategy_ref: "hyde-rrf".to_owned(),
                },
                "champion:retrieval_strategy:hyde-rrf",
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
