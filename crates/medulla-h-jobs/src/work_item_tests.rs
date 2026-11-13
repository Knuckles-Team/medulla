//! Terminal commit of a delegated run through the signed dispatch chain:
//! the whole `AddNode` -> `ClaimWorkItem` -> `CommitWorkItemResult` journey,
//! then reads of the stored trace and its `produced` link.
use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::protocol::{GraphType, Method, ResultPayload};
use medulla_result_types::epistemic_operations::{
    ClaimWorkItemRequest, ClaimWorkItemRequestSchemaVersion, ClaimWorkItemResult,
};
use medulla_result_types::outcome_bundle::{
    CommitOutcomeBundle, OutcomeCompleteness, ReceiptNode, ReceiptNodeKind, RunEvent,
    TerminalOutcomeExtension, OUTCOME_BUNDLE_VERSION,
};
use medulla_server_api::state::ServerState;
use medulla_server_auth::auth::{dispatch_test_on_heap, scoped_test_request_on, ScopedTestCaller};
use std::sync::Arc;
use tokio::sync::RwLock;

const SECRET: &str = "terminal-trace-test-secret";
const ISSUER: &str = "terminal-trace-test-issuer";
const TENANT: &str = "tenant-shared";
const GRAPH: &str = "terminal-trace-test";
const ITEM: &str = "work-trace-l3";
const TRACE: &str = "trace:terminal";
const WORKER: &str = "worker-a";

fn digest_for(byte: char) -> String {
    byte.to_string().repeat(64)
}

async fn call(
    state: &Arc<RwLock<ServerState>>,
    id: u64,
    key: &str,
    method: Method,
) -> medulla_protocol_types::protocol::Response {
    call_as(state, id, key, method, TENANT, &["*"]).await
}

/// `call` for a caller of another tenant or with a narrower scope set.
async fn call_as(
    state: &Arc<RwLock<ServerState>>,
    id: u64,
    key: &str,
    method: Method,
    tenant: &str,
    scopes: &[&str],
) -> medulla_protocol_types::protocol::Response {
    let request = scoped_test_request_on(
        SECRET,
        id,
        GRAPH,
        method,
        ScopedTestCaller {
            principal: ISSUER,
            tenant,
            scopes,
            nonce: &format!("terminal-trace-nonce-{id}"),
            idempotency_key: key,
        },
    );
    dispatch_test_on_heap(state, request).await
}

fn payload_value(response: medulla_protocol_types::protocol::Response) -> serde_json::Value {
    assert!(response.error.is_none(), "{:?}", response.error);
    match response.result.expect("a result payload") {
        ResultPayload::Json(value) => value,
        ResultPayload::Raw(bytes) => rmp_serde::from_slice(&bytes).unwrap(),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn trace_record(run_id: &str) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "run_id": run_id,
        "task_signature": "sig:terminal",
        "model_id": "model:terminal",
        "steps": [{"index": 0, "kind": "tool_call", "action_ref": "tool:fixture", "outcome": "ok"}],
        "outcome": "succeeded",
        "started_at_ms": 1
    })
}

