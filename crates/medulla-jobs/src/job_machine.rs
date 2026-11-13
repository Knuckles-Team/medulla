//! MEDULLA-CQ-ORCH-R811.1 — the analytics job as one explicit state machine.
//!
//! `JobStage::step` is the one transition function of `AnalyticsJobMachine`. The edge table is
//! J01 to J15. A failure with retries left moves to Backoff and a failure with retries exhausted
//! moves to Failed; a Failed job never revives. Reclaim of a cancel-requested job ends in
//! Cancelled, so the cancel request is kept by the state itself. The lease belongs to Running,
//! Publishing and CancelRequested and `not_before` belongs to Backoff: the guards read them as
//! [`JobFacts`], so no flag beside the state decides an edge.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};

/// The state of one analytics job. Its stored text is the `:storedForm` of the ontology instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStage {
    /// The `submitted` state: queued, waiting for a worker.
    Submitted,
    /// The `running` state: a worker holds the lease.
    Running,
    /// The `publishing` state: the typed result is durable, the claims are not yet committed.
    Publishing,
    /// The `cancel_requested` state: the lease holder is asked to stop.
    CancelRequested,
    /// The `backoff` state: a failed attempt waits for `not_before`.
    Backoff,
    /// The `succeeded` state.
    Succeeded,
    /// The `failed` state: retries are exhausted. It never revives.
    Failed,
    /// The `cancelled` state.
    Cancelled,
}

/// An event of `AnalyticsJobMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JobEvent {
    /// The `Claim` event: a worker takes a lease.
    Claim,
    /// The `Publish` event: the typed result is staged.
    Publish,
    /// The `Complete` event: the claims committed.
    Complete,
    /// The `Fail` event: an attempt failed; the guards pick retry or exhausted.
    Fail,
    /// The `Cancel` event: cancel a job nobody holds.
    Cancel,
    /// The `RequestCancel` event: ask the lease holder to stop.
    RequestCancel,
    /// The `ConfirmCancel` event: the lease holder confirms it stopped.
    ConfirmCancel,
    /// The `Reclaim` event: the lease lapsed.
    Reclaim,
}

impl MachineState for JobStage {
    fn stored(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Running => "running",
            Self::Publishing => "publishing",
            Self::CancelRequested => "cancel_requested",
            Self::Backoff => "backoff",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl MachineEvent for JobEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Claim => "Claim",
            Self::Publish => "Publish",
            Self::Complete => "Complete",
            Self::Fail => "Fail",
            Self::Cancel => "Cancel",
            Self::RequestCancel => "RequestCancel",
            Self::ConfirmCancel => "ConfirmCancel",
            Self::Reclaim => "Reclaim",
        }
    }
}

/// Guard of `AnalyticsJobMachine`: a worker takes a lease.
pub const JOB_GUARD_WORKER_TAKES_LEASE: &str = "WorkerTakesLease";
/// Guard of `AnalyticsJobMachine`: `not_before` has passed and a worker takes a lease.
pub const JOB_GUARD_NOT_BEFORE_PASSED_WORKER_TAKES_LEASE: &str = "NotBeforePassedWorkerTakesLease";
/// Guard of `AnalyticsJobMachine`: the lease owner matches and the lease is live.
pub const JOB_GUARD_LEASE_OWNER_MATCHES_LEASE_LIVE: &str = "LeaseOwnerMatchesLeaseLive";
/// Guard of `AnalyticsJobMachine`: attempts are below the maximum.
pub const JOB_GUARD_ATTEMPTS_BELOW_MAX: &str = "AttemptsBelowMax";
/// Guard of `AnalyticsJobMachine`: attempts reached the maximum.
pub const JOB_GUARD_ATTEMPTS_REACHED_MAX: &str = "AttemptsReachedMax";
/// Guard of `AnalyticsJobMachine`: the lease is live.
pub const JOB_GUARD_LEASE_LIVE: &str = "LeaseLive";
/// Guard of `AnalyticsJobMachine`: the lease owner matches.
pub const JOB_GUARD_LEASE_OWNER_MATCHES: &str = "LeaseOwnerMatches";
/// Guard of `AnalyticsJobMachine`: the lease has lapsed.
pub const JOB_GUARD_LEASE_LAPSED: &str = "LeaseLapsed";

