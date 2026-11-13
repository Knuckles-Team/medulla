//! Knob tables of the memory, participant and profile families
//! (MEDULLA-DECISION-ENGINE-R179.8). Each module declares its own
//! `KNOBS` table; [`REGISTRY`] lists them for the knob registry.

pub mod context;
pub mod memory;
pub mod participant;
pub mod placement;
pub mod profile;
pub mod task_plan;
pub mod resolve_entity;
pub mod round_verdict;
pub mod route;
pub mod schema_mapping;
pub mod topology;
pub mod trajectory;

use super::knobs::KnobSpec;

/// The family tables this crate registers.
pub const REGISTRY: &[&[KnobSpec]] = &[
    context::KNOBS,
    memory::KNOBS,
    participant::KNOBS,
    placement::KNOBS,
    profile::KNOBS,
    task_plan::KNOBS,
    resolve_entity::KNOBS,
    round_verdict::KNOBS,
    route::KNOBS,
    schema_mapping::KNOBS,
    topology::KNOBS,
    trajectory::KNOBS,
    super::ingestion_admission::KNOBS,
    super::statistical::run_credit::KNOBS,
    super::thought_search::KNOBS,
    super::capability_designation::KNOBS,
    super::statistical::distilled::KNOBS,
    super::delegation_quality::KNOBS,
    super::field_shielding::KNOBS,
    super::paper_admission::KNOBS,
];
