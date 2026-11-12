//! Knob tables of the memory, participant and profile families
//! (MEDULLA-DECISION-ENGINE-R179.8). Each module declares its own
//! `KNOBS` table; [`REGISTRY`] lists them for the knob registry.

pub mod memory;
pub mod participant;
pub mod profile;

use super::knobs::KnobSpec;

/// The family tables this crate registers.
pub const REGISTRY: &[&[KnobSpec]] = &[
    memory::KNOBS,
    participant::KNOBS,
    profile::KNOBS,
    super::ingestion_admission::KNOBS,
];
