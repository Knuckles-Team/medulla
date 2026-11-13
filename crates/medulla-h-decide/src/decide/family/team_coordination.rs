//! The `team_coordination` question family (MEDULLA-DECISION-ENGINE-R170.1).
//!
//! Team and channel questions over the A2A team vocabulary. The signal model is
//! `medulla_decision_types::decision::team_coordination`; the fallback body and
//! its premises are the rows that follow R170.1.

use super::DecisionFamily;

/// The family of the `team_coordination` question kind.
pub struct TeamCoordinationFamily;

impl DecisionFamily for TeamCoordinationFamily {}