/// A receipt whose engine-derived fields (outbox id, payload digest) are left
/// blank: the dispatch compiler binds them to the native mutation batch.
fn receipt(
    bundle: &CommitOutcomeBundle,
    kind: ReceiptNodeKind,
    node_id: &str,
    trace_run: &str,
) -> ReceiptNode {
    let kind_name = match kind {
        ReceiptNodeKind::RunTrace => "run_trace",
        ReceiptNodeKind::ToolCall => "tool_call",
        ReceiptNodeKind::OutcomeEvaluation => "outcome_evaluation",
        ReceiptNodeKind::ModelRequest => "model_request",
        ReceiptNodeKind::RecursiveTraceNode => "recursive_trace_node",
    };
    let payload_ref = format!("cas:receipt:{node_id}");
    let mut properties = serde_json::json!({
        "node_id": node_id, "kind": kind_name,
        "delegation_id": bundle.delegation_id, "delegator_id": bundle.delegator_id,
        "selected_agent_id": bundle.selected_agent_id,
        "executor_lease_actor": bundle.executor_lease_actor,
        "outcome": bundle.outcome, "work_item_id": bundle.work_item_id,
        "run_id": bundle.run_id, "fence_token": bundle.fence_token,
        "result_ref": bundle.result_ref, "result_digest": bundle.result_digest,
        "event_sequence": bundle.event_sequence, "completeness": bundle.completeness,
        "missing_refs": bundle.missing_refs, "outbox_id": "",
        "payload_ref": payload_ref, "capability_digest": bundle.capability_digest,
        "catalog_digest": bundle.catalog_digest, "policy_digest": bundle.policy_digest,
        "model_digest": bundle.model_digest, "payload": {"fixture": true}
    });
    if kind == ReceiptNodeKind::RunTrace {
        properties["run_trace"] = trace_record(trace_run);
    }
    ReceiptNode {
        node_id: node_id.to_string(),
        kind,
        delegation_id: bundle.delegation_id.clone(),
        work_item_id: bundle.work_item_id.clone(),
        run_id: bundle.run_id.clone(),
        fence_token: bundle.fence_token,
        result_ref: bundle.result_ref.clone(),
        outbox_id: String::new(),
        payload_ref,
        payload_digest: String::new(),
        properties_msgpack: rmp_serde::to_vec_named(&properties).unwrap(),
    }
}

fn extension(fence: u64, trace_run: &str) -> TerminalOutcomeExtension {
    extension_with(fence, trace_run, &[])
}

/// A recursive trace node receipt carrying `record` under `recursive_trace`.
fn recursive_receipt(bundle: &CommitOutcomeBundle, record: &serde_json::Value) -> ReceiptNode {
    let id = record["node_id"].as_str().unwrap();
    let mut node = receipt(bundle, ReceiptNodeKind::RecursiveTraceNode, id, "");
    let mut properties: serde_json::Value =
        rmp_serde::from_slice(&node.properties_msgpack).unwrap();
    properties["recursive_trace"] = record.clone();
    node.properties_msgpack = rmp_serde::to_vec_named(&properties).unwrap();
    node
}

fn trace_node(id: &str, parent: Option<&str>, children: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1, "node_id": id, "run_id": "run:terminal",
        "run_revision": 1, "step_id": format!("step:{id}"), "step_revision": 1, "kind": "code_run",
        "parent_node_id": parent, "child_ids": children,
        "input_refs": [], "output_refs": [],
        "evidence_refs": [{"ref_id": format!("evidence:{id}"), "digest": digest_for('a')}],
        "source_revision": 1, "trace_snapshot_id": "snapshot:1"
    })
}

fn extension_with(
    fence: u64,
    trace_run: &str,
    recursive: &[serde_json::Value],
) -> TerminalOutcomeExtension {
    let bundle = CommitOutcomeBundle {
        schema_version: OUTCOME_BUNDLE_VERSION,
        delegation_id: "delegation:terminal".into(),
        delegator_id: "agent:delegator-a".into(),
        selected_agent_id: "agent:selected-b".into(),
        executor_lease_actor: WORKER.into(),
        outcome: "succeeded".into(),
        work_item_id: ITEM.into(),
        fence_token: fence,
        run_id: "run:terminal".into(),
        result_ref: Some("cas:result:terminal".into()),
        result_digest: Some(digest_for('a')),
        artifacts: Vec::new(),
        trace_ref: TRACE.into(),
        tool_call_refs: vec!["toolcall:terminal:0".into()],
        outcome_ref: "outcome:terminal".into(),
        capability_digest: digest_for('b'),
        catalog_digest: digest_for('c'),
        policy_digest: digest_for('d'),
        model_digest: digest_for('e'),
        event_sequence: 1,
        completeness: OutcomeCompleteness::Complete,
        missing_refs: Vec::new(),
        outbox_id: String::new(),
        langfuse_observation_refs: Vec::new(),
    };
    let receipt_nodes = vec![
        receipt(&bundle, ReceiptNodeKind::RunTrace, TRACE, trace_run),
        receipt(
            &bundle,
            ReceiptNodeKind::ToolCall,
            "toolcall:terminal:0",
            trace_run,
        ),
        receipt(
            &bundle,
            ReceiptNodeKind::OutcomeEvaluation,
            "outcome:terminal",
            trace_run,
        ),
    ];
    let mut receipt_nodes = receipt_nodes;
    receipt_nodes.extend(
        recursive
            .iter()
            .map(|record| recursive_receipt(&bundle, record)),
    );
    let run_event = RunEvent {
        schema_version: OUTCOME_BUNDLE_VERSION,
        delegation_id: "delegation:terminal".into(),
        delegator_id: "agent:delegator-a".into(),
        selected_agent_id: "agent:selected-b".into(),
        executor_lease_actor: WORKER.into(),
        outcome: "succeeded".into(),
        work_item_id: ITEM.into(),
        run_id: "run:terminal".into(),
        fence_token: fence,
        outbox_id: String::new(),
        result_ref: Some("cas:result:terminal".into()),
        capability_digest: digest_for('b'),
        catalog_digest: digest_for('c'),
        policy_digest: digest_for('d'),
        model_digest: digest_for('e'),
        event_sequence: 1,
        completeness: OutcomeCompleteness::Complete,
        missing_refs: Vec::new(),
        kind: "outcome".into(),
        tool_call_ref: None,
        outcome_ref: Some("outcome:terminal".into()),
        payload_digest: digest_for('f'),
        timestamp_ms: 10,
        cursor_token: "cursor:terminal:1".into(),
        carrier_digest: digest_for('0'),
    };
    TerminalOutcomeExtension {
        outcome_bundle: bundle,
        receipt_nodes,
        run_event,
    }
}

