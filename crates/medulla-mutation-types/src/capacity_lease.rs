//! GOC-21 — distributed `CapacityCell`/`CapacityLease` wire contract and the pure
//! fencing/admission algorithm the engine dispatch layer enforces.
//!
//! Reuses GOC-03's fencing concept (`crates/medulla-types/src/commit_descriptor.rs`,
//! branch `goc/goc-03-commit-currency`, unmerged as of this writing — see this
//! module's own doc note below) rather than inventing a second one:
//! `CommitDescriptor` pairs a monotonic `authority_epoch` (the authority's own
//! leadership/term counter) with a random non-zero `fencing_token` minted at
//! commit time, and a projection/reader must reject anything carrying a token
//! that does not match the current authority's record. `CapacityLease` mirrors
//! that exact shape for capacity ownership instead of commit ownership:
//! `lease_epoch` is the cell authority's monotonic epoch (bumped on
//! failover/repartition — a stale scheduler on an old epoch cannot acquire),
//! and `fence_token` is a non-zero, monotonically-increasing-per-cell value
//! minted at acquire time that every renew/release/commit call must present
//! unchanged; a caller presenting a stale `lease_epoch` OR a `fence_token` that
//! does not match the ledger's live record for that `lease_id` is rejected
//! (`CapacityDenial::StaleFence` / `StaleEpoch`), never silently upgraded.
//!
//! ## Premise note (verified against `medulla` `main` at
//! `02594f7`, 2026-08-16)
//!
//! `CommitDescriptor`/`authority_epoch`/`fencing_token` exist ONLY on the
//! unmerged branch `goc/goc-03-commit-currency`
//! (`crates/medulla-types/src/commit_descriptor.rs`); they are not present on `main`
//! and this module's branch is cut from `main`, so `CapacityLease` cannot
//! literally import `CommitDescriptor` without taking an undeclared
//! cross-lane dependency on GOC-03 landing first (GOC-21's declared hard deps
//! are GOC-18/19/20, not GOC-03). This module therefore VENDORS the identical
//! epoch+fence shape and CAS discipline as its own fields
//! (`lease_epoch`/`fence_token`) rather than a type import, and is written so
//! that swapping to a shared `commit_descriptor::Fencing` type (if GOC-03 lands
//! first and a follow-up lane chooses to unify them) is a mechanical rename,
//! not a redesign. Reported as an open dependency-ordering item, not silently
//! worked around.
//!
//! ## Scope
//!
//! Pure data + pure algorithm, no I/O — this crate is the bottom of the engine
//! DAG (see `lib.rs`). The authoritative server-side dispatch/mutation/
//! state-machine wiring (durable persistence of `CapacityCell`/`CapacityLease`
//! rows, fence-token minting, and RPC handlers) lives in
//! `src/redb_store/capacity_lease.rs`; what IS here is the reusable
//! admission/fairness/expiry algorithm those handlers must call so there is
//! exactly one policy implementation to audit, plus the unit tests that are
//! this lane's "known-bad proof": an expired or wrong-fenced holder is
//! rejected, and a background flood cannot push a reserved-floor request's
//! wait past this module's declared bound (see `try_acquire`'s doc for the
//! exact bound).

use std::collections::BTreeMap;

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefusalReason,
    TransitionRefused,
};
use serde::{Deserialize, Serialize};

/// Current wire/on-disk schema version for this module's types.
pub const CAPACITY_LEASE_VERSION: u16 = 1;

fn validate_non_empty(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    Ok(())
}

/// The resource a `CapacityCell` doles out. Mirrors the lane doc's "CPU/LLM/GPU
/// capacity" plus the existing AU gates this composes with
/// (`resource_priority.py`'s shared-LLM gate, `worker_scheduler.py`'s host
/// worker pool, `gpu_group_budget.py`'s per-GPU/model floors).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CapacityResourceClass {
    /// Shared LLM generator admission slots (composes with `resource_priority.py`).
    LlmGenerator,
    /// A separate embedding endpoint's admission slots — never contends with
    /// `LlmGenerator` (mirrors `resource_priority.py`'s separate embedding gate key).
    LlmEmbedding,
    /// GPU compute units (composes with `gpu_group_budget.py`).
    Gpu,
    /// Host worker pool slots (composes with `worker_scheduler.py`).
    Worker,
    /// CPU-bound compute slots.
    Cpu,
    /// Message-broker/engine dispatch admission slots.
    Broker,
    /// Resident memory in bytes (placement headroom, MEDULLA-PLACEMENT-R001.8).
    Memory,
    /// One cargo slot of a build host (MEDULLA-PLACEMENT-R786.1): the cell is
    /// `build-slots:<host>`, each lease takes `amount = 1`.
    BuildSlot,
}

/// The same four priority tiers `resource_priority.py::PriorityClass` defines,
/// vendored here (not imported — this crate has no Python interop and must not
/// depend on AU) so a `CapacityLease` carries the identical vocabulary the AU
/// scheduler already enforces at the LLM gate. `#[derive(Ord)]` on a fieldless
/// enum compares by DECLARATION ORDER, so listing `Interactive` first and
/// `BackgroundIngestion` last gives `Interactive < Orchestration < Hydration <
/// BackgroundIngestion`, matching `core/cognitive_scheduler.py::SchedulerPriority`'s
/// "lower value = higher priority" convention. `Orchestration`/`Hydration` are
/// NOT equal-priority under this `Ord` (AU's equal LLM-gate WEIGHT for the two
/// is a separate lookup table, `_PRIORITY_WEIGHTS`, not an enum-ordinal fact —
/// see `resource_priority.py`); only `may_spend_reserved_floor` below is a
/// hard cell-admission rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum LeasePriority {
    Interactive,
    Orchestration,
    Hydration,
    BackgroundIngestion,
    /// A land gate (MEDULLA-PLACEMENT-R786.1): the first served class of a build-slot cell.
    LandGate,
    /// A merge train: served after a land gate and before any lane.
    Train,
    /// An agent lane: the one build-slot class that never spends the reserved floor.
    Lane,
}

impl LeasePriority {
    /// False for `BackgroundIngestion` and `Lane` — the classes a `CapacityCell`'s
    /// `reserved_floor` is never available to (mirrors
    /// `PriorityClass.is_interactive_floor`'s "the one class that yields").
    pub fn may_spend_reserved_floor(self) -> bool {
        !matches!(
            self,
            LeasePriority::BackgroundIngestion | LeasePriority::Lane
        )
    }

    /// True for the three build-slot classes.
    pub fn is_build_slot_class(self) -> bool {
        matches!(
            self,
            LeasePriority::LandGate | LeasePriority::Train | LeasePriority::Lane
        )
    }

    /// Whether a cell of `resource_class` admits this priority (MEDULLA-PLACEMENT-R786.1): a
    /// build-slot cell admits exactly the three build-slot classes and no other resource
    /// class admits them.
    pub fn allowed_for(self, resource_class: CapacityResourceClass) -> bool {
        self.is_build_slot_class() == (resource_class == CapacityResourceClass::BuildSlot)
    }
}

/// A durable capacity pool. `capacity` is the cell's total admittable amount at
/// its current `epoch`; `reserved_floor` is the slice never available to
/// `BackgroundIngestion` (invariant: "Interactive and orchestration floors are
/// reserved; background uses spare capacity but cannot starve a tenant
/// indefinitely").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CapacityCell {
    pub cell_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub resource_class: CapacityResourceClass,
    pub capacity: u64,
    pub reserved_floor: u64,
    /// Monotonic authority epoch (GOC-03 fencing concept, vendored — see module
    /// doc). Bumped on cell-authority failover/repartition; an acquire/renew
    /// carrying an epoch below this is a stale-authority rejection, never a
    /// best-effort read of a foreign shape.
    pub epoch: u64,
    /// sha256 digest of the policy/catalog/model snapshot this cell was
    /// provisioned under (checked at every hop per the lane doc's "Security and
    /// privacy" section); opaque here, verified upstream.
    pub policy_digest: String,
    pub updated_at_ms: u64,
    /// EH-406: the optional error-budget throttle. Its `ceiling` gates new
    /// admissions and never exceeds `capacity`; automatic steps
    /// (`ThrottleCapacityCell`) move only the ceiling, and only the operator
    /// (`UpdateCapacityCell`) declares the policy or the capacity itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub throttle: Option<crate::capacity_throttle::CapacityThrottle>,
}

impl CapacityCell {
    pub fn validate(&self) -> Result<(), String> {
        validate_non_empty("cell_id", &self.cell_id)?;
        validate_non_empty("policy_digest", &self.policy_digest)?;
        if self.reserved_floor > self.capacity {
            return Err("reserved_floor must not exceed capacity".to_string());
        }
        if self.epoch == 0 {
            return Err(
                "epoch must be non-zero (0 is reserved for 'never provisioned')".to_string(),
            );
        }
        if let Some(throttle) = &self.throttle {
            throttle.validate(self.capacity)?;
        }
        Ok(())
    }

    /// The ceiling new admissions are measured against: the declared
    /// `capacity`, narrowed by the throttle when one is declared.
    pub fn effective_capacity(&self) -> u64 {
        self.throttle.as_ref().map_or(self.capacity, |throttle| {
            throttle.ceiling.min(self.capacity)
        })
    }

    /// Capacity available to `priority` right now, given `already_leased`
    /// active-lease amount at this cell. `BackgroundIngestion` can only spend
    /// the spare (non-floor) capacity; every other priority may also draw on
    /// the reserved floor.
    pub fn available_for(&self, priority: LeasePriority, already_leased: u64) -> u64 {
        let effective = self.effective_capacity();
        let ceiling = if priority.may_spend_reserved_floor() {
            effective
        } else {
            effective.saturating_sub(self.reserved_floor)
        };
        ceiling.saturating_sub(already_leased.min(ceiling))
    }
}

/// Lifecycle state of one `CapacityLease` (MEDULLA-PLACEMENT-R786.2). A renewal is a
/// count (`renewed_count`), not a state, and a deadline passed before the sweeper stored
/// `Expired` is derived by [`CapacityLease::effective_status`]: a timestamp records a
/// fact, not a state. The retired stored texts `active` and `reclaimed` read as
/// `Granted` and `Expired`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CapacityLeaseState {
    /// Created and not yet placed: the one state a lease starts in.
    Requested,
    /// Waiting in its cell's fair queue for a slot.
    Queued,
    /// Holds its slot and has not started work.
    #[serde(alias = "active")]
    Granted,
    /// Holds its slot and runs.
    Running,
    /// The holder gave the slot back (or cancelled its wait). Terminal.
    Released,
    /// A deadline passed and the sweeper returned the slot. Terminal.
    #[serde(alias = "reclaimed")]
    Expired,
    /// An administrator took the slot back. Terminal.
    Revoked,
}

