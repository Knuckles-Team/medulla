//! The WorkItem queue row as one explicit state machine (MEDULLA-ORCH-VOCAB-R812).
//!
//! [`WORK_ITEM_MACHINE`] is the authoritative definition: eight states, four terminal
//! states and the edge table W01 to W26. [`WorkItemStatus::apply`] is the only transition
//! function a store writer calls; it runs the core generic
//! [`medulla_types::transition_refused::transition`] over this table and refuses every
//! other pair of state and event with the typed [`TransitionRefused`]. The persisted state
//! is the one `status` field holding [`WorkItemStatus::as_stored`]; terminality and lease
//! liveness are derived, never stored.
//!
//! Lease shape: Leased and Running mean the lease is active. Commit, Defer and Cancel
//! release it; LeaseExpire reclaims it. Expiry is derived from `lease_expires_at` and the
//! clock; no WorkItem state is named Expired.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, StateMachineApply, TransitionRefused,
};
use serde::{Deserialize, Serialize};

pub use crate::work_item_read::WorkItemStatus;

/// The machine instance name: the `machine` field of every WorkItem refusal and the
/// local name of the ontology instance `:WorkItemMachine`.
pub const WORK_ITEM_MACHINE_NAME: &str = "WorkItemMachine";

/// How a commit ended, as the commit event carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommitOutcome {
    /// The work succeeded.
    Succeeded,
    /// The work failed and must not be retried.
    FailedFinal,
    /// The work failed and may be retried while attempts remain.
    FailedRetryable,
    /// The worker cancelled the work.
    Cancelled,
}

impl CommitOutcome {
    /// The commit outcome of a `CommitWorkItemResult` outcome verb and retry flag.
    pub fn from_commit(outcome: &str, retryable: bool) -> Option<Self> {
        match (outcome, retryable) {
            ("succeeded", _) => Some(Self::Succeeded),
            ("cancelled", _) => Some(Self::Cancelled),
            ("failed", false) => Some(Self::FailedFinal),
            ("failed", true) => Some(Self::FailedRetryable),
            _ => None,
        }
    }
}

/// Every event of the WorkItem machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkItemEvent {
    /// The last open dependency cleared.
    DepsSatisfied,
    /// A worker claimed the item.
    Claim,
    /// The lease holder's first heartbeat: the work started.
    Start,
    /// A running lease holder renewed its lease.
    Heartbeat,
    /// The lease holder committed an outcome.
    Commit(CommitOutcome),
    /// The item is cancelled now.
    Cancel,
    /// A cancel reached the item while its lease was live; it is stamped only.
    RequestCancel,
    /// The lease holder handed the item back for a later retry.
    Defer,
    /// The lease lapsed and the claim sweep reclaimed it.
    LeaseExpire,
}

impl WorkItemEvent {
    /// Every event, in declaration order.
    pub const ALL: [Self; 12] = [
        Self::DepsSatisfied,
        Self::Claim,
        Self::Start,
        Self::Heartbeat,
        Self::Commit(CommitOutcome::Succeeded),
        Self::Commit(CommitOutcome::FailedFinal),
        Self::Commit(CommitOutcome::FailedRetryable),
        Self::Commit(CommitOutcome::Cancelled),
        Self::Cancel,
        Self::RequestCancel,
        Self::Defer,
        Self::LeaseExpire,
    ];

    /// The ontology local name of this event: `:WorkItemMachine_Event_<this>`.
    pub fn ontology_name(self) -> &'static str {
        match self {
            Self::DepsSatisfied => "DepsSatisfied",
            Self::Claim => "Claim",
            Self::Start => "Start",
            Self::Heartbeat => "Heartbeat",
            Self::Commit(CommitOutcome::Succeeded) => "CommitSucceeded",
            Self::Commit(CommitOutcome::FailedFinal) => "CommitFailedFinal",
            Self::Commit(CommitOutcome::FailedRetryable) => "CommitFailedRetryable",
            Self::Commit(CommitOutcome::Cancelled) => "CommitCancelled",
            Self::Cancel => "Cancel",
            Self::RequestCancel => "RequestCancel",
            Self::Defer => "Defer",
            Self::LeaseExpire => "LeaseExpire",
        }
    }
}

