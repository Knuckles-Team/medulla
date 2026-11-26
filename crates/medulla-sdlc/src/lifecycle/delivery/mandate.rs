//! Mandate revalidation before a resume (MEDULLA-SDLC-R019.6).
//!
//! A person's event resumes an ended delivery only when four facts hold: the codebase is not
//! in mode `reference`, every scope row is still open, the actor still holds `source:write`,
//! and an inferred mandate also sees mode `evolving`. A policy version change never cancels a
//! live delivery; it adds a `policy-changed` input that names the changed fact.

use medulla_source_types::delivery::{
    Delivery, DeliveryInput, FeedbackKind, InputSource, MandateFacts, MandateRevoked,
    MandateStrength,
};
use medulla_source_types::lifecycle::ProjectMode;

/// Check the mandate of an ended delivery against `now`. The first failed fact refuses
/// `MANDATE_REVOKED` naming it; the delivery stays ended.
pub fn revalidate_mandate(
    delivery: &Delivery,
    now: &MandateFacts,
) -> Result<(), MandateRevoked> {
    if now.mode == ProjectMode::Reference {
        return Err(MandateRevoked("the codebase is in mode reference".to_owned()));
    }
    if !now.rows_open {
        return Err(MandateRevoked("a scope row is no longer open".to_owned()));
    }
    if !now.actor_can_write {
        return Err(MandateRevoked(format!(
            "{} no longer holds source:write",
            delivery.mandate.actor
        )));
    }
    if delivery.mandate.strength == MandateStrength::Inferred && now.mode != ProjectMode::Evolving {
        return Err(MandateRevoked(
            "an inferred mandate needs mode evolving".to_owned(),
        ));
    }
    Ok(())
}

/// The delivery after a policy version change: a live delivery keeps running and gains one
/// `policy-changed` input naming the old and new version. An ended delivery, or an unchanged
/// version, is returned as it is.
pub fn note_policy_change(delivery: &Delivery, now: &MandateFacts) -> Delivery {
    if delivery.ended.is_some() || delivery.mandate.policy_version == now.policy_version {
        return delivery.clone();
    }
    let mut next = delivery.clone();
    let seq = next.next_seq();
    next.inputs.push(DeliveryInput {
        event_id: format!("policy-changed:{seq}"),
        kind: FeedbackKind::PolicyChanged,
        source: InputSource::Automation,
        seq,
        detail: Some(format!(
            "policy_version {} -> {}",
            delivery.mandate.policy_version, now.policy_version
        )),
    });
    next
}