const fn edge(
    id: &'static str,
    from: JobStage,
    event: JobEvent,
    guard: Option<&'static str>,
    to: JobStage,
) -> Edge<JobStage, JobEvent, &'static str> {
    Edge {
        id,
        from,
        event,
        guard,
        to,
    }
}

/// `AnalyticsJobMachine`: the closed edge table. Every other pair of state and event is refused.
pub static ANALYTICS_JOB_MACHINE: MachineDef<JobStage, JobEvent, &'static str> = MachineDef {
    name: "AnalyticsJobMachine",
    states: &[
        JobStage::Submitted,
        JobStage::Running,
        JobStage::Publishing,
        JobStage::CancelRequested,
        JobStage::Backoff,
        JobStage::Succeeded,
        JobStage::Failed,
        JobStage::Cancelled,
    ],
    initial: JobStage::Submitted,
    terminals: &[JobStage::Succeeded, JobStage::Failed, JobStage::Cancelled],
    events: &[
        JobEvent::Claim,
        JobEvent::Publish,
        JobEvent::Complete,
        JobEvent::Fail,
        JobEvent::Cancel,
        JobEvent::RequestCancel,
        JobEvent::ConfirmCancel,
        JobEvent::Reclaim,
    ],
    guards: &[
        JOB_GUARD_WORKER_TAKES_LEASE,
        JOB_GUARD_NOT_BEFORE_PASSED_WORKER_TAKES_LEASE,
        JOB_GUARD_LEASE_OWNER_MATCHES_LEASE_LIVE,
        JOB_GUARD_ATTEMPTS_BELOW_MAX,
        JOB_GUARD_ATTEMPTS_REACHED_MAX,
        JOB_GUARD_LEASE_LIVE,
        JOB_GUARD_LEASE_OWNER_MATCHES,
        JOB_GUARD_LEASE_LAPSED,
    ],
    edges: &[
        edge(
            "J01",
            JobStage::Submitted,
            JobEvent::Claim,
            Some(JOB_GUARD_WORKER_TAKES_LEASE),
            JobStage::Running,
        ),
        edge(
            "J02",
            JobStage::Backoff,
            JobEvent::Claim,
            Some(JOB_GUARD_NOT_BEFORE_PASSED_WORKER_TAKES_LEASE),
            JobStage::Running,
        ),
        edge(
            "J03",
            JobStage::Running,
            JobEvent::Publish,
            Some(JOB_GUARD_LEASE_OWNER_MATCHES_LEASE_LIVE),
            JobStage::Publishing,
        ),
        edge(
            "J04",
            JobStage::Publishing,
            JobEvent::Complete,
            Some(JOB_GUARD_LEASE_OWNER_MATCHES_LEASE_LIVE),
            JobStage::Succeeded,
        ),
        edge(
            "J05",
            JobStage::Running,
            JobEvent::Fail,
            Some(JOB_GUARD_ATTEMPTS_BELOW_MAX),
            JobStage::Backoff,
        ),
        edge(
            "J06",
            JobStage::Running,
            JobEvent::Fail,
            Some(JOB_GUARD_ATTEMPTS_REACHED_MAX),
            JobStage::Failed,
        ),
        edge(
            "J07",
            JobStage::Publishing,
            JobEvent::Fail,
            Some(JOB_GUARD_ATTEMPTS_BELOW_MAX),
            JobStage::Backoff,
        ),
        edge(
            "J08",
            JobStage::Publishing,
            JobEvent::Fail,
            Some(JOB_GUARD_ATTEMPTS_REACHED_MAX),
            JobStage::Failed,
        ),
        edge(
            "J09",
            JobStage::Submitted,
            JobEvent::Cancel,
            None,
            JobStage::Cancelled,
        ),
        edge(
            "J10",
            JobStage::Backoff,
            JobEvent::Cancel,
            None,
            JobStage::Cancelled,
        ),
        edge(
            "J11",
            JobStage::Running,
            JobEvent::RequestCancel,
            Some(JOB_GUARD_LEASE_LIVE),
            JobStage::CancelRequested,
        ),
        edge(
            "J12",
            JobStage::CancelRequested,
            JobEvent::ConfirmCancel,
            Some(JOB_GUARD_LEASE_OWNER_MATCHES),
            JobStage::Cancelled,
        ),
        edge(
            "J13",
            JobStage::Running,
            JobEvent::Reclaim,
            Some(JOB_GUARD_LEASE_LAPSED),
            JobStage::Submitted,
        ),
        edge(
            "J14",
            JobStage::Publishing,
            JobEvent::Reclaim,
            Some(JOB_GUARD_LEASE_LAPSED),
            JobStage::Submitted,
        ),
        edge(
            "J15",
            JobStage::CancelRequested,
            JobEvent::Reclaim,
            Some(JOB_GUARD_LEASE_LAPSED),
            JobStage::Cancelled,
        ),
    ],
};

