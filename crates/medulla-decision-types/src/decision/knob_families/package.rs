//! The `package.*` knobs (MEDULLA-DECISION-ENGINE-R160.2): how many recorded runs, and what
//! share of them must pass, before `PublishSkillPackage` publishes a rendered package. Both
//! are `Fixed`: no tenant policy moves them, so the validation reads the spec defaults.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// The default of `package.min_runs`.
pub const DEFAULT_MIN_RUNS: u64 = 1;
/// The default of `package.min_pass_rate`, in micro-share (a whole share is 1,000,000).
pub const DEFAULT_MIN_PASS_RATE_PPM: u64 = 1_000_000;

/// Fewest recorded runs a package needs before it can publish.
pub const MIN_RUNS: KnobSpec = KnobSpec {
    name: "package.min_runs",
    unit: "runs",
    default: KnobValue::Integer(DEFAULT_MIN_RUNS as i64),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(1000),
    safety: KnobSafety::Fixed,
};

/// Smallest share of recorded runs that must pass, in micro-share.
pub const MIN_PASS_RATE: KnobSpec = KnobSpec {
    name: "package.min_pass_rate",
    unit: "micro-share",
    default: KnobValue::Integer(DEFAULT_MIN_PASS_RATE_PPM as i64),
    min: KnobValue::Integer(0),
    max: KnobValue::Integer(1_000_000),
    safety: KnobSafety::Fixed,
};

/// Every `package.*` knob, each `Fixed`.
pub const KNOBS: &[KnobSpec] = &[MIN_RUNS, MIN_PASS_RATE];

/// The values package validation reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageKnobs {
    /// `knob package.min_runs`, runs.
    pub min_runs: u64,
    /// `knob package.min_pass_rate`, micro-share.
    pub min_pass_rate_ppm: u64,
}

/// The package knob values: the spec defaults, because both knobs are `Fixed`.
pub fn knob_values() -> PackageKnobs {
    PackageKnobs {
        min_runs: DEFAULT_MIN_RUNS,
        min_pass_rate_ppm: DEFAULT_MIN_PASS_RATE_PPM,
    }
}
