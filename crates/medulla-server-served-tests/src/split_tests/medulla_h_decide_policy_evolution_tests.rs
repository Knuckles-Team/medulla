//! The served `PolicyEvolution` path: the five immutable policy records, written
//! through the real request boundary in dependency order, replayed, and read
//! back under another tenant (MEDULLA-DECISION-ENGINE-R811).

use medulla_h_decide::decide::stat_test_support::Harness;
use medulla_protocol_types::protocol::{Method, Request, Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use serde_json::{json, Value};

const GRAPH: &str = "__commons__";
const TENANT: &str = "tenant-shared";
const OTHER_TENANT: &str = "tenant-other";
const WRITER: &str = "trainer";
const TRAJECTORY: &str = "trajectory:0001";
const TRAJECTORY_STEPS: u64 = 3;

fn hex(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}

async fn dispatch(h: &Harness, tenant: &str, scopes: &[&str], method: Value) -> Response {
    let method: Method = serde_json::from_value(method).expect("a well-formed method");
    Box::pin(
        medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
            &h.state,
            Request {
                id: 811,
                graph: GRAPH.into(),
                auth_token: String::new(),
                agent_id: Some(WRITER.into()),
                method,
                trace_id: None,
            },
            VerifiedRequestContext::verified_for_test_with_scopes(WRITER, tenant, scopes),
        ),
    )
    .await
}

fn value_of(label: &str, response: Response) -> Value {
    assert!(
        response.error.is_none(),
        "{label} was refused: {response:?}"
    );
    match response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("{label} result does not decode: {error}")),
        Some(ResultPayload::Count(count)) => json!(count),
        Some(ResultPayload::String(text)) => json!(text),
        Some(ResultPayload::Json(value)) => value,
        other => panic!("{label} answered an unexpected payload: {other:?}"),
    }
}

/// Upload `bytes` as the writer's own blob and return its digest.
async fn upload(h: &Harness, bytes: &[u8]) -> String {
    let all = ["*"];
    let cursor = value_of(
        "BlobBegin",
        dispatch(
            h,
            TENANT,
            &all,
            json!({"method": "BlobBegin", "params": {"chunk_size": 1 << 20}}),
        )
        .await,
    );
    value_of(
        "BlobChunkPut",
        dispatch(
            h,
            TENANT,
            &all,
            json!({"method": "BlobChunkPut", "params": {"cursor": cursor, "data": bytes}}),
        )
        .await,
    );
    let digest = value_of(
        "BlobCommit",
        dispatch(
            h,
            TENANT,
            &all,
            json!({"method": "BlobCommit", "params": {"cursor": cursor}}),
        )
        .await,
    );
    digest.as_str().expect("a blob digest").to_string()
}

fn held(digest: &str, bytes: usize, encoding: &str, elements: u32) -> Value {
    json!({"digest": digest, "length": bytes, "encoding": encoding, "elements": elements})
}

async fn capture_body(h: &Harness, capability_id: &str, sampler_id: &str) -> Value {
    let token_ids: Vec<u8> = [1_u32, 2, 7, 9, 11, 40]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let log_q: Vec<u8> = [-0.25_f32, -1.5]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    let mask = [0_u8, 0, 1, 1, 0, 0];
    json!({
        "capability_id": capability_id, "sampler_version_id": sampler_id,
        "trajectory_id": TRAJECTORY, "trajectory_steps": TRAJECTORY_STEPS, "completion": "terminal",
        "token_count": 6, "policy_token_count": 2,
        "token_ids": held(&upload(h, &token_ids).await, token_ids.len(), "u32_le", 6),
        "log_q": held(&upload(h, &log_q).await, log_q.len(), "f32_le", 2),
        "action_mask": held(&upload(h, &mask).await, mask.len(), "u8_mask", 6),
        "reward": {"value_micros": 1_000_000, "verifier_id": "tests", "verifier_digest": hex(9),
                   "evidence": "independent_verifier"},
        "purpose": "training", "trace_fidelity": "full", "captured_at_ms": 5,
    })
}

fn op(name: &str, request: Value) -> Value {
    json!({"method": "PolicyEvolution", "params": {"op": {"op": name, "request": request}}})
}

/// Write `request` twice: the first receipt is `written`, the repeat `replayed`
/// with the same record id. Returns the record id.
async fn write_and_replay(h: &Harness, scope: &str, name: &str, request: Value) -> String {
    let scopes = [scope];
    let first = value_of(name, dispatch(h, TENANT, &scopes, op(name, request.clone())).await);
    assert_eq!(first["disposition"], "written", "{name}: {first}");
    let repeat = value_of(name, dispatch(h, TENANT, &scopes, op(name, request)).await);
    assert_eq!(repeat["disposition"], "replayed", "{name}: {repeat}");
    assert_eq!(repeat["record_id"], first["record_id"], "{name}");
    first["record_id"].as_str().expect("a record id").to_string()
}