impl CapacityLeaseState {
    fn is_terminal(self) -> bool {
        CAPACITY_LEASE_MACHINE.is_terminal(self)
    }

    /// True while the lease counts against its cell's capacity.
    pub fn holds_capacity(self) -> bool {
        matches!(self, Self::Granted | Self::Running)
    }
}

// MEDULLA-PLACEMENT-R786.2 — the capacity lease edge table and its one transition function.
// CapacityLeaseMachine: the row's edge table. The ontology test compares it with state-machines-v1.ttl.
/// An event of `CapacityLeaseMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CapacityLeaseEvent {
    /// The `Enqueue` event: the cell has no capacity now.
    Enqueue,
    /// The `Grant` event: the cell admits the lease.
    Grant,
    /// The `Start` event: the holder begins work.
    Start,
    /// The `Renew` event: the holder heartbeats.
    Renew,
    /// The `Release` event: the holder gives the slot back or cancels its wait.
    Release,
    /// The `Expire` event: the sweeper acts on a passed deadline (renamed from `Reclaim`).
    Expire,
    /// The `Revoke` event: an administrator takes the slot back.
    Revoke,
}

impl MachineState for CapacityLeaseState {
    fn stored(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Queued => "queued",
            Self::Granted => "granted",
            Self::Running => "running",
            Self::Released => "released",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
        }
    }
}

impl MachineEvent for CapacityLeaseEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Enqueue => "Enqueue",
            Self::Grant => "Grant",
            Self::Start => "Start",
            Self::Renew => "Renew",
            Self::Release => "Release",
            Self::Expire => "Expire",
            Self::Revoke => "Revoke",
        }
    }
}

/// Guard of `CapacityLeaseMachine`: the cell has no capacity for the lease's priority now (B01).
pub const CAPACITY_LEASE_GUARD_CELL_FULL: &str = "CellFull";
/// Guard of `CapacityLeaseMachine`: the cell admits the lease now (B02).
pub const CAPACITY_LEASE_GUARD_CELL_ADMITS: &str = "CellAdmits";
/// Guard of `CapacityLeaseMachine`: the lease heads its priority's queue and the cell admits it (B03).
pub const CAPACITY_LEASE_GUARD_HEADS_QUEUE: &str = "HeadsQueueAndCellAdmits";
/// Guard of `CapacityLeaseMachine`: can_renew holds: the owner matches and the lease is live
/// (B04 to B06).
pub const CAPACITY_LEASE_GUARD_CAN_RENEW: &str = "CanRenew";
/// Guard of `CapacityLeaseMachine`: can_release holds: the owner matches (B07 to B09).
pub const CAPACITY_LEASE_GUARD_CAN_RELEASE: &str = "CanRelease";
/// Guard of `CapacityLeaseMachine`: the caller holds `capacity:admin` (B10 to B12).
pub const CAPACITY_LEASE_GUARD_CALLER_IS_ADMIN: &str = "CallerHoldsAdmin";
/// Guard of `CapacityLeaseMachine`: the deadline has passed; the sweeper acts (B13 to B15).
pub const CAPACITY_LEASE_GUARD_LEASE_UNTIL_PASSED: &str = "LeaseUntilPassed";

/// The facts the guards of `CapacityLeaseMachine` read. A caller states each fact it has
/// established; a fact it has not established is false, so the guard fails closed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CapacityLeaseFacts {
    /// The caller's actor digest equals the lease's.
    pub owner_matches: bool,
    /// The lease's deadline has not passed.
    pub live: bool,
    /// The caller holds `capacity:admin`.
    pub caller_holds_admin: bool,
    /// The deadline of the lease's current state has passed.
    pub deadline_passed: bool,
    /// The cell admits the lease's priority and amount now.
    pub cell_admits: bool,
    /// The lease heads its priority's queue.
    pub heads_queue: bool,
}

impl CapacityLeaseFacts {
    /// Whether the named guard holds under these facts. An unknown name does not hold.
    pub fn holds(&self, guard: &str) -> bool {
        match guard {
            CAPACITY_LEASE_GUARD_CELL_FULL => !self.cell_admits,
            CAPACITY_LEASE_GUARD_CELL_ADMITS => self.cell_admits,
            CAPACITY_LEASE_GUARD_HEADS_QUEUE => self.heads_queue && self.cell_admits,
            CAPACITY_LEASE_GUARD_CAN_RENEW => self.owner_matches && self.live,
            CAPACITY_LEASE_GUARD_CAN_RELEASE => self.owner_matches,
            CAPACITY_LEASE_GUARD_CALLER_IS_ADMIN => self.caller_holds_admin,
            CAPACITY_LEASE_GUARD_LEASE_UNTIL_PASSED => self.deadline_passed,
            _ => false,
        }
    }
}

/// `CapacityLeaseMachine`: the closed edge table. Every other pair of state and event is refused.
pub static CAPACITY_LEASE_MACHINE: MachineDef<
    CapacityLeaseState,
    CapacityLeaseEvent,
    &'static str,
> = MachineDef {
    name: "CapacityLeaseMachine",
    states: &[
        CapacityLeaseState::Requested,
        CapacityLeaseState::Queued,
        CapacityLeaseState::Granted,
        CapacityLeaseState::Running,
        CapacityLeaseState::Released,
        CapacityLeaseState::Expired,
        CapacityLeaseState::Revoked,
    ],
    initial: CapacityLeaseState::Requested,
    terminals: &[
        CapacityLeaseState::Released,
        CapacityLeaseState::Expired,
        CapacityLeaseState::Revoked,
    ],
    events: &[
        CapacityLeaseEvent::Enqueue,
        CapacityLeaseEvent::Grant,
        CapacityLeaseEvent::Start,
        CapacityLeaseEvent::Renew,
        CapacityLeaseEvent::Release,
        CapacityLeaseEvent::Expire,
        CapacityLeaseEvent::Revoke,
    ],
    guards: &[
        CAPACITY_LEASE_GUARD_CELL_FULL,
        CAPACITY_LEASE_GUARD_CELL_ADMITS,
        CAPACITY_LEASE_GUARD_HEADS_QUEUE,
        CAPACITY_LEASE_GUARD_CAN_RENEW,
        CAPACITY_LEASE_GUARD_CAN_RELEASE,
        CAPACITY_LEASE_GUARD_CALLER_IS_ADMIN,
        CAPACITY_LEASE_GUARD_LEASE_UNTIL_PASSED,
    ],
    edges: &[
        Edge {
            id: "B01",
            from: CapacityLeaseState::Requested,
            event: CapacityLeaseEvent::Enqueue,
            guard: Some(CAPACITY_LEASE_GUARD_CELL_FULL),
            to: CapacityLeaseState::Queued,
        },
        Edge {
            id: "B02",
            from: CapacityLeaseState::Requested,
            event: CapacityLeaseEvent::Grant,
            guard: Some(CAPACITY_LEASE_GUARD_CELL_ADMITS),
            to: CapacityLeaseState::Granted,
        },
        Edge {
            id: "B03",
            from: CapacityLeaseState::Queued,
            event: CapacityLeaseEvent::Grant,
            guard: Some(CAPACITY_LEASE_GUARD_HEADS_QUEUE),
            to: CapacityLeaseState::Granted,
        },
        Edge {
            id: "B04",
            from: CapacityLeaseState::Granted,
            event: CapacityLeaseEvent::Start,
            guard: Some(CAPACITY_LEASE_GUARD_CAN_RENEW),
            to: CapacityLeaseState::Running,
        },
        Edge {
            id: "B05",
            from: CapacityLeaseState::Granted,
            event: CapacityLeaseEvent::Renew,
            guard: Some(CAPACITY_LEASE_GUARD_CAN_RENEW),
            to: CapacityLeaseState::Granted,
        },
        Edge {
            id: "B06",
            from: CapacityLeaseState::Running,
            event: CapacityLeaseEvent::Renew,
            guard: Some(CAPACITY_LEASE_GUARD_CAN_RENEW),
            to: CapacityLeaseState::Running,
        },
        Edge {
            id: "B07",
            from: CapacityLeaseState::Granted,
            event: CapacityLeaseEvent::Release,
            guard: Some(CAPACITY_LEASE_GUARD_CAN_RELEASE),
            to: CapacityLeaseState::Released,
        },
        Edge {
            id: "B08",
            from: CapacityLeaseState::Running,
            event: CapacityLeaseEvent::Release,
            guard: Some(CAPACITY_LEASE_GUARD_CAN_RELEASE),
            to: CapacityLeaseState::Released,
        },
        Edge {
            id: "B09",
            from: CapacityLeaseState::Queued,
            event: CapacityLeaseEvent::Release,
            guard: Some(CAPACITY_LEASE_GUARD_CAN_RELEASE),
            to: CapacityLeaseState::Released,
        },
        Edge {
            id: "B10",
            from: CapacityLeaseState::Queued,
            event: CapacityLeaseEvent::Revoke,
            guard: Some(CAPACITY_LEASE_GUARD_CALLER_IS_ADMIN),
            to: CapacityLeaseState::Revoked,
        },
        Edge {
            id: "B11",
            from: CapacityLeaseState::Granted,
            event: CapacityLeaseEvent::Revoke,
            guard: Some(CAPACITY_LEASE_GUARD_CALLER_IS_ADMIN),
            to: CapacityLeaseState::Revoked,
        },
        Edge {
            id: "B12",
            from: CapacityLeaseState::Running,
            event: CapacityLeaseEvent::Revoke,
            guard: Some(CAPACITY_LEASE_GUARD_CALLER_IS_ADMIN),
            to: CapacityLeaseState::Revoked,
        },
        Edge {
            id: "B13",
            from: CapacityLeaseState::Queued,
            event: CapacityLeaseEvent::Expire,
            guard: Some(CAPACITY_LEASE_GUARD_LEASE_UNTIL_PASSED),
            to: CapacityLeaseState::Expired,
        },
        Edge {
            id: "B14",
            from: CapacityLeaseState::Granted,
            event: CapacityLeaseEvent::Expire,
            guard: Some(CAPACITY_LEASE_GUARD_LEASE_UNTIL_PASSED),
            to: CapacityLeaseState::Expired,
        },
        Edge {
            id: "B15",
            from: CapacityLeaseState::Running,
            event: CapacityLeaseEvent::Expire,
            guard: Some(CAPACITY_LEASE_GUARD_LEASE_UNTIL_PASSED),
            to: CapacityLeaseState::Expired,
        },
    ],
};