impl MachineEvent for WorkItemEvent {
    fn name(self) -> &'static str {
        match self {
            Self::DepsSatisfied => "deps_satisfied",
            Self::Claim => "claim",
            Self::Start => "start",
            Self::Heartbeat => "heartbeat",
            Self::Commit(CommitOutcome::Succeeded) => "commit_succeeded",
            Self::Commit(CommitOutcome::FailedFinal) => "commit_failed_final",
            Self::Commit(CommitOutcome::FailedRetryable) => "commit_failed_retryable",
            Self::Commit(CommitOutcome::Cancelled) => "commit_cancelled",
            Self::Cancel => "cancel",
            Self::RequestCancel => "request_cancel",
            Self::Defer => "defer",
            Self::LeaseExpire => "lease_expire",
        }
    }
}

impl MachineState for WorkItemStatus {
    fn stored(self) -> &'static str {
        self.as_stored()
    }
}

/// Every guard of the WorkItem machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkItemGuard {
    /// No dependency is open.
    NoOpenDependency,
    /// The next retry time has passed and an attempt remains.
    RetryDueAttemptLeft,
    /// The owner, epoch and fence match and the lease is live.
    FenceLive,
    /// [`Self::FenceLive`] and attempts are below the maximum.
    FenceLiveAttemptLeft,
    /// [`Self::FenceLive`] and attempts reached the maximum.
    FenceLiveAttemptsSpent,
    /// [`Self::FenceLive`] and neither the epoch nor the fence counter overflows.
    FenceLiveCountersOk,
    /// The lease is live.
    LeaseLive,
    /// The lease has expired.
    LeaseExpired,
    /// The lease has expired and an attempt remains.
    LeaseExpiredAttemptLeft,
    /// The lease has expired and attempts reached the maximum.
    LeaseExpiredAttemptsSpent,
}

impl WorkItemGuard {
    /// The ontology local name of this guard: `:WorkItemMachine_Guard_<this>`.
    pub fn ontology_name(self) -> &'static str {
        match self {
            Self::NoOpenDependency => "NoOpenDependency",
            Self::RetryDueAttemptLeft => "RetryDueAttemptLeft",
            Self::FenceLive => "FenceLive",
            Self::FenceLiveAttemptLeft => "FenceLiveAttemptLeft",
            Self::FenceLiveAttemptsSpent => "FenceLiveAttemptsSpent",
            Self::FenceLiveCountersOk => "FenceLiveCountersOk",
            Self::LeaseLive => "LeaseLive",
            Self::LeaseExpired => "LeaseExpired",
            Self::LeaseExpiredAttemptLeft => "LeaseExpiredAttemptLeft",
            Self::LeaseExpiredAttemptsSpent => "LeaseExpiredAttemptsSpent",
        }
    }

    /// Whether this guard holds under `facts`.
    pub fn holds(self, facts: &WorkItemFacts) -> bool {
        let fence_live = facts.fence_matches && facts.lease_live;
        match self {
            Self::NoOpenDependency => facts.open_dependencies == 0,
            Self::RetryDueAttemptLeft => facts.retry_due && facts.reclaim_attempt_left(),
            Self::FenceLive => fence_live,
            Self::FenceLiveAttemptLeft => fence_live && facts.attempt_left(),
            Self::FenceLiveAttemptsSpent => fence_live && !facts.attempt_left(),
            Self::FenceLiveCountersOk => fence_live && facts.counters_ok,
            Self::LeaseLive => facts.lease_live,
            Self::LeaseExpired => !facts.lease_live,
            Self::LeaseExpiredAttemptLeft => !facts.lease_live && facts.reclaim_attempt_left(),
            Self::LeaseExpiredAttemptsSpent => !facts.lease_live && !facts.reclaim_attempt_left(),
        }
    }
}

/// The facts the WorkItem guards read. The store writer computes them from the row and
/// the caller's request; the machine never reads a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkItemFacts {
    /// Dependencies not yet succeeded.
    pub open_dependencies: u64,
    /// The next retry time has passed.
    pub retry_due: bool,
    /// Attempts started so far.
    pub attempts: u64,
    /// The attempt ceiling (at least one).
    pub max_attempts: u64,
    /// The item's external effect may have happened (MEDULLA-DURABLE-KERNEL-R820.2). Such
    /// an item keeps its attempt on reclaim and is never dead-lettered by a lease lapse.
    pub effect_started: bool,
    /// The caller's owner, epoch and fence match the row.
    pub fence_matches: bool,
    /// The lease has not expired.
    pub lease_live: bool,
    /// Neither the lease epoch nor the fencing token overflows when advanced.
    pub counters_ok: bool,
}

