//! Stewardship rules (MEDULLA-TYPED-PACKS-R802.2).
//!
//! Pure functions over the records in `medulla_catalog_types::stewardship`: which
//! assignments govern a target, who may review a change to it, how a change
//! request moves, and when a certification lapses. Callers (the stewardship
//! handler, glossary, pack contracts, data products) import this module and
//! never the handler crate.

use medulla_types::change_request_machine::ChangeRequestState;
use medulla_types::change_request_machine::CHANGE_REQUEST_MACHINE;
use {
    medulla_catalog_source_types::stewardship::Certification,
    medulla_catalog_source_types::stewardship::ChangeRequest,
    medulla_catalog_source_types::stewardship::StewardRole,
    medulla_catalog_source_types::stewardship::StewardshipAssignment,
    medulla_catalog_source_types::stewardship::StewardshipFieldError,
    medulla_catalog_source_types::stewardship::StewardshipScope,
    medulla_catalog_source_types::stewardship::MAX_CHANGE_REQUEST_REVIEWERS,
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

impl TargetScopes {
    /// The scopes a wire target names.
    pub fn of_target(
        target: &medulla_catalog_source_types::stewardship::StewardshipTarget,
    ) -> Self {
        Self {
            asset: target.asset.clone(),
            concept_scheme: target.concept_scheme.clone(),
            domain: target.domain.clone(),
        }
    }
}

/// The owners and the stewards that govern `target`, each sorted and without repeats: the
/// roles a caller reads to decide whom to admit (MEDULLA-TYPED-PACKS-R802.3, R108.18).
pub fn governing(
    assignments: &[StewardshipAssignment],
    target: &TargetScopes,
) -> (Vec<String>, Vec<String>) {
    let pick = |role: StewardRole| {
        let mut names: Vec<String> = resolve(assignments, target)
            .into_iter()
            .filter(|a| a.role == role)
            .map(|a| a.principal.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    };
    (pick(StewardRole::Owner), pick(StewardRole::Steward))
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
/// then `approved` to `applied`. Everything else is refused. The move runs through
/// `ChangeRequestMachine` (MEDULLA-DATAPLAT-VOCAB-R009.2).
pub fn transition(
    from: ChangeRequestState,
    to: ChangeRequestState,
) -> Result<ChangeRequestState, StewardshipError> {
    CHANGE_REQUEST_MACHINE
        .edges
        .iter()
        .find(|edge| edge.from == from && edge.to == to)
        .and_then(|edge| from.apply(edge.event).ok())
        .ok_or(StewardshipError::IllegalTransition { from, to })
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

/// Why a review or an apply of a change request is refused (MEDULLA-TYPED-PACKS-R802.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewRefusal {
    /// The principal is not one of the request's resolved owners or stewards.
    RoleMissing,
    /// The principal opened the request it would approve.
    SelfApproval,
    /// The target's digest is no longer the one recorded when the request was opened.
    Stale { opened_at: String, now: String },
}

impl std::fmt::Display for ReviewRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RoleMissing => {
                f.write_str("the principal is not an owner or steward of the target")
            }
            Self::SelfApproval => f.write_str("the approver opened the change request"),
            Self::Stale { opened_at, now } => write!(
                f,
                "the target changed since the request was opened (digest {opened_at}, now {now})"
            ),
        }
    }
}

impl std::error::Error for ReviewRefusal {}

/// Whether `principal` may review or apply `request`, in the order role, self-approval,
/// staleness. `approving` is false for a rejection, which the opener may make of its own
/// request. `opened_digest` is the target digest recorded when the request was opened and
/// `current_digest` the target's digest now; a request whose target moved is stale.
pub fn authorize_review(
    request: &ChangeRequest,
    opened_by: &str,
    principal: &str,
    approving: bool,
    opened_digest: &str,
    current_digest: &str,
) -> Result<(), ReviewRefusal> {
    if !request
        .reviewers
        .iter()
        .any(|reviewer| reviewer == principal)
    {
        return Err(ReviewRefusal::RoleMissing);
    }
    if approving && opened_by == principal {
        return Err(ReviewRefusal::SelfApproval);
    }
    if opened_digest != current_digest {
        return Err(ReviewRefusal::Stale {
            opened_at: opened_digest.to_string(),
            now: current_digest.to_string(),
        });
    }
    Ok(())
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
