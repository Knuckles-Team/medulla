//! The transition event ring (MEDULLA-OBSERVE-R004.4).
//!
//! Every accepted transition of a runtime machine appends one [`TransitionEvent`] to a ring of
//! `observe.event_ring_capacity` events and logs it at DEBUG with target `medulla::transition`.
//! A transition into `Failed`, `Stalled` or `Degraded` logs at ERROR or WARN. A refused
//! transition logs at WARN with the fields of the refusal and is kept in a second bounded ring.
//! The apply wrappers of the task registry, the readiness registry and the request registry
//! are the callers; they hold the one write path of each machine's state.
//!
//! An event is bounded: the subject is cut at [`MAX_SUBJECT_BYTES`] and the trace id at
//! [`MAX_TRACE_ID_BYTES`], so the ring stays at a fixed size per event and holds no payload.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use medulla_types::transition_refused::TransitionRefused;
use parking_lot::Mutex;

/// The ring capacity before the server sets `observe.event_ring_capacity`.
pub const DEFAULT_EVENT_RING_CAPACITY: usize = 4096;
/// The longest subject kept, in bytes.
pub const MAX_SUBJECT_BYTES: usize = 128;
/// The longest trace id kept, in bytes: a W3C `traceparent` is at most 55 characters.
pub const MAX_TRACE_ID_BYTES: usize = 55;

/// One accepted transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionEvent {
    /// The machine name, such as `TaskMachine`.
    pub machine: &'static str,
    /// What the machine instance belongs to: a task name, a graph name or a request id.
    pub subject: String,
    /// The stored text of the state before.
    pub from: &'static str,
    /// The event name.
    pub event: &'static str,
    /// The stored text of the state after.
    pub to: &'static str,
    /// Unix milliseconds of the transition.
    pub at_unix_ms: u64,
    /// The caller's trace id, when the transition ran under one.
    pub trace_id: Option<String>,
}

/// One refused transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefusedEvent {
    /// What the machine instance belongs to.
    pub subject: String,
    /// The refusal, with its machine, state, event and reason.
    pub refused: TransitionRefused,
    /// Unix milliseconds of the refusal.
    pub at_unix_ms: u64,
}

#[derive(Default)]
struct Rings {
    accepted: VecDeque<(u64, TransitionEvent)>,
    refused: VecDeque<RefusedEvent>,
}

/// A bounded ring of transition events. The oldest event leaves when the ring is full.
pub struct EventRing {
    capacity: std::sync::atomic::AtomicUsize,
    sequence: AtomicU64,
    refused_total: AtomicU64,
    rings: Mutex<Rings>,
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn cut(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

impl EventRing {
    /// An empty ring holding at most `capacity` events of each kind.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: std::sync::atomic::AtomicUsize::new(capacity.max(1)),
            sequence: AtomicU64::new(0),
            refused_total: AtomicU64::new(0),
            rings: Mutex::new(Rings::default()),
        }
    }

    /// Set the capacity: the server's `observe.event_ring_capacity`. A smaller capacity drops
    /// the oldest events at the next append.
    pub fn set_capacity(&self, capacity: usize) {
        self.capacity.store(capacity.max(1), Ordering::Relaxed);
    }

    /// Append an accepted transition and return its sequence number, which starts at 1 and
    /// never repeats. Logs at DEBUG, and at WARN or ERROR for a transition into a fault state.
    pub fn push(&self, mut event: TransitionEvent) -> u64 {
        event.subject = cut(&event.subject, MAX_SUBJECT_BYTES);
        event.trace_id = event.trace_id.map(|id| cut(&id, MAX_TRACE_ID_BYTES));
        log_accepted(&event);
        let capacity = self.capacity.load(Ordering::Relaxed);
        let mut rings = self.rings.lock();
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        rings.accepted.push_back((seq, event));
        while rings.accepted.len() > capacity {
            rings.accepted.pop_front();
        }
        seq
    }

    /// Record a refused transition: it logs at WARN with the fields of the refusal and is kept
    /// in the refusal ring.
    pub fn push_refused(&self, subject: &str, refused: &TransitionRefused) {
        tracing::warn!(
            target: "medulla::transition",
            subject = %cut(subject, MAX_SUBJECT_BYTES),
            machine = %refused.machine,
            from = %refused.from,
            event = %refused.event,
            reason = refused.reason.as_str(),
            "transition refused"
        );
        self.refused_total.fetch_add(1, Ordering::Relaxed);
        let capacity = self.capacity.load(Ordering::Relaxed);
        let mut rings = self.rings.lock();
        rings.refused.push_back(RefusedEvent {
            subject: cut(subject, MAX_SUBJECT_BYTES),
            refused: refused.clone(),
            at_unix_ms: unix_ms(),
        });
        while rings.refused.len() > capacity {
            rings.refused.pop_front();
        }
    }

    /// The accepted events still held, ascending by sequence number.
    pub fn snapshot(&self) -> Vec<(u64, TransitionEvent)> {
        self.rings.lock().accepted.iter().cloned().collect()
    }

    /// The last `count` accepted events, ascending by sequence number.
    pub fn last(&self, count: usize) -> Vec<(u64, TransitionEvent)> {
        let rings = self.rings.lock();
        let skip = rings.accepted.len().saturating_sub(count);
        rings.accepted.iter().skip(skip).cloned().collect()
    }

    /// The refused transitions still held, oldest first.
    pub fn refused(&self) -> Vec<RefusedEvent> {
        self.rings.lock().refused.iter().cloned().collect()
    }

    /// The sequence number of the last accepted event; 0 before the first.
    pub fn sequence(&self) -> u64 {
        self.sequence.load(Ordering::SeqCst)
    }

    /// How many transitions were refused since the ring was built.
    pub fn refused_total(&self) -> u64 {
        self.refused_total.load(Ordering::Relaxed)
    }

    /// The number of accepted events held.
    pub fn len(&self) -> usize {
        self.rings.lock().accepted.len()
    }

    /// Whether no accepted event is held.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn log_accepted(event: &TransitionEvent) {
    macro_rules! emit {
        ($level:ident) => {
            tracing::$level!(
                target: "medulla::transition",
                machine = event.machine,
                subject = %event.subject,
                from = event.from,
                event = event.event,
                to = event.to,
                trace_id = event.trace_id.as_deref().unwrap_or(""),
                "transition"
            )
        };
    }
    match event.to {
        "failed" => emit!(error),
        "stalled" | "degraded" => emit!(warn),
        _ => emit!(debug),
    }
}

/// Record an accepted transition on the process-wide ring. The apply wrappers call this with the
/// stored text of the states and the event name.
pub fn record_accepted(
    machine: &'static str,
    subject: &str,
    from: &'static str,
    event: &'static str,
    to: &'static str,
    trace_id: Option<&str>,
) {
    crate::events().push(TransitionEvent {
        machine,
        subject: subject.to_string(),
        from,
        event,
        to,
        at_unix_ms: unix_ms(),
        trace_id: trace_id.map(str::to_string),
    });
}

/// Record a refused transition on the process-wide ring.
pub fn record_refused(subject: &str, refused: &TransitionRefused) {
    crate::events().push_refused(subject, refused);
}
