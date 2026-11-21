//! MEDULLA-CQ-ORCH-R700.3, R700.4, R700.5: the served `ProcessCompile` through the real
//! `dispatch` shell over a redb-backed `ServerState`.
//!
//! A compile reads the tenant-visible process, writes the next `WorkflowPattern` version and
//! its `compiledFromProcess` edge as one batch under a compare-and-set on `expected_version`, and a
//! refusal writes nothing.

#![cfg(feature = "redb")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::process_import::ProcessCompiled;
use serde_json::{json, Value};

const SECRET: &str = "process-compile-served-secret";
const ENCRYPTION_KEY: &str = "process-compile-served-key-0123456789ab";
const GRAPH: &str = "processes";
const PROCESS: &str = "process:invoice";

async fn send(state: &test_support::SharedState, id: u64, method: Method) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await
}

fn payload<T: serde::de::DeserializeOwned>(response: Response) -> T {
    assert!(response.error.is_none(), "refused: {:?}", response.error);
    match response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).expect("typed answer"),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed answer"),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn core_version(state: &test_support::SharedState) -> u64 {
    state
        .try_read()
        .unwrap()
        .registry
        .get(GRAPH)
        .unwrap()
        .core
        .version()
}

fn stored(state: &test_support::SharedState, id: &str) -> Option<serde_json::Map<String, Value>> {
    let guard = state.try_read().unwrap();
    let blob = guard.registry.get(GRAPH).unwrap().core.get_node_properties(id)?;
    medulla_types::msgpack::decode_property_object(&blob).ok()
}

fn node(id: &str, class: &str) -> Value {
    json!({"op": "upsert_node", "id": id, "properties": {"type": class, "name": id}})
}

fn flow(source: &str, target: &str, relationship: &str, loop_gate: bool) -> Value {
    json!({"op": "upsert_edge", "source": source, "target": target,
           "properties": {"relationship": relationship, "loopGate": loop_gate}})
}

/// A process of a start event, a user task and an end event: `hasStep` to the start, a
/// `handsOffTo` chain through the rest.
fn chain() -> Vec<Value> {
    vec![
        node(PROCESS, "Process"),
        node("step:start", "startEvent"),
        node("step:check", "userTask"),
        node("step:end", "endEvent"),
        flow(PROCESS, "step:start", "hasStep", false),
        flow("step:start", "step:check", "handsOffTo", false),
        flow("step:check", "step:end", "handsOffTo", false),
    ]
}

async fn seed(state: &test_support::SharedState, id: u64, operations: Vec<Value>) {
    let response = send(
        state,
        id,
        Method::BatchUpdate {
            operations_msgpack: rmp_serde::to_vec_named(&operations).unwrap(),
        },
    )
    .await;
    assert!(response.error.is_none(), "seed refused: {:?}", response.error);
}

async fn compile(
    state: &test_support::SharedState,
    id: u64,
    process_iri: &str,
    expected_version: Option<u32>,
) -> Response {
    send(
        state,
        id,
        Method::ProcessCompile {
            process_iri: process_iri.into(),
            expected_version,
        },
    )
    .await
}

async fn graph_state(
    name: &str,
) -> (
    std::path::PathBuf,
    test_support::SharedPersistence,
    test_support::SharedState,
) {
    test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        name,
        common::current_isolation(),
        GRAPH,
    )
    .await
}

