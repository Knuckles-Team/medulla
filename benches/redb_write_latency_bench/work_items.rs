//! Native work-item groups of the redb write-latency bench. Each group runs the
//! production row code over a throwaway shard from the `medulla-work-store`
//! `test-support` fixtures, writes a receipt with the `BenchmarkClaim` fields
//! (configuration, measured result, passed), and reports a breach when its p99 or
//! memory budget is missed. The nightly job runs them; they never gate a pull
//! request.
//!
//! * `run_owner_recovery` (MEDULLA-DURABLE-KERNEL-R028.4.2): `run_owner::read_admission`
//!   over a store of 10 000 WorkItems, p99 under 10 ms.
//! * `current_plan_publication_readback` (MEDULLA-DURABLE-KERNEL-R028.5.4):
//!   `current_plan::publish` and `current_plan::read_current` of a 64-task
//!   snapshot on one claimed run, p99 under 10 ms and transient memory at most 8 MiB.
//! * `paired_pending_consumption` (MEDULLA-DURABLE-KERNEL-R039.4.2): the plan-bound
//!   pending call consumed through `test_shard::apply`, which reaches
//!   `control_lease/plan_pair.rs::consume`; commit-phase p99 under 10 ms.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use medulla_protocol_types::protocol::{Method, ResultPayload};
use medulla_store::redb_store::DurableCrypto;
use medulla_work_store::redb_store::work_item::test_shard::{self, run_fixture, GRAPH};
use medulla_work_store::redb_store::work_item::{current_plan, run_owner};

/// Live and peak heap bytes, for the transient-memory budgets.
pub struct CountingAlloc;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged to the system allocator.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `alloc` above with this `layout`.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

/// Reset the peak to the current live bytes and return that baseline.
fn memory_mark() -> usize {
    let live = LIVE.load(Ordering::Relaxed);
    PEAK.store(live, Ordering::Relaxed);
    live
}

/// Peak bytes above `baseline` since [`memory_mark`].
fn memory_since(baseline: usize) -> usize {
    PEAK.load(Ordering::Relaxed).saturating_sub(baseline)
}

const P99_BUDGET_MS: f64 = 10.0;
const CURRENT_PLAN_MEMORY_BUDGET: usize = 8 * 1024 * 1024;
const WORK_ITEMS: usize = 10_000;
const SEED_CHUNK: usize = 250;
const PLAN_TASKS: usize = 64;
const IDENTIFIER_BYTES: usize = 512;
const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

fn percentile_ms(samples: &[f64], fraction: f64) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize] * 1_000.0
}

fn hardware() -> String {
    format!(
        "{}-{} cpus={}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(0, |n| n.get())
    )
}

fn build_profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// One group's outcome: the receipt fields and whether it met its budgets.
pub struct GroupResult {
    pub case: &'static str,
    pub receipt: serde_json::Value,
    pub breach: Option<String>,
}

fn result(
    case: &'static str,
    workload: String,
    samples: &[f64],
    transient_bytes: Option<usize>,
    memory_budget: Option<usize>,
) -> GroupResult {
    let p50 = percentile_ms(samples, 0.50);
    let p99 = percentile_ms(samples, 0.99);
    let mut breaches = Vec::new();
    if p99 >= P99_BUDGET_MS {
        breaches.push(format!("p99 {p99:.3} ms reaches {P99_BUDGET_MS} ms"));
    }
    if let (Some(bytes), Some(budget)) = (transient_bytes, memory_budget) {
        if bytes > budget {
            breaches.push(format!("transient memory {bytes} B exceeds {budget} B"));
        }
    }
    let passed = breaches.is_empty();
    let configuration = format!(
        "workload={workload}; build_profile={}; hardware={}",
        build_profile(),
        hardware()
    );
    let measured_result = match transient_bytes {
        Some(bytes) => format!(
            "n={} p50={p50:.3}ms p99={p99:.3}ms transient_bytes={bytes}",
            samples.len()
        ),
        None => format!("n={} p50={p50:.3}ms p99={p99:.3}ms", samples.len()),
    };
    println!("\n{case}: {configuration}\n  {measured_result} passed={passed}");
    GroupResult {
        case,
        receipt: serde_json::json!({
            "case": case,
            "workload": workload,
            "build_profile": build_profile(),
            "hardware": hardware(),
            "p50_ms": p50,
            "p99_ms": p99,
            "transient_bytes": transient_bytes,
            "configuration": configuration,
            "measured_result": measured_result,
            "passed": passed,
        }),
        breach: (!passed).then(|| format!("{case}: {}", breaches.join("; "))),
    }
}

// ── run_owner_recovery (MEDULLA-DURABLE-KERNEL-R028.4.2) ─────────────────────

fn run_id(i: usize) -> String {
    format!("run-{i:05}")
}

