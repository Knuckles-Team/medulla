//! MEDULLA-DECISION-ENGINE-R713.8 nightly budget: the `decide/skill_telemetry` group.
//!
//! Ingests 1,000,000 skill invocations through the served `Method::SkillTelemetry` handler in
//! batches of 500, then reads the twelve measures of one skill. Budget: measures read p95 at most
//! 50 ms. The report is one JSON object on stdout; a missed budget exits non-zero, which fails the
//! nightly run. A benchmark never gates a PR. The baseline entry lives in
//! `benches/baselines/decide.json`.
//!
//! Run: cargo bench --features full --bench skill_telemetry

use std::time::{Duration, Instant};

use medulla_decision_types::skill_telemetry::{principal_pseudonym, SkillTelemetryOp, Window};
use medulla_h_decide::decide::stat_test_support::{Harness, TENANT};
use medulla_protocol_types::protocol::{Method, Request, Response};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use serde_json::{json, Value};

const GRAPH: &str = "__commons__";
const INVOCATIONS: usize = 1_000_000;
const BATCH: usize = 500;
const PRINCIPALS: usize = 1_000;
const DAY_MS: u64 = 86_400_000;
const DAYS: u64 = 30;
const SKILL: &str = "skill:bench@1";
const MEASURES_P95_MS: f64 = 50.0;
const MEASURE_READS: usize = 100;

async fn send(h: &Harness, scopes: &[&str], op: SkillTelemetryOp) -> Response {
    Box::pin(
        medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
            &h.state,
            Request {
                id: 7138,
                graph: GRAPH.into(),
                auth_token: String::new(),
                agent_id: Some("telemetry".into()),
                method: Method::SkillTelemetry { op: Box::new(op) },
                trace_id: None,
            },
            VerifiedRequestContext::verified_for_test_with_scopes("telemetry", TENANT, scopes),
        ),
    )
    .await
}

fn event(index: usize, pseudonyms: &[String]) -> Value {
    let started = (index as u64 % DAYS) * DAY_MS + index as u64 % 1_000;
    json!({
        "invocation_id": format!("bench-{index}"),
        "tenant_id": TENANT,
        "skill_ref": SKILL,
        "skill_version": "1",
        "chosen_by": if index % 4 == 0 { "user" } else { "agent" },
        "principal_pseudonym": pseudonyms[index % PRINCIPALS],
        "run_id": format!("bench-run-{}", index % 64),
        "started_ms": started,
        "ended_ms": started + 50 + (index as u64 % 200),
        "outcome": if index % 10 == 0 { json!({"error": {"code": "TOOL_TIMEOUT"}}) } else { json!("ok") },
        "cost_delta_micro_usd": (index % 7) as i64,
        "tokens_in": 10,
        "tokens_out": 20
    })
}

fn percentile_ms(samples: &mut [Duration], quantile: f64) -> f64 {
    samples.sort();
    let index = ((samples.len() as f64 * quantile).ceil() as usize).saturating_sub(1);
    samples[index.min(samples.len() - 1)].as_secs_f64() * 1_000.0
}

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let pseudonyms: Vec<String> = (0..PRINCIPALS)
        .map(|n| principal_pseudonym(TENANT, &format!("bench-user-{n}"), 1))
        .collect();
    let p95 = runtime.block_on(async {
        let h = Harness::with_isolation(ServerState::test_isolation("telemetry")).await;
        let core = h
            .state
            .read()
            .await
            .registry
            .get(GRAPH)
            .expect("graph exists")
            .core
            .clone();
        core.add_node(
            SKILL.to_string(),
            rmp_serde::to_vec_named(&json!({"type": "Skill", "tenant_id": TENANT})).unwrap(),
        );
        for start in (0..INVOCATIONS).step_by(BATCH) {
            let events = (start..start + BATCH)
                .map(|n| event(n, &pseudonyms))
                .collect();
            let response = send(
                &h,
                &["telemetry:write"],
                SkillTelemetryOp::Ingest {
                    graph: GRAPH.into(),
                    events,
                },
            )
            .await;
            assert!(response.error.is_none(), "batch at {start}: {response:?}");
        }
        let mut reads = Vec::with_capacity(MEASURE_READS);
        for _ in 0..MEASURE_READS {
            let read = SkillTelemetryOp::Measures {
                graph: GRAPH.into(),
                skill_ref: SKILL.into(),
                window: Window {
                    start_ms: 0,
                    end_ms: DAYS * DAY_MS,
                },
            };
            let started = Instant::now();
            let response = send(&h, &["telemetry:read"], read).await;
            reads.push(started.elapsed());
            assert!(response.error.is_none(), "{response:?}");
        }
        percentile_ms(&mut reads, 0.95)
    });
    println!(
        "{{\"bench\":\"decide/skill_telemetry\",\"invocations\":{INVOCATIONS},\
         \"measures_p95_ms\":{p95:.3},\"budgets\":{{\"measures_p95_ms\":{MEASURES_P95_MS}}}}}"
    );
    if p95 > MEASURES_P95_MS {
        eprintln!("decide/skill_telemetry exceeded its budget");
        std::process::exit(1);
    }
}
