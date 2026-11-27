//! Private implementation module for the analytics job handler.

use super::prelude_jobs::*;
use super::prelude_std::*;
use super::*;

/// How often a running kernel's job status is polled, in milliseconds: the lease divided
/// by three, never above 5000 and never zero (MEDULLA-DURABLE-KERNEL-R082.7).
pub(super) fn status_tick_ms(lease_ms: u64) -> u64 {
    (lease_ms / 3).clamp(1, 5_000)
}

/// The closed set of failure codes a colocated attempt records
/// (MEDULLA-DURABLE-KERNEL-R082.8). A failure record carries one of these and nothing
/// else: no payload text, endpoint or identity.
pub(super) const FAILURE_CODES: &[&str] = &[
    "invalid_payload",
    "kernel_failure",
    "deadline_exceeded",
    "cpu_budget_exceeded",
    "worker_execution_failed",
    "downtime_budget_exhausted",
    "downtime_body_missing",
];

/// Release a failed fenced attempt under one code of [`FAILURE_CODES`] at `now_ms`. The
/// store sets the durable retry time from `RetryPolicy.backoff_ms` and ends the job
/// at `max_attempts`; a code outside the closed set is refused before any write.
pub(super) fn fail_attempt(
    store: &JobStore,
    job_id: &str,
    worker_ref: &str,
    epoch: u64,
    code: &str,
    now_ms: i64,
) -> Result<medulla_jobs::AnalyticsJob, String> {
    if !FAILURE_CODES.contains(&code) {
        return Err(format!("failure code {code} is outside the closed set"));
    }
    store
        .fail_attempt_fenced(job_id, worker_ref, epoch, code, now_ms)
        .map_err(|error| error.to_string())
}