impl CapacityLeaseState {
    /// The one transition function of `CapacityLeaseMachine`. `guard_holds` answers each guard
    /// by name; an unguarded edge always holds.
    pub fn apply(
        self,
        event: CapacityLeaseEvent,
        guard_holds: impl Fn(&'static str) -> bool,
    ) -> Result<Self, TransitionRefused> {
        transition(&CAPACITY_LEASE_MACHINE, self, event, guard_holds).map(|edge| edge.to)
    }

    /// [`Self::apply`] with the guards answered from stated facts.
    pub fn apply_with(
        self,
        event: CapacityLeaseEvent,
        facts: &CapacityLeaseFacts,
    ) -> Result<Self, TransitionRefused> {
        self.apply(event, |guard| facts.holds(guard))
    }
}

/// The newest transitions one lease keeps (`CapacityStatus` returns them in order).
pub const MAX_LEASE_TRANSITIONS: usize = 50;

/// One applied transition, recorded as a fact in the same write as the state it produced
/// (MEDULLA-PLACEMENT-R786.6). It holds no secret and no command line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CapacityTransitionRecord {
    pub lease_id: String,
    pub cell_id: String,
    pub from: CapacityLeaseState,
    pub event: CapacityLeaseEvent,
    pub to: CapacityLeaseState,
    pub actor_digest: String,
    pub priority: LeasePriority,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_seq: Option<u64>,
    pub at_ms: u64,
    pub fence_token: u64,
}

/// One tenant's durable claim on a `CapacityCell`. Field set matches the lane
/// doc's frozen schema.
// `Eq` is restored here because `cost_budget_micros` is now an exact integer —
// see its field doc. `Hash` is deliberately NOT derived: nothing keys a map or
// set by a whole lease, and adding it would force `Hash` onto
// `CapacityResourceClass` and `LeasePriority` purely to satisfy a capability no
// caller has asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CapacityLease {
    pub schema_version: u16,
    pub lease_id: String,
    pub work_item_id: String,
    pub tenant_ref: String,
    /// Opaque authenticated actor identity (GOC-15 verified carrier digest).
    pub actor_digest: String,
    pub cell_id: String,
    pub resource_class: CapacityResourceClass,
    pub amount: u64,
    pub priority: LeasePriority,
    /// Non-zero, unguessable-in-practice token minted at acquire time (GOC-03
    /// fencing concept, vendored). Renew/release must present the exact value
    /// last minted for this `lease_id`; a mismatch is `StaleFence`.
    pub fence_token: u64,
    /// The `CapacityCell::epoch` this lease was acquired under. A renew/release
    /// presenting a lease whose `lease_epoch` is behind the cell's CURRENT
    /// epoch is `StaleEpoch` — the cell was repartitioned/failed-over and this
    /// holder's authority no longer applies, independent of whether its
    /// `expires_at_ms` has passed yet.
    pub lease_epoch: u64,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub renewed_count: u32,
    /// Spend ceiling in **micro-units (1e-6) of the deployment's accounting
    /// currency** — exact integer, never a float.
    ///
    /// This was `Option<f64>` and that was wrong three ways, not just
    /// inconvenient for the `Eq` derive:
    ///
    /// 1. **`NaN`/`±Inf`/negative were representable and unvalidated.**
    ///    `validate()` checks neither budget field, so a `NaN` ceiling passed
    ///    silently. A budget is a *control*; a control that admits a value
    ///    which compares false against everything fails open.
    /// 2. **Float accumulation is inexact and non-associative.** A distributed
    ///    cost ledger summing leases must reach the same total on every node
    ///    regardless of summation order. `f64` does not guarantee that.
    /// 3. **`NaN` is not JSON-representable.** It serializes to a bare `NaN`
    ///    token that strict parsers reject, so a non-finite ceiling is an
    ///    interop hazard at the AU boundary, not merely an odd number.
    ///
    /// An unsigned integer makes all three unrepresentable rather than
    /// validated-against, and matches `token_budget` — the sibling ceiling in
    /// this same struct, which was already exact. `None` means unbounded;
    /// `Some(0)` means "may spend nothing", a genuinely different state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_budget_micros: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    pub idempotency_key: String,
    pub state: CapacityLeaseState,
    /// The place in its cell's queue, taken when the lease enters `Queued`
    /// (MEDULLA-PLACEMENT-R786.5): the next value of a per-cell monotonic counter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_seq: Option<u64>,
    /// The deadline of a queued lease; the sweeper expires it after this
    /// (MEDULLA-PLACEMENT-R786.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_expires_at_ms: Option<u64>,
    /// The newest [`MAX_LEASE_TRANSITIONS`] applied transitions, oldest first
    /// (MEDULLA-PLACEMENT-R786.6).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<CapacityTransitionRecord>,
}

impl CapacityLease {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != CAPACITY_LEASE_VERSION {
            return Err(format!(
                "unsupported capacity lease version {} (expected {})",
                self.schema_version, CAPACITY_LEASE_VERSION
            ));
        }
        for (field, value) in [
            ("lease_id", &self.lease_id),
            ("work_item_id", &self.work_item_id),
            ("tenant_ref", &self.tenant_ref),
            ("actor_digest", &self.actor_digest),
            ("cell_id", &self.cell_id),
            ("idempotency_key", &self.idempotency_key),
        ] {
            validate_non_empty(field, value)?;
        }
        if self.amount == 0 {
            return Err("amount must be non-zero".to_string());
        }
        if self.fence_token == 0 {
            return Err("fence_token must be non-zero".to_string());
        }
        if self.lease_epoch == 0 {
            return Err("lease_epoch must be non-zero".to_string());
        }
        if self.expires_at_ms <= self.issued_at_ms {
            return Err("expires_at_ms must be after issued_at_ms".to_string());
        }
        Ok(())
    }

    /// The deadline the current state is measured against: the queue deadline of a queued
    /// lease, the holder deadline otherwise.
    pub fn deadline_ms(&self) -> u64 {
        match (self.state, self.queue_expires_at_ms) {
            (CapacityLeaseState::Queued, Some(queue_deadline)) => queue_deadline,
            _ => self.expires_at_ms,
        }
    }

    /// Whether `now_ms` has reached the deadline of a lease that is not terminal.
    pub fn deadline_passed(&self, now_ms: u64) -> bool {
        now_ms >= self.deadline_ms()
    }

    /// The holder TTL this lease was requested with. Meaningful until its first grant, which
    /// restarts the deadline from the grant instant.
    pub fn ttl_ms(&self) -> u64 {
        self.expires_at_ms.saturating_sub(self.issued_at_ms)
    }

    /// True once `now_ms` has reached the deadline OR the lease is already terminal. A caller
    /// must treat an expired lease as fenced even if it never sees an `Expired`
    /// transition (the sweep is a cleanup, not the authority — the authority is the
    /// timestamp comparison itself, so two independent sweepers can never race on "is it
    /// expired").
    pub fn is_expired(&self, now_ms: u64) -> bool {
        self.state.is_terminal() || self.deadline_passed(now_ms)
    }

    /// The state a read at `now_ms` reports: `Expired` for a non-terminal lease past its
    /// deadline that the sweeper has not stored yet (MEDULLA-PLACEMENT-R782.1). A read-side
    /// derivation only: nothing stores it.
    pub fn effective_status(&self, now_ms: u64) -> CapacityLeaseState {
        if !self.state.is_terminal() && self.deadline_passed(now_ms) {
            CapacityLeaseState::Expired
        } else {
            self.state
        }
    }

    /// Fire `event` on this lease: apply the machine under `facts`, store the state it
    /// returns and record the transition (MEDULLA-PLACEMENT-R786.2 and R786.6). A refusal
    /// leaves the lease unchanged and records nothing. This is the one writer of `state`.
    pub fn fire(
        &mut self,
        event: CapacityLeaseEvent,
        facts: &CapacityLeaseFacts,
        at_ms: u64,
    ) -> Result<CapacityLeaseState, TransitionRefused> {
        let from = self.state;
        let to = from.apply_with(event, facts)?;
        self.state = to;
        if self.transitions.len() >= MAX_LEASE_TRANSITIONS {
            self.transitions.remove(0);
        }
        self.transitions.push(CapacityTransitionRecord {
            lease_id: self.lease_id.clone(),
            cell_id: self.cell_id.clone(),
            from,
            event,
            to,
            actor_digest: self.actor_digest.clone(),
            priority: self.priority,
            queue_seq: self.queue_seq,
            at_ms,
            fence_token: self.fence_token,
        });
        Ok(to)
    }
}

/// Why an acquire/renew/release was refused. Every variant is a case the lane
/// doc's acceptance gates require to be an explicit rejection, never a
/// fail-open admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum CapacityDenial {
    /// Not enough spare/floor capacity for this priority right now.
    Exhausted { available: u64, requested: u64 },
    /// The caller's `lease_epoch` is behind the cell's current epoch — the cell
    /// was repartitioned/failed over. THE fencing rejection this lane's
    /// "known-bad proof" targets.
    StaleEpoch { cell_epoch: u64, caller_epoch: u64 },
    /// The caller's `fence_token` does not match the ledger's live record for
    /// this `lease_id`. The other half of the fencing rejection.
    StaleFence,
    /// The lease has reached its deadline (or is already terminal); a
    /// renew/release/commit from an expired holder is rejected outright —
    /// nothing "keeps working" past expiry.
    LeaseExpired,
    /// `lease_id` unknown to the ledger.
    NotFound,
    /// Replaying `(tenant_ref, idempotency_key)` against a DIFFERENT demand
    /// shape than the original (mirrors `CommitDescriptor`'s idempotency
    /// invariant): same key must mean same request, never a shape mismatch
    /// silently accepted.
    IdempotencyConflict,
    /// The cell's resource class does not admit this priority (MEDULLA-PLACEMENT-R786.1): a
    /// build-slot cell admits only `LandGate`, `Train` and `Lane`, and no other resource class
    /// admits them. Wire code `PRIORITY_NOT_ALLOWED_FOR_RESOURCE`.
    PriorityNotAllowedForResource {
        priority: LeasePriority,
        resource_class: CapacityResourceClass,
    },
    /// The machine has no edge for this state and event, or its guard failed
    /// (MEDULLA-PLACEMENT-R786.3). Wire code `TRANSITION_REFUSED`.
    TransitionRefused {
        machine: String,
        from: String,
        event: String,
        why: TransitionRefusalReason,
    },
}

