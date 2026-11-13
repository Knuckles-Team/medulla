//! Knob tables of the memory, participant and profile families
//! (MEDULLA-DECISION-ENGINE-R179.8). Each module declares its own
//! `KNOBS` table; [`REGISTRY`] lists them for the knob registry.

pub mod memory;
pub mod participant;
pub mod placement;
pub mod profile;
pub mod task_plan;
pub mod topology;
pub mod trajectory;

use super::knobs::KnobSpec;

/// The family tables this crate registers.
pub const REGISTRY: &[&[KnobSpec]] = &[
    memory::KNOBS,
    participant::KNOBS,
    placement::KNOBS,
    profile::KNOBS,
    task_plan::KNOBS,
    topology::KNOBS,
    trajectory::KNOBS,
    super::ingestion_admission::KNOBS,
    super::statistical::run_credit::KNOBS,
    super::thought_search::KNOBS,
    super::capability_designation::KNOBS,
];