fn work_item_method(now_ms: u64) -> Method {
    let props = serde_json::json!({
        "node_type": "WorkItem", "tenant": TENANT, "status": "ready",
        "lease_owner": null, "last_lease_owner": null,
        "attempt": 0, "lease_epoch": 0, "fencing_token": 0, "lease_expires_at": 0.0,
        "max_attempts": 3,
        "created_at": now_ms as f64 / 1000.0, "updated_at": now_ms as f64 / 1000.0,
        "heartbeat_at": now_ms as f64 / 1000.0, "next_retry_at": 0.0,
        "prio_bucket": 0, "kind": "delegated_run", "payload_ref": "terminal-trace-payload",
        "downstream_ids": [], "dep_count": 0, "backoff_base_s": 1.0,
        "metadata": {
            "delegation_id": "delegation:terminal", "run_id": "run:terminal",
            "agent_id": "agent:selected-b", "capability_digest": digest_for('b')
        },
        "context": {"agent_id": "agent:delegator-a"},
        "catalog_digest": digest_for('c'), "policy_digest": digest_for('d'),
        "model_digest": digest_for('e')
    });
    Method::AddNode {
        node_id: ITEM.to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&props).unwrap(),
    }
}

fn commit_method(key: &str, lease_epoch: u64, fence: u64, trace_run: &str) -> Method {
    commit_method_with(key, lease_epoch, fence, trace_run, &[])
}

fn commit_method_with(
    key: &str,
    lease_epoch: u64,
    fence: u64,
    trace_run: &str,
    recursive: &[serde_json::Value],
) -> Method {
    Method::CommitWorkItemResult {
        tenant: TENANT.into(),
        work_item_id: ITEM.into(),
        worker_id: WORKER.into(),
        lease_epoch,
        fencing_token: fence,
        idempotency_key: key.into(),
        outcome: "succeeded".into(),
        result_ref: Some("cas:result:terminal".into()),
        outcome_extension: Some(Box::new(extension_with(fence, trace_run, recursive))),
        run_completion: None,
        error_ref: None,
        retryable: false,
        now_ms: medulla_server_api::txn::now_ms(),
    }
}

async fn fixture() -> (tempfile::TempDir, Arc<RwLock<ServerState>>) {
    let dir = tempfile::tempdir().unwrap();
    let mut state = medulla_server::seams_install::new_state_for_test(
        SECRET,
        ServerState::test_isolation(ISSUER),
    );
    state.persist_dir = Some(dir.path().to_string_lossy().into_owned());
    let backend =
        RedbBackend::open(dir.path().join("graphs").to_string_lossy().into_owned(), 64).unwrap();
    state.persistence = Some(Arc::new(backend));
    let state = Arc::new(RwLock::new(state));
    let create = scoped_test_request_on(
        SECRET,
        1,
        "__commons__",
        Method::CreateGraph {
            graph_name: GRAPH.into(),
            graph_type: GraphType::Global,
        },
        ScopedTestCaller {
            principal: ISSUER,
            tenant: TENANT,
            scopes: &["*"],
            nonce: "terminal-trace-create",
            idempotency_key: "terminal-trace-create",
        },
    );
    let created = dispatch_test_on_heap(&state, create).await;
    assert!(created.error.is_none(), "{:?}", created.error);
    (dir, state)
}

