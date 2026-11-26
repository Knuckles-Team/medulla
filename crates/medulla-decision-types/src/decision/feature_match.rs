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

use medulla_types::refusal_code::RefusalCode;

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

/// A `Close` names a `closed_by` change the repository index does not know.
pub const LEDGER_CLOSE_NO_CHANGE: RefusalCode = RefusalCode::new("LEDGER_CLOSE_NO_CHANGE");
/// A compare-and-set on a ledger entry read another revision than it holds.
pub const LEDGER_REVISION_CONFLICT: RefusalCode = RefusalCode::new("LEDGER_REVISION_CONFLICT");
/// Two features of one block share a high-entropy name and nothing else links them.
pub const MATCH_HIGH_ENTROPY_NAME_ONLY: RefusalCode =
    RefusalCode::new("MATCH_HIGH_ENTROPY_NAME_ONLY");

/// The refusal table of the feature ledger and the `feature_match` family
/// (MEDULLA-DECISION-ENGINE-R707.4).
pub const REFUSALS: &[RefusalCode] = &[
    LEDGER_CLOSE_NO_CHANGE,
    LEDGER_REVISION_CONFLICT,
    MATCH_HIGH_ENTROPY_NAME_ONLY,
];

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

impl LedgerEntry {
    /// The only writer of `status`: delegates to [`LedgerStatus::apply`] and
    /// leaves the entry untouched when the pair is refused.
    pub fn transition(&mut self, event: LedgerEvent) -> Result<(), TransitionRefused> {
        self.status = self.status.apply(event)?;
        Ok(())
    }
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

impl FeatureLedgerOp {
    /// Whether this operation changes durable state.
    pub fn is_mutation(&self) -> bool {
        !matches!(self, FeatureLedgerOp::Get { .. })
    }

    /// The authorization action this operation needs.
    pub fn authz_action(&self) -> &'static str {
        if self.is_mutation() {
            "agent:feature-ledger-write"
        } else {
            "agent:feature-ledger-read"
        }
    }
}

/// What a `FeatureLedger` operation answers: the entries it touched or read,
/// in feature id order (MEDULLA-DECISION-ENGINE-R707.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FeatureLedgerView {
    pub entries: Vec<LedgerEntry>,
}

/// The typed answer of a `FeatureMatch` question: the scored matches, the
/// merge clusters, how many features were past the block cap, and the plans
/// that close the gaps (MEDULLA-DECISION-ENGINE-R707.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FeatureMatchAnswer {
    pub matches: Vec<Match>,
    pub clusters: Vec<Vec<String>>,
    pub unscored: u64,
    pub plans: Vec<PlanProposal>,
}

/// The option id of the match between `local` and `external`.
pub fn match_option_id(local: &str, external: &str) -> String {
    format!("match:{local}:{external}")
}

/// The option id of a plan.
pub fn plan_option_id(plan_id: &str) -> String {
    format!("plan:{plan_id}")
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

    // spec: MEDULLA-DECISION-ENGINE-R831.2
    #[test]
    fn every_legal_transition_applies() {
        let legal = [
            (LedgerStatus::Open, LedgerEvent::Plan, LedgerStatus::Planned),
            (
                LedgerStatus::Planned,
                LedgerEvent::Close,
                LedgerStatus::Closed,
            ),
        ];
        assert_eq!(legal.len(), FEATURE_LEDGER_MACHINE.edges.len());
        for (from, event, to) in legal {
            assert_eq!(from.apply(event), Ok(to));
            let mut entry = entry_in(from);
            assert_eq!(entry.transition(event), Ok(()));
            assert_eq!(entry.status, to);
        }
    }

    fn entry_in(status: LedgerStatus) -> LedgerEntry {
        LedgerEntry {
            feature_id: "f1".into(),
            status,
            closed_by: None,
            revision: 0,
            match_ids: Vec::new(),
            plan_id: None,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R831.2
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        for state in STATES {
            for event in EVENTS {
                if FEATURE_LEDGER_MACHINE
                    .edges
                    .iter()
                    .any(|e| e.from == state && e.event == event)
                {
                    continue;
                }
                let refused = state.apply(event).unwrap_err();
                assert_eq!(refused.machine, "FeatureLedgerMachine");
                assert_eq!(refused.from, state.stored());
                assert_eq!(refused.event, event.name());
                assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
                // The holder keeps its state when the transition is refused.
                let mut entry = entry_in(state);
                assert_eq!(entry.transition(event), Err(refused));
                assert_eq!(entry.status, state);
            }
        }
    }

    /// Lines that assign a `status` field outside the transition function.
    // spec: MEDULLA-DECISION-ENGINE-R831.2
    #[test]
    fn no_writer_sets_status_outside_transition() {
        for source in [
            include_str!("../../../medulla-decision-stats/src/decision/feature_match.rs"),
            include_str!("../../../medulla-h-decide/src/decide/feature_ledger.rs"),
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let writes: Vec<&str> = production
                .lines()
                .map(str::trim)
                .filter(|l| !l.starts_with("//") && l.contains(".status = "))
                .collect();
            assert!(writes.is_empty(), "{writes:?}");
        }
        let own = include_str!("feature_match.rs");
        let production = own.split("#[cfg(test)]").next().unwrap();
        let writes: Vec<&str> = production
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//") && l.contains(".status = "))
            .collect();
        assert_eq!(writes, vec!["self.status = self.status.apply(event)?;"]);
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
    #[test]
    fn ledger_ops_classify_reads_and_writes() {
        let get = FeatureLedgerOp::Get {
            feature_ids: Vec::new(),
        };
        let open = FeatureLedgerOp::Open {
            feature_id: "f".into(),
            match_ids: Vec::new(),
        };
        assert!(!get.is_mutation());
        assert_eq!(get.authz_action(), "agent:feature-ledger-read");
        assert!(open.is_mutation());
        assert_eq!(open.authz_action(), "agent:feature-ledger-write");
        assert_eq!(match_option_id("a", "b"), "match:a:b");
        assert_eq!(plan_option_id("p"), "plan:p");
    }
}
