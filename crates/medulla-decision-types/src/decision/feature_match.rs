//! MEDULLA-DECISION-ENGINE-R707.1: the feature-match model -- the rows of a
//! feature matrix, the matches between local and external features, and the
//! feature ledger with the plans that close its gaps.
//!
//! The ledger status is the explicit `FeatureLedgerMachine`: `Open` is
//! initial, `Plan` enters `Planned`, `Close` enters the terminal `Closed`.
//! [`LedgerStatus::apply`] is the only function that produces a new status;
//! an illegal pair is the typed `TRANSITION_REFUSED` naming the machine, the
//! current status and the event. The ontology description is
//! `FeatureLedgerMachine` in `state-machines-v1.ttl`.

use serde::{Deserialize, Serialize};

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// One feature of a studied project or of the local system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FeatureMatrixRow {
    pub feature_id: String,
    pub project: String,
    pub name: String,
    /// The ontology label the feature is blocked under.
    pub label: String,
    pub token_digest: String,
}

/// A scored pairing of a local feature with an external one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Match {
    pub local: String,
    pub external: String,
    /// Similarity in parts per million, `0..=1_000_000`.
    pub score_ppm: u32,
}

/// The status of one ledger entry: the closed state set of `FeatureLedgerMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum LedgerStatus {
    /// A gap is recorded and has no plan.
    Open,
    /// A plan proposes to close the gap.
    Planned,
    /// A known change closed the gap.
    Closed,
}

/// The closed event set of `FeatureLedgerMachine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LedgerEvent {
    /// A plan was attached to the entry.
    Plan,
    /// A change closed the entry.
    Close,
}

impl MachineState for LedgerStatus {
    fn stored(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Planned => "planned",
            Self::Closed => "closed",
        }
    }
}

impl MachineEvent for LedgerEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Close => "Close",
        }
    }
}

/// `FeatureLedgerMachine`: the closed edge table. Every other pair is refused.
pub static FEATURE_LEDGER_MACHINE: MachineDef<LedgerStatus, LedgerEvent, &'static str> =
    MachineDef {
        name: "FeatureLedgerMachine",
        states: &[
            LedgerStatus::Open,
            LedgerStatus::Planned,
            LedgerStatus::Closed,
        ],
        initial: LedgerStatus::Open,
        terminals: &[LedgerStatus::Closed],
        events: &[LedgerEvent::Plan, LedgerEvent::Close],
        guards: &[],
        edges: &[
            Edge {
                id: "F01",
                from: LedgerStatus::Open,
                event: LedgerEvent::Plan,
                guard: None,
                to: LedgerStatus::Planned,
            },
            Edge {
                id: "F02",
                from: LedgerStatus::Planned,
                event: LedgerEvent::Close,
                guard: None,
                to: LedgerStatus::Closed,
            },
        ],
    };