/// The wire code of [`CapacityDenial::PriorityNotAllowedForResource`].
pub const PRIORITY_NOT_ALLOWED_FOR_RESOURCE: &str = "PRIORITY_NOT_ALLOWED_FOR_RESOURCE";

impl From<TransitionRefused> for CapacityDenial {
    fn from(refused: TransitionRefused) -> Self {
        Self::TransitionRefused {
            machine: refused.machine,
            from: refused.from,
            event: refused.event,
            why: refused.reason,
        }
    }
}

/// The leases of one cell that wait in its queue and that the cell now admits, in the order
/// they are granted (MEDULLA-PLACEMENT-R786.5). The one fair-queue policy: the store and the
/// reference ledger both call it.
///
/// Classes are considered in rank order (`LandGate`, then `Train`, then `Lane`). Within a
/// class the order is `queue_seq`, strictly first in first out: when the head of a class is
/// not admitted, no later lease of that class is considered, so none overtakes it.
/// `leased` is the amount the cell already lends; each granted lease adds its own amount
/// before the next is priced. `Lane` never spends the reserved floor because
/// [`CapacityCell::available_for`] refuses it.
pub fn grant_order(cell: &CapacityCell, leased: u64, queued: &[&CapacityLease]) -> Vec<String> {
    let mut by_rank: Vec<&CapacityLease> = queued
        .iter()
        .copied()
        .filter(|lease| lease.state == CapacityLeaseState::Queued && lease.cell_id == cell.cell_id)
        .collect();
    by_rank.sort_by_key(|lease| (lease.priority, lease.queue_seq.unwrap_or(u64::MAX)));
    let mut granted = Vec::new();
    let mut leased = leased;
    let mut blocked: Vec<LeasePriority> = Vec::new();
    for lease in by_rank {
        if blocked.contains(&lease.priority) {
            continue;
        }
        if cell.available_for(lease.priority, leased) >= lease.amount {
            leased = leased.saturating_add(lease.amount);
            granted.push(lease.lease_id.clone());
        } else {
            blocked.push(lease.priority);
        }
    }
    granted
}

/// What one sweep did: the leases it expired and the queued leases it then granted.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CapacitySweep {
    pub expired: Vec<String>,
    pub granted: Vec<String>,
}

/// Bounded, in-memory reference ledger for the `CapacityCell`s it is shown. This is the pure
/// algorithm a durable server-side store wraps (GOC-21-W04); it holds no I/O
/// and is deliberately small enough to be exhaustively unit-tested here as this
/// lane's fencing/fairness proof. A production store additionally persists
/// every state transition durably before acking (mirrors `work_item.py`'s
/// "commit-before-ack" discipline) — that persistence is out of this crate's
/// scope (bottom of the DAG, no I/O).
///
/// A request for a build-slot cell that the cell cannot admit waits in the cell's fair queue
/// (MEDULLA-PLACEMENT-R786.5); a request for any other resource class that cannot be
/// admitted is denied `Exhausted` and never stored, so only build-slot leases pass through
/// `Requested`/`Queued`.
#[derive(Debug, Default)]
pub struct CapacityLedger {
    leases: BTreeMap<String, CapacityLease>,
    /// Monotonic per-cell fence counter; the ONLY minter of `fence_token`, so
    /// two concurrent acquires against the same ledger can never mint the same
    /// token (the in-process analogue of the durable store's atomic counter).
    next_fence: u64,
    /// `(tenant_ref, idempotency_key) -> lease_id` replay table.
    idempotency: BTreeMap<(String, String), String>,
    /// The next `queue_seq` of each cell.
    next_queue_seq: BTreeMap<String, u64>,
    queue_ttl_ms: u64,
}

/// How long a queued lease waits before the sweeper expires it (the knob
/// `capacity.build_slot.queue_ttl_ms` default, MEDULLA-PLACEMENT-R786.4).
pub const DEFAULT_QUEUE_TTL_MS: u64 = 7_200_000;

/// Inputs for one atomic capacity admission attempt.
///
/// Keeping the request as one value makes the admission boundary explicit and
/// prevents callers from accidentally reordering identity, demand, and timing
/// fields. The borrowed fields are copied into the durable lease by
/// [`CapacityLedger::try_acquire`].
#[derive(Debug)]
pub struct CapacityAcquireRequest<'a> {
    pub cell: &'a CapacityCell,
    pub lease_id: String,
    pub work_item_id: &'a str,
    pub tenant_ref: &'a str,
    pub actor_digest: &'a str,
    pub resource_class: CapacityResourceClass,
    pub amount: u64,
    pub priority: LeasePriority,
    pub idempotency_key: &'a str,
    pub now_ms: u64,
    pub ttl_ms: u64,
}

impl CapacityLedger {
    pub fn new() -> Self {
        Self {
            leases: BTreeMap::new(),
            next_fence: 1,
            idempotency: BTreeMap::new(),
            next_queue_seq: BTreeMap::new(),
            queue_ttl_ms: DEFAULT_QUEUE_TTL_MS,
        }
    }

    /// The same ledger with its queue deadline set to `queue_ttl_ms`.
    pub fn with_queue_ttl_ms(mut self, queue_ttl_ms: u64) -> Self {
        self.queue_ttl_ms = queue_ttl_ms;
        self
    }

    fn active_leased_amount(&self, cell: &CapacityCell, now_ms: u64) -> u64 {
        self.leases
            .values()
            .filter(|l| {
                l.cell_id == cell.cell_id && l.state.holds_capacity() && !l.is_expired(now_ms)
            })
            .map(|l| l.amount)
            .sum()
    }

    /// Atomic CAS acquire: admits iff `cell.available_for(priority, already_leased)
    /// >= amount`. Mints a fresh, non-zero, ledger-unique `fence_token` and
    /// stamps `lease_epoch = cell.epoch`. Idempotent: replaying the same
    /// `(tenant_ref, idempotency_key)` returns the SAME lease (never mints a
    /// second one) as long as the demand shape matches; a shape mismatch is
    /// `IdempotencyConflict`. A priority the cell's resource class does not admit is
    /// `PriorityNotAllowedForResource`. The lease is `Granted` (edge B02) or, for a
    /// build-slot cell without capacity, `Queued` (edge B01).
    pub fn try_acquire(
        &mut self,
        request: CapacityAcquireRequest<'_>,
    ) -> Result<CapacityLease, CapacityDenial> {
        let CapacityAcquireRequest {
            cell,
            lease_id,
            work_item_id,
            tenant_ref,
            actor_digest,
            resource_class,
            amount,
            priority,
            idempotency_key,
            now_ms,
            ttl_ms,
        } = request;
        let replay_key = (tenant_ref.to_string(), idempotency_key.to_string());
        if let Some(existing_id) = self.idempotency.get(&replay_key) {
            let existing = self
                .leases
                .get(existing_id)
                .expect("idempotency index points at a live lease");
            if existing.cell_id == cell.cell_id
                && existing.resource_class == resource_class
                && existing.amount == amount
                && existing.work_item_id == work_item_id
            {
                return Ok(existing.clone());
            }
            return Err(CapacityDenial::IdempotencyConflict);
        }
        if !priority.allowed_for(cell.resource_class) {
            return Err(CapacityDenial::PriorityNotAllowedForResource {
                priority,
                resource_class: cell.resource_class,
            });
        }

        let already = self.active_leased_amount(cell, now_ms);
        let available = cell.available_for(priority, already);
        let queues = cell.resource_class == CapacityResourceClass::BuildSlot;
        if available < amount && !queues {
            return Err(CapacityDenial::Exhausted {
                available,
                requested: amount,
            });
        }

        let fence_token = self.next_fence;
        self.next_fence += 1;
        let mut lease = CapacityLease {
            schema_version: CAPACITY_LEASE_VERSION,
            lease_id: lease_id.clone(),
            work_item_id: work_item_id.to_string(),
            tenant_ref: tenant_ref.to_string(),
            actor_digest: actor_digest.to_string(),
            cell_id: cell.cell_id.clone(),
            resource_class,
            amount,
            priority,
            fence_token,
            lease_epoch: cell.epoch,
            issued_at_ms: now_ms,
            expires_at_ms: now_ms + ttl_ms,
            renewed_count: 0,
            cost_budget_micros: None,
            token_budget: None,
            idempotency_key: idempotency_key.to_string(),
            state: CapacityLeaseState::Requested,
            queue_seq: None,
            queue_expires_at_ms: None,
            transitions: Vec::new(),
        };
        let admitted = available >= amount;
        let event = if admitted {
            CapacityLeaseEvent::Grant
        } else {
            let seq = self.next_queue_seq.entry(cell.cell_id.clone()).or_insert(0);
            *seq += 1;
            lease.queue_seq = Some(*seq);
            lease.queue_expires_at_ms = Some(now_ms.saturating_add(self.queue_ttl_ms));
            CapacityLeaseEvent::Enqueue
        };
        let facts = CapacityLeaseFacts {
            cell_admits: admitted,
            ..CapacityLeaseFacts::default()
        };
        lease.fire(event, &facts, now_ms)?;
        self.leases.insert(lease_id.clone(), lease.clone());
        self.idempotency.insert(replay_key, lease_id);
        Ok(lease)
    }

    /// The fence and liveness checks every holder call shares: unknown lease, a deadline
    /// already passed, a stale cell epoch, a fence that does not match.
    fn check_holder(
        lease: &CapacityLease,
        fence_token: u64,
        lease_epoch: u64,
        cell_epoch: u64,
        now_ms: u64,
    ) -> Result<(), CapacityDenial> {
        if lease.is_expired(now_ms) {
            return Err(CapacityDenial::LeaseExpired);
        }
        if lease_epoch < cell_epoch {
            return Err(CapacityDenial::StaleEpoch {
                cell_epoch,
                caller_epoch: lease_epoch,
            });
        }
        if lease.fence_token != fence_token {
            return Err(CapacityDenial::StaleFence);
        }
        Ok(())
    }

