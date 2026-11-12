//! Stewardship rules (MEDULLA-TYPED-PACKS-R802.2).
//!
//! Pure functions over the records in `medulla_types::stewardship`: which
//! assignments govern a target, who may review a change to it, how a change
//! request moves, and when a certification lapses. Callers (the stewardship
//! handler, glossary, pack contracts, data products) import this module and
//! never the handler crate.

use medulla_types::stewardship::{
    Certification, ChangeRequest, ChangeRequestState, StewardRole, StewardshipAssignment,
    StewardshipFieldError, StewardshipScope, MAX_CHANGE_REQUEST_REVIEWERS,
};

/// `knob stewardship.certification_ttl_days` default.
pub const CERTIFICATION_TTL_DAYS_DEFAULT: u32 = 365;
/// `knob stewardship.certification_ttl_days` lower bound, days.
pub const CERTIFICATION_TTL_DAYS_MIN: u32 = 30;
/// `knob stewardship.certification_ttl_days` upper bound, days.
pub const CERTIFICATION_TTL_DAYS_MAX: u32 = 1095;

const SECS_PER_DAY: u64 = 24 * 60 * 60;

/// The scopes a target belongs to, most specific first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetScopes {
    pub asset: Option<String>,
    pub concept_scheme: Option<String>,
    pub domain: Option<String>,
}

/// Why a stewardship operation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StewardshipError {
    /// The move is not on the change-request state machine.
    IllegalTransition {
        from: ChangeRequestState,
        to: ChangeRequestState,
    },
    /// No owner or steward governs the target, so nobody could review.
    NoReviewers,
    /// The certification lifetime is outside the knob range.
    TtlOutOfRange(u32),
    /// A record field failed validation.
    Field(StewardshipFieldError),
}

impl std::fmt::Display for StewardshipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IllegalTransition { from, to } => {
                write!(f, "change request cannot move from {from:?} to {to:?}")
            }
            Self::NoReviewers => f.write_str("no owner or steward governs the target"),
            Self::TtlOutOfRange(days) => write!(
                f,
                "certification ttl {days} days is outside \
                 [{CERTIFICATION_TTL_DAYS_MIN}, {CERTIFICATION_TTL_DAYS_MAX}]"
            ),
            Self::Field(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StewardshipError {}

impl From<StewardshipFieldError> for StewardshipError {
    fn from(error: StewardshipFieldError) -> Self {
        Self::Field(error)
    }
}

fn level_matches(scope: &StewardshipScope, level: usize, target: &TargetScopes) -> bool {
    match (level, scope) {
        (0, StewardshipScope::Asset(id)) => target.asset.as_deref() == Some(id),
        (1, StewardshipScope::ConceptScheme(id)) => target.concept_scheme.as_deref() == Some(id),
        (2, StewardshipScope::Domain(id)) => target.domain.as_deref() == Some(id),
        _ => false,
    }
}

/// The assignments that govern `target`: those at the most specific level that
/// has any (asset, then concept scheme, then domain). Order is preserved.
pub fn resolve<'a>(
    assignments: &'a [StewardshipAssignment],
    target: &TargetScopes,
) -> Vec<&'a StewardshipAssignment> {
    (0..3)
        .map(|level| {
            assignments
                .iter()
                .filter(|a| level_matches(&a.scope, level, target))
                .collect::<Vec<_>>()
        })
        .find(|found| !found.is_empty())
        .unwrap_or_default()
}

/// Principals who may review a change to `target`: its resolved owners and
/// stewards, sorted and without repeats, capped at the reviewer bound.
pub fn reviewers(assignments: &[StewardshipAssignment], target: &TargetScopes) -> Vec<String> {
    let mut names: Vec<String> = resolve(assignments, target)
        .into_iter()
        .filter(|a| matches!(a.role, StewardRole::Owner | StewardRole::Steward))
        .map(|a| a.principal.clone())
        .collect();
    names.sort();
    names.dedup();
    names.truncate(MAX_CHANGE_REQUEST_REVIEWERS);
    names
}

/// The next state of a change request: `open` to `approved` or `rejected`,
/// then `approved` to `applied`. Everything else is refused.
pub fn transition(
    from: ChangeRequestState,
    to: ChangeRequestState,
) -> Result<ChangeRequestState, StewardshipError> {
    use ChangeRequestState::{Applied, Approved, Open, Rejected};
    match (from, to) {
        (Open, Approved) | (Open, Rejected) | (Approved, Applied) => Ok(to),
        _ => Err(StewardshipError::IllegalTransition { from, to }),
    }
}

/// Open a change request for `target`, naming its governing reviewers.
pub fn open_change_request(
    target: &str,
    proposed_digest: &str,
    assignments: &[StewardshipAssignment],
    scopes: &TargetScopes,
) -> Result<ChangeRequest, StewardshipError> {
    let reviewers = reviewers(assignments, scopes);
    if reviewers.is_empty() {
        return Err(StewardshipError::NoReviewers);
    }
    let request = ChangeRequest {
        target: target.to_string(),
        proposed_digest: proposed_digest.to_string(),
        state: ChangeRequestState::Open,
        reviewers,
    };
    request.validate()?;
    Ok(request)
}

/// Certify `asset` at `now` (unix seconds); it lapses after `ttl_days`.
pub fn certify(
    asset: &str,
    level: &str,
    by: &str,
    now: u64,
    ttl_days: u32,
) -> Result<Certification, StewardshipError> {
    if !(CERTIFICATION_TTL_DAYS_MIN..=CERTIFICATION_TTL_DAYS_MAX).contains(&ttl_days) {
        return Err(StewardshipError::TtlOutOfRange(ttl_days));
    }
    let certification = Certification {
        asset: asset.to_string(),
        level: level.to_string(),
        by: by.to_string(),
        expires_at: now.saturating_add(u64::from(ttl_days) * SECS_PER_DAY),
    };
    certification.validate()?;
    Ok(certification)
}