pub(super) async fn execute_claim(
    state: &Arc<RwLock<ServerState>>,
    store: &Arc<JobStore>,
    claim: WorkerClaim,
) -> Result<(), String> {
    let worker_ref = claim.lease.worker_ref.clone();
    let epoch = claim.lease.epoch;
    let job_id = claim.job.job_id.clone();
    let staged = if matches!(&claim.job.state, JobState::Publishing { .. }) {
        claim.job
    } else {
        let payload = claim
            .job
            .input_payload
            .clone()
            .ok_or_else(|| "analytics input payload is unavailable".to_string())?;
        let kind: JobKind = medulla_types::msgpack::decode_bounded(
            &payload,
            medulla_types::msgpack::MsgpackLimits::new(
                MAX_JOB_INPUT_BYTES,
                MAX_JOB_INPUT_ITEMS,
                medulla_types::msgpack::DEFAULT_MAX_DEPTH,
            ),
        )
        .map_err(|_| "analytics input decoding failed".to_string())?;
        let (transactions, min_support, min_confidence, algorithm) = match kind {
            JobKind::MineAssociate {
                transactions,
                min_support,
                min_confidence,
                algorithm,
            } => (transactions, min_support, min_confidence, algorithm),
            JobKind::DatasetProfile {
                dataset,
                columns,
                owner_scope,
                tenant_scope,
                tenant,
                agent,
            } => {
                return execute_profile_claim(
                    state,
                    store,
                    &claim.job,
                    &worker_ref,
                    epoch,
                    ProfileTarget {
                        dataset,
                        columns,
                        owner_scope,
                        tenant_scope,
                        tenant,
                        agent,
                    },
                )
                .await;
            }
            JobKind::QualityEvaluate {
                target,
                tenant_scope,
                tenant,
                agent,
                ..
            } => {
                return execute_quality_claim(
                    state,
                    store,
                    &claim.job,
                    &worker_ref,
                    epoch,
                    QualityTarget {
                        target,
                        tenant_scope,
                        tenant,
                        agent,
                    },
                )
                .await;
            }
            JobKind::SourceQueryLogIngest {
                source,
                owner_scope,
                tenant_scope,
            } => {
                return execute_query_log_claim(
                    state,
                    store,
                    &claim.job,
                    &worker_ref,
                    epoch,
                    QueryLogTarget {
                        source,
                        owner_scope,
                        tenant_scope,
                    },
                )
                .await;
            }
            JobKind::UsageCluster {
                term_bindings,
                tenant_scope,
                ..
            } => {
                return execute_usage_cluster_claim(
                    state,
                    store,
                    &claim.job,
                    &worker_ref,
                    epoch,
                    UsageClusterTarget {
                        term_bindings: term_bindings
                            .into_iter()
                            .map(|binding| (binding.asset, binding.term))
                            .collect(),
                        tenant_scope,
                    },
                )
                .await;
            }
            JobKind::AnalyticalQuery { query, agent, .. } => {
                return execute_analytical_claim(
                    state,
                    store,
                    &claim.job,
                    &worker_ref,
                    epoch,
                    AnalyticalTarget { query, agent },
                )
                .await;
            }
            #[cfg(feature = "program-optimization")]
            JobKind::ProgramOptimize { request_msgpack } => {
                return execute_program_claim(
                    state,
                    store,
                    &claim.job,
                    &worker_ref,
                    epoch,
                    payload,
                    request_msgpack,
                )
                .await;
            }
            // `jobs` always compiles the variant in; without this crate's
            // executor the claim fails closed.
            #[cfg(not(feature = "program-optimization"))]
            JobKind::ProgramOptimize { .. } => {
                return Err("program optimization jobs are not built into this worker".to_string());
            }
        };
        let algorithm = parse_algorithm(&algorithm)?;
        store
            .checkpoint_fenced(
                &job_id,
                &worker_ref,
                epoch,
                Checkpoint {
                    progress: 0.1,
                    label: "mining".to_string(),
                    state_blob: Some(payload),
                    updated_at_ms: unix_ms(),
                },
                unix_ms(),
            )
            .map_err(|error| error.to_string())?;

        let cancellation = Arc::new(AtomicBool::new(false));
        let kernel_token = cancellation.clone();
        let mut kernel = tokio::task::spawn_blocking(move || {
            association::mine_labeled_cancellable(
                &transactions,
                min_support,
                min_confidence,
                algorithm,
                kernel_token,
            )
        });
        let began = Instant::now();
        // The status tick and the lease renewal follow the claim's own lease:
        // `min(5000 ms, lease_ms / 3)` (MEDULLA-DURABLE-KERNEL-R082.7).
        let lease_ms = u64::try_from(
            claim
                .lease
                .expires_at_ms
                .saturating_sub(claim.lease.acquired_at_ms),
        )
        .unwrap_or(0)
        .max(1);
        let mut ticker = tokio::time::interval(Duration::from_millis(status_tick_ms(lease_ms)));
        let mut stop_reason: Option<&'static str> = None;
        let rules = loop {
            tokio::select! {
                joined = &mut kernel => {
                    match joined {
                        Ok(Ok(rules)) if stop_reason.is_none() => break rules,
                        Ok(Err(_)) | Ok(Ok(_)) => {
                            match stop_reason {
                                Some("cancelled") => {
                                    let _ = store.mark_cancelled_fenced(
                                        &job_id, &worker_ref, epoch, unix_ms(),
                                    );
                                }
                                // A lost lease is not a failure code: the attempt is no
                                // longer ours to record, and the fence refuses the write.
                                Some(reason) => {
                                    let _ = fail_attempt(
                                        store, &job_id, &worker_ref, epoch, reason, unix_ms(),
                                    );
                                }
                                None => {
                                    let _ = fail_attempt(
                                        store, &job_id, &worker_ref, epoch, "kernel_failure", unix_ms(),
                                    );
                                }
                            }
                            return Ok(());
                        }
                        Err(_) => {
                            let _ = fail_attempt(
                                store, &job_id, &worker_ref, epoch, "kernel_failure", unix_ms(),
                            );
                            return Ok(());
                        }
                    }
                }
                _ = ticker.tick() => {
                    let now = unix_ms();
                    let current = match store.get(&job_id) {
                        Ok(job) => job,
                        Err(_) => {
                            cancellation.store(true, Ordering::Relaxed);
                            stop_reason = Some("lease_lost");
                            continue;
                        }
                    };
                    let cpu_exceeded = current
                        .policy
                        .resources
                        .cpu_ms
                        .or(current.policy.quota_cpu_ms)
                        .is_some_and(|limit| began.elapsed().as_millis() as u64 >= limit);
                    stop_reason = if current.state.cancel_requested() {
                        Some("cancelled")
                    } else if current.deadline_exceeded(now) {
                        Some("deadline_exceeded")
                    } else if cpu_exceeded {
                        Some("cpu_budget_exceeded")
                    } else {
                        None
                    };
                    if stop_reason.is_some() {
                        cancellation.store(true, Ordering::Relaxed);
                    } else if store
                        .renew_lease(&job_id, &worker_ref, epoch, now, lease_ms)
                        .is_err()
                    {
                        cancellation.store(true, Ordering::Relaxed);
                        stop_reason = Some("lease_lost");
                    }
                }
            }
        };
        let now = unix_ms();
        store
            .checkpoint_fenced(
                &job_id,
                &worker_ref,
                epoch,
                Checkpoint {
                    progress: 0.9,
                    label: "computed".to_string(),
                    state_blob: None,
                    updated_at_ms: now,
                },
                now,
            )
            .map_err(|error| error.to_string())?;
        let result = typed_association_result(&claim.job, &rules)?;
        store
            .stage_result_fenced(&job_id, &worker_ref, epoch, result, unix_ms())
            .map_err(|error| error.to_string())?
    };

    publish_staged_result(state, store, staged, &worker_ref, epoch).await
}
