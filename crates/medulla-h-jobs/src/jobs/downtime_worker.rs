//! Private implementation module for the analytics job handler: the colocated downtime worker
//! (MEDULLA-DURABLE-KERNEL-R101.7).
//!
//! One worker is the only claimant of the `downtime` capability. It runs the installed
//! [`DowntimeBody`] of a claimed job in bounded steps, checkpoints after each, stops the job
//! when its budget is spent and hands it back at a step boundary while foreground load asks
//! for the machine.

use super::prelude_jobs::*;
use super::prelude_std::*;
use super::*;
use medulla_jobs::downtime::preempt_requested;
use medulla_server_api::downtime::{
    downtime_body, DowntimeStep, DowntimeStepInput, FOREGROUND_ACTIVITY,
};

/// Milliseconds on the process's monotonic clock: the production step clock of
/// [`run_downtime_claim`], which a test replaces with a virtual one.
fn monotonic_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Whether foreground load asks downtime work to yield now.
fn foreground_preempts() -> bool {
    let knobs = medulla_server_api::knob_config::knobs();
    let reading = FOREGROUND_ACTIVITY.reading(unix_ms().max(0) as u64);
    preempt_requested(reading.peak_rps, knobs.downtime_idle_max_rps)
}

/// The worker loop: claim a downtime job only while the foreground leaves room, run it, and
/// settle a failed attempt the way the analytics pool does.
pub(super) async fn downtime_worker_loop(
    _state: Arc<RwLock<ServerState>>,
    store: Arc<JobStore>,
    timing: AnalyticsWorkerTiming,
) {
    let worker_ref = medulla_server_api::batch_compile::opaque_coordinator_key(
        "downtime-worker",
        "native",
        "0",
    );
    let capabilities = vec![opaque_worker_capability(DOWNTIME_CAPABILITY)];
    let quota = TenantJobQuota {
        max_active: usize::MAX,
        max_reserved_cpu_ms: u64::MAX,
    };
    loop {
        if foreground_preempts() {
            tokio::time::sleep(Duration::from_millis(timing.poll_ms)).await;
            continue;
        }
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
                let step_deadline_ms =
                    medulla_server_api::knob_config::knobs().downtime_preempt_deadline_ms / 2;
                if run_downtime_claim(
                    &store,
                    claim,
                    timing.lease_ms,
                    &foreground_preempts,
                    step_deadline_ms,
                    &monotonic_ms,
                )
                .await
                .is_err()
                {
                    settle_failed_claim(&store, &job_id, &lease);
                }
            }
            Ok(None) | Err(_) => tokio::time::sleep(Duration::from_millis(timing.poll_ms)).await,
        }
    }
}