    /// Renew: rejects a caller whose `fence_token`/`lease_epoch` do not match
    /// the ledger's live record, OR whose lease has already expired — an
    /// expired holder that "keeps working" is exactly the defect this rejects.
    /// Edges B05 and B06; a queued or terminal lease is `TransitionRefused`.
    pub fn try_renew(
        &mut self,
        lease_id: &str,
        fence_token: u64,
        lease_epoch: u64,
        cell_epoch: u64,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<CapacityLease, CapacityDenial> {
        let lease = self
            .leases
            .get_mut(lease_id)
            .ok_or(CapacityDenial::NotFound)?;
        if lease.state.is_terminal() {
            // A terminal state refuses every event: the typed refusal is the answer.
            lease.fire(
                CapacityLeaseEvent::Renew,
                &CapacityLeaseFacts::default(),
                now_ms,
            )?;
        }
        Self::check_holder(lease, fence_token, lease_epoch, cell_epoch, now_ms)?;
        let facts = CapacityLeaseFacts {
            owner_matches: true,
            live: true,
            ..CapacityLeaseFacts::default()
        };
        lease.fire(CapacityLeaseEvent::Renew, &facts, now_ms)?;
        lease.expires_at_ms = now_ms + ttl_ms;
        lease.renewed_count += 1;
        Ok(lease.clone())
    }

    /// Start: a granted lease begins work (edge B04). Same fencing checks as renew.
    pub fn try_start(
        &mut self,
        lease_id: &str,
        fence_token: u64,
        lease_epoch: u64,
        cell_epoch: u64,
        now_ms: u64,
    ) -> Result<CapacityLease, CapacityDenial> {
        let lease = self
            .leases
            .get_mut(lease_id)
            .ok_or(CapacityDenial::NotFound)?;
        if lease.state.is_terminal() {
            // A terminal state refuses every event: the typed refusal is the answer.
            lease.fire(
                CapacityLeaseEvent::Start,
                &CapacityLeaseFacts::default(),
                now_ms,
            )?;
        }
        Self::check_holder(lease, fence_token, lease_epoch, cell_epoch, now_ms)?;
        let facts = CapacityLeaseFacts {
            owner_matches: true,
            live: true,
            ..CapacityLeaseFacts::default()
        };
        lease.fire(CapacityLeaseEvent::Start, &facts, now_ms)?;
        Ok(lease.clone())
    }

    /// Release: same fencing checks as renew, then transitions terminal (edges B07 to B09).
    /// The caller then runs [`Self::grant_next`] for the freed slot. A repeated release of a
    /// released lease is `TransitionRefused` with `terminal_state`.
    pub fn try_release(
        &mut self,
        lease_id: &str,
        fence_token: u64,
        lease_epoch: u64,
        cell_epoch: u64,
        now_ms: u64,
    ) -> Result<(), CapacityDenial> {
        let lease = self
            .leases
            .get_mut(lease_id)
            .ok_or(CapacityDenial::NotFound)?;
        if lease.state.is_terminal() {
            // A terminal state refuses every event: the typed refusal is the answer.
            lease.fire(
                CapacityLeaseEvent::Release,
                &CapacityLeaseFacts::default(),
                now_ms,
            )?;
        }
        Self::check_holder(lease, fence_token, lease_epoch, cell_epoch, now_ms)?;
        let facts = CapacityLeaseFacts {
            owner_matches: true,
            ..CapacityLeaseFacts::default()
        };
        lease.fire(CapacityLeaseEvent::Release, &facts, now_ms)?;
        Ok(())
    }

    /// Revoke: an administrator takes a lease back (edges B10 to B12). `caller_holds_admin`
    /// states the scope check the dispatch layer already made.
    pub fn try_revoke(
        &mut self,
        lease_id: &str,
        caller_holds_admin: bool,
        now_ms: u64,
    ) -> Result<CapacityLease, CapacityDenial> {
        let lease = self
            .leases
            .get_mut(lease_id)
            .ok_or(CapacityDenial::NotFound)?;
        let facts = CapacityLeaseFacts {
            caller_holds_admin,
            ..CapacityLeaseFacts::default()
        };
        lease.fire(CapacityLeaseEvent::Revoke, &facts, now_ms)?;
        Ok(lease.clone())
    }

    /// Fire `Expire` on every non-terminal lease past its deadline: a queued lease past
    /// `queue_expires_at_ms`, a granted or running lease past `expires_at_ms`
    /// (edges B13 to B15). Returns the expired lease ids. Idempotent per lease.
    pub fn reclaim_expired(&mut self, now_ms: u64) -> Vec<String> {
        let mut expired = Vec::new();
        for (id, lease) in self.leases.iter_mut() {
            let facts = CapacityLeaseFacts {
                deadline_passed: lease.deadline_passed(now_ms),
                ..CapacityLeaseFacts::default()
            };
            if lease
                .fire(CapacityLeaseEvent::Expire, &facts, now_ms)
                .is_ok()
            {
                expired.push(id.clone());
            }
        }
        expired
    }

    /// Grant every queued lease of `cell` that it now admits, in [`grant_order`] (edge B03).
    /// Returns the granted lease ids in grant order.
    pub fn grant_next(&mut self, cell: &CapacityCell, now_ms: u64) -> Vec<String> {
        let leased = self.active_leased_amount(cell, now_ms);
        let order = {
            let queued: Vec<&CapacityLease> = self.leases.values().collect();
            grant_order(cell, leased, &queued)
        };
        let mut granted = Vec::with_capacity(order.len());
        for id in order {
            let lease = self.leases.get_mut(&id).expect("ordered lease exists");
            let facts = CapacityLeaseFacts {
                cell_admits: true,
                heads_queue: true,
                ..CapacityLeaseFacts::default()
            };
            if lease
                .fire(CapacityLeaseEvent::Grant, &facts, now_ms)
                .is_ok()
            {
                // The holder's deadline starts at the grant, not at the request.
                lease.expires_at_ms = now_ms + lease.ttl_ms();
                granted.push(id);
            }
        }
        granted
    }

    /// One sweeper pass: expire what passed its deadline, then grant queued leases into the
    /// slots that freed (MEDULLA-PLACEMENT-R786.4 and R786.5).
    pub fn sweep(&mut self, cells: &[&CapacityCell], now_ms: u64) -> CapacitySweep {
        let expired = self.reclaim_expired(now_ms);
        let mut granted = Vec::new();
        for cell in cells {
            granted.extend(self.grant_next(cell, now_ms));
        }
        CapacitySweep { expired, granted }
    }

    pub fn get(&self, lease_id: &str) -> Option<&CapacityLease> {
        self.leases.get(lease_id)
    }
}

#[cfg(test)]
mod tests {
    // MEDULLA-PLACEMENT-R786.2 — the capacity lease machine.
    use super::{
        CapacityLeaseEvent, CapacityLeaseFacts, CapacityLeaseState, CAPACITY_LEASE_MACHINE,
    };
    use medulla_types::transition_refused::{MachineState, TransitionRefusalReason};

    fn capacity_edge_case(
        id: &str,
        from: CapacityLeaseState,
        event: CapacityLeaseEvent,
        to: CapacityLeaseState,
    ) {
        let edge = CAPACITY_LEASE_MACHINE
            .edges
            .iter()
            .find(|e| e.id == id)
            .unwrap();
        assert_eq!((edge.from, edge.event, edge.to), (from, event, to));
        // Only this edge's guard holds, so a sibling edge on the same event cannot fire.
        assert_eq!(from.apply(event, |g| Some(g) == edge.guard), Ok(to));
        if edge.guard.is_some() {
            let refused = from.apply(event, |_| false).unwrap_err();
            assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed);
        }
        // The same edge fires from the named facts, and from no others.
        let facts = facts_for(edge.guard.unwrap_or_default());
        assert_eq!(from.apply_with(event, &facts), Ok(to), "{id} facts");
        // With no fact established the edge does not fire, except B01: a cell that has not
        // been shown to admit the lease queues it, the safe side.
        if edge.guard != Some(super::CAPACITY_LEASE_GUARD_CELL_FULL) {
            assert!(from
                .apply_with(event, &CapacityLeaseFacts::default())
                .is_err());
        }
    }

    /// The facts under which exactly `guard` holds.
    fn facts_for(guard: &str) -> CapacityLeaseFacts {
        let mut facts = CapacityLeaseFacts::default();
        match guard {
            super::CAPACITY_LEASE_GUARD_CELL_FULL => {}
            super::CAPACITY_LEASE_GUARD_CELL_ADMITS => facts.cell_admits = true,
            super::CAPACITY_LEASE_GUARD_HEADS_QUEUE => {
                facts.cell_admits = true;
                facts.heads_queue = true;
            }
            super::CAPACITY_LEASE_GUARD_CAN_RENEW => {
                facts.owner_matches = true;
                facts.live = true;
            }
            super::CAPACITY_LEASE_GUARD_CAN_RELEASE => facts.owner_matches = true,
            super::CAPACITY_LEASE_GUARD_CALLER_IS_ADMIN => facts.caller_holds_admin = true,
            super::CAPACITY_LEASE_GUARD_LEASE_UNTIL_PASSED => facts.deadline_passed = true,
            other => panic!("unknown guard {other}"),
        }
        facts
    }

    /// Every (state, event) pair with no edge.
    fn capacity_illegal_pairs() -> Vec<(CapacityLeaseState, CapacityLeaseEvent)> {
        let machine = &CAPACITY_LEASE_MACHINE;
        let mut out = Vec::new();
        for &state in machine.states {
            for &event in machine.events {
                if !machine
                    .edges
                    .iter()
                    .any(|e| e.from == state && e.event == event)
                {
                    out.push((state, event));
                }
            }
        }
        out
    }

    macro_rules! edge_tests {
        ($($name:ident, $id:literal, $from:ident, $event:ident, $to:ident;)*) => {$(
            // spec: MEDULLA-PLACEMENT-R786.2
            // spec: MEDULLA-PLACEMENT-R782.1
            #[test]
            fn $name() {
                capacity_edge_case(
                    $id,
                    CapacityLeaseState::$from,
                    CapacityLeaseEvent::$event,
                    CapacityLeaseState::$to,
                );
            }
        )*};
    }

    edge_tests! {
        edge_b01_requested_enqueue, "B01", Requested, Enqueue, Queued;
        edge_b02_requested_grant, "B02", Requested, Grant, Granted;
        edge_b03_queued_grant, "B03", Queued, Grant, Granted;
        edge_b04_granted_start, "B04", Granted, Start, Running;
        edge_b05_granted_renew, "B05", Granted, Renew, Granted;
        edge_b06_running_renew, "B06", Running, Renew, Running;
        edge_b07_granted_release, "B07", Granted, Release, Released;
        edge_b08_running_release, "B08", Running, Release, Released;
        edge_b09_queued_release, "B09", Queued, Release, Released;
        edge_b10_queued_revoke, "B10", Queued, Revoke, Revoked;
        edge_b11_granted_revoke, "B11", Granted, Revoke, Revoked;
        edge_b12_running_revoke, "B12", Running, Revoke, Revoked;
        edge_b13_queued_expire, "B13", Queued, Expire, Expired;
        edge_b14_granted_expire, "B14", Granted, Expire, Expired;
        edge_b15_running_expire, "B15", Running, Expire, Expired;
    }