/// Whether the trace node exists and which edges leave the work item.
async fn trace_and_links(
    state: &Arc<RwLock<ServerState>>,
    id: u64,
) -> (serde_json::Value, Vec<(String, serde_json::Value)>) {
    let node = payload_value(
        call(
            state,
            id,
            &format!("read-node-{id}"),
            Method::GetNodeProperties {
                node_id: TRACE.into(),
            },
        )
        .await,
    );
    let edges = payload_value(
        call(
            state,
            id + 1,
            &format!("read-edges-{id}"),
            Method::GetEdgesPage {
                after: None,
                limit: 100,
            },
        )
        .await,
    );
    let links = edges
        .as_array()
        .unwrap()
        .iter()
        .filter(|edge| edge[0] == ITEM && edge[1] == TRACE)
        .map(|edge| {
            let bytes: Vec<u8> = serde_json::from_value(edge[3].clone()).unwrap();
            (
                edge[1].as_str().unwrap().to_string(),
                rmp_serde::from_slice::<serde_json::Value>(&bytes).unwrap(),
            )
        })
        .collect();
    (node, links)
}

// spec: MEDULLA-DECISION-ENGINE-R133.8
#[tokio::test]
async fn terminal_commit_with_run_trace_receipt() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (_dir, state) = fixture().await;
    let now = medulla_server_api::txn::now_ms();
    let added = call(&state, 2, "terminal-trace-add", work_item_method(now)).await;
    assert!(added.error.is_none(), "{:?}", added.error);
    let claimed: ClaimWorkItemResult = serde_json::from_value(payload_value(
        call(
            &state,
            3,
            "terminal-trace-claim",
            Method::ClaimWorkItem {
                request: ClaimWorkItemRequest {
                    schema_version: ClaimWorkItemRequestSchemaVersion::V1,
                    tenant_ref: TENANT.into(),
                    work_item_id: Some(ITEM.into()),
                    queue_ref: None,
                    resource_class: None,
                    fairness_group: None,
                    worker_ref: WORKER.into(),
                    now_ms: now,
                    lease_ms: 60_000,
                    max_tenant_in_flight: 1,
                },
            },
        )
        .await,
    ))
    .unwrap();
    assert!(claimed.claimed, "{claimed:?}");
    let epoch = claimed.lease_epoch.unwrap();
    let fence = claimed.fencing_token.unwrap();

    // A trace that names another run is refused at admission: nothing stored.
    let refused = call(
        &state,
        4,
        "terminal-trace-foreign",
        commit_method("terminal-trace-foreign", epoch, fence, "run:other"),
    )
    .await;
    let error = refused.error.as_deref().expect("a foreign run is refused");
    assert!(error.contains("RUN_TRACE_INVALID"), "{error}");
    let missing = call(
        &state,
        5,
        "read-before",
        Method::GetNodeProperties {
            node_id: TRACE.into(),
        },
    )
    .await;
    let absent = missing.error.is_some()
        || match &missing.result {
            None => true,
            Some(ResultPayload::Json(value)) => value.is_null(),
            Some(ResultPayload::Raw(bytes)) => {
                rmp_serde::from_slice::<serde_json::Value>(bytes).is_ok_and(|value| value.is_null())
            }
            Some(_) => false,
        };
    assert!(absent, "a refused commit stores no trace: {missing:?}");

    // The terminal commit stores the trace and links it from the work item.
    let committed = payload_value(
        call(
            &state,
            6,
            "terminal-trace-ok",
            commit_method("terminal-trace-ok", epoch, fence, "run:terminal"),
        )
        .await,
    );
    assert_eq!(committed["status"], "succeeded", "{committed}");
    let (node, links) = trace_and_links(&state, 7).await;
    assert_eq!(node["run_trace"]["run_id"], "run:terminal");
    assert_eq!(node["run_trace"]["steps"][0]["action_ref"], "tool:fixture");
    assert_eq!(links.len(), 1, "one produced link: {links:?}");
    assert_eq!(links[0].1["relationship"], "produced");
}

