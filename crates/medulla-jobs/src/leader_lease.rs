//! The executor leader lease: one record in the tenant graph, one explicit state machine
//! (MEDULLA-PLACEMENT-R751.761).
//!
//! The lease machine has the closed states `Free`, `Held` and `Expired` and the closed events
//! `Acquire`, `Renew`, `Expire` and `Release`. [`LeaseRecord::apply`] is the only writer of the
//! state; an illegal transition is refused with the typed [`TransitionRefused`] naming the
//! machine, the current state and the event. Two engines contend on one shared store by
//! compare-and-set on the record's `revision` ([`LeaseCell`]); the loser re-reads and sees the
//! winner. [`DaemonLeadership::authorize`](crate::schedule::DaemonLeadership::authorize) reads the
//! record through [`LeaseRecord::leadership`].

use std::sync::Mutex;

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};

use crate::schedule::DaemonLeadership;

/// Graph node id of the lease record in a tenant graph.
pub const LEASE_NODE_ID: &str = "leader_lease:schedules";
/// Node type of the lease record.
pub const LEASE_NODE_TYPE: &str = "LeaderLease";
/// The daemon name the schedule tick runs as.
pub const SCHEDULE_DAEMON: &str = "schedule-tick";

/// The closed state set of the lease machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseState {
    Free,
    Held,
    Expired,
}

impl MachineState for LeaseState {
    fn stored(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Held => "held",
            Self::Expired => "expired",
        }
    }
}

/// The closed event set of the lease machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseEvent {
    Acquire,
    Renew,
    Expire,
    Release,
}

impl MachineEvent for LeaseEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Acquire => "acquire",
            Self::Renew => "renew",
            Self::Expire => "expire",
            Self::Release => "release",
        }
    }
}

/// The named guards of the lease machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseGuard {
    /// The acting host holds the lease and it has not lapsed.
    HolderLive,
    /// The acting host holds the lease.
    HolderMatches,
    /// The lease time has passed.
    Lapsed,
}

/// The lease machine: `Free` start, no terminal state (a lease is reused).
pub const LEASE_MACHINE: MachineDef<LeaseState, LeaseEvent, LeaseGuard> = MachineDef {
    name: "LeaderLeaseMachine",
    states: &[LeaseState::Free, LeaseState::Held, LeaseState::Expired],
    initial: LeaseState::Free,
    terminals: &[],
    events: &[
        LeaseEvent::Acquire,
        LeaseEvent::Renew,
        LeaseEvent::Expire,
        LeaseEvent::Release,
    ],
    guards: &[
        LeaseGuard::HolderLive,
        LeaseGuard::HolderMatches,
        LeaseGuard::Lapsed,
    ],
    edges: &[
        Edge {
            id: "K01",
            from: LeaseState::Free,
            event: LeaseEvent::Acquire,
            guard: None,
            to: LeaseState::Held,
        },
        Edge {
            id: "K02",
            from: LeaseState::Held,
            event: LeaseEvent::Renew,
            guard: Some(LeaseGuard::HolderLive),
            to: LeaseState::Held,
        },
        Edge {
            id: "K03",
            from: LeaseState::Held,
            event: LeaseEvent::Expire,
            guard: Some(LeaseGuard::Lapsed),
            to: LeaseState::Expired,
        },
        Edge {
            id: "K04",
            from: LeaseState::Held,
            event: LeaseEvent::Release,
            guard: Some(LeaseGuard::HolderMatches),
            to: LeaseState::Free,
        },
        Edge {
            id: "K05",
            from: LeaseState::Expired,
            event: LeaseEvent::Acquire,
            guard: None,
            to: LeaseState::Held,
        },
    ],
};

/// The stored lease record. `revision` is the compare-and-set token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseRecord {
    pub state: LeaseState,
    pub holder: Option<String>,
    pub fence: u64,
    pub revision: u64,
    pub expires_ms: u64,
}

impl Default for LeaseRecord {
    fn default() -> Self {
        Self {
            state: LEASE_MACHINE.initial,
            holder: None,
            fence: 0,
            revision: 0,
            expires_ms: 0,
        }
    }
}