    // spec: MEDULLA-PLACEMENT-R786.2
    // spec: MEDULLA-PLACEMENT-R782.1
    #[test]
    fn every_illegal_pair_is_refused() {
        let machine = &CAPACITY_LEASE_MACHINE;
        let pairs = capacity_illegal_pairs();
        assert_eq!(machine.states.len() * machine.events.len(), 49);
        assert_eq!(pairs.len(), 34, "49 pairs less the 15 edges");
        for (state, event) in pairs {
            let refused = state.apply(event, |_| true).unwrap_err();
            let expected = if machine.is_terminal(state) {
                TransitionRefusalReason::TerminalState
            } else {
                TransitionRefusalReason::IllegalTransition
            };
            assert_eq!(refused.reason, expected, "{state:?} {event:?}");
            assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
        }
        for &terminal in machine.terminals {
            for &event in machine.events {
                assert!(terminal.apply(event, |_| true).is_err());
            }
        }
        // A renew after release is refused (MEDULLA-PLACEMENT-R782.1).
        assert!(CapacityLeaseState::Released
            .apply(CapacityLeaseEvent::Renew, |_| true)
            .is_err());
    }

    // spec: MEDULLA-PLACEMENT-R786.2
    // spec: MEDULLA-PLACEMENT-R782.1
    #[test]
    fn refusal_carries_machine_from_event() {
        let refused = CapacityLeaseState::Released
            .apply(CapacityLeaseEvent::Renew, |_| true)
            .unwrap_err();
        assert_eq!(refused.machine, "CapacityLeaseMachine");
        assert_eq!(refused.from, CapacityLeaseState::Released.stored());
        assert_eq!(refused.event, "Renew");
        assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
    }

    /// The retired stored texts read as their successors; no other text is a state.
    #[test]
    fn retired_stored_texts_read_as_granted_and_expired() {
        let read = |text: &str| serde_json::from_str::<CapacityLeaseState>(&format!("\"{text}\""));
        assert_eq!(read("active").unwrap(), CapacityLeaseState::Granted);
        assert_eq!(read("reclaimed").unwrap(), CapacityLeaseState::Expired);
        assert!(read("renewed").is_err());
        for &state in CAPACITY_LEASE_MACHINE.states {
            assert_eq!(read(state.stored()).unwrap(), state);
        }
    }

