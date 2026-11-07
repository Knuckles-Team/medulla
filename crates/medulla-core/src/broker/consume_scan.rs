//! Picking the next deliverable message for a consumer
//! (CONCEPT:MEDULLA-KG.compute.groups-qos-prefetch-honoring).
//!
//! One advisory pass over a queue's message nodes classifies every row for the calling
//! consumer — another consumer's live lease, this consumer's own in-flight message, a
//! TTL-expired row to dead-letter, a not-yet-due row, or a claimable one — and returns the
//! preferred candidate together with the counts and the expired ids the caller needs. The
//! pass is advisory only: the core revalidates the winner inside one topology transaction,
//! so nothing here can publish a stale delivery.

use super::message_machine::{Claimable as MachineClaimable, MessageFacts, MessageState};
use super::*;
use medulla_types::transition_refused::MachineState;

/// A claim candidate distilled from a scanned message node.
pub(super) struct Candidate {
    pub(super) id: String,
    priority: i64,
    seq: i64,
    pub(super) status: String,
    pub(super) lease_until: Option<u64>,
}

/// Total order for the claim pick (CONCEPT:MEDULLA-KG.compute.priority-queues): highest priority first, then
/// oldest seq (FIFO within a band), ties broken by id for determinism. Returns the
/// preferred of `a` (incumbent) and `b` (challenger).
fn prefer(a: Option<Candidate>, b: Candidate) -> Option<Candidate> {
    match a {
        None => Some(b),
        Some(cur) => {
            let b_wins = b.priority > cur.priority
                || (b.priority == cur.priority && b.seq < cur.seq)
                || (b.priority == cur.priority && b.seq == cur.seq && b.id < cur.id);
            if b_wins {
                Some(b)
            } else {
                Some(cur)
            }
        }
    }
}

/// One advisory pass over a queue's messages for a consumer.
pub(super) struct QueueScan {
    /// How many of the queue's messages this consumer already holds under a live lease.
    pub(super) inflight: u32,
    /// The preferred claim candidate: highest priority, then oldest seq, then lowest id.
    pub(super) best: Option<Candidate>,
    /// TTL-expired messages stepped over, to be dead-lettered before the next pass.
    pub(super) expired: Vec<String>,
}

/// Scan every message of `label`, classifying each for `consumer` at `now_ms`.
pub(super) fn scan_queue(core: &GraphCore, label: &str, consumer: &str, now_ms: u64) -> QueueScan {
    let mut scan = QueueScan {
        inflight: 0,
        best: None,
        expired: Vec::new(),
    };
    for (id, blob) in &core.get_nodes_by_label(label, 0) {
        let Ok(v) = decode_property(blob) else {
            continue;
        };
        let Some(obj) = v.as_object() else { continue };
        match claimability(obj, consumer, now_ms) {
            Claimability::Skip => {}
            Claimability::InFlight => scan.inflight += 1,
            Claimability::Expired => scan.expired.push(id.clone()),
            Claimability::Claimable(stored) => {
                scan.best = prefer(scan.best, scan_candidate(id, obj, stored));
            }
        }
    }
    scan
}

/// What one scanned queue message is to the consuming call.
enum Claimability {
    /// Not claimable: done/unknown status, another consumer's live lease, or not yet due
    /// (MEDULLA-279).
    Skip,
    /// Held by THIS consumer under a live lease — it counts against the prefetch ceiling.
    InFlight,
    /// TTL-expired (MEDULLA-277): never deliver it; dead-letter it lazily instead.
    Expired,
    /// Claimable now from the stored state the core revalidates.
    Claimable(MessageState),
}

/// The guard facts of one stored message.
pub(super) fn message_facts(obj: &serde_json::Map<String, serde_json::Value>) -> MessageFacts {
    MessageFacts {
        lease_until: f_u64(obj, "lease_until"),
        deliver_at: f_u64(obj, "deliver_at"),
        expires_at: f_u64(obj, "expires_at"),
        claimant_matches: false,
        attempts_below_max: false,
    }
}

/// Classify one queue message for `consumer` at `now_ms` with `MessageState::claimable`.
fn claimability(
    obj: &serde_json::Map<String, serde_json::Value>,
    consumer: &str,
    now_ms: u64,
) -> Claimability {
    let Some(state) = MessageState::from_stored(f_str(obj, "status")) else {
        return Claimability::Skip; // done / unknown → not claimable
    };
    match state.claimable(&message_facts(obj), now_ms) {
        MachineClaimable::No => Claimability::Skip,
        MachineClaimable::LiveLease if f_str(obj, "owner_consumer") == consumer => {
            Claimability::InFlight
        }
        MachineClaimable::LiveLease => Claimability::Skip,
        MachineClaimable::Expired => Claimability::Expired,
        MachineClaimable::Yes(stored) => Claimability::Claimable(stored),
    }
}

/// The scan candidate for a claimable message; it carries the stored state unchanged.
fn scan_candidate(
    id: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
    stored: MessageState,
) -> Candidate {
    Candidate {
        id: id.to_string(),
        priority: f_i64(obj, "priority", 0),
        seq: f_i64(obj, "seq", i64::MAX),
        status: stored.stored().into(),
        lease_until: f_u64(obj, "lease_until"),
    }
}

/// Dead-letter every TTL-expired message the scan stepped over (MEDULLA-277), id-sorted so DLQ
/// seq order is deterministic across replay.
pub(super) fn dead_letter_expired(
    core: &GraphCore,
    queue: &str,
    mut expired: Vec<String>,
    now_ms: u64,
) {
    expired.sort();
    for id in expired {
        if let Some(props) = core.get_node_properties(&id) {
            if let Ok(v) = decode_property(&props) {
                dead_letter(core, queue, &id, &v, "expired", now_ms);
            }
        }
    }
}