impl LedgerStatus {
    /// The one transition function of `FeatureLedgerMachine`.
    pub fn apply(self, event: LedgerEvent) -> Result<Self, TransitionRefused> {
        transition(&FEATURE_LEDGER_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

/// One feature's place in the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LedgerEntry {
    pub feature_id: String,
    /// The one persisted state field; written only through [`LedgerStatus::apply`].
    pub status: LedgerStatus,
    /// The change that closed the entry; set only once `status` is `closed`.
    pub closed_by: Option<String>,
    /// Compare-and-set counter of the entry.
    pub revision: u64,
    pub match_ids: Vec<String>,
    pub plan_id: Option<String>,
}

/// A plan that proposes to close a set of gaps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PlanProposal {
    pub plan_id: String,
    pub feature_ids: Vec<String>,
    pub rationale_digest: String,
}

/// The operations of the feature ledger. Closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum FeatureLedgerOp {
    /// Record a gap with the matches it came from.
    Open {
        feature_id: String,
        match_ids: Vec<String>,
    },
    /// Attach a plan to open entries.
    Plan {
        plan: PlanProposal,
        expected_revision: u64,
    },
    /// Close a planned entry by a known change.
    Close {
        feature_id: String,
        closed_by: String,
        expected_revision: u64,
    },
    /// Read entries.
    Get { feature_ids: Vec<String> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_types_round_trip() {
        let row = FeatureMatrixRow {
            feature_id: "f1".into(),
            project: "p".into(),
            name: "Rate limiter".into(),
            label: "eg:capability/limits".into(),
            token_digest: "d".into(),
        };
        let pair = Match {
            local: "f1".into(),
            external: "f2".into(),
            score_ppm: 750_000,
        };
        let entry = LedgerEntry {
            feature_id: "f1".into(),
            status: LedgerStatus::Planned,
            closed_by: None,
            revision: 2,
            match_ids: vec!["m1".into()],
            plan_id: Some("plan-1".into()),
        };
        let plan = PlanProposal {
            plan_id: "plan-1".into(),
            feature_ids: vec!["f1".into()],
            rationale_digest: "r".into(),
        };
        fn round<T>(value: &T)
        where
            T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
        {
            let json = serde_json::to_string(value).unwrap();
            assert_eq!(&serde_json::from_str::<T>(&json).unwrap(), value);
        }
        round(&row);
        round(&pair);
        round(&entry);
        round(&plan);
        for status in [
            LedgerStatus::Open,
            LedgerStatus::Planned,
            LedgerStatus::Closed,
        ] {
            round(&status);
        }
        for op in [
            FeatureLedgerOp::Open {
                feature_id: "f1".into(),
                match_ids: vec![],
            },
            FeatureLedgerOp::Plan {
                plan,
                expected_revision: 1,
            },
            FeatureLedgerOp::Close {
                feature_id: "f1".into(),
                closed_by: "c".into(),
                expected_revision: 2,
            },
            FeatureLedgerOp::Get {
                feature_ids: vec!["f1".into()],
            },
        ] {
            round(&op);
        }
        let extra = r#"{"local":"a","external":"b","score_ppm":1,"extra":1}"#;
        assert!(serde_json::from_str::<Match>(extra).is_err());
        let unknown_status = serde_json::from_str::<LedgerStatus>("\"archived\"");
        assert!(unknown_status.is_err());
        assert!(serde_json::from_str::<FeatureLedgerOp>(r#"{"op":"erase"}"#).is_err());
    }

    const STATES: [LedgerStatus; 3] = [
        LedgerStatus::Open,
        LedgerStatus::Planned,
        LedgerStatus::Closed,
    ];
    const EVENTS: [LedgerEvent; 2] = [LedgerEvent::Plan, LedgerEvent::Close];

    #[test]
    fn every_legal_ledger_transition_is_taken_and_every_other_pair_refused() {
        let legal = [
            (LedgerStatus::Open, LedgerEvent::Plan, LedgerStatus::Planned),
            (
                LedgerStatus::Planned,
                LedgerEvent::Close,
                LedgerStatus::Closed,
            ),
        ];
        for state in STATES {
            for event in EVENTS {
                let result = state.apply(event);
                match legal.iter().find(|(f, e, _)| *f == state && *e == event) {
                    Some((_, _, to)) => assert_eq!(result, Ok(*to)),
                    None => {
                        let refused = result.unwrap_err();
                        assert_eq!(refused.machine, "FeatureLedgerMachine");
                        assert_eq!(refused.from, state.stored());
                        assert_eq!(refused.event, event.name());
                    }
                }
            }
        }
    }

    const MACHINES: &str = include_str!("../../../medulla-core/ontology/state-machines-v1.ttl");

    #[test]
    fn the_ontology_describes_the_ledger_machine() {
        assert!(MACHINES.contains(":FeatureLedgerMachine a :StateMachine"));
        let cap = |s: &str| format!("{}{}", s[..1].to_uppercase(), &s[1..]);
        for state in STATES {
            assert!(MACHINES.contains(&format!(":FeatureLedgerMachine_{} a", cap(state.stored()))));
            assert!(MACHINES.contains(&format!(":storedForm \"{}\"", state.stored())));
        }
        for event in EVENTS {
            assert!(MACHINES.contains(&format!(":FeatureLedgerMachine_Event_{} a", event.name())));
        }
        for edge in FEATURE_LEDGER_MACHINE.edges {
            let block = format!(
                ":FeatureLedgerMachine_{} a :FeatureLedgerTransition",
                edge.id
            );
            let start = MACHINES.find(&block).expect("edge declared");
            let text = &MACHINES[start..];
            let text = &text[..text.find(" .\n").unwrap()];
            assert!(text.contains(&format!(
                ":fromState :FeatureLedgerMachine_{}",
                cap(edge.from.stored())
            )));
            assert!(text.contains(&format!(
                ":toState :FeatureLedgerMachine_{}",
                cap(edge.to.stored())
            )));
            assert!(text.contains(&format!(
                ":firedBy :FeatureLedgerMachine_Event_{}",
                edge.event.name()
            )));
        }
    }
}
