//! MEDULLA-DECISION-ENGINE-R801.6 nightly budget: the `usage/ledger` group.
//!
//! Appends 1,000,000 usage records through the served `Method::Usage` handler in 1,000
//! batches of 1,000, then reads a 30-day rollup. Budgets: append p99 2 ms per batch,
//! rollup p99 20 ms, and under 128 MiB of resident growth for the buckets. The report is
//! one JSON object on stdout; a missed budget exits non-zero, which fails the nightly run.
//! A benchmark never gates a PR. The baseline entry lives in `benches/baselines/decide.json`.
//!
//! Run: cargo bench --features full --bench usage_ledger

use std::time::{Duration, Instant};

use medulla_decision_types::usage::{
    PriceCatalog, UsageOp, UsageOutcome, UsageReadView, UsageRecord,
};
use medulla_h_decide::decide::stat_test_support::{Harness, TENANT};
use medulla_protocol_types::protocol::{Method, Request, Response};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

const BATCHES: usize = 1_000;
const BATCH: usize = 1_000;
const DAYS: u64 = 30;
const DAY_MS: u64 = 86_400_000;
const APPEND_P99_MS: f64 = 2.0;
const ROLLUP_P99_MS: f64 = 20.0;
const BUCKET_MIB: f64 = 128.0;
const ROLLUP_READS: usize = 100;

async fn send(h: &Harness, scopes: &[&str], op: UsageOp) -> Response {
    Box::pin(
        medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
            &h.state,
            Request {
                id: 8016,
                graph: "__commons__".into(),
                auth_token: String::new(),
                agent_id: Some("ledger".into()),
                method: Method::Usage { op: Box::new(op) },
                trace_id: None,
            },
            VerifiedRequestContext::verified_for_test_with_scopes("ledger", TENANT, scopes),
        ),
    )
    .await
}

fn record(index: usize, base_ms: u64) -> UsageRecord {
    let day = (index / BATCH) as u64 % DAYS;
    UsageRecord {
        schema_version: 1,
        recorded_at_ms: base_ms + day * DAY_MS + (index % BATCH) as u64,
        child_id: format!("child-{}", index % 64),
        operation: "decide".to_string(),
        tenant: TENANT.to_string(),
        principal: String::new(),
        run_id: format!("run-{index}"),
        model_key: "m1".to_string(),
        tokens_in: 1_000,
        tokens_out: 200,
        tokens_cached: 100,
        latency_ms: 50,
        outcome: UsageOutcome::Ok,
        content_digest: format!("{:064x}", index),
        project_id: String::new(),
        agent_id: String::new(),
    }
}

fn percentile_ms(samples: &mut [Duration], quantile: f64) -> f64 {
    samples.sort();
    let index = ((samples.len() as f64 * quantile).ceil() as usize).saturating_sub(1);
    samples[index.min(samples.len() - 1)].as_secs_f64() * 1_000.0
}

/// Resident high-water mark in MiB (`VmHWM`), 0 where procfs is absent.
fn high_water_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .and_then(|rest| {
                    rest.trim()
                        .trim_end_matches("kB")
                        .trim()
                        .parse::<f64>()
                        .ok()
                })
        })
        .map_or(0.0, |kib| kib / 1024.0)
}

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let base_ms = 46_000 * DAY_MS;
    let (append_p99, rollup_p99, bucket_mib) = runtime.block_on(async {
        let h = Harness::with_isolation(ServerState::test_isolation("ledger")).await;
        let catalog: PriceCatalog = serde_json::from_value(serde_json::json!({
            "entries": [{
                "schema_version": 1, "model_key": "m1", "valid_from_ms": 1,
                "in_per_mtok_micro": 3_000_000, "out_per_mtok_micro": 15_000_000,
                "cached_per_mtok_micro": 300_000
            }]
        }))
        .expect("a price catalog");
        let stored = send(&h, &["usage:write"], UsageOp::PutPrice { catalog }).await;
        assert!(stored.error.is_none(), "{stored:?}");

        let before = high_water_mib();
        let mut appends = Vec::with_capacity(BATCHES);
        for batch in 0..BATCHES {
            let records = (batch * BATCH..(batch + 1) * BATCH)
                .map(|index| record(index, base_ms))
                .collect();
            let started = Instant::now();
            let response = send(&h, &["usage:write"], UsageOp::Append { records }).await;
            appends.push(started.elapsed());
            assert!(response.error.is_none(), "batch {batch}: {response:?}");
        }
        let mut rollups = Vec::with_capacity(ROLLUP_READS);
        for _ in 0..ROLLUP_READS {
            let read = UsageOp::Read {
                group_by: None,
                from_ms: base_ms,
                to_ms: base_ms + DAYS * DAY_MS,
                model_key: None,
                view: UsageReadView::Rollup,
                cursor: None,
            };
            let started = Instant::now();
            let response = send(&h, &["usage:read"], read).await;
            rollups.push(started.elapsed());
            assert!(response.error.is_none(), "{response:?}");
        }
        (
            percentile_ms(&mut appends, 0.99),
            percentile_ms(&mut rollups, 0.99),
            (high_water_mib() - before).max(0.0),
        )
    });
    println!(
        "{{\"bench\":\"usage/ledger\",\"records\":{},\"append_batch_p99_ms\":{append_p99:.3},\
         \"rollup_p99_ms\":{rollup_p99:.3},\"bucket_mib\":{bucket_mib:.1},\
         \"budgets\":{{\"append_batch_p99_ms\":{APPEND_P99_MS},\"rollup_p99_ms\":{ROLLUP_P99_MS},\
         \"bucket_mib\":{BUCKET_MIB}}}}}",
        BATCHES * BATCH
    );
    if append_p99 > APPEND_P99_MS || rollup_p99 > ROLLUP_P99_MS || bucket_mib > BUCKET_MIB {
        eprintln!("usage/ledger exceeded its budget");
        std::process::exit(1);
    }
}