impl LeaseRecord {
    /// The one transition function: the next record (revision advanced by one), or the typed
    /// refusal. Nothing else writes `state`.
    pub fn apply(
        &self,
        event: LeaseEvent,
        host: &str,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<Self, TransitionRefused> {
        let is_holder = self.holder.as_deref() == Some(host);
        let lapsed = now_ms >= self.expires_ms;
        let edge = transition(&LEASE_MACHINE, self.state, event, |guard| match guard {
            LeaseGuard::HolderLive => is_holder && !lapsed,
            LeaseGuard::HolderMatches => is_holder,
            LeaseGuard::Lapsed => lapsed,
        })?;
        let mut next = self.clone();
        next.state = edge.to;
        next.revision = self.revision + 1;
        match event {
            LeaseEvent::Acquire => {
                next.fence = self.fence + 1;
                next.holder = Some(host.to_string());
                next.expires_ms = now_ms + ttl_ms;
            }
            LeaseEvent::Renew => next.expires_ms = now_ms + ttl_ms,
            LeaseEvent::Expire => {}
            LeaseEvent::Release => {
                next.holder = None;
                next.expires_ms = 0;
            }
        }
        Ok(next)
    }

    /// The fence view `DaemonLeadership::authorize` reads: a lease that is not `Held` has no
    /// holder.
    pub fn leadership(&self) -> DaemonLeadership {
        DaemonLeadership {
            daemon: SCHEDULE_DAEMON.to_string(),
            holder: match self.state {
                LeaseState::Held => self.holder.clone(),
                _ => None,
            },
            fence: self.fence,
            expires_ms: self.expires_ms,
        }
    }
}

/// The shared store of one lease record, updated by compare-and-set on `revision`.
pub trait LeaseCell {
    /// The current record (the initial `Free` record when none is stored).
    fn read(&self) -> LeaseRecord;
    /// Store `next` only when the stored revision is still `expected_revision`.
    fn compare_and_set(&self, expected_revision: u64, next: &LeaseRecord) -> bool;
}

/// An in-memory cell, for tests and single-process use.
#[derive(Default)]
pub struct MemoryLeaseCell(Mutex<LeaseRecord>);

impl LeaseCell for MemoryLeaseCell {
    fn read(&self) -> LeaseRecord {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn compare_and_set(&self, expected_revision: u64, next: &LeaseRecord) -> bool {
        let mut stored = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if stored.revision != expected_revision {
            return false;
        }
        *stored = next.clone();
        true
    }
}

/// Try to become (or stay) the leader. Returns the fence when `host` holds the lease after the
/// call, `None` when another live holder has it. Every step is one compare-and-set; a lost race
/// re-reads and decides again.
pub fn contend(
    cell: &dyn LeaseCell,
    host: &str,
    now_ms: u64,
    ttl_ms: u64,
) -> Result<Option<u64>, TransitionRefused> {
    for _ in 0..4 {
        let record = cell.read();
        let lapsed = now_ms >= record.expires_ms;
        let holder = record.holder.as_deref() == Some(host);
        let event = match record.state {
            LeaseState::Free | LeaseState::Expired => LeaseEvent::Acquire,
            LeaseState::Held if holder && !lapsed => LeaseEvent::Renew,
            LeaseState::Held if lapsed => LeaseEvent::Expire,
            LeaseState::Held => return Ok(None),
        };
        let next = record.apply(event, host, now_ms, ttl_ms)?;
        if cell.compare_and_set(record.revision, &next) && next.state == LeaseState::Held {
            return Ok(Some(next.fence));
        }
    }
    Ok(None)
}

impl LeaseRecord {
    /// The stored node fields (the compare-and-set update).
    pub fn to_fields(&self) -> serde_json::Map<String, serde_json::Value> {
        use serde_json::Value;
        let mut fields = serde_json::Map::new();
        fields.insert("type".into(), LEASE_NODE_TYPE.into());
        fields.insert("state".into(), self.state.stored().into());
        fields.insert(
            "holder".into(),
            self.holder.clone().map_or(Value::Null, Value::from),
        );
        fields.insert("fence".into(), self.fence.into());
        fields.insert("revision".into(), self.revision.into());
        fields.insert("expires_ms".into(), self.expires_ms.into());
        fields
    }

    /// Read a stored record; `None` when a field is missing or the state is not in the machine.
    pub fn from_fields(fields: &serde_json::Value) -> Option<Self> {
        Some(Self {
            state: LEASE_MACHINE.state_from_stored(fields.get("state")?.as_str()?)?,
            holder: fields.get("holder")?.as_str().map(str::to_string),
            fence: fields.get("fence")?.as_u64()?,
            revision: fields.get("revision")?.as_u64()?,
            expires_ms: fields.get("expires_ms")?.as_u64()?,
        })
    }
}