// spec: MEDULLA-DECISION-ENGINE-R811.3
#[tokio::test]
async fn capture_train_evaluate_and_promote() {
    let h = Harness::with_isolation(ServerState::test_isolation(WRITER)).await;
    h.seed_node(
        GRAPH,
        TRAJECTORY,
        json!({"type": "Trajectory", "step_count": TRAJECTORY_STEPS}),
    )
    .await;

    let capability = json!({
        "provider": "vllm", "endpoint_ref": "endpoint:gb10", "base_checkpoint_digest": hex(1),
        "tokenizer_digest": hex(2), "decode_params_digest": hex(3),
        "artifact_destination_ref": "artifacts:new", "probe_digest": hex(4), "probed_at_ms": 1,
        "logprobs": {"chosen_token": true},
        "controls": {"capture": {"enabled": true, "scope": "ops"},
                     "train": {"enabled": true, "scope": "ops"}},
    });
    let capability_id =
        write_and_replay(&h, "admin:policy-capability", "put_capability", capability).await;

    // The capture names the version that sampled it, so the base version is
    // registered before the capture is committed.
    let base = json!({
        "checkpoint_digest": hex(1), "tokenizer_digest": hex(2),
        "artifact_ref": "artifacts:base", "origin": {"origin": "base"},
    });
    let base_id = write_and_replay(
        &h,
        "admin:model-policy-register",
        "register_model_policy_version",
        base,
    )
    .await;

    let capture = capture_body(&h, &capability_id, &base_id).await;
    let capture_id =
        write_and_replay(&h, "policy:capture-write", "commit_capture", capture).await;

    let run = json!({
        "capability_id": capability_id, "work_item_id": "work:1", "base_version_id": base_id,
        "input_capture_ids": [capture_id],
        "method": {"method": "klpo", "estimator": {"estimator": "sampled_token"}},
        "adapter": {"adapter": "lora", "rank": 8},
        "trainer_image_digest": hex(10), "hyperparameters_digest": hex(11),
        "status": {"status": "succeeded",
                   "output": {"artifact_digest": hex(12), "artifact_ref": "artifacts:v2"}},
    });
    let run_id = write_and_replay(&h, "admin:training-run-write", "commit_training_run", run).await;

    let trained = json!({
        "checkpoint_digest": hex(1), "adapter_digest": hex(12), "tokenizer_digest": hex(2),
        "artifact_ref": "artifacts:v2", "parent_version_id": base_id,
        "origin": {"origin": "trained", "training_run_id": run_id},
    });
    let trained_id = write_and_replay(
        &h,
        "admin:model-policy-register",
        "register_model_policy_version",
        trained,
    )
    .await;

    let evaluation = json!({
        "subject": {"kind": "model_policy", "family_ref": "family:policy"},
        "revision_ref": trained_id, "champion_revision_ref": base_id,
        "evaluation_set_digest": hex(20), "evaluator_digest": hex(21), "evaluator_kind": "annotation",
        "held_out_trajectory_ids": ["trajectory:0002"],
        "metrics": {"task_success_ppm": 800_000, "baseline_task_success_ppm": 600_000,
                    "cost_micros": 10, "latency_p50_ms": 5, "latency_p95_ms": 9, "gpu_seconds": 1,
                    "trace_completeness_ppm": 1_000_000, "unsupported_replay_mass_ppm": 0},
        "safety": "passed", "verdict": "accepted",
    });
    write_and_replay(
        &h,
        "admin:policy-evaluation-write",
        "commit_policy_evaluation",
        evaluation,
    )
    .await;

    let get = |record_id: &str| op("get", json!({"record_id": record_id}));
    let read = value_of(
        "get",
        dispatch(&h, TENANT, &["policy:read"], get(&run_id)).await,
    );
    assert_eq!(read["record_id"], run_id);
    assert_eq!(read["record"]["kind"], "training_run");
    assert!(read["recorded_by"].as_str().is_some_and(|by| !by.is_empty()));

    // Another tenant reading the same id is told there is no such record: the
    // run is never returned.
    let other = value_of(
        "get under another tenant",
        dispatch(&h, OTHER_TENANT, &["policy:read"], get(&run_id)).await,
    );
    assert!(other.is_null(), "another tenant saw the record: {other}");
}
