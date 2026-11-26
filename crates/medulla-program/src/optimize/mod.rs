//! Deterministic candidate selection kernels for recursive-run policies.

pub mod front;

pub use front::{
    reflective_front, FrontError, FrontSummary, ParetoFront, PolicyCandidate, ScoreboardRow,
    DEFAULT_FRONT_MAX_SIZE,
};
