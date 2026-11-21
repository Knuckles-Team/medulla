//! Private implementation module for the analytics job handler.

use super::prelude_jobs::*;
use super::prelude_std::*;
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AnalyticsWorkerTiming {
    lease_ms: u64,
    poll_ms: u64,
}

static POOL_STARTED: OnceLock<()> = OnceLock::new();

/// Whether the colocated pool has already been started by this process.
pub(super) fn job_workers_started() -> bool {
    POOL_STARTED.get().is_some()
}

/// The scheduler policy in force, read the way `worker_ttl_ms` reads it: the stored
/// setting, else the policy built from the startup knobs. It sizes the colocated pool
/// and sets its lease and poll timing (MEDULLA-DURABLE-KERNEL-R082.9). An error already
/// carries its code (`SCHEDULER_CONFIG_INVALID` or `INTERNAL`).
pub(super) async fn analytics_pool_config(
    state: &Arc<RwLock<ServerState>>,
) -> Result<medulla_work_types::scheduler_config::SchedulerConfig, String> {
    let store = {
        let current = state.read().await;
        medulla_persistence::scheduler_config_store::shared_scheduler_config_store(&current)
            .map_err(|error| format!("INTERNAL: {error}"))?
    };
    crate::scheduler_config::effective_config(&store)
}

/// Start the optional bounded colocated executor pool. Setting the count to zero
/// leaves execution to authenticated remote workers using the coordinator ops. The pool
/// keeps the timing it starts with; the next process start reads the policy in force.
pub(super) fn ensure_job_workers(
    state: Arc<RwLock<ServerState>>,
    store: Arc<JobStore>,
    config: &medulla_work_types::scheduler_config::SchedulerConfig,
) {
    let timing = AnalyticsWorkerTiming {
        lease_ms: config.analytics_lease_ms,
        poll_ms: config.analytics_poll_ms,
    };
    let workers = config.analytics_workers as usize;
    POOL_STARTED.get_or_init(|| {
        // Gauge state must advance when leases expire even if no worker or
        // client RPC arrives. This coordinator task is deliberately independent
        // of the pool size so remote-only deployments remain autoscalable.
        let metric_store = store.clone();
        tokio::spawn(async move {
            loop {
                refresh_job_metrics(&metric_store);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        super::schedule_tick::ensure_schedule_tick(state.clone());
        for slot in 0..workers {
            tokio::spawn(worker_loop(state.clone(), store.clone(), slot, timing));
        }
    });
}

pub(super) fn refresh_job_metrics(store: &JobStore) {
    if let Ok((ready, active, publishing)) = store.metric_counts(unix_ms()) {
        medulla_kernel::metrics::process().set_analytics_job_counts(ready, active, publishing);
    }
}

pub(super) async fn worker_loop(
    state: Arc<RwLock<ServerState>>,
    store: Arc<JobStore>,
    slot: usize,
    timing: AnalyticsWorkerTiming,
) {
    let worker_ref = medulla_server_api::batch_compile::opaque_coordinator_key(
        "analytics-worker",
        "native",
        &slot.to_string(),
    );
    let capabilities: Vec<_> = [
        Some(opaque_worker_capability("mining.association")),
        Some(opaque_worker_capability("pool:default")),
        program_optimization_capability(),
    ]
    .into_iter()
    .flatten()
    .collect();
    let quota = tenant_worker_quota();
    loop {
        match store.claim_next(
            &worker_ref,
            &capabilities,
            unix_ms(),
            timing.lease_ms,
            quota,
        ) {
            Ok(Some(claim)) => {
                let job_id = claim.job.job_id.clone();
                let lease = claim.lease.clone();
                if execute_claim(&state, &store, claim).await.is_err() {
                    // Compute failures consume the current attempt and obey durable
                    // retry backoff. Publication failures retain the complete result
                    // and release only ownership so another replica can replay it.
                    if let Ok(current) = store.get(&job_id) {
                        match current.state {
                            JobState::Running { .. } => {
                                let _ = fail_attempt(
                                    &store,
                                    &job_id,
                                    &lease.worker_ref,
                                    lease.epoch,
                                    "worker_execution_failed",
                                    unix_ms(),
                                );
                            }
                            JobState::Publishing { .. } => {
                                let _ = store.release_publication_lease_fenced(
                                    &job_id,
                                    &lease.worker_ref,
                                    lease.epoch,
                                    unix_ms(),
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
            Ok(None) | Err(_) => tokio::time::sleep(Duration::from_millis(timing.poll_ms)).await,
        }
    }
}

/// The program-optimization capability, advertised only by builds that carry the handler.
fn program_optimization_capability() -> Option<String> {
    #[cfg(feature = "program-optimization")]
    {
        Some(opaque_worker_capability("program.optimization"))
    }
    #[cfg(not(feature = "program-optimization"))]
    {
        None
    }
}