impl WorkItemFacts {
    /// Attempts are below the maximum.
    pub fn attempt_left(&self) -> bool {
        self.attempts < self.max_attempts.max(1)
    }

    /// An attempt remains for a reclaim or a claim: below the maximum, or the effect
    /// started (the reclaim continues the same attempt).
    pub fn reclaim_attempt_left(&self) -> bool {
        self.effect_started || self.attempt_left()
    }
}

/// Why a WorkItem reached a terminal state: the vocabulary consumers map their own
/// terminal states onto (MEDULLA-ORCH-VOCAB-R812.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum WorkItemTerminalReason {
    /// The work succeeded.
    Succeeded,
    /// The work failed.
    Failed,
    /// The work was cancelled.
    Cancelled,
    /// A retryable failure exhausted its attempts.
    RetriesExhausted,
}

use CommitOutcome as C;
use WorkItemEvent as Ev;
use WorkItemGuard as G;
use WorkItemStatus as S;

const fn edge(id: &'static str, from: S, event: Ev, guard: Option<G>, to: S) -> Edge<S, Ev, G> {
    Edge {
        id,
        from,
        event,
        guard,
        to,
    }
}

/// The WorkItem edge table W01 to W26, in evaluation order.
pub const WORK_ITEM_EDGES: &[Edge<S, Ev, G>] = &[
    edge(
        "W01",
        S::Submitted,
        Ev::DepsSatisfied,
        Some(G::NoOpenDependency),
        S::Ready,
    ),
    edge(
        "W02",
        S::Ready,
        Ev::Claim,
        Some(G::RetryDueAttemptLeft),
        S::Leased,
    ),
    edge("W03", S::Leased, Ev::Start, Some(G::FenceLive), S::Running),
    edge(
        "W04",
        S::Running,
        Ev::Heartbeat,
        Some(G::FenceLive),
        S::Running,
    ),
    edge(
        "W05",
        S::Running,
        Ev::Commit(C::Succeeded),
        Some(G::FenceLive),
        S::Succeeded,
    ),
    edge(
        "W06",
        S::Running,
        Ev::Commit(C::FailedFinal),
        Some(G::FenceLive),
        S::Failed,
    ),
    edge(
        "W07",
        S::Running,
        Ev::Commit(C::Cancelled),
        Some(G::FenceLive),
        S::Cancelled,
    ),
    edge(
        "W08",
        S::Running,
        Ev::Commit(C::FailedRetryable),
        Some(G::FenceLiveAttemptLeft),
        S::Ready,
    ),
    edge(
        "W09",
        S::Running,
        Ev::Commit(C::FailedRetryable),
        Some(G::FenceLiveAttemptsSpent),
        S::DeadLetter,
    ),
    edge(
        "W10",
        S::Leased,
        Ev::Commit(C::Succeeded),
        Some(G::FenceLive),
        S::Succeeded,
    ),
    edge(
        "W11",
        S::Leased,
        Ev::Commit(C::FailedFinal),
        Some(G::FenceLive),
        S::Failed,
    ),
    edge(
        "W12",
        S::Leased,
        Ev::Commit(C::Cancelled),
        Some(G::FenceLive),
        S::Cancelled,
    ),
    edge(
        "W13",
        S::Leased,
        Ev::Commit(C::FailedRetryable),
        Some(G::FenceLiveAttemptLeft),
        S::Ready,
    ),
    edge(
        "W14",
        S::Leased,
        Ev::Commit(C::FailedRetryable),
        Some(G::FenceLiveAttemptsSpent),
        S::DeadLetter,
    ),
    edge("W15", S::Submitted, Ev::Cancel, None, S::Cancelled),
    edge("W16", S::Ready, Ev::Cancel, None, S::Cancelled),
    edge(
        "W17",
        S::Leased,
        Ev::Cancel,
        Some(G::LeaseExpired),
        S::Cancelled,
    ),
    edge(
        "W18",
        S::Running,
        Ev::Cancel,
        Some(G::LeaseExpired),
        S::Cancelled,
    ),
    edge(
        "W19",
        S::Leased,
        Ev::RequestCancel,
        Some(G::LeaseLive),
        S::Leased,
    ),
    edge(
        "W20",
        S::Running,
        Ev::RequestCancel,
        Some(G::LeaseLive),
        S::Running,
    ),
    edge(
        "W21",
        S::Leased,
        Ev::Defer,
        Some(G::FenceLiveCountersOk),
        S::Ready,
    ),
    edge(
        "W22",
        S::Running,
        Ev::Defer,
        Some(G::FenceLiveCountersOk),
        S::Ready,
    ),
    edge(
        "W23",
        S::Leased,
        Ev::LeaseExpire,
        Some(G::LeaseExpiredAttemptLeft),
        S::Ready,
    ),
    edge(
        "W24",
        S::Running,
        Ev::LeaseExpire,
        Some(G::LeaseExpiredAttemptLeft),
        S::Ready,
    ),
    edge(
        "W25",
        S::Leased,
        Ev::LeaseExpire,
        Some(G::LeaseExpiredAttemptsSpent),
        S::DeadLetter,
    ),
    edge(
        "W26",
        S::Running,
        Ev::LeaseExpire,
        Some(G::LeaseExpiredAttemptsSpent),
        S::DeadLetter,
    ),
];