/// Submit the work item and lease it; returns the lease epoch and fence.
async fn add_and_claim(state: &Arc<RwLock<ServerState>>, tag: &str, first_id: u64) -> (u64, u64) {
    let now = medulla_server_api::txn::now_ms();
    let added = call(
        state,
        first_id,
        &format!("{tag}-add"),
        work_item_method(now),
    )
    .await;
    assert!(added.error.is_none(), "{:?}", added.error);
    let claimed: ClaimWorkItemResult = serde_json::from_value(payload_value(
        call(
            state,
            first_id + 1,
            &format!("{tag}-claim"),
            Method::ClaimWorkItem {
                request: ClaimWorkItemRequest {
                    schema_version: ClaimWorkItemRequestSchemaVersion::V1,
                    tenant_ref: TENANT.into(),
                    work_item_id: Some(ITEM.into()),
                    queue_ref: None,
                    resource_class: None,
                    fairness_group: None,
                    worker_ref: WORKER.into(),
                    now_ms: now,
                    lease_ms: 60_000,
                    max_tenant_in_flight: 1,
                },
            },
        )
        .await,
    ))
    .unwrap();
    assert!(claimed.claimed, "{claimed:?}");
    (claimed.lease_epoch.unwrap(), claimed.fencing_token.unwrap())
}

// spec: MEDULLA-DECISION-ENGINE-R133.7.2
#[tokio::test]
async fn cyclic_trace_refuses_without_state_change() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (_dir, state) = fixture().await;
    let (epoch, fence) = add_and_claim(&state, "cyclic-trace", 2).await;
    let cycle = [
        trace_node("rt:x", Some("rt:y"), &["rt:y"]),
        trace_node("rt:y", Some("rt:x"), &["rt:x"]),
    ];
    let refused = call(
        &state,
        4,
        "cyclic-trace-refused",
        commit_method_with("cyclic-trace-refused", epoch, fence, "run:terminal", &cycle),
    )
    .await;
    let error = refused.error.as_deref().expect("a cycle is refused");
    assert!(error.contains("TRACE_CYCLE"), "{error}");

    // The same lease still commits: the refusal changed nothing, and no node of
    // the refused snapshot or the run trace was stored.
    for (id, node) in [(5, TRACE), (6, "rt:x"), (7, "rt:y")] {
        let read = call(
            &state,
            id,
            &format!("cyclic-read-{id}"),
            Method::GetNodeProperties {
                node_id: node.into(),
            },
        )
        .await;
        let absent = read.error.is_some()
            || match &read.result {
                None => true,
                Some(ResultPayload::Json(value)) => value.is_null(),
                Some(ResultPayload::Raw(bytes)) => {
                    rmp_serde::from_slice::<serde_json::Value>(bytes)
                        .is_ok_and(|value| value.is_null())
                }
                Some(_) => false,
            };
        assert!(absent, "node {node} is not stored: {read:?}");
    }
    let committed = payload_value(
        call(
            &state,
            8,
            "cyclic-trace-ok",
            commit_method("cyclic-trace-ok", epoch, fence, "run:terminal"),
        )
        .await,
    );
    assert_eq!(committed["status"], "succeeded", "{committed}");
}

/// A nested trace of four nodes (root -> a -> c, root -> b) of mixed kinds.
fn nested_trace() -> [serde_json::Value; 4] {
    let mut nodes = [
        trace_node("rt:root", None, &["rt:a", "rt:b"]),
        trace_node("rt:a", Some("rt:root"), &["rt:c"]),
        trace_node("rt:b", Some("rt:root"), &[]),
        trace_node("rt:c", Some("rt:a"), &[]),
    ];
    nodes[1]["kind"] = "sub_call".into();
    nodes[2]["kind"] = "intermediate".into();
    nodes
}