impl JobStage {
    /// The one transition function of `AnalyticsJobMachine`. `guard_holds` answers each guard by
    /// name; an unguarded edge always holds. An illegal pair is refused with `TransitionRefused`
    /// and nothing changes.
    pub fn step(
        self,
        event: JobEvent,
        guard_holds: impl Fn(&'static str) -> bool,
    ) -> Result<Self, TransitionRefused> {
        transition(&ANALYTICS_JOB_MACHINE, self, event, guard_holds).map(|edge| edge.to)
    }

    /// The state whose stored text is `stored`, if the machine declares one.
    pub fn from_stored(stored: &str) -> Option<Self> {
        ANALYTICS_JOB_MACHINE.state_from_stored(stored)
    }
}

/// The lease, attempt and backoff facts the job guards read, at one instant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JobFacts {
    /// When the current lease expires (Unix ms); `None` when no lease is held.
    pub lease_expires_at_ms: Option<i64>,
    /// The caller is the worker that owns the lease.
    pub lease_owner_matches: bool,
    /// A worker is taking a lease now (a claim).
    pub worker_takes_lease: bool,
    /// Attempts made are below the retry policy's maximum.
    pub attempts_below_max: bool,
    /// The earliest time a Backoff job may be claimed (Unix ms).
    pub not_before_ms: i64,
}

impl JobFacts {
    /// Whether the lease is held and unexpired at `now_ms`.
    pub fn lease_live(&self, now_ms: i64) -> bool {
        self.lease_expires_at_ms.is_some_and(|at| at > now_ms)
    }