/// Run one claimed downtime job until it finishes, spends its budget or yields.
/// `preempt` is asked at every step boundary; `clock_ms` is the monotonic clock that meters
/// each step's cost against the job's CPU budget.
pub(super) async fn run_downtime_claim(
    store: &JobStore,
    claim: WorkerClaim,
    lease_ms: u64,
    preempt: &(dyn Fn() -> bool + Sync),
    step_deadline_ms: u64,
    clock_ms: &(dyn Fn() -> u64 + Sync),
) -> Result<(), String> {
    let job = claim.job;
    let job_id = job.job_id.clone();
    let worker_ref = claim.lease.worker_ref.clone();
    let epoch = claim.lease.epoch;
    let Some(body) = downtime_body(&job.algo.algorithm) else {
        fail_attempt(
            store,
            &job_id,
            &worker_ref,
            epoch,
            "downtime_body_missing",
            unix_ms(),
        )?;
        return Ok(());
    };
    let mut state_blob = job
        .state
        .checkpoint()
        .and_then(|checkpoint| checkpoint.state_blob.clone());
    let cpu_budget = job.policy.resources.cpu_ms;
    let memory_budget = job.policy.resources.memory_bytes;
    let job = Arc::new(job);
    let mut used_cpu_ms = 0u64;
    let mut peak_memory_bytes = 0u64;
    let mut renewed_at_ms = unix_ms();
    let mut steps = 0u32;
    loop {
        if preempt() {
            store
                .yield_running_fenced(&job_id, &worker_ref, epoch, unix_ms())
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
        let started = clock_ms();
        let (step_body, step_job, step_blob) = (body.clone(), job.clone(), state_blob.clone());
        let step = tokio::task::spawn_blocking(move || {
            step_body.step(DowntimeStepInput {
                job: &*step_job,
                state_blob: step_blob.as_deref(),
                step_deadline_ms,
            })
        })
        .await
        .map_err(|error| error.to_string())??;
        used_cpu_ms = used_cpu_ms.saturating_add(clock_ms().saturating_sub(started));
        let working_bytes = match &step {
            DowntimeStep::Continue { working_bytes, .. }
            | DowntimeStep::Done { working_bytes, .. } => *working_bytes,
        };
        peak_memory_bytes = peak_memory_bytes.max(working_bytes);
        if cpu_budget.is_some_and(|budget| used_cpu_ms > budget)
            || memory_budget.is_some_and(|budget| peak_memory_bytes > budget)
        {
            fail_attempt(
                store,
                &job_id,
                &worker_ref,
                epoch,
                "downtime_budget_exhausted",
                unix_ms(),
            )?;
            return Ok(());
        }
        match step {
            DowntimeStep::Continue {
                state_blob: next, ..
            } => {
                steps = steps.saturating_add(1);
                let now = unix_ms();
                if now.saturating_sub(renewed_at_ms) >= (lease_ms / 2) as i64 {
                    store
                        .renew_lease(&job_id, &worker_ref, epoch, now, lease_ms)
                        .map_err(|error| error.to_string())?;
                    renewed_at_ms = now;
                }
                store
                    .checkpoint_fenced(
                        &job_id,
                        &worker_ref,
                        epoch,
                        Checkpoint {
                            progress: f64::from(steps) / (f64::from(steps) + 1.0),
                            label: "downtime_step".to_string(),
                            state_blob: Some(next.clone()),
                            updated_at_ms: now,
                        },
                        now,
                    )
                    .map_err(|error| error.to_string())?;
                state_blob = Some(next);
            }
            DowntimeStep::Done { result, .. } => {
                let result = result.with_usage(Some(medulla_jobs::ResultUsage {
                    cpu_ms: used_cpu_ms,
                    peak_memory_bytes,
                    ..medulla_jobs::ResultUsage::default()
                }));
                let staged = store
                    .stage_result_fenced(&job_id, &worker_ref, epoch, result, unix_ms())
                    .map_err(|error| error.to_string())?;
                store
                    .complete_publication_fenced(&staged.job_id, &worker_ref, epoch, unix_ms())
                    .map_err(|error| error.to_string())?;
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_jobs::model::JobPlacement;
    use medulla_server_api::downtime::{install_downtime_body, DowntimeBody};
    use std::sync::atomic::AtomicUsize;

    const INTENT: &str = "test_downtime_steps_r101_7";

    /// A body of `total` steps of 30 ms each on a virtual clock; its state is the count of
    /// steps done.
    struct SteppedBody {
        total: u8,
        done: Arc<AtomicUsize>,
        clock: Arc<std::sync::atomic::AtomicU64>,
    }

    impl DowntimeBody for SteppedBody {
        fn step(&self, input: DowntimeStepInput<'_>) -> Result<DowntimeStep, String> {
            self.clock.fetch_add(30, std::sync::atomic::Ordering::SeqCst);
            let before = input.state_blob.and_then(|blob| blob.first()).copied().unwrap_or(0);
            let after = before + 1;
            self.done.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if after >= self.total {
                let result = super::super::downtime_complete::downtime_output_result(
                    input.job,
                    &serde_json::json!({ "steps": after }),
                )?;
                Ok(DowntimeStep::Done {
                    result,
                    working_bytes: 1_024,
                })
            } else {
                Ok(DowntimeStep::Continue {
                    state_blob: vec![after],
                    working_bytes: 1_024,
                })
            }
        }
    }

    fn submit(store: &JobStore, cpu_ms: u64) -> String {
        store
            .submit(SubmitSpec {
                input_snapshot: InputSnapshotHandle::new("__commons__", 0)
                    .with_dataset("", "r101-7".to_string()),
                policy: JobPolicy {
                    tenant: "__engine__".into(),
                    actor: "engine:downtime".into(),
                    purpose: "downtime work: test".into(),
                    resources: medulla_jobs::ResourceBudget {
                        cpu_ms: Some(cpu_ms),
                        memory_bytes: Some(1 << 20),
                        ..medulla_jobs::ResourceBudget::default()
                    },
                    placement: JobPlacement {
                        required_capabilities: vec![opaque_worker_capability(
                            DOWNTIME_CAPABILITY,
                        )],
                        ..JobPlacement::default()
                    },
                    ..JobPolicy::default()
                },
                algo: AlgoVersion {
                    family: format!("downtime.{INTENT}"),
                    algorithm: INTENT.into(),
                    params_digest: "params".into(),
                    code_version: "test".into(),
                    env_version: "test".into(),
                },
                input_payload: None,
                max_attempts: 1,
                backoff_ms: 0,
            })
            .expect("the downtime job is submitted")
            .job_id
    }

    fn claim(store: &JobStore) -> WorkerClaim {
        store
            .claim_next(
                "downtime-worker-ref",
                &[opaque_worker_capability(DOWNTIME_CAPABILITY)],
                unix_ms(),
                60_000,
                TenantJobQuota::default(),
            )
            .expect("the claim is answered")
            .expect("the downtime job is ready")
    }

    // spec: MEDULLA-DURABLE-KERNEL-R101.7
    #[tokio::test]
    async fn budget_stop_and_preempt_yield() {
        let done = Arc::new(AtomicUsize::new(0));
        let virtual_clock = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let virtual_clock_reader = virtual_clock.clone();
        install_downtime_body(
            INTENT,
            Arc::new(SteppedBody {
                total: 5,
                done: done.clone(),
                clock: virtual_clock.clone(),
            }),
        );
        let clock_ms = move || virtual_clock_reader.load(std::sync::atomic::Ordering::SeqCst);
        let dir = tempfile::tempdir().unwrap();
        let store = super::super::core::open_job_store(dir.path()).expect("job store opens");

        // A 60 ms budget over 30 ms steps stops the job with the budget code and no later step.
        let spent = submit(&store, 60);
        run_downtime_claim(&store, claim(&store), 60_000, &|| false, 125, &clock_ms)
            .await
            .unwrap();
        let JobState::Failed { reason, .. } = store.get(&spent).unwrap().state else {
            panic!("a spent budget fails the job")
        };
        assert_eq!(reason, "downtime_budget_exhausted");
        assert!(done.load(std::sync::atomic::Ordering::SeqCst) <= 3);

        // A preempt reading at a step boundary yields: no lease, the attempt given back and
        // the last checkpoint kept.
        let job_id = submit(&store, 10_000);
        done.store(0, std::sync::atomic::Ordering::SeqCst);
        let seen = done.clone();
        run_downtime_claim(
            &store,
            claim(&store),
            60_000,
            &move || seen.load(std::sync::atomic::Ordering::SeqCst) >= 2,
            125,
            &clock_ms,
        )
        .await
        .unwrap();
        let yielded = store.get(&job_id).unwrap();
        let JobState::Submitted { checkpoint: Some(checkpoint) } = &yielded.state else {
            panic!("a yield leaves the job claimable with its checkpoint: {:?}", yielded.state)
        };
        assert_eq!(checkpoint.state_blob.as_deref(), Some(&[2u8][..]));
        assert_eq!(yielded.retry.attempts_made, 0);

        // The next claim resumes from the checkpoint and succeeds with its usage filled.
        let resumed = claim(&store);
        assert!(matches!(
            &resumed.job.state,
            JobState::Running { checkpoint, .. } if checkpoint.state_blob.as_deref() == Some(&[2u8][..])
        ));
        run_downtime_claim(&store, resumed, 60_000, &|| false, 125, &clock_ms)
            .await
            .unwrap();
        let finished = store.get(&job_id).unwrap();
        assert!(matches!(finished.state, JobState::Succeeded { .. }));
        let usage = finished.output.and_then(|output| output.usage).unwrap();
        assert!(usage.cpu_ms > 0 && usage.cpu_ms <= 10_000);
    }
}