// spec: MEDULLA-CQ-ORCH-R700.3
#[tokio::test(flavor = "multi_thread")]
async fn second_compile_writes_next_version_and_cas_refuses_stale() {
    let (_dir, _backend, state) = graph_state("process-compile-served-cas").await;
    seed(&state, 1, chain()).await;

    let before = core_version(&state);
    let first: ProcessCompiled = payload(compile(&state, 2, PROCESS, Some(0)).await);
    assert_eq!(first.version, 1);
    assert_eq!(first.workflow_id, "workflow:process:invoice:v1");
    assert_eq!(core_version(&state), before + 1, "one compile commits one batch");
    let node = stored(&state, &first.workflow_id).expect("the version is stored");
    assert_eq!(node["type"], "WorkflowPattern");
    assert_eq!(node["workflowDigest"], first.workflow_digest.as_str());

    let second: ProcessCompiled = payload(compile(&state, 3, PROCESS, Some(1)).await);
    assert_eq!(second.version, 2);
    assert_eq!(second.workflow_digest, first.workflow_digest, "an unchanged process");
    assert!(stored(&state, "workflow:process:invoice:v1").is_some(), "v1 is kept");

    let version = core_version(&state);
    let stale = compile(&state, 4, PROCESS, Some(1)).await;
    assert_eq!(stale.error.as_deref(), Some("WORKFLOW_VERSION_CONFLICT"));
    assert!(stored(&state, "workflow:process:invoice:v3").is_none());
    assert_eq!(core_version(&state), version, "a stale compile writes nothing");
}

// spec: MEDULLA-CQ-ORCH-R700.4
#[tokio::test(flavor = "multi_thread")]
async fn compile_refusals_leave_state_unchanged() {
    let (_dir, _backend, state) = graph_state("process-compile-served-refusals").await;
    let mut cyclic = chain();
    cyclic.push(node("process:loop", "Process"));
    cyclic.extend([
        node("loop:a", "userTask"),
        node("loop:b", "userTask"),
        flow("process:loop", "loop:a", "hasStep", false),
        flow("loop:a", "loop:b", "handsOffTo", false),
        flow("loop:b", "loop:a", "handsOffTo", false),
    ]);
    cyclic.extend([
        node("process:open", "Process"),
        node("open:start", "startEvent"),
        node("open:a", "userTask"),
        flow("process:open", "open:start", "hasStep", false),
        flow("open:start", "open:a", "handsOffTo", false),
    ]);
    seed(&state, 1, cyclic).await;

    let version = core_version(&state);
    let cases = [
        ("process:missing", "PROCESS_UNKNOWN", ""),
        ("process:loop", "PROCESS_CYCLE_WITHOUT_GATE", "loop:a, loop:b"),
        ("process:open", "PROCESS_TASK_UNBOUND", "open:a"),
    ];
    for (offset, (process, code, named)) in cases.into_iter().enumerate() {
        let response = compile(&state, 10 + offset as u64, process, None).await;
        assert_eq!(response.error.as_deref(), Some(code), "{process}");
        let detail = response.error_detail.unwrap_or_default();
        assert!(detail.contains(named), "{process}: {detail}");
        assert!(response.result.is_none(), "{process}: a refusal has no answer");
        assert_eq!(core_version(&state), version, "{process}: nothing committed");
    }
    assert!(stored(&state, "workflow:process:loop:v1").is_none());
}

// spec: MEDULLA-CQ-ORCH-R700.5
#[tokio::test(flavor = "multi_thread")]
async fn replay_reproduces_recorded_digest() {
    let (_dir, _backend, state) = graph_state("process-compile-served-replay").await;
    seed(&state, 1, chain()).await;

    let first: ProcessCompiled = payload(compile(&state, 2, PROCESS, None).await);
    assert_eq!(first.process_revision.len(), 64);
    assert!(first
        .process_revision
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    let edge_node = stored(&state, &first.workflow_id).unwrap();
    assert_eq!(edge_node["processRevision"], first.process_revision.as_str());

    // Replay over the cited revision: the same process gives the recorded digest.
    let replay: ProcessCompiled = payload(compile(&state, 3, PROCESS, Some(1)).await);
    assert_eq!(replay.workflow_digest, first.workflow_digest);
    assert_eq!(replay.process_revision, first.process_revision);

    // Editing the process changes its revision and its digest.
    seed(
        &state,
        4,
        vec![
            node("step:review", "userTask"),
            flow("step:check", "step:review", "handsOffTo", false),
            flow("step:review", "step:end", "handsOffTo", false),
        ],
    )
    .await;
    let edited: ProcessCompiled = payload(compile(&state, 5, PROCESS, Some(2)).await);
    assert_ne!(edited.process_revision, first.process_revision);
    assert_ne!(edited.workflow_digest, first.workflow_digest);
}
