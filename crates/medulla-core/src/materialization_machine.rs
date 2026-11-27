//! MEDULLA-INCREMENTAL-VIEWS-R016.2 — the materialization phase of a graph image as one
//! explicit state machine (organization constitution Article XXXI).
//!
//! `MaterializationPhase::apply` is the only writer of `MaterializationManifest.phase`. An image
//! starts catalog-only, pages in as `Partial`, settles `Complete` when the content-derived
//! indexes came up valid, and is `Failed` otherwise. A `Complete` image is never paged in again.
//! The manifest's validity is derived from the phase: an image is valid exactly when `Complete`.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};

use crate::registry::MaterializationPhase;

impl MachineState for MaterializationPhase {
    fn stored(self) -> &'static str {
        match self {
            Self::CatalogOnly => "catalog_only",
            Self::Partial => "partial",
            Self::Complete => "complete",
            Self::Failed => "failed",
        }
    }
}

/// An event of `MaterializationMachine`: one per writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MaterializationEvent {
    /// The image failed (`MaterializationManifest::fail`, or a finish whose indexes are invalid).
    Fail,
    /// A page of material was added to the resident image (`advance_partial_manifest`).
    AdvancePartialManifest,
    /// Materialization finished with valid indexes (`finish_materialization`).
    FinishMaterialization,
}

impl MachineEvent for MaterializationEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Fail => "Fail",
            Self::AdvancePartialManifest => "AdvancePartialManifest",
            Self::FinishMaterialization => "FinishMaterialization",
        }
    }
}

/// The facts the guards read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterializationFacts {
    /// Every content-derived secondary index came up valid (guard of M04, M05, M06 and M10).
    pub indexes_valid: bool,
}

/// Guard of `MaterializationMachine`: the rebuilt secondary indexes are valid.
pub const MATERIALIZATION_GUARD_INDEXES_VALID: &str = "IndexesValid";

/// `MaterializationMachine`: the closed edge table. Every other pair is refused.
pub static MATERIALIZATION_MACHINE: MachineDef<
    MaterializationPhase,
    MaterializationEvent,
    &'static str,
> = MachineDef {
    name: "MaterializationMachine",
    states: &[
        MaterializationPhase::CatalogOnly,
        MaterializationPhase::Partial,
        MaterializationPhase::Complete,
        MaterializationPhase::Failed,
    ],
    initial: MaterializationPhase::CatalogOnly,
    terminals: &[],
    events: &[
        MaterializationEvent::Fail,
        MaterializationEvent::AdvancePartialManifest,
        MaterializationEvent::FinishMaterialization,
    ],
    guards: &[MATERIALIZATION_GUARD_INDEXES_VALID],
    edges: &[
        Edge {
            id: "M01",
            from: MaterializationPhase::CatalogOnly,
            event: MaterializationEvent::AdvancePartialManifest,
            guard: None,
            to: MaterializationPhase::Partial,
        },
        Edge {
            id: "M02",
            from: MaterializationPhase::Partial,
            event: MaterializationEvent::AdvancePartialManifest,
            guard: None,
            to: MaterializationPhase::Partial,
        },
        Edge {
            id: "M03",
            from: MaterializationPhase::Failed,
            event: MaterializationEvent::AdvancePartialManifest,
            guard: None,
            to: MaterializationPhase::Partial,
        },
        Edge {
            id: "M04",
            from: MaterializationPhase::CatalogOnly,
            event: MaterializationEvent::FinishMaterialization,
            guard: Some(MATERIALIZATION_GUARD_INDEXES_VALID),
            to: MaterializationPhase::Complete,
        },
        Edge {
            id: "M05",
            from: MaterializationPhase::Partial,
            event: MaterializationEvent::FinishMaterialization,
            guard: Some(MATERIALIZATION_GUARD_INDEXES_VALID),
            to: MaterializationPhase::Complete,
        },
        Edge {
            id: "M06",
            from: MaterializationPhase::Failed,
            event: MaterializationEvent::FinishMaterialization,
            guard: Some(MATERIALIZATION_GUARD_INDEXES_VALID),
            to: MaterializationPhase::Complete,
        },
        Edge {
            id: "M07",
            from: MaterializationPhase::CatalogOnly,
            event: MaterializationEvent::Fail,
            guard: None,
            to: MaterializationPhase::Failed,
        },
        Edge {
            id: "M08",
            from: MaterializationPhase::Partial,
            event: MaterializationEvent::Fail,
            guard: None,
            to: MaterializationPhase::Failed,
        },
        Edge {
            id: "M09",
            from: MaterializationPhase::Complete,
            event: MaterializationEvent::Fail,
            guard: None,
            to: MaterializationPhase::Failed,
        },
        Edge {
            id: "M10",
            from: MaterializationPhase::Failed,
            event: MaterializationEvent::Fail,
            guard: None,
            to: MaterializationPhase::Failed,
        },
        Edge {
            id: "M11",
            from: MaterializationPhase::Complete,
            event: MaterializationEvent::FinishMaterialization,
            guard: Some(MATERIALIZATION_GUARD_INDEXES_VALID),
            to: MaterializationPhase::Complete,
        },
    ],
};