fn critique(
    root: &str,
    tenant: &str,
) -> medulla_decision_types::decision::recursive_trace::TraceCritiqueQuery {
    medulla_decision_types::decision::recursive_trace::TraceCritiqueQuery {
        tenant: tenant.into(),
        root_node_id: root.into(),
        snapshot_id: "snapshot:1".into(),
        snapshot_revision: 1,
        projection: Vec::new(),
        kinds: Vec::new(),
        cursor: None,
        limit: 8,
    }
}

async fn read_critique_page(
    state: &Arc<RwLock<ServerState>>,
    id: u64,
    query: medulla_decision_types::decision::recursive_trace::TraceCritiqueQuery,
) -> medulla_decision_types::decision::recursive_trace::TraceCritiquePage {
    let response = call(
        state,
        id,
        &format!("critique-read-{id}"),
        Method::TraceCritique { query },
    )
    .await;
    serde_json::from_value(payload_value(response)).expect("a critique page")
}

fn page_ids(
    nodes: &[medulla_decision_types::decision::recursive_trace::TraceCritiqueNode],
) -> Vec<&str> {
    nodes.iter().map(|node| node.node_id.as_str()).collect()
}

/// Commit the nested trace through signed dispatch.
async fn commit_nested_trace(state: &Arc<RwLock<ServerState>>, tag: &str) {
    let (epoch, fence) = add_and_claim(state, tag, 2).await;
    let committed = payload_value(
        call(
            state,
            4,
            &format!("{tag}-commit"),
            commit_method_with(
                &format!("{tag}-commit"),
                epoch,
                fence,
                "run:terminal",
                &nested_trace(),
            ),
        )
        .await,
    );
    assert_eq!(committed["status"], "succeeded", "{committed}");
}

// spec: MEDULLA-DECISION-ENGINE-R133.7.3
#[tokio::test]
async fn trace_critique_reads_complete_ancestry() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (_dir, state) = fixture().await;
    commit_nested_trace(&state, "critique-ancestry").await;

    // A subtree read returns the selected nodes and every ancestor to the root.
    let mut query = critique("rt:a", TENANT);
    query.projection = vec!["evidence_refs".into()];
    let page = read_critique_page(&state, 10, query).await;
    assert_eq!(page_ids(&page.nodes), ["rt:a", "rt:c"]);
    assert_eq!(page_ids(&page.ancestry), ["rt:root"]);
    assert_eq!(page.snapshot_id, "snapshot:1");
    assert_eq!(page.source_refs.len(), 2);
    assert!(page.next_cursor.is_none());
    assert!(page.nodes.iter().all(|node| node.evidence_refs.is_some()));
    assert!(page
        .ancestry
        .iter()
        .all(|node| node.evidence_refs.is_none()));

    // Pages resume through the opaque cursor in pre-order.
    let mut query = critique("rt:root", TENANT);
    query.limit = 2;
    let first = read_critique_page(&state, 11, query.clone()).await;
    assert_eq!(page_ids(&first.nodes), ["rt:root", "rt:a"]);
    query.cursor = first.next_cursor;
    assert!(query.cursor.is_some(), "more nodes follow");
    let second = read_critique_page(&state, 12, query).await;
    assert_eq!(page_ids(&second.nodes), ["rt:c", "rt:b"]);
    assert_eq!(page_ids(&second.ancestry), ["rt:a", "rt:root"]);
    assert!(second.next_cursor.is_none());

    // The kind filter keeps only the named kinds; content is absent unless projected.
    let mut query = critique("rt:root", TENANT);
    query.kinds = vec!["sub_call".into()];
    let filtered = read_critique_page(&state, 13, query).await;
    assert_eq!(page_ids(&filtered.nodes), ["rt:a"]);
    assert!(filtered.nodes[0].evidence_refs.is_none());
}

