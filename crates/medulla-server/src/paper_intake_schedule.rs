//! The boot registration of the paper intake schedule (MEDULLA-REPO-INGEST-R006.6).
//!
//! The schedule `paper_intake` is a `Schedule` node of the commons graph written under the engine
//! service principal. Its interval is the knob `paper_intake.interval` seconds, aligned to UTC
//! (an interval that divides the day fires on the same wall-clock boundaries every day). The
//! schedule tick of `medulla-h-jobs` fires it: each firing enqueues one `paper_intake` task on
//! the ingest queue. Registering again with the same interval writes nothing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use medulla_h_jobs::jobs::schedule_node_operation;
use medulla_jobs::schedule::{
    ScheduleRecord, ScheduleSpec, PAPER_INTAKE_SCHEDULE, PAPER_INTAKE_TASK_KIND,
};
use medulla_jobs::schedule_run::ScheduleBudget;
use medulla_protocol_types::protocol::{Method, Request, ResultPayload};
use medulla_server_api::carrier::CarrierAuthority;
use medulla_server_api::state::ServerState;
use medulla_types::schedule_machine::ScheduleState;
use tokio::sync::RwLock;

/// The service actor the registration runs under.
pub const PAPER_INTAKE_ACTOR: &str = "engine:paper-intake";

const COMMONS_GRAPH: &str = "__commons__";

static CALLS: AtomicU64 = AtomicU64::new(0);

/// The record the schedule holds for `interval_s`, owned by `owner` in `tenant`.
pub fn paper_intake_record(tenant: &str, owner: &str, interval_s: u64) -> ScheduleRecord {
    let knobs = medulla_server_api::knob_config::knobs();
    ScheduleRecord {
        name: PAPER_INTAKE_SCHEDULE.to_string(),
        owner: owner.to_string(),
        tenant: tenant.to_string(),
        spec: ScheduleSpec::Interval {
            every_ms: interval_s.saturating_mul(1_000),
            start_ms: 0,
        },
        task_kind: PAPER_INTAKE_TASK_KIND.to_string(),
        state: ScheduleState::default(),
        resumed_ms: 0,
        budget: ScheduleBudget {
            max_run_s: knobs.jobs_schedule_budget_run_s,
            max_tokens: knobs.jobs_schedule_budget_tokens,
            period_s: knobs.jobs_schedule_budget_period_s,
        },
        exclusion_group: None,
        min_spacing_s: 0,
    }
}

/// Register the schedule in the commons graph; `true` when a write happened. A commons graph
/// that is not resident yet registers nothing and answers `false`, so a later pass registers it.
pub async fn register(state: &Arc<RwLock<ServerState>>) -> Result<bool, String> {
    let (resident, anonymous, server) = {
        let current = state.read().await;
        (
            current.registry.get(COMMONS_GRAPH).is_some(),
            current.local_anonymous,
            current.context.clone(),
        )
    };
    if !resident {
        return Ok(false);
    }
    let context = crate::commons_reconcile::service_context(
        anonymous,
        &server,
        PAPER_INTAKE_ACTOR,
        format!("paper-intake:{}", CALLS.fetch_add(1, Ordering::Relaxed)),
    )?;
    let owner = CarrierAuthority::from_verified(&context)?
        .actor_scope()
        .to_string();
    let tenant = context.tenant().to_string();
    let interval_s = medulla_server_api::knob_config::knobs().paper_intake_interval;
    let record = paper_intake_record(&tenant, &owner, interval_s);
    let request = |method: Method| Request {
        id: 0,
        graph: COMMONS_GRAPH.to_string(),
        auth_token: String::new(),
        agent_id: Some(PAPER_INTAKE_ACTOR.to_string()),
        method,
        trace_id: None,
    };
    let held = medulla_server_api::api::reentry::dispatch_verified_request(
        state,
        request(Method::GetNodeProperties {
            node_id: ScheduleRecord::node_id(&tenant, PAPER_INTAKE_SCHEDULE),
        }),
        context.clone(),
    )
    .await;
    let existing: Option<ScheduleRecord> = match held.result {
        Some(ResultPayload::Raw(bytes)) => medulla_types::msgpack::decode_property_object(&bytes)
            .ok()
            .and_then(|node| {
                node.get("schedule_body")?
                    .as_str()
                    .and_then(|body| serde_json::from_str(body).ok())
            }),
        Some(ResultPayload::Json(serde_json::Value::Object(node))) => node
            .get("schedule_body")
            .and_then(serde_json::Value::as_str)
            .and_then(|body| serde_json::from_str(body).ok()),
        _ => None,
    };
    // The same interval is already registered: keep it, with its pause state and cursor.
    if existing
        .as_ref()
        .is_some_and(|held| held.spec == record.spec)
    {
        return Ok(false);
    }
    // A changed interval keeps the schedule's pause state and resume time.
    let record = match existing {
        Some(held) => ScheduleRecord {
            state: held.state,
            resumed_ms: held.resumed_ms,
            ..record
        },
        None => record,
    };
    let operation = schedule_node_operation(&record, &tenant)?;
    let operations_msgpack = rmp_serde::to_vec_named(&vec![operation])
        .map_err(|_| "schedule batch encoding failed".to_string())?;
    let written = medulla_server_api::api::reentry::dispatch_verified_request(
        state,
        request(Method::BatchUpdate { operations_msgpack }),
        context,
    )
    .await;
    match written.refusal_text() {
        Some(refusal) => Err(refusal),
        None => Ok(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-REPO-INGEST-R006.6
    #[test]
    fn the_record_fires_every_interval_aligned_to_utc() {
        let record = paper_intake_record("acme", "owner", 21_600);
        record.validate().expect("the record is a valid schedule");
        assert_eq!(record.task_kind, "paper_intake");
        // Two boundaries, six hours apart, from the epoch grid.
        let six_hours = 21_600_000u64;
        let windows = record
            .spec
            .due_windows(5 * six_hours + 1, 7 * six_hours + 1)
            .unwrap();
        assert_eq!(windows, vec![6 * six_hours, 7 * six_hours]);
    }
}