/// Every WorkItem state, in declaration order.
pub const WORK_ITEM_STATES: [S; 8] = [
    S::Submitted,
    S::Ready,
    S::Leased,
    S::Running,
    S::Succeeded,
    S::Failed,
    S::Cancelled,
    S::DeadLetter,
];

/// The authoritative WorkItem machine definition.
pub const WORK_ITEM_MACHINE: MachineDef<S, Ev, G> = MachineDef {
    name: WORK_ITEM_MACHINE_NAME,
    states: &WORK_ITEM_STATES,
    initial: S::Submitted,
    terminals: &[S::Succeeded, S::Failed, S::Cancelled, S::DeadLetter],
    events: &WorkItemEvent::ALL,
    guards: &[
        G::NoOpenDependency,
        G::RetryDueAttemptLeft,
        G::FenceLive,
        G::FenceLiveAttemptLeft,
        G::FenceLiveAttemptsSpent,
        G::FenceLiveCountersOk,
        G::LeaseLive,
        G::LeaseExpired,
        G::LeaseExpiredAttemptLeft,
        G::LeaseExpiredAttemptsSpent,
    ],
    edges: WORK_ITEM_EDGES,
};

/// A stored `status` text that names no WorkItem state, such as the retired `completed`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("WORK_ITEM_STATUS_UNKNOWN: WorkItem carries an unrecognized status '{stored}'")]
pub struct UnknownWorkItemStatus {
    /// The stored text.
    pub stored: String,
}

impl WorkItemStatus {
    /// The state a submitted item is admitted in. The machine starts in Submitted; with no
    /// open dependency, W01 (DepsSatisfied) fires at admission and the item is Ready.
    pub fn initial(open_dependencies: u64) -> Self {
        let facts = WorkItemFacts {
            open_dependencies,
            ..WorkItemFacts::default()
        };
        WORK_ITEM_MACHINE
            .initial
            .apply(WorkItemEvent::DepsSatisfied, &facts)
            .unwrap_or(WORK_ITEM_MACHINE.initial)
    }

    /// The only terminal test: a terminal state accepts no event.
    pub fn is_terminal(self) -> bool {
        WORK_ITEM_MACHINE.is_terminal(self)
    }

    /// Leased or Running: the lease is active.
    pub fn holds_lease(self) -> bool {
        matches!(self, Self::Leased | Self::Running)
    }

    /// Whether a stored `status` text names a terminal state. An unknown text does not.
    pub fn is_terminal_text(stored: &str) -> bool {
        Self::from_stored(stored).is_some_and(Self::is_terminal)
    }

    /// Whether a stored `status` text names a lease-holding state (Leased or Running).
    pub fn holds_lease_text(stored: &str) -> bool {
        Self::from_stored(stored).is_some_and(Self::holds_lease)
    }

