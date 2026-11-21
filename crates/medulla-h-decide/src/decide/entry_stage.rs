//! The served entry admission stage (MEDULLA-DECISION-ENGINE-R132.3).
//!
//! A question may carry retrieved memory or knowledge entries in its param
//! `retrieved_entries`, each with a trust tier and the digest of its source.
//! The stage admits them once through `admit_entries` against the policy's
//! known source digests and turns each admitted entry, in `entry_id` order, into
//! one premise: `Observation` for a verified entry and `Claim` for an attested
//! or unverified one. A refused entry becomes no premise. A question without the
//! param is untouched.

use medulla_decision_outcome_types::decision::entry_trust::admit_entries;
use medulla_decision_types::decision::entry_types::{RetrievedEntry, TrustTier};
use medulla_decision_types::decision::statistical::{DecideRequest, TypedValue};
use medulla_decision_types::decision::{
    DecisionPolicy, PremiseClass, PremiseProvenance, PremiseRef,
};

use super::stat_support::invalid_parameter;

/// The request param carrying the entries.
pub(super) const RETRIEVED_ENTRIES_PARAM: &str = "retrieved_entries";

/// The entries the request carries; none when the param is absent.
fn entries(request: &DecideRequest) -> Result<Option<&[RetrievedEntry]>, String> {
    let mut found = request
        .params
        .iter()
        .filter(|param| param.name == RETRIEVED_ENTRIES_PARAM);
    let Some(param) = found.next() else {
        return Ok(None);
    };
    if found.next().is_some() {
        return Err(invalid_parameter(format!(
            "parameter {RETRIEVED_ENTRIES_PARAM} is given more than once"
        )));
    }
    match &param.value {
        TypedValue::RetrievedEntries(entries) => Ok(Some(entries.as_slice())),
        _ => Err(invalid_parameter(format!(
            "parameter {RETRIEVED_ENTRIES_PARAM} has the wrong type"
        ))),
    }
}

/// The premises of the entries the policy admits.
pub(super) fn apply(
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Vec<PremiseRef>, String> {
    let Some(entries) = entries(request)? else {
        return Ok(Vec::new());
    };
    let admission = admit_entries(entries, &policy.entry_trust);
    Ok(admission
        .admitted
        .into_iter()
        .map(|entry| PremiseRef {
            subject: entry.subject,
            fact: format!("{}={}", entry.fact_key, entry.value),
            class: match entry.trust.tier {
                TrustTier::Verified => PremiseClass::Observation,
                TrustTier::Attested | TrustTier::Unverified => PremiseClass::Claim,
            },
            provenance: PremiseProvenance::RetrievedEntry {
                entry_id: entry.entry_id,
                source_digest: entry.trust.source_digest,
            },
        })
        .collect())
}