// spec: MEDULLA-DECISION-ENGINE-R133.7.3
#[tokio::test]
async fn trace_critique_refusals_leak_nothing() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (_dir, state) = fixture().await;
    commit_nested_trace(&state, "critique-refusals").await;
    let protected = digest_for('a');
    let refused = |response: medulla_protocol_types::protocol::Response, code: &str| {
        let error = response.error.as_deref().expect("a refusal").to_string();
        assert!(error.contains(code), "{code} in {error}");
        assert!(response.result.is_none(), "a refusal returns no payload");
        assert!(!error.contains(&protected), "no protected field in {error}");
        error
    };
    let read = |id: u64, query, tenant: &'static str, scopes: &'static [&'static str]| {
        let state = state.clone();
        async move {
            call_as(
                &state,
                id,
                &format!("critique-refusal-{id}"),
                Method::TraceCritique { query },
                tenant,
                scopes,
            )
            .await
        }
    };

    // A projection of content without trace:read-content.
    let mut query = critique("rt:root", TENANT);
    query.projection = vec!["input_refs".into(), "evidence_refs".into()];
    refused(
        read(10, query, TENANT, &["trace:read"]).await,
        "TRACE_PROJECTION_UNAUTHORIZED",
    );

    // A cursor that is not this read's.
    let mut query = critique("rt:root", TENANT);
    query.limit = 1;
    let first = read_critique_page(&state, 11, query.clone()).await;
    let mut other_root = critique("rt:a", TENANT);
    other_root.cursor = first.next_cursor;
    refused(
        read(12, other_root, TENANT, &["*"]).await,
        "TRACE_STALE_CURSOR",
    );
    let mut garbage = critique("rt:root", TENANT);
    garbage.cursor = Some("not-a-cursor".into());
    refused(
        read(13, garbage, TENANT, &["*"]).await,
        "TRACE_STALE_CURSOR",
    );

    // A snapshot id or revision the root does not carry.
    let mut wrong_id = critique("rt:root", TENANT);
    wrong_id.snapshot_id = "snapshot:other".into();
    refused(
        read(14, wrong_id, TENANT, &["*"]).await,
        "TRACE_SNAPSHOT_MISMATCH",
    );
    let mut wrong_revision = critique("rt:root", TENANT);
    wrong_revision.snapshot_revision = 7;
    refused(
        read(15, wrong_revision, TENANT, &["*"]).await,
        "TRACE_SNAPSHOT_MISMATCH",
    );

    // A root that is not stored.
    let missing = refused(
        read(16, critique("rt:absent", TENANT), TENANT, &["*"]).await,
        "TRACE_INCOMPLETE_ANCESTRY",
    );

    // A request tenant other than the verified carrier tenant.
    refused(
        read(17, critique("rt:root", "tenant-other"), TENANT, &["*"]).await,
        "ACCESS_DENIED",
    );

    // Another tenant's read of this tenant's root is refused exactly like a missing root.
    let foreign = read(
        18,
        critique("rt:root", "tenant-other"),
        "tenant-other",
        &["*"],
    )
    .await
    .error
    .expect("a foreign root is refused");
    assert!(!foreign.contains(&protected), "{foreign}");
    assert_eq!(
        foreign.replace("rt:root", "rt:absent"),
        missing,
        "a foreign root reads like a missing one"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R133.7
#[tokio::test]
async fn nested_trace_ingests_and_reads_back() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (_dir, state) = fixture().await;
    commit_nested_trace(&state, "nested-trace").await;

    // Every stored node reads back by id with the record the commit carried.
    for node in nested_trace() {
        let id = node["node_id"].as_str().unwrap();
        let stored = payload_value(
            call(
                &state,
                20,
                &format!("nested-read-{id}"),
                Method::GetNodeProperties { node_id: id.into() },
            )
            .await,
        );
        assert_eq!(stored["recursive_trace"], node, "{id}");
        assert_eq!(stored["node_type"], "RecursiveTraceNode");
    }

    // The critique read returns the whole tree at the one snapshot, each leaf
    // with its complete ancestry.
    let mut query = critique("rt:root", TENANT);
    query.projection = vec!["evidence_refs".into()];
    let page = read_critique_page(&state, 21, query).await;
    assert_eq!(page_ids(&page.nodes), ["rt:root", "rt:a", "rt:c", "rt:b"]);
    assert!(page.ancestry.is_empty());
    assert_eq!(page.snapshot_id, "snapshot:1");
    let leaf = read_critique_page(&state, 22, critique("rt:c", TENANT)).await;
    assert_eq!(page_ids(&leaf.nodes), ["rt:c"]);
    assert_eq!(page_ids(&leaf.ancestry), ["rt:a", "rt:root"]);
}
