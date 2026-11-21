//! The body of the `plan_evolution` downtime job (MEDULLA-FEDERATED-QUERY-R091.3).
//!
//! The downtime tick submits one `plan_evolution` job per window. The downtime worker runs
//! this body, because a replay needs the engine's registered foreign sources: for each tenant
//! with recorded fragments it takes the `pushdown_choice` candidates, replays the baseline
//! plan and the candidate `federation.min_replays` times each under the tenant's own
//! registry, and keeps the candidate only when `evaluate_shape` says it measurably wins with
//! the same results. A fragment whose source no longer resolves for its tenant is never
//! replayed; its kept plan is dropped and the receipt says `authority_expired`. The job
//! completes with `{ "receipts": [..] }`.

use std::sync::Arc;

use medulla_federation::federation::ForeignSourceRegistry;
use medulla_federation::federation_opt::plan_evolution::{
    self, Decision, EvolutionPolicy, EvolutionWork, KeptPlan, PlanEvolutionRefusal,
    PlanObservation, ReplayMeasure, RevertReason,
};
use medulla_server_api::downtime::{DowntimeBody, DowntimeStep, DowntimeStepInput};
use medulla_server_api::state::ServerState;
use medulla_server_ops::api::query_txn::ForeignStateExt;
use serde::Serialize;
use tokio::sync::RwLock;

/// What one candidate's evolution run decided.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlanEvolutionReceipt {
    /// The tenant whose query shape was evolved.
    pub tenant: String,
    /// The digest of the query shape the candidate plan answers.
    pub shape_digest: String,
    /// The digest of the plan in force before the run.
    pub baseline_digest: String,
    /// The digest of the candidate plan the run measured.
    pub candidate_digest: String,
    /// `kept`, `too_few_replays`, `results_differ`, `no_measured_gain` or `authority_expired`.
    pub outcome: String,
    /// The downtime job that ran the evolution.
    pub job_id: String,
    /// The registered refusal code when the run was refused (`PLAN_RESULT_MISMATCH` or
    /// `PLAN_AUTHORITY_EXPIRED`); the baseline stays in force.
    pub refusal: Option<String>,
    /// How many times the baseline plan replayed (MEDULLA-FEDERATED-QUERY-R091.5).
    pub baseline_replays: u32,
    /// How many times the candidate plan replayed.
    pub candidate_replays: u32,
    /// The baseline's p50 latency over its replays, in microseconds.
    pub p50_before_us: u64,
    /// The candidate's p50 latency over its replays, in microseconds.
    pub p50_after_us: u64,
    /// The baseline's p50 bytes fetched.
    pub bytes_before: u64,
    /// The candidate's p50 bytes fetched.
    pub bytes_after: u64,
}

/// The evolution policy the registered `federation.*` knobs resolve to.
fn policy() -> EvolutionPolicy {
    let knobs = medulla_server_api::knob_config::knobs();
    EvolutionPolicy {
        top_shapes: usize::try_from(knobs.federation_evolution_top_shapes).unwrap_or(usize::MAX),
        min_gain_percent: knobs.federation_min_gain,
        min_replays: usize::try_from(knobs.federation_min_replays).unwrap_or(usize::MAX),
        drift_revalidate_percent: knobs.federation_drift_revalidate,
    }
}

/// The body of the `plan_evolution` intent, installed by `seams_install`: one step reads the
/// tenants' foreign-source registries from the shared state, evolves every candidate and
/// finishes the job with its receipts.
pub struct PlanEvolutionBody {
    state: Arc<RwLock<ServerState>>,
    runtime: tokio::runtime::Handle,
}

impl PlanEvolutionBody {
    /// A body that reads `state` on `runtime`.
    pub fn new(state: Arc<RwLock<ServerState>>, runtime: tokio::runtime::Handle) -> Self {
        Self { state, runtime }
    }
}

impl DowntimeBody for PlanEvolutionBody {
    fn step(&self, input: DowntimeStepInput<'_>) -> Result<DowntimeStep, String> {
        // The worker runs a step on a blocking thread, so waiting on the shared state here
        // never stalls the runtime's own workers.
        let tenant_registries = self.runtime.block_on(async {
            let current = self.state.read().await;
            let catalog = current.foreign_sources();
            plan_evolution::observed_tenants()
                .into_iter()
                .map(|tenant| {
                    let registry = catalog
                        .registry_for_cache_salt(&tenant, &current.isolation)
                        .map(|owned| owned.into_registry());
                    (tenant, registry)
                })
                .collect::<Vec<(String, Option<ForeignSourceRegistry>)>>()
        });
        let receipts = evolve(&tenant_registries, &policy(), &input.job.job_id);
        let output = serde_json::json!({ "receipts": receipts });
        let result = medulla_h_jobs::jobs::downtime_output_result(input.job, &output)?;
        Ok(DowntimeStep::Done {
            result,
            working_bytes: 0,
        })
    }
}

