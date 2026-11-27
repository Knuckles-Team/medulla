//! Lock holders and waiters (MEDULLA-OBSERVE-R006.4).
//!
//! A named lock site reports who holds it, for how long, how many callers wait and the
//! longest wait seen. [`LockSite::acquire`] wraps the future that takes a lock: it counts the
//! waiter, records the wait when the lock arrives and returns a [`Held`] that names the holder
//! (the current request, else the current registered task) until it is dropped. A hold longer
//! than the warn threshold logs at WARN with the holder when it ends.
//! [`LockSite::wait`] is the same for a shared lock with no single holder: it counts waiters
//! and the longest wait only.
//!
//! Cost on an uncontended acquire: one registry lock, one clock read and one map update.

use std::collections::BTreeMap;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use crate::registry::requests;

/// The default `observe.lock_hold_warn_ms`.
pub const DEFAULT_LOCK_HOLD_WARN_MS: u64 = 5_000;

/// Idle sites kept beyond this many are forgotten, least recently used first.
const MAX_IDLE_SITES: usize = 1_024;

/// One lock site, as `Introspect locks` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockRow {
    /// The lock name.
    pub name: String,
    /// The holder: `request:<id>` or `task:<name>`; `None` while free or unattributed.
    pub holder: Option<String>,
    /// Milliseconds the current holder has held the lock; zero while free.
    pub hold_ms: u64,
    /// Callers waiting now.
    pub waiters: u32,
    /// The longest wait this site has seen, in milliseconds.
    pub longest_wait_ms: u64,
}

struct Holder {
    name: String,
    since: Instant,
}

#[derive(Default)]
struct SiteState {
    holder: Option<Holder>,
    waiters: u32,
    longest_wait_ms: u64,
    touched: u64,
}

struct Shared {
    sites: Mutex<BTreeMap<String, SiteState>>,
    warn_ms: AtomicU64,
    clock: AtomicU64,
}

/// The registry of lock sites. Cloning shares it.
#[derive(Clone)]
pub struct LockRegistry {
    shared: Arc<Shared>,
}

impl Default for LockRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl LockRegistry {
    /// An empty registry with the default hold-warn threshold.
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                sites: Mutex::new(BTreeMap::new()),
                warn_ms: AtomicU64::new(DEFAULT_LOCK_HOLD_WARN_MS),
                clock: AtomicU64::new(0),
            }),
        }
    }

    /// Set `observe.lock_hold_warn_ms`.
    pub fn set_hold_warn_ms(&self, ms: u64) {
        self.shared.warn_ms.store(ms, Ordering::Relaxed);
    }

    /// The site named `name`. Sites are created on first use.
    pub fn site(&self, name: impl Into<String>) -> LockSite {
        LockSite {
            shared: Arc::clone(&self.shared),
            name: name.into(),
        }
    }

    /// Every site, ascending by name.
    pub fn snapshot(&self) -> Vec<LockRow> {
        self.shared
            .sites
            .lock()
            .iter()
            .map(|(name, state)| LockRow {
                name: name.clone(),
                holder: state.holder.as_ref().map(|holder| holder.name.clone()),
                hold_ms: state
                    .holder
                    .as_ref()
                    .map_or(0, |holder| holder.since.elapsed().as_millis() as u64),
                waiters: state.waiters,
                longest_wait_ms: state.longest_wait_ms,
            })
            .collect()
    }
}

/// One named lock site.
pub struct LockSite {
    shared: Arc<Shared>,
    name: String,
}

fn holder_name() -> String {
    if let Some(request) = requests::current() {
        return format!("request:{}", request.request_id());
    }
    match crate::spawn::current_task_name() {
        Some(task) => format!("task:{task}"),
        None => "task:unattributed".to_string(),
    }
}

impl LockSite {
    fn begin_wait(&self) -> Instant {
        let mut sites = self.shared.sites.lock();
        let touched = self.shared.clock.fetch_add(1, Ordering::Relaxed);
        let state = sites.entry(self.name.clone()).or_default();
        state.waiters += 1;
        state.touched = touched;
        let started = Instant::now();
        if let Some(request) = requests::current() {
            request.begin_wait(&self.name);
        }
        started
    }