    /// Lines that assign the state field directly (`.<field> = <Type>::`), outside the
    /// file's `#[cfg(test)] mod tests`.
    fn direct_state_writes<'a>(source: &'a str, field: &str, ty: &str) -> Vec<&'a str> {
        let needle = format!(".{field} = {ty}::");
        let mut out = Vec::new();
        let mut previous = "";
        for line in source.lines().map(str::trim) {
            if previous == "#[cfg(test)]" && line.starts_with("mod tests") {
                break;
            }
            previous = line;
            if !line.starts_with("//") && line.contains(&needle) {
                out.push(line);
            }
        }
        out
    }

    // spec: MEDULLA-PLACEMENT-R786.3
    // spec: MEDULLA-PLACEMENT-R782.1
    #[test]
    fn no_writer_sets_state_outside_apply() {
        let mut roots = vec![concat!(env!("CARGO_MANIFEST_DIR"), "/src/capacity_lease.rs").into()];
        let store = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../medulla-resource-store/src/redb_store/capacity_lease"
        );
        roots.push(format!("{store}.rs"));
        for entry in std::fs::read_dir(store).expect("store submodules") {
            let path = entry.expect("entry").path();
            if path.extension().is_some_and(|ext| ext == "rs") {
                roots.push(path.to_string_lossy().into_owned());
            }
            if path.is_dir() {
                for inner in std::fs::read_dir(&path).expect("nested submodules") {
                    let inner = inner.expect("entry").path();
                    if inner.extension().is_some_and(|ext| ext == "rs") {
                        roots.push(inner.to_string_lossy().into_owned());
                    }
                }
            }
        }
        assert!(roots.len() >= 3, "the scan must see the writer sources");
        for path in roots {
            let source = std::fs::read_to_string(&path).expect("writer source");
            // A test module of its own file is the only place a literal state may be set.
            if path.ends_with("tests.rs") || path.contains("/tests/") {
                continue;
            }
            let writes = direct_state_writes(&source, "state", "CapacityLeaseState");
            assert!(
                writes.is_empty(),
                "direct state writes in {path}: {writes:?}"
            );
        }
    }

    // spec: MEDULLA-PLACEMENT-R782.1
    #[test]
    fn every_transition_and_refusal_is_covered() {
        let machine = &CAPACITY_LEASE_MACHINE;
        assert_eq!(machine.edges.len(), 15);
        assert_eq!(machine.states.len(), 7);
        assert_eq!(machine.events.len(), 7);
        assert_eq!(machine.terminals.len(), 3);
        let mut ids: Vec<_> = machine.edges.iter().map(|e| e.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), machine.edges.len(), "edge ids are unique");
        for edge in machine.edges {
            assert!(
                !machine.is_terminal(edge.from),
                "{} leaves a terminal state",
                edge.id
            );
        }
        for &state in machine.states {
            assert_eq!(machine.state_from_stored(state.stored()), Some(state));
            for &event in machine.events {
                let legal = state.apply(event, |_| true).is_ok();
                let has_edge = machine.edges_from(state).any(|e| e.event == event);
                assert_eq!(legal, has_edge, "{state:?} {event:?}");
            }
        }
    }

    use super::*;

    fn cell(epoch: u64) -> CapacityCell {
        CapacityCell {
            cell_id: "cell-llm-1".to_string(),
            parent_id: None,
            resource_class: CapacityResourceClass::LlmGenerator,
            capacity: 10,
            reserved_floor: 2,
            epoch,
            policy_digest: "d".repeat(64),
            updated_at_ms: 0,
            throttle: None,
        }
    }

    /// EH-406: a narrowed throttle ceiling gates new admissions, reserved floor
    /// included, and never lifts the cell above its declared capacity.
    #[test]
    fn admission_is_measured_against_the_throttle_ceiling() {
        use crate::capacity_throttle::{CapacityThrottle, CapacityThrottlePolicy};
        let policy = CapacityThrottlePolicy {
            error_budget_ppm: 50_000,
            recovery_ppm: 10_000,
            min_samples: 1,
            decrease_per_mille: 500,
            increase_step: 1,
            floor: 1,
            cooldown_ms: 0,
        };
        let mut throttled = cell(1);
        let mut throttle = CapacityThrottle::open(policy, 10);
        throttle.ceiling = 1;
        throttled.throttle = Some(throttle);
        assert_eq!(throttled.effective_capacity(), 1);
        assert_eq!(throttled.available_for(LeasePriority::Interactive, 0), 1);
        assert_eq!(
            throttled.available_for(LeasePriority::BackgroundIngestion, 0),
            0
        );
        throttled.throttle.as_mut().unwrap().ceiling = 11;
        assert!(
            throttled.validate().is_err(),
            "a ceiling above capacity is refused"
        );
    }

    /// KNOWN-BAD PROOF 1: an expired lease holder is rejected on renew, not
    /// silently allowed to keep working.
    #[test]
    fn expired_holder_is_rejected_on_renew() {
        let mut ledger = CapacityLedger::new();
        let c = cell(1);
        let lease = ledger
            .try_acquire(CapacityAcquireRequest {
                cell: &c,
                lease_id: "lease-1".into(),
                work_item_id: "wi-1",
                tenant_ref: "tenant-a",
                actor_digest: "actor-a",
                resource_class: CapacityResourceClass::LlmGenerator,
                amount: 1,
                priority: LeasePriority::Interactive,
                idempotency_key: "idem-1",
                now_ms: 0,
                ttl_ms: 1_000,
            })
            .expect("acquire admits under capacity");

        // Time passes well beyond the 1000ms TTL — the holder never renewed.
        let now_ms = 5_000;
        let err = ledger
            .try_renew(
                &lease.lease_id,
                lease.fence_token,
                lease.lease_epoch,
                c.epoch,
                now_ms,
                1_000,
            )
            .expect_err("an expired lease must be rejected, never silently renewed");
        assert_eq!(err, CapacityDenial::LeaseExpired);

        // And release, likewise, does not let a stale-but-terminal-by-time
        // holder pretend it still owned the capacity.
        let err2 = ledger
            .try_release(
                &lease.lease_id,
                lease.fence_token,
                lease.lease_epoch,
                c.epoch,
                now_ms,
            )
            .expect_err("an expired lease must also be rejected on release");
        assert_eq!(err2, CapacityDenial::LeaseExpired);
    }

    /// KNOWN-BAD PROOF 2: a lease minted under an OLD cell epoch (the cell was
    /// repartitioned/failed over) is rejected even though its timestamp has
    /// not yet expired — the classic "stale worker keeps working" defect.
    #[test]
    fn stale_epoch_holder_is_rejected_even_if_unexpired() {
        let mut ledger = CapacityLedger::new();
        let c_old = cell(1);
        let lease = ledger
            .try_acquire(CapacityAcquireRequest {
                cell: &c_old,
                lease_id: "lease-2".into(),
                work_item_id: "wi-2",
                tenant_ref: "tenant-a",
                actor_digest: "actor-a",
                resource_class: CapacityResourceClass::LlmGenerator,
                amount: 1,
                priority: LeasePriority::Interactive,
                idempotency_key: "idem-2",
                now_ms: 0,
                ttl_ms: 1_000_000, // huge TTL: NOT a timestamp-expiry story
            })
            .expect("acquire admits under capacity");

        // Cell fails over: epoch bumps from 1 -> 2.
        let cell_epoch_now = 2;
        let err = ledger
            .try_renew(
                &lease.lease_id,
                lease.fence_token,
                lease.lease_epoch,
                cell_epoch_now,
                500,
                1_000,
            )
            .expect_err("a stale-epoch holder must be rejected regardless of its timestamp TTL");
        assert_eq!(
            err,
            CapacityDenial::StaleEpoch {
                cell_epoch: 2,
                caller_epoch: 1,
            }
        );
    }

    /// KNOWN-BAD PROOF 2b: a forged/guessed fence token on an otherwise
    /// current-epoch, unexpired lease is rejected.
    #[test]
    fn wrong_fence_token_is_rejected() {
        let mut ledger = CapacityLedger::new();
        let c = cell(1);
        let lease = ledger
            .try_acquire(CapacityAcquireRequest {
                cell: &c,
                lease_id: "lease-3".into(),
                work_item_id: "wi-3",
                tenant_ref: "tenant-a",
                actor_digest: "actor-a",
                resource_class: CapacityResourceClass::LlmGenerator,
                amount: 1,
                priority: LeasePriority::Interactive,
                idempotency_key: "idem-3",
                now_ms: 0,
                ttl_ms: 10_000,
            })
            .expect("acquire admits under capacity");

        let forged = lease.fence_token + 1;
        let err = ledger
            .try_renew(
                &lease.lease_id,
                forged,
                lease.lease_epoch,
                c.epoch,
                100,
                1_000,
            )
            .expect_err("a mismatched fence token must be rejected");
        assert_eq!(err, CapacityDenial::StaleFence);
    }

    /// KNOWN-BAD PROOF 3: reclaim_expired frees capacity so a NEW acquire can
    /// succeed after the old holder's TTL lapses without ever calling release —
    /// proving the ledger doesn't require a cooperating holder to recover.
    #[test]
    fn reclaim_frees_capacity_without_cooperating_holder() {
        let mut ledger = CapacityLedger::new();
        let c = cell(1);
        // Fill the cell to capacity (10) with one greedy, never-releasing lease.
        //
        // The hog must be a priority that MAY spend the reserved floor. This
        // cell is capacity 10 / reserved_floor 2, and `BackgroundIngestion` is
        // the single priority barred from the floor (`may_spend_reserved_floor`),
        // so it can never hold more than 8 — it structurally cannot fill the
        // cell, and asking it for 10 is `Exhausted { available: 8 }`. Using it
        // here would also break this test's second step: with only 8 held, an
        // `Interactive` request for 1 would still find 2 free and be ADMITTED,
        // so the "second acquire is exhausted" assertion would be vacuous.
        ledger
            .try_acquire(CapacityAcquireRequest {
                cell: &c,
                lease_id: "hog".into(),
                work_item_id: "wi-hog",
                tenant_ref: "tenant-hog",
                actor_digest: "actor-hog",
                resource_class: CapacityResourceClass::LlmGenerator,
                amount: 10,
                priority: LeasePriority::Interactive,
                idempotency_key: "idem-hog",
                now_ms: 0,
                ttl_ms: 1_000,
            })
            .expect("fills the cell");

        // A second acquire right away is exhausted — the hog never released.
        let denied = ledger.try_acquire(CapacityAcquireRequest {
            cell: &c,
            lease_id: "second".into(),
            work_item_id: "wi-2",
            tenant_ref: "tenant-b",
            actor_digest: "actor-b",
            resource_class: CapacityResourceClass::LlmGenerator,
            amount: 1,
            priority: LeasePriority::Interactive,
            idempotency_key: "idem-2",
            now_ms: 500,
            ttl_ms: 1_000,
        });
        assert!(matches!(denied, Err(CapacityDenial::Exhausted { .. })));

        // Past TTL, reclaim_expired sweeps the hog even though it never called
        // release, and the freed capacity is immediately usable.
        let reclaimed = ledger.reclaim_expired(2_000);
        assert_eq!(reclaimed, vec!["hog".to_string()]);
        let admitted = ledger
            .try_acquire(CapacityAcquireRequest {
                cell: &c,
                lease_id: "third".into(),
                work_item_id: "wi-3",
                tenant_ref: "tenant-b",
                actor_digest: "actor-b",
                resource_class: CapacityResourceClass::LlmGenerator,
                amount: 1,
                priority: LeasePriority::Interactive,
                idempotency_key: "idem-3",
                now_ms: 2_100,
                ttl_ms: 1_000,
            })
            .expect("capacity was reclaimed, so this now admits");
        assert_eq!(admitted.amount, 1);
    }

    /// A BackgroundIngestion flood can fill every unit of SPARE capacity but can
    /// never touch the reserved_floor (2 of 10 here) — Interactive can always
    /// acquire up to the floor regardless of flood size. This is the
    /// cell-level half of the "flood cannot starve interactive" proof; the
    /// tenant-vs-tenant DRR half lives in the AU scheduler
    /// (the meristem harness).
    #[test]
    fn background_flood_cannot_touch_reserved_floor() {
        let mut ledger = CapacityLedger::new();
        let c = cell(1);
        // Flood with BackgroundIngestion up to the spare ceiling (10 - 2 = 8).
        for i in 0..8 {
            ledger
                .try_acquire(CapacityAcquireRequest {
                    cell: &c,
                    lease_id: format!("flood-{i}"),
                    work_item_id: "wi-flood",
                    tenant_ref: "tenant-flood",
                    actor_digest: "actor-flood",
                    resource_class: CapacityResourceClass::LlmGenerator,
                    amount: 1,
                    priority: LeasePriority::BackgroundIngestion,
                    idempotency_key: format!("idem-flood-{i}").as_str(),
                    now_ms: 0,
                    ttl_ms: 1_000_000,
                })
                .unwrap_or_else(|e| {
                    panic!("flood unit {i} should admit into spare capacity: {e:?}")
                });
        }
        // A 9th BackgroundIngestion unit is denied — spare is exhausted.
        let ninth = ledger.try_acquire(CapacityAcquireRequest {
            cell: &c,
            lease_id: "flood-8".into(),
            work_item_id: "wi-flood",
            tenant_ref: "tenant-flood",
            actor_digest: "actor-flood",
            resource_class: CapacityResourceClass::LlmGenerator,
            amount: 1,
            priority: LeasePriority::BackgroundIngestion,
            idempotency_key: "idem-flood-8",
            now_ms: 0,
            ttl_ms: 1_000_000,
        });
        assert!(matches!(ninth, Err(CapacityDenial::Exhausted { .. })));

        // Yet Interactive can still acquire from the untouched 2-unit floor.
        let interactive = ledger
            .try_acquire(CapacityAcquireRequest {
                cell: &c,
                lease_id: "interactive-1".into(),
                work_item_id: "wi-int",
                tenant_ref: "tenant-int",
                actor_digest: "actor-int",
                resource_class: CapacityResourceClass::LlmGenerator,
                amount: 2,
                priority: LeasePriority::Interactive,
                idempotency_key: "idem-int-1",
                now_ms: 0,
                ttl_ms: 1_000,
            })
            .expect("interactive must be able to draw the full reserved floor despite the flood");
        assert_eq!(interactive.amount, 2);
    }

    #[test]
    fn idempotent_replay_returns_same_lease_not_a_second_one() {
        let mut ledger = CapacityLedger::new();
        let c = cell(1);
        let first = ledger
            .try_acquire(CapacityAcquireRequest {
                cell: &c,
                lease_id: "lease-idem".into(),
                work_item_id: "wi-1",
                tenant_ref: "tenant-a",
                actor_digest: "actor-a",
                resource_class: CapacityResourceClass::LlmGenerator,
                amount: 1,
                priority: LeasePriority::Interactive,
                idempotency_key: "idem-shared",
                now_ms: 0,
                ttl_ms: 1_000,
            })
            .unwrap();
        let second = ledger
            .try_acquire(CapacityAcquireRequest {
                cell: &c,
                lease_id: "lease-idem-DIFFERENT-id".into(),
                work_item_id: "wi-1",
                tenant_ref: "tenant-a",
                actor_digest: "actor-a",
                resource_class: CapacityResourceClass::LlmGenerator,
                amount: 1,
                priority: LeasePriority::Interactive,
                idempotency_key: "idem-shared",
                now_ms: 1,
                ttl_ms: 1_000,
            })
            .unwrap();
        assert_eq!(first.fence_token, second.fence_token);
        assert_eq!(first.lease_id, second.lease_id);
    }

    #[test]
    fn idempotency_conflict_on_shape_mismatch() {
        let mut ledger = CapacityLedger::new();
        let c = cell(1);
        ledger
            .try_acquire(CapacityAcquireRequest {
                cell: &c,
                lease_id: "lease-a".into(),
                work_item_id: "wi-1",
                tenant_ref: "tenant-a",
                actor_digest: "actor-a",
                resource_class: CapacityResourceClass::LlmGenerator,
                amount: 1,
                priority: LeasePriority::Interactive,
                idempotency_key: "idem-shared",
                now_ms: 0,
                ttl_ms: 1_000,
            })
            .unwrap();
        let conflict = ledger.try_acquire(CapacityAcquireRequest {
            cell: &c,
            lease_id: "lease-b".into(),
            work_item_id: "wi-1",
            tenant_ref: "tenant-a",
            actor_digest: "actor-a",
            resource_class: CapacityResourceClass::LlmGenerator,
            amount: 999,
            priority: LeasePriority::Interactive,
            idempotency_key: "idem-shared",
            now_ms: 1,
            ttl_ms: 1_000,
        });
        assert!(matches!(conflict, Err(CapacityDenial::IdempotencyConflict)));
    }

    #[test]
    fn cell_validate_rejects_floor_exceeding_capacity() {
        let mut c = cell(1);
        c.reserved_floor = c.capacity + 1;
        assert!(c.validate().is_err());
    }

    #[test]
    fn lease_validate_rejects_zero_fence_token() {
        let mut c = cell(1);
        c.reserved_floor = 0;
        let mut ledger = CapacityLedger::new();
        let mut lease = ledger
            .try_acquire(CapacityAcquireRequest {
                cell: &c,
                lease_id: "lease-v".into(),
                work_item_id: "wi-1",
                tenant_ref: "tenant-a",
                actor_digest: "actor-a",
                resource_class: CapacityResourceClass::LlmGenerator,
                amount: 1,
                priority: LeasePriority::Interactive,
                idempotency_key: "idem-v",
                now_ms: 0,
                ttl_ms: 1_000,
            })
            .unwrap();
        lease.fence_token = 0;
        assert!(lease.validate().is_err());
    }

    fn slot_cell(capacity: u64, reserved_floor: u64) -> CapacityCell {
        CapacityCell {
            cell_id: "build-slots:r820".to_string(),
            resource_class: CapacityResourceClass::BuildSlot,
            capacity,
            reserved_floor,
            ..cell(1)
        }
    }

    fn slot_request<'a>(
        cell: &'a CapacityCell,
        n: u32,
        priority: LeasePriority,
        now_ms: u64,
        ttl_ms: u64,
        keys: &'a [String],
    ) -> CapacityAcquireRequest<'a> {
        CapacityAcquireRequest {
            cell,
            lease_id: format!("lease-{n}"),
            work_item_id: "wi-slot",
            tenant_ref: "tenant-a",
            actor_digest: "holder",
            resource_class: CapacityResourceClass::BuildSlot,
            amount: 1,
            priority,
            idempotency_key: &keys[n as usize],
            now_ms,
            ttl_ms,
        }
    }

    fn keys() -> Vec<String> {
        (0..16).map(|n| format!("key-{n}")).collect()
    }

    // spec: MEDULLA-PLACEMENT-R786.1
    #[test]
    fn build_slot_priorities_rank_and_only_land_gate_and_train_spend_the_floor() {
        assert!(LeasePriority::LandGate < LeasePriority::Train);
        assert!(LeasePriority::Train < LeasePriority::Lane);
        assert!(LeasePriority::LandGate.may_spend_reserved_floor());
        assert!(LeasePriority::Train.may_spend_reserved_floor());
        assert!(!LeasePriority::Lane.may_spend_reserved_floor());
        let cell = slot_cell(4, 1);
        assert_eq!(cell.available_for(LeasePriority::LandGate, 1), 3);
        assert_eq!(cell.available_for(LeasePriority::Train, 1), 3);
        assert_eq!(cell.available_for(LeasePriority::Lane, 1), 2);
        // Only the reserved slot is free: a lane is denied, the other two are admitted.
        assert_eq!(cell.available_for(LeasePriority::Lane, 3), 0);
        assert_eq!(cell.available_for(LeasePriority::LandGate, 3), 1);
        assert_eq!(cell.available_for(LeasePriority::Train, 3), 1);
        // The other resource classes refuse the three priorities, and a build-slot cell
        // refuses the older ones.
        let llm = cell_with(CapacityResourceClass::LlmGenerator);
        let keys = keys();
        let mut ledger = CapacityLedger::new();
        for priority in [
            LeasePriority::LandGate,
            LeasePriority::Train,
            LeasePriority::Lane,
        ] {
            let mut request = slot_request(&llm, 0, priority, 0, 1_000, &keys);
            request.resource_class = CapacityResourceClass::LlmGenerator;
            assert_eq!(
                ledger.try_acquire(request),
                Err(CapacityDenial::PriorityNotAllowedForResource {
                    priority,
                    resource_class: CapacityResourceClass::LlmGenerator,
                })
            );
        }
        let mut request = slot_request(&cell, 1, LeasePriority::Interactive, 0, 1_000, &keys);
        request.priority = LeasePriority::Interactive;
        assert!(matches!(
            ledger.try_acquire(request),
            Err(CapacityDenial::PriorityNotAllowedForResource { .. })
        ));
    }

    fn cell_with(resource_class: CapacityResourceClass) -> CapacityCell {
        CapacityCell {
            resource_class,
            ..cell(1)
        }
    }

    // spec: MEDULLA-PLACEMENT-R786.4
    #[test]
    fn a_lease_that_stops_renewing_expires_and_returns_its_slot() {
        let cell = slot_cell(1, 0);
        let keys = keys();
        let mut ledger = CapacityLedger::new();
        let first = ledger
            .try_acquire(slot_request(&cell, 1, LeasePriority::Lane, 0, 1_000, &keys))
            .expect("granted");
        assert_eq!(first.state, CapacityLeaseState::Granted);
        let second = ledger
            .try_acquire(slot_request(
                &cell,
                2,
                LeasePriority::Lane,
                10,
                1_000,
                &keys,
            ))
            .expect("queued");
        assert_eq!(second.state, CapacityLeaseState::Queued);
        // A heartbeat before the deadline moves the deadline; the sweep finds nothing.
        let renewed = ledger
            .try_renew(&first.lease_id, first.fence_token, 1, 1, 900, 1_000)
            .expect("renews");
        assert_eq!(renewed.expires_at_ms, 1_900);
        assert_eq!(ledger.sweep(&[&cell], 1_500), CapacitySweep::default());
        // The holder goes silent: past the deadline the sweep expires it and, in the same
        // sweep, grants the next queued lease.
        let swept = ledger.sweep(&[&cell], 2_000);
        assert_eq!(swept.expired, vec![first.lease_id.clone()]);
        assert_eq!(swept.granted, vec![second.lease_id.clone()]);
        assert_eq!(
            ledger.get(&first.lease_id).unwrap().state,
            CapacityLeaseState::Expired
        );
        let granted = ledger.get(&second.lease_id).unwrap();
        assert_eq!(granted.state, CapacityLeaseState::Granted);
        assert_eq!(
            granted.expires_at_ms, 3_000,
            "the deadline starts at the grant"
        );
        // A renew by the expired holder is the terminal-state refusal that tells it to stop.
        let refused = ledger
            .try_renew(&first.lease_id, first.fence_token, 1, 1, 2_100, 1_000)
            .unwrap_err();
        assert!(matches!(
            refused,
            CapacityDenial::TransitionRefused {
                why: TransitionRefusalReason::TerminalState,
                ..
            }
        ));
    }

    // spec: MEDULLA-PLACEMENT-R786.4
    #[test]
    fn a_queued_lease_expires_at_its_queue_deadline() {
        let cell = slot_cell(1, 0);
        let keys = keys();
        let mut ledger = CapacityLedger::new().with_queue_ttl_ms(500);
        ledger
            .try_acquire(slot_request(
                &cell,
                1,
                LeasePriority::Lane,
                0,
                10_000,
                &keys,
            ))
            .expect("granted");
        let queued = ledger
            .try_acquire(slot_request(
                &cell,
                2,
                LeasePriority::Lane,
                0,
                10_000,
                &keys,
            ))
            .expect("queued");
        assert_eq!(queued.queue_expires_at_ms, Some(500));
        assert_eq!(ledger.sweep(&[&cell], 499), CapacitySweep::default());
        assert_eq!(ledger.sweep(&[&cell], 500).expired, vec![queued.lease_id]);
    }

    // spec: MEDULLA-PLACEMENT-R786.5
    #[test]
    fn land_gate_and_train_are_granted_before_lane_and_each_class_is_fifo() {
        // Two slots, one reserved. A lane and a train fill the cell; four more queue.
        let cell = slot_cell(2, 1);
        let keys = keys();
        let mut ledger = CapacityLedger::new();
        let lane_holder = ledger
            .try_acquire(slot_request(
                &cell,
                5,
                LeasePriority::Lane,
                0,
                100_000,
                &keys,
            ))
            .unwrap();
        let train_holder = ledger
            .try_acquire(slot_request(
                &cell,
                0,
                LeasePriority::Train,
                0,
                100_000,
                &keys,
            ))
            .unwrap();
        assert_eq!(train_holder.state, CapacityLeaseState::Granted);
        assert_eq!(lane_holder.state, CapacityLeaseState::Granted);
        for (n, priority) in [
            (1, LeasePriority::Lane),
            (2, LeasePriority::Lane),
            (3, LeasePriority::Train),
            (4, LeasePriority::LandGate),
        ] {
            let lease = ledger
                .try_acquire(slot_request(
                    &cell,
                    n,
                    priority,
                    u64::from(n),
                    100_000,
                    &keys,
                ))
                .unwrap();
            assert_eq!(lease.state, CapacityLeaseState::Queued, "lease {n}");
        }
        // Everything queued is in the full order: the three classes by rank, each in
        // arrival order, whatever the arrival order across classes was.
        let wide = CapacityCell {
            capacity: 6,
            ..cell.clone()
        };
        let queued: Vec<&CapacityLease> = ledger.leases.values().collect();
        assert_eq!(
            grant_order(&wide, 1, &queued),
            ["lease-4", "lease-3", "lease-1", "lease-2"]
        );
        // One free slot (the train holder's): the land gate takes it, not the earlier lanes.
        ledger
            .try_release(&train_holder.lease_id, train_holder.fence_token, 1, 1, 10)
            .unwrap();
        assert_eq!(ledger.grant_next(&cell, 11), ["lease-4"]);
        // The land gate leaves: the train waiting behind the lanes goes first.
        let gate = ledger.get("lease-4").unwrap().clone();
        ledger
            .try_release(&gate.lease_id, gate.fence_token, 1, 1, 12)
            .unwrap();
        assert_eq!(ledger.grant_next(&cell, 13), ["lease-3"]);
        // The lane holder leaves: only the reserved slot is free and a lane never takes it.
        ledger
            .try_release(&lane_holder.lease_id, lane_holder.fence_token, 1, 1, 14)
            .unwrap();
        assert!(ledger.grant_next(&cell, 15).is_empty());
        // The train leaves: both slots are free; the non-reserved one goes to the first lane
        // and the second lane waits, because the other slot is the reserved one.
        let train = ledger.get("lease-3").unwrap().clone();
        ledger
            .try_release(&train.lease_id, train.fence_token, 1, 1, 16)
            .unwrap();
        assert_eq!(ledger.grant_next(&cell, 17), ["lease-1"]);
        assert_eq!(
            ledger.get("lease-2").unwrap().state,
            CapacityLeaseState::Queued
        );
    }

    // spec: MEDULLA-PLACEMENT-R786.5
    #[test]
    fn a_blocked_head_blocks_its_own_class_only() {
        let cell = slot_cell(2, 1);
        let keys = keys();
        let mut ledger = CapacityLedger::new();
        for n in [0, 5] {
            ledger
                .try_acquire(slot_request(
                    &cell,
                    n,
                    LeasePriority::Train,
                    0,
                    100_000,
                    &keys,
                ))
                .unwrap();
        }
        for n in [1, 2] {
            ledger
                .try_acquire(slot_request(
                    &cell,
                    n,
                    LeasePriority::Lane,
                    1,
                    100_000,
                    &keys,
                ))
                .unwrap();
        }
        let queued: Vec<&CapacityLease> = ledger.leases.values().collect();
        // Only the reserved slot is free: no lane is admitted, and the second lane does not
        // overtake the first.
        assert!(grant_order(&cell, 1, &queued).is_empty());
        // Both slots free: the non-reserved slot goes to the first lane alone.
        assert_eq!(grant_order(&cell, 0, &queued), ["lease-1"]);
    }
}