/// Evolve every tenant's candidates and answer one receipt per candidate.
fn evolve(
    tenants: &[(String, Option<ForeignSourceRegistry>)],
    policy: &EvolutionPolicy,
    job_id: &str,
) -> Vec<PlanEvolutionReceipt> {
    let mut receipts = Vec::new();
    for (tenant, registry) in tenants {
        for work in plan_evolution::pushdown_work(tenant, policy) {
            receipts.push(evolve_one(tenant, registry.as_ref(), &work, policy, job_id));
        }
    }
    receipts
}

/// The plan `observation` ran, as a plan a replay is forced to follow.
fn forced(observation: &PlanObservation, job_id: &str) -> KeptPlan {
    KeptPlan {
        plan_digest: observation.plan_digest.clone(),
        strategy: observation.strategy,
        limit_pushed: observation.limit_pushed.is_some(),
        receipt_job_id: job_id.to_string(),
        evidence: Default::default(),
    }
}

/// Replay one candidate against its baseline and keep or revert it.
fn evolve_one(
    tenant: &str,
    registry: Option<&ForeignSourceRegistry>,
    work: &EvolutionWork,
    policy: &EvolutionPolicy,
    job_id: &str,
) -> PlanEvolutionReceipt {
    let shape = &work.candidate.shape_digest;
    let receipt = |outcome: &str,
                   refusal: Option<PlanEvolutionRefusal>,
                   measured: Option<(&[ReplayMeasure], &[ReplayMeasure])>| {
        // All measurements are zero when nothing replayed (`authority_expired`).
        let none: &[ReplayMeasure] = &[];
        let (baseline, candidate) = measured.unwrap_or((none, none));
        let evidence = plan_evolution::evidence_of(&work.candidate.baseline_digest, baseline, candidate);
        PlanEvolutionReceipt {
        tenant: tenant.to_string(),
        shape_digest: shape.clone(),
        baseline_digest: work.candidate.baseline_digest.clone(),
        candidate_digest: work.candidate.plan_digest.clone(),
        outcome: outcome.to_string(),
        job_id: job_id.to_string(),
        refusal: refusal.map(|refusal| refusal.code().to_string()),
        baseline_replays: u32::try_from(baseline.len()).unwrap_or(u32::MAX),
        candidate_replays: evidence.replays,
        p50_before_us: evidence.p50_before_us,
        p50_after_us: evidence.p50_after_us,
        bytes_before: evidence.bytes_before,
        bytes_after: evidence.bytes_after,
        }
    };
    // Both plans replay the same recorded input, so their results are comparable.
    let input = &work.observed.replay;
    let Some(remote) =
        registry.and_then(|registry| plan_evolution::resolve_replay(&input.source, registry))
    else {
        plan_evolution::clear(tenant, shape);
        return receipt("authority_expired", Some(PlanEvolutionRefusal::AuthorityExpired), None);
    };
    let replays = |observation: &PlanObservation| -> Vec<ReplayMeasure> {
        let plan = forced(observation, job_id);
        (0..policy.min_replays)
            .filter_map(|_| plan_evolution::replay_fragment(remote.as_ref(), input, &plan).ok())
            .collect()
    };
    let baseline = replays(&work.baseline);
    let candidate = replays(&work.observed);
    match plan_evolution::evaluate_shape(policy, &baseline, &candidate) {
        Decision::Keep => {
            let mut kept = forced(&work.observed, job_id);
            kept.evidence =
                plan_evolution::evidence_of(&work.candidate.baseline_digest, &baseline, &candidate);
            plan_evolution::keep(tenant, shape, kept);
            receipt("kept", None, Some((&baseline, &candidate)))
        }
        Decision::Revert(reason) => {
            plan_evolution::clear(tenant, shape);
            receipt(
                match reason {
                    RevertReason::TooFewReplays => "too_few_replays",
                    RevertReason::ResultsDiffer => "results_differ",
                    RevertReason::NoMeasuredGain => "no_measured_gain",
                },
                reason.refusal(),
                Some((&baseline, &candidate)),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R091.3
    #[test]
    fn a_fragment_whose_authority_expired_is_cleared_not_replayed() {
        let tenant = "plan-evolution-expired-tenant";
        let shape = "plan-evolution-expired-shape";
        let observation = PlanObservation {
            shape_digest: shape.into(),
            plan_digest: "baseline".into(),
            sources: vec!["registered:gone#00000000".into()],
            pushdowns: Vec::new(),
            latency_us: 1,
            rows_fetched: 1,
            bytes_fetched: 0,
            cache_hit: false,
            graph_revision: 0,
            strategy: Default::default(),
            limit_pushed: None,
            keys_pushed: 0,
            replay: Default::default(),
        };
        let candidate = plan_evolution::PlanCandidate {
            shape_digest: shape.into(),
            kind: plan_evolution::CandidateKind::PushdownChoice,
            plan_digest: "candidate".into(),
            baseline_digest: "baseline".into(),
        };
        let work = EvolutionWork {
            candidate,
            baseline: observation.clone(),
            observed: observation,
        };
        plan_evolution::keep(
            tenant,
            shape,
            KeptPlan {
                plan_digest: "candidate".into(),
                strategy: Default::default(),
                limit_pushed: false,
                receipt_job_id: "earlier".into(),
                evidence: Default::default(),
            },
        );
        let policy = EvolutionPolicy {
            top_shapes: 8,
            min_gain_percent: 10,
            min_replays: 5,
            drift_revalidate_percent: 25,
        };
        let receipt = evolve_one(tenant, None, &work, &policy, "job-7");
        assert_eq!(receipt.outcome, "authority_expired");
        assert_eq!(receipt.refusal.as_deref(), Some("PLAN_AUTHORITY_EXPIRED"));
        assert_eq!(receipt.job_id, "job-7");
        assert!(plan_evolution::kept_plan(tenant, shape).is_none());
    }

    // spec: MEDULLA-FEDERATED-QUERY-R091.5
    #[test]
    fn receipt_names_measurements() {
        let tenant = "plan-evolution-measured-tenant";
        let shape = "plan-evolution-measured-shape";
        let observation = PlanObservation {
            shape_digest: shape.into(),
            plan_digest: "baseline".into(),
            sources: vec!["registered:gone#00000000".into()],
            pushdowns: Vec::new(),
            latency_us: 1,
            rows_fetched: 1,
            bytes_fetched: 0,
            cache_hit: false,
            graph_revision: 0,
            strategy: Default::default(),
            limit_pushed: None,
            keys_pushed: 0,
            replay: Default::default(),
        };
        let work = EvolutionWork {
            candidate: plan_evolution::PlanCandidate {
                shape_digest: shape.into(),
                kind: plan_evolution::CandidateKind::PushdownChoice,
                plan_digest: "candidate".into(),
                baseline_digest: "baseline".into(),
            },
            baseline: observation.clone(),
            observed: observation,
        };
        let policy = EvolutionPolicy {
            top_shapes: 8,
            min_gain_percent: 10,
            min_replays: 5,
            drift_revalidate_percent: 25,
        };
        // An unresolvable source replays nothing: every measurement is zero.
        let receipt = evolve_one(tenant, None, &work, &policy, "job-8");
        assert_eq!(
            (
                receipt.baseline_replays,
                receipt.candidate_replays,
                receipt.p50_before_us,
                receipt.p50_after_us,
                receipt.bytes_before,
                receipt.bytes_after
            ),
            (0, 0, 0, 0, 0, 0)
        );
        // The evidence a kept plan carries is the p50 of each side's measurements.
        let run = |latency_us: u64, bytes_fetched: u64| ReplayMeasure {
            latency_us,
            bytes_fetched,
            results: vec!["a".into()],
        };
        let before = [run(100, 900), run(300, 700), run(200, 800)];
        let after = [run(10, 90), run(30, 70), run(20, 80)];
        let evidence = plan_evolution::evidence_of("baseline", &before, &after);
        assert_eq!(evidence.replays, 3);
        assert_eq!((evidence.p50_before_us, evidence.p50_after_us), (200, 20));
        assert_eq!((evidence.bytes_before, evidence.bytes_after), (800, 80));
    }
}