    /// Answer each guard of `AnalyticsJobMachine` at `now_ms`.
    pub fn guard(&self, now_ms: i64) -> impl Fn(&'static str) -> bool + '_ {
        move |guard| {
            let live = self.lease_live(now_ms);
            match guard {
                JOB_GUARD_WORKER_TAKES_LEASE => self.worker_takes_lease,
                JOB_GUARD_NOT_BEFORE_PASSED_WORKER_TAKES_LEASE => {
                    self.not_before_ms <= now_ms && self.worker_takes_lease
                }
                JOB_GUARD_LEASE_OWNER_MATCHES_LEASE_LIVE => self.lease_owner_matches && live,
                JOB_GUARD_ATTEMPTS_BELOW_MAX => self.attempts_below_max,
                JOB_GUARD_ATTEMPTS_REACHED_MAX => !self.attempts_below_max,
                JOB_GUARD_LEASE_LIVE => live,
                JOB_GUARD_LEASE_OWNER_MATCHES => self.lease_owner_matches,
                JOB_GUARD_LEASE_LAPSED => !live,
                _ => false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{JobEvent, JobFacts, JobStage, ANALYTICS_JOB_MACHINE, JOB_GUARD_LEASE_LAPSED};
    use medulla_types::transition_refused::{MachineState, TransitionRefusalReason};

    fn edge_case(id: &str, from: JobStage, event: JobEvent, to: JobStage) {
        let edge = ANALYTICS_JOB_MACHINE
            .edges
            .iter()
            .find(|e| e.id == id)
            .unwrap();
        assert_eq!((edge.from, edge.event, edge.to), (from, event, to));
        // Only this edge's guard holds, so a sibling edge on the same event cannot fire.
        assert_eq!(from.step(event, |g| Some(g) == edge.guard), Ok(to), "{id}");
        if edge.guard.is_some() {
            let refused = from.step(event, |_| false).unwrap_err();
            assert_eq!(refused.reason, TransitionRefusalReason::GuardFailed, "{id}");
        }
    }

    /// Every (state, event) pair with no edge.
    fn illegal_pairs() -> Vec<(JobStage, JobEvent)> {
        let machine = &ANALYTICS_JOB_MACHINE;
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

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j01_submitted_claim() {
        edge_case(
            "J01",
            JobStage::Submitted,
            JobEvent::Claim,
            JobStage::Running,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j02_backoff_claim() {
        edge_case("J02", JobStage::Backoff, JobEvent::Claim, JobStage::Running);
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j03_running_publish() {
        edge_case(
            "J03",
            JobStage::Running,
            JobEvent::Publish,
            JobStage::Publishing,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j04_publishing_complete() {
        edge_case(
            "J04",
            JobStage::Publishing,
            JobEvent::Complete,
            JobStage::Succeeded,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j05_running_fail_retry() {
        edge_case("J05", JobStage::Running, JobEvent::Fail, JobStage::Backoff);
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j06_running_fail_exhausted() {
        edge_case("J06", JobStage::Running, JobEvent::Fail, JobStage::Failed);
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j07_publishing_fail_retry() {
        edge_case(
            "J07",
            JobStage::Publishing,
            JobEvent::Fail,
            JobStage::Backoff,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j08_publishing_fail_exhausted() {
        edge_case(
            "J08",
            JobStage::Publishing,
            JobEvent::Fail,
            JobStage::Failed,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j09_submitted_cancel() {
        edge_case(
            "J09",
            JobStage::Submitted,
            JobEvent::Cancel,
            JobStage::Cancelled,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j10_backoff_cancel() {
        edge_case(
            "J10",
            JobStage::Backoff,
            JobEvent::Cancel,
            JobStage::Cancelled,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j11_running_request_cancel() {
        edge_case(
            "J11",
            JobStage::Running,
            JobEvent::RequestCancel,
            JobStage::CancelRequested,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j12_cancel_requested_confirm_cancel() {
        edge_case(
            "J12",
            JobStage::CancelRequested,
            JobEvent::ConfirmCancel,
            JobStage::Cancelled,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j13_running_reclaim() {
        edge_case(
            "J13",
            JobStage::Running,
            JobEvent::Reclaim,
            JobStage::Submitted,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j14_publishing_reclaim() {
        edge_case(
            "J14",
            JobStage::Publishing,
            JobEvent::Reclaim,
            JobStage::Submitted,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn edge_j15_cancel_requested_reclaim() {
        edge_case(
            "J15",
            JobStage::CancelRequested,
            JobEvent::Reclaim,
            JobStage::Cancelled,
        );
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn every_illegal_pair_is_refused() {
        let machine = &ANALYTICS_JOB_MACHINE;
        let pairs = illegal_pairs();
        assert!(!pairs.is_empty());
        for (state, event) in pairs {
            let refused = state.step(event, |_| true).unwrap_err();
            let expected = if machine.is_terminal(state) {
                TransitionRefusalReason::TerminalState
            } else {
                TransitionRefusalReason::IllegalTransition
            };
            assert_eq!(refused.reason, expected, "{state:?} {event:?}");
        }
        // A Failed job never revives: no event leaves it.
        for &event in machine.events {
            let refused = JobStage::Failed.step(event, |_| true).unwrap_err();
            assert_eq!(refused.reason, TransitionRefusalReason::TerminalState);
        }
        for &terminal in machine.terminals {
            for &event in machine.events {
                assert!(terminal.step(event, |_| true).is_err());
            }
        }
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn refusal_carries_machine_from_event() {
        let refused = JobStage::Submitted
            .step(JobEvent::Publish, |_| true)
            .unwrap_err();
        assert_eq!(refused.machine, "AnalyticsJobMachine");
        assert_eq!(refused.from, JobStage::Submitted.stored());
        assert_eq!(refused.event, "Publish");
        assert_eq!(refused.code().as_str(), "TRANSITION_REFUSED");
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn every_transition_and_refusal_is_covered() {
        let machine = &ANALYTICS_JOB_MACHINE;
        assert_eq!(machine.edges.len(), 15);
        assert_eq!(machine.states.len(), 8);
        let mut ids: Vec<_> = machine.edges.iter().map(|e| e.id).collect();
        ids.sort_unstable();
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
            assert_eq!(JobStage::from_stored(state.stored()), Some(state));
            for &event in machine.events {
                let legal = state.step(event, |_| true).is_ok();
                let has_edge = machine.edges_from(state).any(|e| e.event == event);
                assert_eq!(legal, has_edge, "{state:?} {event:?}");
            }
        }
    }

    // spec: MEDULLA-CQ-ORCH-R811.1
    #[test]
    fn job_facts_answer_the_lease_and_retry_guards() {
        let held = JobFacts {
            lease_expires_at_ms: Some(100),
            lease_owner_matches: true,
            worker_takes_lease: false,
            attempts_below_max: true,
            not_before_ms: 50,
        };
        // A live lease holds until its expiry instant; a lapsed one reclaims.
        assert!(held.guard(99)("LeaseLive"));
        assert!(!held.guard(100)("LeaseLive"));
        assert!(held.guard(100)(JOB_GUARD_LEASE_LAPSED));
        assert_eq!(
            JobStage::Running.step(JobEvent::Reclaim, held.guard(99)),
            Err(JobStage::Running
                .step(JobEvent::Reclaim, |_| false)
                .unwrap_err())
        );
        assert_eq!(
            JobStage::Running.step(JobEvent::Reclaim, held.guard(100)),
            Ok(JobStage::Submitted)
        );
        assert_eq!(
            JobStage::CancelRequested.step(JobEvent::Reclaim, held.guard(100)),
            Ok(JobStage::Cancelled)
        );
        // Retries left go to Backoff; exhausted go to Failed.
        let exhausted = JobFacts {
            attempts_below_max: false,
            ..held
        };
        assert_eq!(
            JobStage::Running.step(JobEvent::Fail, held.guard(10)),
            Ok(JobStage::Backoff)
        );
        assert_eq!(
            JobStage::Running.step(JobEvent::Fail, exhausted.guard(10)),
            Ok(JobStage::Failed)
        );
        // A Backoff job is claimable only once `not_before` has passed.
        let claim = JobFacts {
            worker_takes_lease: true,
            ..held
        };
        assert!(JobStage::Backoff
            .step(JobEvent::Claim, claim.guard(49))
            .is_err());
        assert_eq!(
            JobStage::Backoff.step(JobEvent::Claim, claim.guard(50)),
            Ok(JobStage::Running)
        );
    }
}
