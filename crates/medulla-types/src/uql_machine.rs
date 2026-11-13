//! MEDULLA-FEDERATED-QUERY-R738.17: the lifecycle of one `EXPLAIN` / `PROFILE` stage report as
//! an explicit state machine (`UqlMachine`).
//!
//! `Planned` is the initial state: the report carries the cost model's estimate. The one event
//! `Measure` enters the terminal `Measured`, when `PROFILE` attaches the actual rows and time.
//! [`UqlStage::apply`] is the only function that produces a new stage, and a refused event is
//! the typed `TRANSITION_REFUSED` naming the machine, the current state and the event. The
//! state lives in `UqlStageReport.stage`; `UqlStageReport::planned` builds a report in the
//! initial state and `UqlStageReport::measured` is the one writer of `rows` and `micros`. The
//! ontology description is `UqlMachine` in `state-machines-v1.ttl`.

use crate::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use crate::wire::{UqlStage, UqlStageReport};

/// The closed event set of `UqlMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UqlStageEvent {
    /// The stage ran under `PROFILE` and its rows and time are attached.
    Measure,
}

impl MachineState for UqlStage {
    fn stored(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Measured => "measured",
        }
    }
}

impl MachineEvent for UqlStageEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Measure => "Measure",
        }
    }
}

/// `UqlMachine`: the closed edge table. Every other pair is refused.
pub static UQL_MACHINE: MachineDef<UqlStage, UqlStageEvent, &'static str> = MachineDef {
    name: "UqlMachine",
    states: &[UqlStage::Planned, UqlStage::Measured],
    initial: UqlStage::Planned,
    terminals: &[UqlStage::Measured],
    events: &[UqlStageEvent::Measure],
    guards: &[],
    edges: &[Edge {
        id: "U01",
        from: UqlStage::Planned,
        event: UqlStageEvent::Measure,
        guard: None,
        to: UqlStage::Measured,
    }],
};

impl UqlStage {
    /// The stage of a report before any measurement.
    pub const INITIAL: UqlStage = UqlStage::Planned;

    /// The one transition function of `UqlMachine`.
    pub fn apply(self, event: UqlStageEvent) -> Result<Self, TransitionRefused> {
        transition(&UQL_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

impl UqlStageReport {
    /// A report in the initial state: the estimate only, no measurement.
    pub fn planned(clause: String, estimated_rows: f64) -> Self {
        Self {
            clause,
            stage: UqlStage::INITIAL,
            estimated_rows,
            rows: None,
            micros: None,
        }
    }

    /// Attach the measured rows and time: the one writer of `rows` and `micros`. A report that
    /// is already measured is refused with the typed `TRANSITION_REFUSED` and left unchanged.
    pub fn measured(self, rows: u64, micros: u64) -> Result<Self, TransitionRefused> {
        let stage = self.stage.apply(UqlStageEvent::Measure)?;
        Ok(Self {
            stage,
            rows: Some(rows),
            micros: Some(micros),
            ..self
        })
    }

    /// A report built from a stage that already ran under `PROFILE`: a planned report taken
    /// through `Measure`. The edge from the initial state is declared (`U01`) and proven by
    /// `every_legal_transition_applies`, so the transition cannot be refused here.
    pub fn profiled(clause: String, estimated_rows: f64, rows: u64, micros: u64) -> Self {
        Self::planned(clause, estimated_rows)
            .measured(rows, micros)
            .expect("UqlMachine U01: Planned --Measure--> Measured is a declared edge")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition_refused::TransitionRefusalReason;

    // spec: MEDULLA-FEDERATED-QUERY-R738.17
    #[test]
    fn every_legal_transition_applies() {
        assert_eq!(UQL_MACHINE.initial, UqlStage::INITIAL);
        for edge in UQL_MACHINE.edges {
            assert_eq!(edge.from.apply(edge.event), Ok(edge.to), "{}", edge.id);
        }
        let planned = UqlStageReport::planned("MATCH ()".into(), 3.0);
        assert_eq!(planned.stage, UqlStage::Planned);
        assert_eq!((planned.rows, planned.micros), (None, None));
        let measured = planned.measured(2, 40).unwrap();
        assert_eq!(measured.stage, UqlStage::Measured);
        assert_eq!((measured.rows, measured.micros), (Some(2), Some(40)));
        assert_eq!(measured.clause, "MATCH ()");
        let profiled = UqlStageReport::profiled("MATCH ()".into(), 3.0, 2, 40);
        assert_eq!(profiled, measured);
        for stage in UQL_MACHINE.states {
            let wire = serde_json::to_string(stage).unwrap();
            assert_eq!(wire, format!("\"{}\"", stage.stored()));
            assert_eq!(serde_json::from_str::<UqlStage>(&wire).unwrap(), *stage);
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R738.17
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        let refused = UqlStage::Measured
            .apply(UqlStageEvent::Measure)
            .unwrap_err();
        assert_eq!(refused.machine, "UqlMachine");
        assert_eq!(refused.from, "measured");
        assert_eq!(refused.event, "Measure");
        assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
        assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
        for state in UQL_MACHINE.states {
            for event in UQL_MACHINE.events {
                let listed = UQL_MACHINE
                    .edges
                    .iter()
                    .any(|edge| edge.from == *state && edge.event == *event);
                assert_eq!(state.apply(*event).is_ok(), listed, "{state:?} {event:?}");
            }
        }
        let measured = UqlStageReport::planned("MATCH ()".into(), 1.0)
            .measured(1, 1)
            .unwrap();
        assert!(measured.clone().measured(9, 9).is_err());
        assert_eq!(measured.rows, Some(1));
        assert!(serde_json::from_str::<UqlStage>("\"running\"").is_err());
    }

    /// No production file builds a `UqlStageReport` literal or assigns `.stage = UqlStage::`:
    /// reports are built by `UqlStageReport::planned` and `measured`.
    // spec: MEDULLA-FEDERATED-QUERY-R738.17
    #[test]
    fn no_writer_sets_stage_outside_apply() {
        fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if path.is_dir() {
                    if name != "target" {
                        walk(&path, hits);
                    }
                } else if name.ends_with(".rs")
                    && name != "uql_machine.rs"
                    && name != "wire_query_core.rs"
                {
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    for (number, line) in text.lines().enumerate() {
                        let line = line.trim();
                        if line.starts_with("//") {
                            continue;
                        }
                        if line.contains(".stage = UqlStage::")
                            || line.contains("UqlStageReport {")
                                && !line.contains("struct")
                                && !line.contains("impl")
                                && !line.contains("Vec<")
                        {
                            hits.push(format!("{}:{}", path.display(), number + 1));
                        }
                    }
                }
            }
        }
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut hits = Vec::new();
        walk(&crates, &mut hits);
        assert!(hits.is_empty(), "direct UqlStage writes: {hits:?}");
    }
}