// spec: MEDULLA-DURABLE-KERNEL-R028.4.2
/// Recover owner-bound original submissions from a store of 10 000 WorkItems.
pub fn run_owner_recovery(samples: usize) -> GroupResult {
    let temp = test_shard::open("bench-run-owner-recovery");
    let shard = &temp.shard;
    for chunk in 0..WORK_ITEMS.div_ceil(SEED_CHUNK) {
        let from = chunk * SEED_CHUNK;
        let to = WORK_ITEMS.min(from + SEED_CHUNK);
        test_shard::with_write(shard, &format!("bench-seed-{chunk}"), 1_000, |admitted| {
            for i in from..to {
                let mut request = test_shard::run_request();
                request.run_id = Some(run_id(i));
                request.idempotency_key = format!("run-key-{i:05}");
                request.context.request_id = format!("request-{i:05}");
                test_shard::submit_run_rows(admitted, &request, "owner")?;
            }
            Ok(())
        })
        .expect("seed 10 000 WorkItems");
    }
    let mut measured = Vec::with_capacity(samples);
    for sample in 0..samples {
        // A fixed stride visits the whole store deterministically.
        let run = run_id((sample * 7_919) % WORK_ITEMS);
        let started = Instant::now();
        let view =
            run_owner::read_admission(shard, GRAPH, "tenant", &run, "owner", DurableCrypto::none())
                .expect("read admission");
        measured.push(started.elapsed().as_secs_f64());
        assert!(view.is_some(), "the owner recovers {run}");
    }
    result(
        "run_owner_recovery",
        format!("work_items={WORK_ITEMS} reads={samples}"),
        &measured,
        None,
        None,
    )
}

// ── current_plan_publication_readback (MEDULLA-DURABLE-KERNEL-R028.5.4) ──────