    /// Why this state is terminal, or `None` for a live state.
    pub fn terminal_reason(self) -> Option<WorkItemTerminalReason> {
        match self {
            Self::Succeeded => Some(WorkItemTerminalReason::Succeeded),
            Self::Failed => Some(WorkItemTerminalReason::Failed),
            Self::Cancelled => Some(WorkItemTerminalReason::Cancelled),
            Self::DeadLetter => Some(WorkItemTerminalReason::RetriesExhausted),
            Self::Submitted | Self::Ready | Self::Leased | Self::Running => None,
        }
    }

    /// Decode a stored `status`; an unknown text is a typed error, never a default.
    pub fn decode_stored(stored: &str) -> Result<Self, UnknownWorkItemStatus> {
        Self::from_stored(stored).ok_or_else(|| UnknownWorkItemStatus {
            stored: stored.to_string(),
        })
    }

    /// The one transition function: the next state for `event` under `facts`, or the
    /// typed refusal.
    pub fn apply(
        self,
        event: WorkItemEvent,
        facts: &WorkItemFacts,
    ) -> Result<Self, TransitionRefused> {
        self.apply_edge(event, facts).map(|edge| edge.to)
    }

    /// As [`Self::apply`], returning the edge that fired.
    pub fn apply_edge(
        self,
        event: WorkItemEvent,
        facts: &WorkItemFacts,
    ) -> Result<&'static Edge<S, Ev, G>, TransitionRefused> {
        transition(&WORK_ITEM_MACHINE, self, event, |guard| guard.holds(facts))
    }
}

impl StateMachineApply for WorkItemStatus {
    type Event = WorkItemEvent;
    type Facts = WorkItemFacts;

