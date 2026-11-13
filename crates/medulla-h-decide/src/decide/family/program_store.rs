//! The `program_lookup` question family (MEDULLA-DECISION-ENGINE-R135.1).
//!
//! Which stored compiled program answers one task signature. The model is
//! `medulla_decision_types::decision::program_store`. The family registers the
//! kind with the stage hooks the shared ladder gives every family; the lookup
//! body and its premises are the rows that follow R135.1.

use super::DecisionFamily;

/// The family of the `program_lookup` question kind.
pub struct ProgramLookupFamily;

impl DecisionFamily for ProgramLookupFamily {}