    fn end_wait(&self, started: Instant, hold: Option<Holder>) {
        let waited = started.elapsed().as_millis() as u64;
        let mut sites = self.shared.sites.lock();
        if let Some(state) = sites.get_mut(&self.name) {
            state.waiters = state.waiters.saturating_sub(1);
            state.longest_wait_ms = state.longest_wait_ms.max(waited);
            state.holder = hold;
        }
        if let Some(request) = requests::current() {
            request.end_wait();
        }
    }

    /// Take a lock through `lock`, the future that resolves when the lock is held.
    pub async fn acquire<T, F>(&self, lock: F) -> Held<T>
    where
        F: Future<Output = T>,
    {
        let started = self.begin_wait();
        // A waiter whose future is dropped must not stay counted.
        let waiting = WaitGuard {
            site: self,
            started,
            armed: true,
        };
        let inner = lock.await;
        let holder = Holder {
            name: holder_name(),
            since: Instant::now(),
        };
        let held_name = holder.name.clone();
        let since = holder.since;
        waiting.disarm();
        self.end_wait(started, Some(holder));
        self.trim_idle();
        Held {
            inner: Some(inner),
            shared: Arc::clone(&self.shared),
            name: self.name.clone(),
            holder: held_name,
            since,
        }
    }

    /// Wait for a shared lock that has no single holder: the waiter count and the longest wait
    /// are recorded, no holder is.
    pub async fn wait<T, F>(&self, lock: F) -> T
    where
        F: Future<Output = T>,
    {
        let started = self.begin_wait();
        let waiting = WaitGuard {
            site: self,
            started,
            armed: true,
        };
        let inner = lock.await;
        waiting.disarm();
        let mut sites = self.shared.sites.lock();
        if let Some(state) = sites.get_mut(&self.name) {
            state.waiters = state.waiters.saturating_sub(1);
            state.longest_wait_ms = state
                .longest_wait_ms
                .max(started.elapsed().as_millis() as u64);
        }
        drop(sites);
        if let Some(request) = requests::current() {
            request.end_wait();
        }
        inner
    }

    fn trim_idle(&self) {
        let mut sites = self.shared.sites.lock();
        if sites.len() <= MAX_IDLE_SITES {
            return;
        }
        let mut idle: Vec<(u64, String)> = sites
            .iter()
            .filter(|(_, state)| state.holder.is_none() && state.waiters == 0)
            .map(|(name, state)| (state.touched, name.clone()))
            .collect();
        idle.sort();
        let excess = sites.len() - MAX_IDLE_SITES;
        for (_, name) in idle.into_iter().take(excess) {
            sites.remove(&name);
        }
    }
}

struct WaitGuard<'a> {
    site: &'a LockSite,
    started: Instant,
    armed: bool,
}

impl WaitGuard<'_> {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for WaitGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.site.end_wait_cancelled(self.started);
        }
    }
}

impl LockSite {
    fn end_wait_cancelled(&self, started: Instant) {
        let waited = started.elapsed().as_millis() as u64;
        if let Some(state) = self.shared.sites.lock().get_mut(&self.name) {
            state.waiters = state.waiters.saturating_sub(1);
            state.longest_wait_ms = state.longest_wait_ms.max(waited);
        }
    }
}

/// A held lock. It dereferences to the lock's guard and clears the site's holder when dropped.
pub struct Held<T> {
    inner: Option<T>,
    shared: Arc<Shared>,
    name: String,
    holder: String,
    since: Instant,
}

impl<T> Deref for Held<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.inner
            .as_ref()
            .expect("a held lock owns its guard until it drops")
    }
}

impl<T> DerefMut for Held<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.inner
            .as_mut()
            .expect("a held lock owns its guard until it drops")
    }
}

impl<T> Drop for Held<T> {
    fn drop(&mut self) {
        // Release the lock first, then clear the holder: the next holder never meets a stale row.
        drop(self.inner.take());
        let held_ms = self.since.elapsed().as_millis() as u64;
        if let Some(state) = self.shared.sites.lock().get_mut(&self.name) {
            if state
                .holder
                .as_ref()
                .is_some_and(|holder| holder.name == self.holder && holder.since == self.since)
            {
                state.holder = None;
            }
        }
        let warn_ms = self.shared.warn_ms.load(Ordering::Relaxed);
        if held_ms >= warn_ms {
            tracing::warn!(
                target: "medulla::locks",
                lock = %self.name,
                holder = %self.holder,
                hold_ms = held_ms,
                "a lock was held past observe.lock_hold_warn_ms"
            );
        }
    }
}
