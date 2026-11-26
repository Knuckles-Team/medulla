//! Knowledge flags served through `Method::KnowledgeFlag` (MEDULLA-DECISION-ENGINE-R708.3,
//! R708.4, R708.5): one node per flag, one refusal table, an agent-asserted node linked to the
//! run trace and the human correction.

use super::super::knowledge_flag::handle_knowledge_flag;
use super::super::planned_write::PlannedWrite;
use super::*;
use medulla_decision_types::knowledge_flag::{
    FlagKind, FlagResolution, FlagState, KnowledgeFlagList, KnowledgeFlagOp, KnowledgeFlagReceipt,
    KnowledgeFlagView, KnowledgeKind, RaiseOutcome, FLAG_NOT_FOUND, FLAG_RESOLVE_ROLE_REQUIRED,
    FLAG_RUN_QUOTA_EXCEEDED, FLAG_RUN_UNKNOWN, FLAG_STATE_ILLEGAL, FLAG_TARGET_UNKNOWN,
    FLAG_TEXT_TOO_LARGE, WHAT_OBSERVED_MAX_BYTES,
};
use medulla_protocol_types::protocol::ResultPayload;
use serde_json::{json, Value};

const GRAPH: &str = "__commons__";
const RUN: &str = "run-1";
const TRACE: &str = "trace-node-1";
const TARGET: &str = "skill:triage@3";

fn verified(tenant: &str, scopes: &[&str]) -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_with_scopes("agent-7", tenant, scopes)
}

fn curator(tenant: &str) -> VerifiedRequestContext {
    verified(tenant, &["knowledge:flag", "knowledge:curate"])
}

fn raiser(tenant: &str) -> VerifiedRequestContext {
    verified(tenant, &["knowledge:flag"])
}

async fn core(h: &Harness) -> std::sync::Arc<medulla_graph_core::graph::GraphCore> {
    h.state
        .read()
        .await
        .registry
        .get(GRAPH)
        .expect("graph exists")
        .core
        .clone()
}

fn put(core: &medulla_graph_core::graph::GraphCore, id: &str, props: Value) {
    core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
}

/// Commit a plan the way the router does: its upserts land in the graph.
fn commit(core: &medulla_graph_core::graph::GraphCore, write: &PlannedWrite) {
    for operation in &write.operations {
        match operation["op"].as_str() {
            Some("upsert_node") => put(
                core,
                operation["id"].as_str().unwrap(),
                operation["properties"].clone(),
            ),
            Some("upsert_edge") => {
                core.add_edge(
                    operation["source"].as_str().unwrap().to_string(),
                    operation["target"].as_str().unwrap().to_string(),
                    rmp_serde::to_vec_named(&operation["properties"]).unwrap(),
                )
                .unwrap();
            }
            other => panic!("unexpected operation {other:?}"),
        }
    }
}

fn seed(core: &medulla_graph_core::graph::GraphCore) {
    put(
        core,
        TRACE,
        json!({"node_type": "RunTrace", "run_id": RUN, "tenant": TENANT}),
    );
    put(core, TARGET, json!({"type": "Skill", "tenant_id": TENANT}));
}

async fn run(
    h: &Harness,
    who: &VerifiedRequestContext,
    op: KnowledgeFlagOp,
) -> Result<PlannedWrite, String> {
    handle_knowledge_flag(&h.state, who, op).await
}

