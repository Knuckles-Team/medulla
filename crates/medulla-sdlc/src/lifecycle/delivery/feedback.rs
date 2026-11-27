//! Feedback reaches the delivery that follows the proposal (MEDULLA-SDLC-R019.5).
//!
//! [`route_feedback`] is a pure decision: it reads one source-control event and the
//! deliveries of the tenant and returns the deliveries to write, whether an unrouted failed
//! check files a defect, and whether the event is only recorded. The served method stores the
//! `:FeedbackEvent`, applies the returned deliveries in one batch and files the defect through
//! the one defect book (MEDULLA-SDLC-R003.3).

use std::collections::BTreeSet;

use medulla_source_types::delivery::{
    Delivery, DeliveryInput, DeliveryOutcome, FeedbackRefusal, InputSource, ScmFeedbackEvent,
};

/// What the engine already knows about the world one event lands in.
pub struct FeedbackContext<'a> {
    /// The connectors registered to provide feedback events.
    pub registered_connectors: &'a BTreeSet<String>,
    /// Whether the engine holds the proposal the event names.
    pub proposal_known: bool,
    /// Whether an event with this provider event id is already stored.
    pub already_stored: bool,
    /// The ids of the ended deliveries whose mandate revalidation (R019.6) passed, so a
    /// person's event may resume them.
    pub revalidated: &'a BTreeSet<String>,
}

/// The decision for one event.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FeedbackRouting {
    /// The deliveries to write: each received the event as an input.
    pub deliveries: Vec<Delivery>,
    /// The ids of the ended deliveries a person's event resumed.
    pub resumed: Vec<String>,
    /// An unrouted failed check files a defect (R003.3).
    pub file_defect: bool,
    /// The event was stored before; nothing is written again.
    pub duplicate: bool,
}

fn follows(delivery: &Delivery, proposal: &str) -> bool {
    delivery.proposals.iter().any(|p| p.id == proposal)
        || delivery.follows.iter().any(|id| id == proposal)
}

/// Route `event` to the deliveries that follow its proposal.
///
/// A live delivery that follows the proposal receives the event as an input with the next
/// sequence number. An ended delivery receives it only from a person and only when its
/// mandate revalidated; a delivery a person stopped receives nothing. An event no delivery
/// takes is recorded only, except a failed check, which files a defect.
pub fn route_feedback(
    event: &ScmFeedbackEvent,
    deliveries: &[Delivery],
    context: &FeedbackContext<'_>,
) -> Result<FeedbackRouting, FeedbackRefusal> {
    let blank = |value: &str| value.trim().is_empty();
    if blank(&event.event_id) || blank(&event.proposal_id) || blank(&event.connector) {
        return Err(FeedbackRefusal::EventInvalid(
            "an event names its id, connector and proposal".to_owned(),
        ));
    }
    if !context.registered_connectors.contains(&event.connector) {
        return Err(FeedbackRefusal::SourceUnknown(event.connector.clone()));
    }
    if !context.proposal_known {
        return Err(FeedbackRefusal::ProposalUnknown(event.proposal_id.clone()));
    }
    let mut routing = FeedbackRouting::default();
    if context.already_stored {
        routing.duplicate = true;
        return Ok(routing);
    }
    for delivery in deliveries.iter().filter(|d| follows(d, &event.proposal_id)) {
        let resumes = match delivery.ended {
            None => false,
            Some(DeliveryOutcome::Stopped) => continue,
            Some(_) => {
                if event.source != InputSource::Person
                    || !context.revalidated.contains(&delivery.id)
                {
                    continue;
                }
                true
            }
        };
        let mut next = delivery.clone();
        let seq = next.next_seq();
        next.inputs.push(DeliveryInput {
            event_id: event.event_id.clone(),
            kind: event.kind,
            source: event.source,
            seq,
            detail: None,
        });
        if resumes {
            next.ended = None;
            routing.resumed.push(next.id.clone());
        }
        routing.deliveries.push(next);
    }
    routing.file_defect = routing.deliveries.is_empty() && event.failed;
    Ok(routing)
}
