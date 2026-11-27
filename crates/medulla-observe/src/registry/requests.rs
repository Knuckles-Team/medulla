//! The in-flight request registry (MEDULLA-OBSERVE-R006.2).
//!
//! `dispatch_with_context` calls [`RequestRegistry::enter`] for each request and keeps the
//! returned [`RequestGuard`] for the request's life; the guard removes the row on every exit
//! path, including a dropped future. The request's phase is a
//! [`RequestState`](crate::machine::request::RequestState): the guard and the
//! [`RequestHandle`] it hands out are the only writers, and every change goes through
//! `RequestState::apply`. The handle is also carried in a task-local ([`scope`]), so code
//! deeper in the dispatch path ([`advance`], the lock sites of R006.4) reaches the current
//! request without a parameter.
//!
//! A row carries no payload: the principal is a keyed hash of the caller's id, keyed per
//! process, so a row can be compared with another row and never reversed.

use std::collections::BTreeMap;
use std::future::Future;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use medulla_types::transition_refused::{MachineEvent, MachineState, TransitionRefused};
use parking_lot::Mutex;

use crate::machine::request::{RequestEvent, RequestState, REQUEST_MACHINE};

/// The in-flight bound before the server sets its own (`ServerState::max_in_flight`).
pub const DEFAULT_MAX_IN_FLIGHT: usize = 65_536;

/// One in-flight request, as `Introspect requests` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestRow {
    /// The registry key: ascending in the order requests entered.
    pub key: u64,
    /// The caller's request id.
    pub request_id: u64,
    /// The W3C trace id, when the request carried one.
    pub trace_id: Option<String>,
    /// The method name.
    pub method: &'static str,
    /// The target graph.
    pub graph: String,
    /// A keyed hash of the caller's principal.
    pub principal_ref: String,
    /// The `RequestMachine` phase.
    pub phase: RequestState,
    /// Milliseconds since the request entered.
    pub age_ms: u64,
    /// The lock the request waits for, while `phase` is `WaitingLock`.
    pub waiting_for: Option<String>,
}

/// What a request names when it enters.
pub struct EnterRequest<'a> {
    /// The caller's request id.
    pub request_id: u64,
    /// The W3C trace id, when the request carried one.
    pub trace_id: Option<&'a str>,
    /// The method name.
    pub method: &'static str,
    /// The target graph.
    pub graph: &'a str,
    /// The caller's principal; only its keyed hash is kept.
    pub principal: &'a str,
}

struct Entry {
    request_id: u64,
    trace_id: Option<String>,
    method: &'static str,
    graph: String,
    principal_ref: String,
    phase: RequestState,
    started: Instant,
    waiting_for: Option<String>,
}

struct Shared {
    entries: Mutex<BTreeMap<u64, Entry>>,
    next_key: AtomicU64,
    capacity: AtomicUsize,
    dropped: AtomicU64,
    sequence: AtomicU64,
    mac: std::collections::hash_map::RandomState,
}

/// The in-flight request registry. Cloning shares the registry.
#[derive(Clone)]
pub struct RequestRegistry {
    shared: Arc<Shared>,
}

impl Default for RequestRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_IN_FLIGHT)
    }
}

impl RequestRegistry {
    /// An empty registry holding at most `capacity` requests.
    pub fn new(capacity: usize) -> Self {
        Self {
            shared: Arc::new(Shared {
                entries: Mutex::new(BTreeMap::new()),
                next_key: AtomicU64::new(1),
                capacity: AtomicUsize::new(capacity.max(1)),
                dropped: AtomicU64::new(0),
                sequence: AtomicU64::new(0),
                mac: std::collections::hash_map::RandomState::new(),
            }),
        }
    }

    /// Set the bound: the server's `max_in_flight`.
    pub fn set_capacity(&self, capacity: usize) {
        self.shared
            .capacity
            .store(capacity.max(1), Ordering::Relaxed);
    }

    /// Register a request. The returned guard removes the row when dropped. A request that
    /// arrives with the registry full is not listed (`dropped_total` counts it) and still
    /// runs its machine.
    pub fn enter(&self, request: EnterRequest<'_>) -> RequestGuard {
        let mut hasher = self.shared.mac.build_hasher();
        hasher.write(request.principal.as_bytes());
        let principal_ref = format!("p:{:016x}", hasher.finish());
        let phase = Arc::new(Mutex::new(RequestState::initial()));
        let mut entries = self.shared.entries.lock();
        let key = if entries.len() < self.shared.capacity.load(Ordering::Relaxed) {
            let key = self.shared.next_key.fetch_add(1, Ordering::Relaxed);
            entries.insert(
                key,
                Entry {
                    request_id: request.request_id,
                    trace_id: request.trace_id.map(str::to_string),
                    method: request.method,
                    graph: request.graph.to_string(),
                    principal_ref,
                    phase: RequestState::initial(),
                    started: Instant::now(),
                    waiting_for: None,
                },
            );
            self.shared.sequence.fetch_add(1, Ordering::Relaxed);
            Some(key)
        } else {
            self.shared.dropped.fetch_add(1, Ordering::Relaxed);
            None
        };
        drop(entries);
        RequestGuard {
            handle: RequestHandle {
                shared: Arc::clone(&self.shared),
                key,
                request_id: request.request_id,
                trace_id: request.trace_id.map(str::to_string),
                phase,
            },
        }
    }