    fn apply(
        &self,
        event: WorkItemEvent,
        facts: &WorkItemFacts,
    ) -> Result<Self, TransitionRefused> {
        WorkItemStatus::apply(*self, event, facts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::transition_refused::TransitionRefusalReason as R;

    /// Facts under which every guard of a live leased item holds, except attempts.
    fn live() -> WorkItemFacts {
        WorkItemFacts {
            open_dependencies: 0,
            retry_due: true,
            attempts: 1,
            max_attempts: 3,
            effect_started: false,
            fence_matches: true,
            lease_live: true,
            counters_ok: true,
        }
    }

    fn spent() -> WorkItemFacts {
        WorkItemFacts {
            attempts: 3,
            ..live()
        }
    }

    fn expired() -> WorkItemFacts {
        WorkItemFacts {
            lease_live: false,
            ..live()
        }
    }

    fn fires(from: S, event: Ev, facts: WorkItemFacts, id: &str, to: S) {
        let edge = from.apply_edge(event, &facts).expect("the edge fires");
        assert_eq!((edge.id, edge.to), (id, to));
        assert_eq!(from.apply(event, &facts), Ok(to));
    }

    fn refused(from: S, event: Ev, facts: WorkItemFacts) -> R {
        from.apply(event, &facts).expect_err("refused").reason
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w01_submitted_deps_satisfied() {
        fires(S::Submitted, Ev::DepsSatisfied, live(), "W01", S::Ready);
        let open = WorkItemFacts {
            open_dependencies: 1,
            ..live()
        };
        assert_eq!(
            refused(S::Submitted, Ev::DepsSatisfied, open),
            R::GuardFailed
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w02_ready_claim() {
        fires(S::Ready, Ev::Claim, live(), "W02", S::Leased);
        let backoff = WorkItemFacts {
            retry_due: false,
            ..live()
        };
        assert_eq!(refused(S::Ready, Ev::Claim, backoff), R::GuardFailed);
        assert_eq!(refused(S::Ready, Ev::Claim, spent()), R::GuardFailed);
        let started = WorkItemFacts {
            effect_started: true,
            ..spent()
        };
        fires(S::Ready, Ev::Claim, started, "W02", S::Leased);
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w03_leased_start() {
        fires(S::Leased, Ev::Start, live(), "W03", S::Running);
        assert_eq!(refused(S::Leased, Ev::Start, expired()), R::GuardFailed);
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w04_running_heartbeat() {
        fires(S::Running, Ev::Heartbeat, live(), "W04", S::Running);
        let fenced = WorkItemFacts {
            fence_matches: false,
            ..live()
        };
        assert_eq!(refused(S::Running, Ev::Heartbeat, fenced), R::GuardFailed);
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w05_running_commit_succeeded() {
        fires(
            S::Running,
            Ev::Commit(C::Succeeded),
            live(),
            "W05",
            S::Succeeded,
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w06_running_commit_failed_final() {
        fires(
            S::Running,
            Ev::Commit(C::FailedFinal),
            spent(),
            "W06",
            S::Failed,
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w07_running_commit_cancelled() {
        fires(
            S::Running,
            Ev::Commit(C::Cancelled),
            live(),
            "W07",
            S::Cancelled,
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w08_running_commit_failed_retryable() {
        fires(
            S::Running,
            Ev::Commit(C::FailedRetryable),
            live(),
            "W08",
            S::Ready,
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w09_running_commit_failed_retryable() {
        fires(
            S::Running,
            Ev::Commit(C::FailedRetryable),
            spent(),
            "W09",
            S::DeadLetter,
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w10_leased_commit_succeeded() {
        fires(
            S::Leased,
            Ev::Commit(C::Succeeded),
            live(),
            "W10",
            S::Succeeded,
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w11_leased_commit_failed_final() {
        fires(
            S::Leased,
            Ev::Commit(C::FailedFinal),
            live(),
            "W11",
            S::Failed,
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w12_leased_commit_cancelled() {
        fires(
            S::Leased,
            Ev::Commit(C::Cancelled),
            live(),
            "W12",
            S::Cancelled,
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w13_leased_commit_failed_retryable() {
        fires(
            S::Leased,
            Ev::Commit(C::FailedRetryable),
            live(),
            "W13",
            S::Ready,
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w14_leased_commit_failed_retryable() {
        fires(
            S::Leased,
            Ev::Commit(C::FailedRetryable),
            spent(),
            "W14",
            S::DeadLetter,
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w15_submitted_cancel() {
        fires(
            S::Submitted,
            Ev::Cancel,
            WorkItemFacts::default(),
            "W15",
            S::Cancelled,
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w16_ready_cancel() {
        fires(
            S::Ready,
            Ev::Cancel,
            WorkItemFacts::default(),
            "W16",
            S::Cancelled,
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w17_leased_cancel() {
        fires(S::Leased, Ev::Cancel, expired(), "W17", S::Cancelled);
        assert_eq!(refused(S::Leased, Ev::Cancel, live()), R::GuardFailed);
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w18_running_cancel() {
        fires(S::Running, Ev::Cancel, expired(), "W18", S::Cancelled);
        assert_eq!(refused(S::Running, Ev::Cancel, live()), R::GuardFailed);
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w19_leased_request_cancel() {
        fires(S::Leased, Ev::RequestCancel, live(), "W19", S::Leased);
        assert_eq!(
            refused(S::Leased, Ev::RequestCancel, expired()),
            R::GuardFailed
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w20_running_request_cancel() {
        fires(S::Running, Ev::RequestCancel, live(), "W20", S::Running);
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w21_leased_defer() {
        fires(S::Leased, Ev::Defer, live(), "W21", S::Ready);
        let overflow = WorkItemFacts {
            counters_ok: false,
            ..live()
        };
        assert_eq!(refused(S::Leased, Ev::Defer, overflow), R::GuardFailed);
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w22_running_defer() {
        fires(S::Running, Ev::Defer, live(), "W22", S::Ready);
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w23_leased_lease_expire_retry() {
        fires(S::Leased, Ev::LeaseExpire, expired(), "W23", S::Ready);
        assert_eq!(refused(S::Leased, Ev::LeaseExpire, live()), R::GuardFailed);
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w24_running_lease_expire_retry() {
        fires(S::Running, Ev::LeaseExpire, expired(), "W24", S::Ready);
        // A started effect keeps its attempt and is never dead-lettered by a lapse.
        let started = WorkItemFacts {
            effect_started: true,
            lease_live: false,
            ..spent()
        };
        fires(S::Running, Ev::LeaseExpire, started, "W24", S::Ready);
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w25_leased_lease_expire_exhausted() {
        let lapsed = WorkItemFacts {
            lease_live: false,
            ..spent()
        };
        fires(S::Leased, Ev::LeaseExpire, lapsed, "W25", S::DeadLetter);
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn edge_w26_running_lease_expire_exhausted() {
        let lapsed = WorkItemFacts {
            lease_live: false,
            ..spent()
        };
        fires(S::Running, Ev::LeaseExpire, lapsed, "W26", S::DeadLetter);
    }

    /// An independent reading of the row text, used as the oracle for the exhaustive
    /// refusal test: the state each pair leads to, or `None` when it is refused.
    fn oracle(from: S, event: Ev, f: &WorkItemFacts) -> Option<S> {
        let fence_live = f.fence_matches && f.lease_live;
        let left = f.attempts < f.max_attempts.max(1);
        let reclaim_left = left || f.effect_started;
        let leased = matches!(from, S::Leased | S::Running);
        match (from, event) {
            (S::Submitted, Ev::DepsSatisfied) if f.open_dependencies == 0 => Some(S::Ready),
            (S::Ready, Ev::Claim) if f.retry_due && reclaim_left => Some(S::Leased),
            (S::Leased, Ev::Start) | (S::Running, Ev::Heartbeat) if fence_live => Some(S::Running),
            (_, Ev::Commit(outcome)) if leased && fence_live => Some(match outcome {
                C::Succeeded => S::Succeeded,
                C::FailedFinal => S::Failed,
                C::Cancelled => S::Cancelled,
                C::FailedRetryable if left => S::Ready,
                C::FailedRetryable => S::DeadLetter,
            }),
            (S::Submitted | S::Ready, Ev::Cancel) => Some(S::Cancelled),
            (_, Ev::Cancel) if leased && !f.lease_live => Some(S::Cancelled),
            (_, Ev::RequestCancel) if leased && f.lease_live => Some(from),
            (_, Ev::Defer) if leased && fence_live && f.counters_ok => Some(S::Ready),
            (_, Ev::LeaseExpire) if leased && !f.lease_live && reclaim_left => Some(S::Ready),
            (_, Ev::LeaseExpire) if leased && !f.lease_live => Some(S::DeadLetter),
            _ => None,
        }
    }

    fn every_facts() -> Vec<WorkItemFacts> {
        let mut all = Vec::new();
        for bits in 0u32..(1 << 7) {
            let bit = |n: u32| bits & (1 << n) != 0;
            all.push(WorkItemFacts {
                open_dependencies: u64::from(bit(0)),
                retry_due: bit(1),
                attempts: if bit(2) { 3 } else { 1 },
                max_attempts: 3,
                effect_started: bit(3),
                fence_matches: bit(4),
                lease_live: bit(5),
                counters_ok: bit(6),
            });
        }
        all
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn every_illegal_pair_is_refused() {
        let mut refusals = 0usize;
        for from in WORK_ITEM_STATES {
            for event in WorkItemEvent::ALL {
                for facts in every_facts() {
                    let got = from.apply(event, &facts);
                    assert_eq!(
                        got.as_ref().ok().copied(),
                        oracle(from, event, &facts),
                        "{from:?} on {event:?} under {facts:?}"
                    );
                    if let Err(refusal) = got {
                        refusals += 1;
                        let expected = if from.is_terminal() {
                            R::TerminalState
                        } else if WORK_ITEM_EDGES
                            .iter()
                            .any(|e| e.from == from && e.event == event)
                        {
                            R::GuardFailed
                        } else {
                            R::IllegalTransition
                        };
                        assert_eq!(refusal.reason, expected, "{from:?} on {event:?}");
                    }
                }
            }
        }
        assert!(refusals > 0);
        // Every edge id is unique and every edge is reachable under some facts.
        let mut ids: Vec<&str> = WORK_ITEM_EDGES.iter().map(|e| e.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), 26);
        for edge in WORK_ITEM_EDGES {
            assert!(
                every_facts().iter().any(|f| edge
                    .from
                    .apply_edge(edge.event, f)
                    .is_ok_and(|e| e.id == edge.id)),
                "edge {} never fires",
                edge.id
            );
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn refusal_carries_machine_from_event() {
        let refusal = S::Cancelled
            .apply(Ev::Commit(C::Succeeded), &live())
            .unwrap_err();
        assert_eq!(refusal.machine, "WorkItemMachine");
        assert_eq!(refusal.from, "cancelled");
        assert_eq!(refusal.event, "commit_succeeded");
        assert_eq!(refusal.reason, R::TerminalState);
        assert_eq!(refusal.code().as_str(), "TRANSITION_REFUSED");
        let refusal = S::Ready.apply(Ev::Start, &live()).unwrap_err();
        assert_eq!(
            (refusal.from.as_str(), refusal.reason),
            ("ready", R::IllegalTransition)
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.1
    #[test]
    fn no_writer_sets_state_outside_apply() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../medulla-work-store/src/redb_store/work_item");
        let offenders = raw_status_writers(&dir);
        assert!(offenders.is_empty(), "raw status writes: {offenders:?}");
    }

    /// Source files under `dir` that insert a `status` literal instead of the stored text
    /// of a state `apply` returned.
    pub(crate) fn raw_status_writers(dir: &std::path::Path) -> Vec<String> {
        let mut offenders = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(path) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                // Production code only: everything before the first test module.
                let production = text.split("#[cfg(test)]").next().unwrap_or_default();
                for (n, line) in production.lines().enumerate() {
                    let compact: String = line.split_whitespace().collect();
                    if compact.contains("insert(\"status\".into(),serde_json::Value::String(\"") {
                        offenders.push(format!("{}:{}", path.display(), n + 1));
                    }
                }
            }
        }
        offenders
    }

    // spec: MEDULLA-ORCH-VOCAB-R812
    #[test]
    fn every_transition_and_refusal_is_covered() {
        // One named edge test exists per edge (W01 to W26), and the exhaustive test covers
        // every refusal; this asserts the table those tests name.
        let ids: Vec<&str> = WORK_ITEM_EDGES.iter().map(|e| e.id).collect();
        let expected: Vec<String> = (1..=26).map(|n| format!("W{n:02}")).collect();
        assert_eq!(ids, expected);
        assert_eq!(WORK_ITEM_MACHINE.states.len(), 8);
        assert_eq!(WORK_ITEM_MACHINE.terminals.len(), 4);
        for terminal in WORK_ITEM_MACHINE.terminals {
            assert_eq!(WORK_ITEM_MACHINE.edges_from(*terminal).count(), 0);
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.5
    #[test]
    fn exported_vocabulary_is_stable() {
        let stored: Vec<&str> = WORK_ITEM_STATES.iter().map(|s| s.as_stored()).collect();
        assert_eq!(
            stored,
            [
                "submitted",
                "ready",
                "leased",
                "running",
                "succeeded",
                "failed",
                "cancelled",
                "dead_letter"
            ]
        );
        for state in WORK_ITEM_STATES {
            assert_eq!(serde_json::to_value(state).unwrap(), state.as_stored());
            assert_eq!(WorkItemStatus::decode_stored(state.as_stored()), Ok(state));
        }
        let terminal: Vec<S> = WORK_ITEM_STATES
            .into_iter()
            .filter(|s| s.is_terminal())
            .collect();
        assert_eq!(
            terminal,
            [S::Succeeded, S::Failed, S::Cancelled, S::DeadLetter]
        );
        let names: Vec<&str> = WorkItemEvent::ALL.iter().map(|e| e.name()).collect();
        assert_eq!(
            names,
            [
                "deps_satisfied",
                "claim",
                "start",
                "heartbeat",
                "commit_succeeded",
                "commit_failed_final",
                "commit_failed_retryable",
                "commit_cancelled",
                "cancel",
                "request_cancel",
                "defer",
                "lease_expire",
            ]
        );
        assert_eq!(WorkItemStatus::initial(0), S::Ready);
        assert_eq!(WorkItemStatus::initial(2), S::Submitted);
        // The retired literal is refused with the typed decode error.
        assert!(WorkItemStatus::decode_stored("completed").is_err());
    }

    // spec: MEDULLA-ORCH-VOCAB-R812.5
    #[test]
    fn terminal_reason_covers_every_terminal_state() {
        for state in WORK_ITEM_STATES {
            assert_eq!(
                state.terminal_reason().is_some(),
                state.is_terminal(),
                "{state:?}"
            );
        }
        assert_eq!(
            S::DeadLetter.terminal_reason(),
            Some(WorkItemTerminalReason::RetriesExhausted)
        );
        let texts: Vec<serde_json::Value> = [S::Succeeded, S::Failed, S::Cancelled, S::DeadLetter]
            .iter()
            .map(|s| serde_json::to_value(s.terminal_reason().unwrap()).unwrap())
            .collect();
        assert_eq!(
            texts,
            ["succeeded", "failed", "cancelled", "retries_exhausted"]
        );
    }
}
