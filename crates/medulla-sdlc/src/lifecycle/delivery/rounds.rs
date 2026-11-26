//! Automated review rounds have a budget (MEDULLA-SDLC-R019.9).
//!
//! One round is one distinct automated review run, keyed by `run_key`. The rounds of one
//! proposal and reviewer are counted across every assignment of the delivery. The verdict is
//! `address` while the count is at or below the knob `sdlc.delivery.max_automated_review_rounds`
//! (3 by default) and `stop_addressing` above it. A person's review has no round limit.

use std::collections::BTreeSet;

use medulla_source_types::delivery::{
    Delivery, DeliveryInput, FeedbackKind, InputSource, ReviewRecord, ReviewerKind, RoundVerdict,
};

/// The verdict for the automated review `current` of `reviews`, the stored `:Review` records
/// (the current review included or not: its run key is counted once either way).
pub fn review_round_verdict(
    reviews: &[ReviewRecord],
    current: &ReviewRecord,
    max_rounds: u64,
) -> RoundVerdict {
    if current.reviewer_kind == ReviewerKind::Person {
        return RoundVerdict::Address;
    }
    let mut rounds: BTreeSet<&str> = reviews
        .iter()
        .filter(|r| {
            r.reviewer_kind == ReviewerKind::Automated
                && r.proposal_id == current.proposal_id
                && r.reviewer == current.reviewer
        })
        .map(|r| r.run_key.as_str())
        .collect();
    rounds.insert(current.run_key.as_str());
    if rounds.len() as u64 <= max_rounds {
        RoundVerdict::Address
    } else {
        RoundVerdict::StopAddressing
    }
}

/// Write the verdict as an input of the delivery: the next sequence number, the review event
/// id and the verdict named in `detail`.
pub fn record_round_verdict(
    delivery: &Delivery,
    event_id: &str,
    verdict: RoundVerdict,
) -> Delivery {
    let mut next = delivery.clone();
    let seq = next.next_seq();
    next.inputs.push(DeliveryInput {
        event_id: event_id.to_owned(),
        kind: FeedbackKind::ReviewSubmitted,
        source: InputSource::Automation,
        seq,
        detail: Some(
            match verdict {
                RoundVerdict::Address => "round_verdict:address",
                RoundVerdict::StopAddressing => "round_verdict:stop_addressing",
            }
            .to_owned(),
        ),
    });
    next
}