fn reply<T: serde::de::DeserializeOwned>(write: &PlannedWrite) -> T {
    match &write.reply {
        ResultPayload::Raw(bytes) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn refusal(result: Result<PlannedWrite, String>) -> String {
    match result {
        Err(error) => error,
        Ok(_) => panic!("the operation must refuse"),
    }
}

fn raise_op(target: &str, kind: FlagKind, observed: &str, run_id: &str) -> KnowledgeFlagOp {
    KnowledgeFlagOp::Raise {
        graph: GRAPH.into(),
        knowledge_kind: KnowledgeKind::Skill,
        flag_kind: kind,
        target_ref: target.into(),
        what_observed: observed.into(),
        suggested_correction: Some("ask for the order id first".into()),
        run_id: run_id.into(),
        trace_node_id: Some(TRACE.into()),
        at_ms: 1_700_000_000_000,
    }
}

fn list_op(state: Option<FlagState>) -> KnowledgeFlagOp {
    KnowledgeFlagOp::List {
        graph: GRAPH.into(),
        state,
        limit: None,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R708.3
#[tokio::test]
async fn raise_writes_one_node_and_dedups() {
    let h = Harness::with_isolation(ServerState::test_isolation("agent-7")).await;
    let core = core(&h).await;
    seed(&core);
    let who = raiser(TENANT);

    let first = run(&h, &who, raise_op(TARGET, FlagKind::Wrong, "step 3 is wrong", RUN))
        .await
        .expect("raises");
    let receipt: KnowledgeFlagReceipt = reply(&first);
    assert_eq!(receipt.outcome, Some(RaiseOutcome::Created));
    assert_eq!(receipt.flag.raise_count, 1);
    // One node in the batch, and every edge hangs off it.
    let nodes = first
        .operations
        .iter()
        .filter(|op| op["op"] == "upsert_node" && op["id"] == receipt.flag.flag_id.as_str())
        .count();
    assert_eq!(nodes, 1);
    commit(&core, &first);

    // The same flag again keeps one node and adds one to the count.
    let again = run(&h, &who, raise_op(TARGET, FlagKind::Wrong, "step 3 is wrong", RUN))
        .await
        .expect("dedups");
    let repeat: KnowledgeFlagReceipt = reply(&again);
    assert_eq!(repeat.outcome, Some(RaiseOutcome::Deduplicated));
    assert_eq!(repeat.flag.raise_count, 2);
    assert_eq!(repeat.flag.flag_id, receipt.flag.flag_id);
    assert_eq!(again.operations.len(), 1, "only the node is rewritten");
    commit(&core, &again);

    let listed: KnowledgeFlagList = reply(&run(&h, &who, list_op(None)).await.unwrap());
    assert_eq!(listed.flags.len(), 1);
    assert_eq!(listed.flags[0].raise_count, 2);

    // Another tenant sees none of it.
    let other: KnowledgeFlagList =
        reply(&run(&h, &raiser("tenant-b"), list_op(None)).await.unwrap());
    assert!(other.flags.is_empty());
}

// spec: MEDULLA-DECISION-ENGINE-R708.4
#[tokio::test]
async fn each_refusal_writes_nothing() {
    let h = Harness::with_isolation(ServerState::test_isolation("agent-7")).await;
    let core = core(&h).await;
    seed(&core);
    let who = raiser(TENANT);

    let unknown_target = refusal(
        run(&h, &who, raise_op("skill:nowhere@1", FlagKind::Wrong, "x", RUN)).await,
    );
    assert!(unknown_target.starts_with(FLAG_TARGET_UNKNOWN.as_str()), "{unknown_target}");
    let unknown_run = refusal(run(&h, &who, raise_op(TARGET, FlagKind::Wrong, "x", "run-9")).await);
    assert!(unknown_run.starts_with(FLAG_RUN_UNKNOWN.as_str()), "{unknown_run}");
    let big = "x".repeat(WHAT_OBSERVED_MAX_BYTES + 1);
    let too_large = refusal(run(&h, &who, raise_op(TARGET, FlagKind::Wrong, &big, RUN)).await);
    assert!(too_large.starts_with(FLAG_TEXT_TOO_LARGE.as_str()), "{too_large}");

    // The 21st open flag of one run is refused at the default knob.
    for index in 0..20 {
        let target = format!("topic:t{index}");
        let write = run(&h, &who, raise_op(&target, FlagKind::Missing, "gap", RUN))
            .await
            .expect("within the quota");
        commit(&core, &write);
    }
    let over = refusal(run(&h, &who, raise_op("topic:t20", FlagKind::Missing, "gap", RUN)).await);
    assert!(over.starts_with(FLAG_RUN_QUOTA_EXCEEDED.as_str()), "{over}");

    // Resolve without the curate scope, a missing flag and an illegal event.
    let flag_id = {
        let listed: KnowledgeFlagList = reply(&run(&h, &who, list_op(None)).await.unwrap());
        listed.flags[0].flag_id.clone()
    };
    let resolve = |flag_id: &str, resolution| KnowledgeFlagOp::Resolve {
        graph: GRAPH.into(),
        flag_id: flag_id.into(),
        resolution,
        correction_id: None,
        fixed_by: None,
    };
    let denied = refusal(run(&h, &who, resolve(&flag_id, FlagResolution::Dismiss)).await);
    assert!(denied.starts_with(FLAG_RESOLVE_ROLE_REQUIRED.as_str()), "{denied}");
    let boss = curator(TENANT);
    let missing = refusal(run(&h, &boss, resolve("flag:none", FlagResolution::Dismiss)).await);
    assert!(missing.starts_with(FLAG_NOT_FOUND.as_str()), "{missing}");
    let done = run(&h, &boss, resolve(&flag_id, FlagResolution::Dismiss))
        .await
        .expect("dismisses");
    commit(&core, &done);
    let illegal = refusal(run(&h, &boss, resolve(&flag_id, FlagResolution::Adopt)).await);
    assert!(illegal.starts_with(FLAG_STATE_ILLEGAL.as_str()), "{illegal}");
}

// spec: MEDULLA-DECISION-ENGINE-R708.5
#[tokio::test]
async fn flag_is_agent_asserted_and_linked_to_trace_and_correction() {
    let h = Harness::with_isolation(ServerState::test_isolation("agent-7")).await;
    let core = core(&h).await;
    seed(&core);
    put(
        &core,
        "correction:abc",
        json!({"type": "Correction", "correction_id": "c1", "tenant_id": TENANT}),
    );
    let boss = curator(TENANT);
    let raised = run(&h, &boss, raise_op(TARGET, FlagKind::Wrong, "step 3 is wrong", RUN))
        .await
        .expect("raises");
    let receipt: KnowledgeFlagReceipt = reply(&raised);
    commit(&core, &raised);
    // Every node and edge of the raise shares the one receipt id.
    for operation in &raised.operations {
        let properties = &operation["properties"];
        assert_eq!(properties["receipt_id"], receipt.receipt_id.as_str(), "{operation}");
    }
    let resolved = run(
        &h,
        &boss,
        KnowledgeFlagOp::Resolve {
            graph: GRAPH.into(),
            flag_id: receipt.flag.flag_id.clone(),
            resolution: FlagResolution::Adopt,
            correction_id: Some("c1".into()),
            fixed_by: None,
        },
    )
    .await
    .expect("resolves");
    commit(&core, &resolved);
    let got = run(
        &h,
        &boss,
        KnowledgeFlagOp::Get {
            graph: GRAPH.into(),
            flag_id: receipt.flag.flag_id.clone(),
        },
    )
    .await
    .expect("reads");
    let view: KnowledgeFlagView = reply(&got);
    assert_eq!(view.evidence_class, "agent_asserted");
    assert_eq!(view.flags.as_deref(), Some(TARGET));
    assert_eq!(view.observed_in.as_deref(), Some(TRACE));
    assert_eq!(view.resolved_by.as_deref(), Some("correction:abc"));
    assert_eq!(view.flag.state, FlagState::Curated);
    assert!(view.raised_by.len() > 0);
}