impl MaterializationPhase {
    /// The one transition function of `MaterializationMachine`; the only writer of the phase.
    pub fn apply(
        self,
        event: MaterializationEvent,
        facts: MaterializationFacts,
    ) -> Result<Self, TransitionRefused> {
        transition(&MATERIALIZATION_MACHINE, self, event, |guard| match guard {
            MATERIALIZATION_GUARD_INDEXES_VALID => facts.indexes_valid,
            _ => false,
        })
        .map(|edge| edge.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [MaterializationPhase; 4] = [
        MaterializationPhase::CatalogOnly,
        MaterializationPhase::Partial,
        MaterializationPhase::Complete,
        MaterializationPhase::Failed,
    ];
    const EVENTS: [MaterializationEvent; 3] = [
        MaterializationEvent::Fail,
        MaterializationEvent::AdvancePartialManifest,
        MaterializationEvent::FinishMaterialization,
    ];

    // spec: MEDULLA-INCREMENTAL-VIEWS-R016.2
    #[test]
    fn every_legal_transition_applies() {
        for edge in MATERIALIZATION_MACHINE.edges {
            let facts = MaterializationFacts {
                indexes_valid: edge.guard.is_some(),
            };
            assert_eq!(
                edge.from.apply(edge.event, facts),
                Ok(edge.to),
                "edge {}",
                edge.id
            );
        }
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R016.2
    #[test]
    fn every_illegal_pair_is_refused_with_transition_refused() {
        for from in ALL {
            for event in EVENTS {
                for indexes_valid in [false, true] {
                    let facts = MaterializationFacts { indexes_valid };
                    let legal = MATERIALIZATION_MACHINE.edges.iter().any(|edge| {
                        edge.from == from
                            && edge.event == event
                            && (edge.guard.is_none() || indexes_valid)
                    });
                    let outcome = from.apply(event, facts);
                    if legal {
                        assert!(outcome.is_ok(), "{from:?} {event:?} {indexes_valid}");
                    } else {
                        let refused = outcome.expect_err("an illegal pair is refused");
                        assert_eq!(refused.machine, "MaterializationMachine");
                    }
                }
            }
        }
        // A complete image is never paged in again, and a finish needs valid indexes.
        assert!(MaterializationPhase::Complete
            .apply(
                MaterializationEvent::AdvancePartialManifest,
                MaterializationFacts::default()
            )
            .is_err());
        assert!(MaterializationPhase::Partial
            .apply(
                MaterializationEvent::FinishMaterialization,
                MaterializationFacts::default()
            )
            .is_err());
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R016.2
    #[test]
    fn no_writer_sets_phase_outside_apply() {
        for (file, source) in [
            ("registry.rs", include_str!("registry.rs")),
            ("registry/material.rs", include_str!("registry/material.rs")),
        ] {
            let test_start = source.find("#[cfg(test)]").unwrap_or(source.len());
            for (number, line) in source[..test_start].lines().enumerate() {
                let code = line.trim_start();
                assert!(
                    !(code.contains(".phase =") && !code.contains("==")),
                    "{file}:{} assigns the phase directly: {code}",
                    number + 1
                );
            }
        }
        // `valid` is derived from the phase and has no field.
        assert!(!include_str!("registry.rs").contains("pub valid: bool"));
    }
}