    /// Every in-flight request, ascending by key.
    pub fn snapshot(&self) -> Vec<RequestRow> {
        self.shared
            .entries
            .lock()
            .iter()
            .map(|(key, entry)| RequestRow {
                key: *key,
                request_id: entry.request_id,
                trace_id: entry.trace_id.clone(),
                method: entry.method,
                graph: entry.graph.clone(),
                principal_ref: entry.principal_ref.clone(),
                phase: entry.phase,
                age_ms: entry.started.elapsed().as_millis() as u64,
                waiting_for: entry.waiting_for.clone(),
            })
            .collect()
    }

    /// The number of in-flight requests listed.
    pub fn len(&self) -> usize {
        self.shared.entries.lock().len()
    }

    /// Whether no request is listed.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Requests that were not listed because the registry was full.
    pub fn dropped_total(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }

    /// The count of requests that entered or left: the sequence of a page.
    pub fn sequence(&self) -> u64 {
        self.shared.sequence.load(Ordering::Relaxed)
    }
}

/// The owner of one request's row. Dropping it removes the row.
pub struct RequestGuard {
    handle: RequestHandle,
}

impl RequestGuard {
    /// A cloneable handle to the request's phase.
    pub fn handle(&self) -> RequestHandle {
        self.handle.clone()
    }

    /// Apply one event to the request's machine.
    pub fn apply(&self, event: RequestEvent) -> Result<RequestState, TransitionRefused> {
        self.handle.apply(event)
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if let Some(key) = self.handle.key {
            if self.handle.shared.entries.lock().remove(&key).is_some() {
                self.handle.shared.sequence.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// A cloneable reference to one request's phase.
#[derive(Clone)]
pub struct RequestHandle {
    shared: Arc<Shared>,
    key: Option<u64>,
    request_id: u64,
    trace_id: Option<String>,
    phase: Arc<Mutex<RequestState>>,
}

impl RequestHandle {
    /// The caller's request id.
    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    /// The request's current phase.
    pub fn phase(&self) -> RequestState {
        *self.phase.lock()
    }

    /// Apply one event through `RequestState::apply`: the new phase, or `TRANSITION_REFUSED`
    /// with the phase unchanged.
    pub fn apply(&self, event: RequestEvent) -> Result<RequestState, TransitionRefused> {
        let mut phase = self.phase.lock();
        let from = *phase;
        let subject = self.request_id.to_string();
        let next = match from.apply(event) {
            Ok(next) => next,
            Err(refused) => {
                crate::events::record_refused(&subject, &refused);
                return Err(refused);
            }
        };
        crate::events::record_accepted(
            REQUEST_MACHINE.name,
            &subject,
            from.stored(),
            event.name(),
            next.stored(),
            self.trace_id.as_deref(),
        );
        *phase = next;
        if let Some(key) = self.key {
            if let Some(entry) = self.shared.entries.lock().get_mut(&key) {
                entry.phase = next;
            }
        }
        Ok(next)
    }

    /// The request begins to wait for the lock `lock`: `WaitLock` when it is `Handling`, and
    /// the lock named on its row in any phase.
    pub fn begin_wait(&self, lock: &str) {
        if self.phase() == RequestState::Handling {
            let _ = self.apply(RequestEvent::WaitLock);
        }
        self.set_waiting_for(Some(lock.to_string()));
    }

    /// The request got its lock: `Acquire` when it is `WaitingLock`, and the lock cleared.
    pub fn end_wait(&self) {
        if self.phase() == RequestState::WaitingLock {
            let _ = self.apply(RequestEvent::Acquire);
        }
        self.set_waiting_for(None);
    }

    fn set_waiting_for(&self, lock: Option<String>) {
        if let Some(key) = self.key {
            if let Some(entry) = self.shared.entries.lock().get_mut(&key) {
                entry.waiting_for = lock;
            }
        }
    }
}

tokio::task_local! {
    static CURRENT: RequestHandle;
}

/// Run `future` with `handle` as the current request.
pub fn scope<F: Future>(handle: RequestHandle, future: F) -> impl Future<Output = F::Output> {
    CURRENT.scope(handle, future)
}

/// The request this call runs in, or `None` outside a [`scope`].
pub fn current() -> Option<RequestHandle> {
    CURRENT.try_with(RequestHandle::clone).ok()
}

/// Apply `event` to the current request. A refused transition is logged at WARN and leaves the
/// phase unchanged; outside a request nothing happens.
pub fn advance(event: RequestEvent) {
    if let Some(handle) = current() {
        if let Err(refused) = handle.apply(event) {
            tracing::warn!(
                target: "medulla::request",
                request_id = handle.request_id(),
                code = refused.code().as_str(),
                "the request machine refused an event"
            );
        }
    }
}