fn plan_request(
    revision: u64,
) -> medulla_decision_outcome_types::current_plan::PublishCurrentPlanRequest {
    use medulla_decision_outcome_types::current_plan::{
        CurrentPlanSnapshot, PlanItem, PlanStatus, PublishCurrentPlanRequest,
    };
    PublishCurrentPlanRequest {
        tenant: "tenant".into(),
        worker_id: "worker".into(),
        lease_epoch: 1,
        fencing_token: 2,
        idempotency_key: format!("bench-publish-{revision}"),
        task_plan_proposal: None,
        snapshot: CurrentPlanSnapshot {
            run_id: "run".into(),
            revision,
            observed_at_ms: 1_000,
            expires_at_ms: 9_000,
            items: (0..PLAN_TASKS)
                .map(|task| PlanItem {
                    text: format!("task {task:02} of revision {revision}"),
                    progress: PlanStatus::Pending,
                })
                .collect(),
            task_plan: None,
        },
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R028.5.4
/// Publish and read back a 64-task current plan on one claimed run.
pub fn current_plan_publication_readback(samples: usize) -> GroupResult {
    let temp = test_shard::open("bench-current-plan");
    let shard = &temp.shard;
    run_fixture::seed_claim(shard, |_| {});
    let mut measured = Vec::with_capacity(samples);
    let mut transient = 0usize;
    for revision in 0..samples as u64 {
        let request = plan_request(revision);
        let baseline = memory_mark();
        let started = Instant::now();
        run_fixture::with_rows(shard, &format!("bench-publish-{revision}"), 2_000, |rows| {
            current_plan::publish(
                GRAPH,
                &request,
                Some("owner"),
                rows,
                DurableCrypto::none(),
                2_000,
            )
        })
        .expect("publish the current plan");
        let read = current_plan::read_current(
            shard,
            GRAPH,
            "tenant",
            "run",
            "owner",
            DurableCrypto::none(),
            2_000,
        )
        .expect("read the current plan");
        measured.push(started.elapsed().as_secs_f64());
        transient = transient.max(memory_since(baseline));
        assert_eq!(
            read,
            Some(request.snapshot),
            "readback matches revision {revision}"
        );
    }
    result(
        "current_plan_publication_readback",
        format!("tasks={PLAN_TASKS} publications={samples}"),
        &measured,
        Some(transient),
        Some(CURRENT_PLAN_MEMORY_BUDGET),
    )
}

// ── paired_pending_consumption (MEDULLA-DURABLE-KERNEL-R039.4.2) ─────────────

fn identifier(prefix: &str) -> String {
    let mut id = prefix.to_string();
    id.push_str(&"x".repeat(IDENTIFIER_BYTES - prefix.len()));
    id
}

/// A grant padded so its JSON form is exactly the grant bound (64 KiB).
fn padded_grant(
    mut grant: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let bound = medulla_work_types::control_lease::MAX_CONTROL_LEASE_GRANT_BYTES;
    let base = serde_json::to_vec(&grant).expect("grant json").len();
    // `,"preimage":""` adds 14 bytes around the padding.
    let pad = bound.saturating_sub(base + 14);
    grant.insert("preimage".into(), "p".repeat(pad).into());
    grant
}

fn lease_request(
    lease_id: &str,
    kind: &str,
    grant: serde_json::Map<String, serde_json::Value>,
) -> medulla_work_types::control_lease::IssueControlLeaseRequest {
    medulla_work_types::control_lease::IssueControlLeaseRequest {
        pending_call_registration: None,
        tenant: "tenant".into(),
        lease_id: lease_id.into(),
        kind: kind.into(),
        grant,
        issued_at_ms: 1_000,
        // A 24-hour lifetime: the native span bounds `hard_expires - issued`.
        expires_at_ms: 1_000 + DAY_MS,
        hard_expires_at_ms: 1_000 + DAY_MS,
        idempotency_key: format!("issue:{}", &lease_id[..16]),
    }
}

fn applied(payload: ResultPayload) -> serde_json::Value {
    match payload {
        ResultPayload::Json(value) => value,
        other => panic!("control-lease results are JSON, got {other:?}"),
    }
}

/// One sample: seed a claimed run, issue the plan lease and the plan-bound
/// pending call, then time the paired consumption.
fn paired_sample(sample: usize) -> f64 {
    use medulla_work_types::control_lease::{
        ControlLeaseTarget, PendingCallConsumption, PendingCallFence, PendingCallRegistration,
        PendingPlanPreimage, TransitionControlLeaseRequest, PENDING_CALL_KIND,
    };
    let temp = test_shard::open(&format!("bench-paired-{sample}"));
    let shard = &temp.shard;
    let lease_seconds = (1_000 + DAY_MS) as f64 / 1_000.0;
    run_fixture::seed_claim(shard, |row| {
        row.insert("attempt".into(), 1.into());
        row.insert("work_item_fence".into(), "native-fence".into());
        row.insert("lease_expires_at".into(), lease_seconds.into());
        row.insert("metadata".into(), serde_json::json!({"task": "task-a"}));
    });
    let work_item_id = run_owner::work_item_id(GRAPH, "tenant", "run").expect("work item id");

    let mut plan_grant = serde_json::Map::new();
    plan_grant.insert("tool_ids".into(), serde_json::json!(["deploy"]));
    let plan = lease_request(
        &identifier("plan-"),
        "deployment.plan-kind",
        padded_grant(plan_grant),
    );
    test_shard::apply(
        shard,
        &format!("bench-plan-{sample}"),
        Method::IssueControlLease {
            request: plan.clone(),
        },
        2_000,
    )
    .expect("issue the plan lease");

    let pending_grant = serde_json::json!({
        "task_id": "task-a", "parent_work_item_id": "parent-a",
        "child_work_item_id": work_item_id, "op_id": "tool-a",
        "params_digest": "immutable-source-digest"
    })
    .as_object()
    .expect("grant object")
    .clone();
    let mut pending = lease_request(&identifier("pending-"), PENDING_CALL_KIND, pending_grant);
    let fence = PendingCallFence {
        work_item_id: work_item_id.clone(),
        expected_revision: 1,
        expected_metadata: serde_json::json!({"task": "task-a"})
            .as_object()
            .expect("metadata object")
            .clone(),
    };
    let plan_preimage = PendingPlanPreimage {
        lease_id: plan.lease_id.clone(),
        expected_revision: 1,
        expected_kind: plan.kind.clone(),
        expected_grant: plan.grant.clone(),
    };
    pending.pending_call_registration = Some(PendingCallRegistration {
        plan: Some(plan_preimage.clone()),
        work_item: fence.clone(),
        expected_lease:
            medulla_result_types::epistemic_operations_ext::CasWorkItemMetadataLeaseFence {
                worker_ref: "worker".into(),
                lease_epoch: 1,
                fencing_token: 2,
            },
    });
    test_shard::apply(
        shard,
        &format!("bench-pending-{sample}"),
        Method::IssueControlLease {
            request: pending.clone(),
        },
        2_000,
    )
    .expect("register the plan-bound pending call");

    let consume = TransitionControlLeaseRequest {
        pending_call_consumption: Some(PendingCallConsumption {
            plan: Some(plan_preimage),
            work_item: PendingCallFence {
                expected_revision: 2,
                ..fence
            },
            expected_grant: pending.grant.clone(),
        }),
        tenant: "tenant".into(),
        lease_id: pending.lease_id.clone(),
        expected_revision: 1,
        to: ControlLeaseTarget::Consumed,
        idempotency_key: format!("bench-consume-{sample}"),
        decided_by: String::new(),
    };
    let started = Instant::now();
    let payload = test_shard::apply(
        shard,
        &format!("bench-consume-{sample}"),
        Method::TransitionControlLease { request: consume },
        2_000,
    )
    .expect("consume the pending pair");
    let elapsed = started.elapsed().as_secs_f64();
    let transition = applied(payload);
    assert_eq!(
        transition["outcome"],
        serde_json::json!("applied"),
        "the paired consumption applies: {transition}"
    );
    elapsed
}

// spec: MEDULLA-DURABLE-KERNEL-R039.4.2
/// Paired pending consumption: three rows in one transaction, 64 KiB preimages.
pub fn paired_pending_consumption(samples: usize) -> GroupResult {
    let measured: Vec<f64> = (0..samples).map(paired_sample).collect();
    result(
        "paired_pending_consumption",
        format!(
            "preimage_bytes={} identifier_bytes={IDENTIFIER_BYTES} lifetime_ms={DAY_MS} \
             rows_per_transaction=3 samples={samples}",
            medulla_work_types::control_lease::MAX_CONTROL_LEASE_GRANT_BYTES
        ),
        &measured,
        None,
        None,
    )
}
