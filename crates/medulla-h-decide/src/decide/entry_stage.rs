//! The served entry admission stage (MEDULLA-DECISION-ENGINE-R132.3).
//!
//! A question may carry retrieved memory or knowledge entries in its param
//! `retrieved_entries`, each with a trust tier and the digest of its source.
//! The stage admits them once through `admit_entries` against the policy's
//! known source digests and turns each admitted entry, in `entry_id` order, into
//! one premise: `Observation` for a verified entry and `Claim` for an attested
//! or unverified one. A refused entry becomes no premise. A question without the
//! param is untouched.
//!
//! A required fact (the param `entry.required_facts`, `<subject>#<fact_key>`
//! items) that retrieved entries name but none of which is admitted is poisoned:
//! the outcome is replaced by a `POISONED_ENTRY` abstention
//! (MEDULLA-DECISION-ENGINE-R132.4).

use medulla_decision_outcome_types::decision::entry_trust::admit_entries;
use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::entry_types::{
    EntryCheck, RetrievedEntry, TrustTier, POISONED_ENTRY,
};
use medulla_decision_types::decision::statistical::{
    DecideRequest, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, PremiseClass, PremiseProvenance, PremiseRef,
};
use medulla_types::contract::BoundedVec;

use super::stat_executor::Executed;
use super::stat_support::invalid_parameter;

/// The request param naming the facts the question requires.
pub(super) const REQUIRED_FACTS_PARAM: &str = "entry.required_facts";

/// The required `(subject, fact_key)` pairs, in param order; none when absent.
fn required_facts(request: &DecideRequest) -> Result<Vec<(String, String)>, String> {
    let mut found = request
        .params
        .iter()
        .filter(|param| param.name == REQUIRED_FACTS_PARAM);
    let Some(param) = found.next() else {
        return Ok(Vec::new());
    };
    if found.next().is_some() {
        return Err(invalid_parameter(format!(
            "parameter {REQUIRED_FACTS_PARAM} is given more than once"
        )));
    }
    let TypedValue::IriList(items) = &param.value else {
        return Err(invalid_parameter(format!(
            "parameter {REQUIRED_FACTS_PARAM} has the wrong type"
        )));
    };
    items
        .iter()
        .map(|item| match item.rsplit_once('#') {
            Some((subject, key)) if !subject.is_empty() && !key.is_empty() => {
                Ok((subject.to_string(), key.to_string()))
            }
            _ => Err(invalid_parameter(format!(
                "{REQUIRED_FACTS_PARAM} item {item} is not <subject>#<fact_key>"
            ))),
        })
        .collect()
}

fn check_name(check: EntryCheck) -> &'static str {
    match check {
        EntryCheck::TierOrder => "tier_order",
        EntryCheck::PoisonCheck => "poison_check",
    }
}

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

/// The premises of the entries the policy admits, and an abstention in place of
/// the outcome when a required fact has entries but none is admitted.
pub(super) fn apply(
    request: &DecideRequest,
    policy: &DecisionPolicy,
    executed: Executed,
) -> Result<(Executed, Vec<PremiseRef>), String> {
    let required = required_facts(request)?;
    let Some(entries) = entries(request)? else {
        return Ok((executed, Vec::new()));
    };
    let admission = admit_entries(entries, &policy.entry_trust);
    let poisoned = required.iter().find_map(|(subject, key)| {
        let named = |entry: &&RetrievedEntry| entry.subject == *subject && entry.fact_key == *key;
        if !entries.iter().any(|entry| named(&entry)) {
            return None;
        }
        if admission.admitted.iter().any(|entry| named(&entry)) {
            return None;
        }
        admission.refused.iter().find(|refused| {
            entries
                .iter()
                .any(|entry| entry.entry_id == refused.entry_id && named(&entry))
        })
    });
    let premises = admission
        .admitted
        .iter()
        .map(|entry| PremiseRef {
            subject: entry.subject.clone(),
            fact: format!("{}={}", entry.fact_key, entry.value),
            class: match entry.trust.tier {
                TrustTier::Verified => PremiseClass::Observation,
                TrustTier::Attested | TrustTier::Unverified => PremiseClass::Claim,
            },
            provenance: PremiseProvenance::RetrievedEntry {
                entry_id: entry.entry_id.clone(),
                source_digest: entry.trust.source_digest.clone(),
            },
        })
        .collect();
    let Some(refused) = poisoned else {
        return Ok((executed, premises));
    };
    let detail = format!(
        "entry {} refused by {}",
        refused.entry_id,
        check_name(refused.check)
    );
    // A poisoned required fact answers nothing: the family's answer, route and
    // calibration go with the outcome they described.
    let replaced = Executed {
        matrix: executed.matrix,
        ladder: LadderResult {
            outcome: StatisticalOutcome::Abstained {
                reasons: BoundedVec::new(vec![AbstainReason::Family {
                    family: request.question.kind,
                    code: POISONED_ENTRY,
                    detail,
                }])
                .expect("one reason fits the bound"),
            },
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: executed.seed,
        harness_contrast: None,
        family: None,
        family_premises: Vec::new(),
        route: None,
    };
    Ok((replaced, premises))
}
