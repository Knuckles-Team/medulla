//! The knowledge curation cycle served through `Method::KnowledgeCuration`
//! (MEDULLA-DECISION-ENGINE-R709.3 to R709.7): an evaluated revision per admitted proposal, the
//! refusal table that leaves every flag open, the node and edges of a proposal, the closing of
//! the addressed flags on adoption, and the closed-loop measures.

use super::super::knowledge_curation::handle_knowledge_curation;
use super::super::knowledge_flag::handle_knowledge_flag;
use super::super::loops::handle_loops;
use super::super::planned_write::PlannedWrite;
use super::*;
use medulla_decision_outcome_types::decision::promotion::RevisionStage;
use medulla_decision_outcome_types::decision::revision_list::RevisionRow;
use medulla_decision_types::knowledge_curation::{
    CurationAdmitted, CurationEdit, CurationEditOp, CurationInputs, CurationOp, CurationProposal,
    CurationStatus, LoopMetrics, CURATION_EDIT_TOO_LARGE,
    CURATION_FLAG_UNKNOWN, CURATION_LEASE_REQUIRED, CURATION_NOT_DUE, CURATION_ROLE_REQUIRED,
    REFERENCE_UNRESOLVED, UPDATE_BASE_STALE,
};
use medulla_decision_types::knowledge_flag::{
    FlagKind, FlagState, KnowledgeFlagList, KnowledgeFlagOp, KnowledgeKind,
};
use medulla_decision_types::loops::{LoopOp, LoopView};
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use medulla_protocol_types::protocol::ResultPayload;
use medulla_types::contract::Digest256;
use serde_json::{json, Value};

const GRAPH: &str = "__commons__";
const LOOP: &str = "curation";
const RUN: &str = "run-1";
const TRACE: &str = "trace-node-1";
const SOURCE: &str = "source:orders";
const SKILL: &str = "skill:triage@3";

fn ctx(scopes: &[&str]) -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_with_scopes("curator-1", TENANT, scopes)
}

fn curator() -> VerifiedRequestContext {
    ctx(&["knowledge:flag", "knowledge:curate"])
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

fn reply<T: serde::de::DeserializeOwned>(write: &PlannedWrite) -> T {
    match &write.reply {
        ResultPayload::Raw(bytes) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn digest(text: &str) -> String {
    Digest256::sha256(text.as_bytes()).to_hex()
}

/// Claim the curation loop as `who` and answer the epoch.
async fn claim(h: &Harness, who: &VerifiedRequestContext) -> u64 {
    let response = handle_loops(
        &h.state,
        3,
        who,
        LoopOp::Claim {
            loop_id: LOOP.into(),
            objective_digest: "sha256:curate".into(),
            priority: 3,
        },
    )
    .await;
    let view: LoopView = decode(response).expect("claims the loop");
    view.instance.lease.expect("a lease").epoch
}

async fn seed(h: &Harness) -> std::sync::Arc<medulla_graph_core::graph::GraphCore> {
    let core = core(h).await;
    put(
        &core,
        TRACE,
        json!({"node_type": "RunTrace", "run_id": RUN, "tenant": TENANT}),
    );
    put(&core, SOURCE, json!({"type": "DataSource", "tenant_id": TENANT}));
    put(&core, SKILL, json!({"type": "Skill", "tenant_id": TENANT}));
    put(&core, "orders.amount", json!({"type": "Column", "tenant_id": TENANT}));
    core
}

/// Raise and commit one `Missing` flag on `topic`, answer its id.
async fn raise(
    h: &Harness,
    core: &medulla_graph_core::graph::GraphCore,
    topic: &str,
    at_ms: u64,
) -> String {
    let write = handle_knowledge_flag(
        &h.state,
        &curator(),
        KnowledgeFlagOp::Raise {
            graph: GRAPH.into(),
            knowledge_kind: KnowledgeKind::DataAccess,
            flag_kind: FlagKind::Missing,
            target_ref: format!("topic:{topic}"),
            what_observed: format!("{topic} is not documented"),
            suggested_correction: None,
            run_id: RUN.into(),
            trace_node_id: Some(TRACE.into()),
            at_ms,
        },
    )
    .await
    .expect("raises");
    commit(core, &write);
    let receipt: medulla_decision_types::knowledge_flag::KnowledgeFlagReceipt = reply(&write);
    receipt.flag.flag_id
}

fn proposal(id: &str, flags: &[&str], before: Option<&str>, after: &str, body: &str) -> CurationProposal {
    CurationProposal {
        proposal_id: id.into(),
        flag_ids: flags.iter().map(|flag| flag.to_string()).collect(),
        target_ref: SOURCE.into(),
        knowledge_kind: KnowledgeKind::DataAccess,
        edit: CurationEdit {
            op: CurationEditOp::Update,
            before_digest: before.map(str::to_string),
            after_digest: Some(after.into()),
            body_digest: digest(body),
            supersedes: false,
            references: vec!["orders.amount".into()],
        },
        rationale_trace_ids: vec![TRACE.into()],
        evidence_digest: digest("evidence"),
    }
}

fn propose_op(epoch: u64, proposal: CurationProposal, body: &str) -> CurationOp {
    CurationOp::Propose {
        graph: GRAPH.into(),
        loop_id: LOOP.into(),
        epoch,
        proposal,
        body: body.into(),
    }
}

async fn run(
    h: &Harness,
    who: &VerifiedRequestContext,
    op: CurationOp,
) -> Result<PlannedWrite, String> {
    handle_knowledge_curation(&h.state, who, op).await
}

fn refused(result: Result<PlannedWrite, String>, code: &str) {
    let error = result.err().expect("the operation must refuse");
    assert!(error.starts_with(code), "{code} vs {error}");
}

async fn open_flags(h: &Harness) -> usize {
    let write = handle_knowledge_flag(
        &h.state,
        &curator(),
        KnowledgeFlagOp::List {
            graph: GRAPH.into(),
            state: Some(FlagState::Open),
            limit: None,
        },
    )
    .await
    .expect("lists");
    reply::<KnowledgeFlagList>(&write).flags.len()
}

// spec: MEDULLA-DECISION-ENGINE-R709.3
#[tokio::test]
async fn propose_records_an_evaluated_revision() {
    let h = Harness::with_isolation(ServerState::test_isolation("curator-1")).await;
    let core = seed(&h).await;
    let flag = raise(&h, &core, "refunds", 10).await;
    let who = curator();
    let epoch = claim(&h, &who).await;

    // Inputs serves the open flag with the trace it was observed in.
    let inputs = run(
        &h,
        &who,
        CurationOp::Inputs {
            graph: GRAPH.into(),
            loop_id: LOOP.into(),
            epoch,
            force: false,
            cursor: None,
            limit: None,
        },
    )
    .await
    .expect("inputs");
    let inputs: CurationInputs = reply(&inputs);
    assert!(inputs.due);
    assert_eq!(inputs.flags.len(), 1);
    assert_eq!(inputs.traces[0].trace_node_id, TRACE);

    // A proposal is recorded as an evaluated revision of its subject.
    let body = "orders.amount is renamed to orders.total";
    let admitted = run(
        &h,
        &who,
        propose_op(epoch, proposal("p1", &[&flag], None, "v2", body), body),
    )
    .await
    .expect("admitted");
    let answer: CurationAdmitted = reply(&admitted);
    assert_eq!(answer.stage, "evaluated");
    let store = h.state.write().await.ensure_agent_library().unwrap();
    let rows = store
        .decision_artifacts_with_prefix(TENANT, "revision:", 100)
        .unwrap();
    let revisions: Vec<RevisionRow> = rows
        .iter()
        .map(|(_, bytes)| decode_artifact(bytes, "revision").unwrap())
        .filter(|row: &RevisionRow| row.id == answer.revision_id)
        .collect();
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].stage, RevisionStage::Evaluated);
    assert_eq!(revisions[0].subject_kind, "knowledge_entry");
    assert_eq!(revisions[0].digest, digest(body));

    // A replay answers the same revision and plans nothing new.
    let replay = run(
        &h,
        &who,
        propose_op(epoch, proposal("p1", &[&flag], None, "v2", body), body),
    )
    .await
    .expect("replays");
    assert!(replay.operations.is_empty());
    assert_eq!(reply::<CurationAdmitted>(&replay), answer);
}

// spec: MEDULLA-DECISION-ENGINE-R709.4
#[tokio::test]
async fn refusals_leave_flags_open() {
    let h = Harness::with_isolation(ServerState::test_isolation("curator-1")).await;
    let core = seed(&h).await;
    let flag = raise(&h, &core, "refunds", 10).await;
    let who = curator();
    let body = "a body";
    let inputs = |epoch, force| CurationOp::Inputs {
        graph: GRAPH.into(),
        loop_id: LOOP.into(),
        epoch,
        force,
        cursor: None,
        limit: None,
    };

    // No lease, no curation; no scope, no curation.
    refused(
        run(&h, &who, propose_op(1, proposal("p0", &[&flag], None, "v2", body), body)).await,
        CURATION_LEASE_REQUIRED.as_str(),
    );
    refused(
        run(&h, &ctx(&["knowledge:flag"]), inputs(1, true)).await,
        CURATION_ROLE_REQUIRED.as_str(),
    );
    let epoch = claim(&h, &who).await;
    refused(
        run(&h, &who, propose_op(epoch + 1, proposal("p0", &[&flag], None, "v2", body), body)).await,
        CURATION_LEASE_REQUIRED.as_str(),
    );

    // An unknown flag, a body over 64 KiB, an unresolved reference and a stale base.
    refused(
        run(&h, &who, propose_op(epoch, proposal("p0", &["flag:none"], None, "v2", body), body)).await,
        CURATION_FLAG_UNKNOWN.as_str(),
    );
    let big = "x".repeat(64 * 1024 + 1);
    refused(
        run(&h, &who, propose_op(epoch, proposal("p0", &[&flag], None, "v2", &big), &big)).await,
        CURATION_EDIT_TOO_LARGE.as_str(),
    );
    let mut dangling = proposal("p0", &[&flag], None, "v2", body);
    dangling.edit.references = vec!["orders.missing".into()];
    refused(
        run(&h, &who, propose_op(epoch, dangling, body)).await,
        REFERENCE_UNRESOLVED.as_str(),
    );
    refused(
        run(&h, &who, propose_op(epoch, proposal("p0", &[&flag], Some("v0"), "v2", body), body)).await,
        UPDATE_BASE_STALE.as_str(),
    );

    // Inputs is refused until the cadence elapses unless forced.
    let admitted = run(
        &h,
        &who,
        propose_op(epoch, proposal("p1", &[&flag], None, "v2", body), body),
    )
    .await
    .expect("admitted");
    commit(&core, &admitted);
    refused(run(&h, &who, inputs(epoch, false)).await, CURATION_NOT_DUE.as_str());
    assert!(run(&h, &who, inputs(epoch, true)).await.is_ok());
    // No refusal changed a flag.
    assert_eq!(open_flags(&h).await, 1);
}

// spec: MEDULLA-DECISION-ENGINE-R709.5
#[tokio::test]
async fn adoption_closes_the_addressed_flags() {
    let h = Harness::with_isolation(ServerState::test_isolation("curator-1")).await;
    let core = seed(&h).await;
    let first = raise(&h, &core, "refunds", 10).await;
    let second = raise(&h, &core, "exchanges", 20).await;
    let who = curator();
    let epoch = claim(&h, &who).await;
    let body = "orders.amount is renamed to orders.total";
    let admitted = run(
        &h,
        &who,
        propose_op(epoch, proposal("p1", &[&first, &second], None, "v2", body), body),
    )
    .await
    .expect("admitted");
    let receipt = reply::<CurationAdmitted>(&admitted).receipt_id;
    // The proposal node addresses each flag, reads the trace and proposes the target.
    let edges: Vec<(&str, &str)> = admitted
        .operations
        .iter()
        .filter(|op| op["op"] == "upsert_edge")
        .map(|op| {
            (
                op["properties"]["relationship"].as_str().unwrap(),
                op["target"].as_str().unwrap(),
            )
        })
        .collect();
    assert!(edges.contains(&("addresses", first.as_str())));
    assert!(edges.contains(&("addresses", second.as_str())));
    assert!(edges.contains(&("read", TRACE)));
    assert!(edges.contains(&("curationProposes", SOURCE)));
    assert!(admitted
        .operations
        .iter()
        .all(|op| op["properties"]["receipt_id"] == receipt.as_str()));
    commit(&core, &admitted);

    // Before adoption nothing closes.
    let status = |id: &str| CurationOp::Status {
        graph: GRAPH.into(),
        proposal_id: id.into(),
    };
    let waiting = run(&h, &who, status("p1")).await.expect("status");
    assert!(waiting.operations.is_empty());
    assert_eq!(open_flags(&h).await, 2);

    // The revision becomes the champion (a champion move writes a new row of the same id);
    // the next status closes both flags and links each to the proposal.
    let store = h.state.write().await.ensure_agent_library().unwrap();
    let (_, bytes) = store
        .decision_artifacts_with_prefix(TENANT, "revision:", 100)
        .unwrap()
        .into_iter()
        .next()
        .expect("the revision row");
    let evaluated: RevisionRow = decode_artifact(&bytes, "revision").unwrap();
    let champion = RevisionRow {
        stage: RevisionStage::Champion,
        created_ms: evaluated.created_ms + 1,
        ..evaluated
    };
    store
        .put_decision_artifacts(
            TENANT,
            &[(champion.key(), encode_artifact(&champion).unwrap())],
        )
        .unwrap();
    let closing = run(&h, &who, status("p1")).await.expect("status");
    let state: CurationStatus = reply(&closing);
    assert_eq!(state.stage, "champion");
    assert_eq!(state.adopted.len(), 2);
    assert!(closing.operations.iter().any(|op| {
        op["op"] == "upsert_edge"
            && op["properties"]["relationship"] == "knowledgeFixedBy"
            && op["target"] == "proposal:p1"
    }));
    commit(&core, &closing);
    assert_eq!(open_flags(&h).await, 0);
    // Both flags are `Curated`; a second status closes nothing.
    assert!(state.flags.iter().all(|row| row.state == FlagState::Curated));
    let again = run(&h, &who, status("p1")).await.expect("status");
    assert!(again.operations.is_empty());
}

// spec: MEDULLA-DECISION-ENGINE-R709.7
#[tokio::test]
async fn metrics_report_recurrence_and_latency() {
    let h = Harness::with_isolation(ServerState::test_isolation("curator-1")).await;
    let core = seed(&h).await;
    let who = curator();
    // Three stored flags: two curated (one came back) and one dismissed.
    let row = |id: &str, state: &str, raised: u64, resolved: Option<u64>, times: u64, reopened: bool| {
        let mut props = json!({
            "type": "KnowledgeFlag", "node_type": "KnowledgeFlag", "flag_id": id,
            "tenant_id": TENANT, "knowledge_kind": "domain", "flag_kind": "missing",
            "target_ref": format!("topic:{id}"), "what_observed": "x", "raised_by": "a",
            "run_id": RUN, "raised_at_ms": raised, "state": state, "raise_count": 1,
            "reopened": reopened, "curated_times": times,
        });
        if let Some(resolved) = resolved {
            props["resolved_at_ms"] = resolved.into();
        }
        props
    };
    put(&core, "flag:m1", row("flag:m1", "curated", 100, Some(300), 1, false));
    put(&core, "flag:m2", row("flag:m2", "curated", 200, Some(800), 1, false));
    put(&core, "flag:m3", row("flag:m3", "dismissed", 300, Some(350), 0, false));
    put(&core, "flag:m4", row("flag:m4", "open", 400, None, 1, true));
    let write = run(
        &h,
        &who,
        CurationOp::Metrics {
            graph: GRAPH.into(),
            from_ms: 0,
            to_ms: 1_000,
        },
    )
    .await
    .expect("metrics");
    assert!(write.operations.is_empty());
    let metrics: LoopMetrics = reply(&write);
    assert_eq!(metrics.flags_raised, 4);
    assert_eq!(metrics.flags_curated, 2);
    assert_eq!(metrics.flags_dismissed, 1);
    assert_eq!(metrics.median_flag_to_fix_ms, 400);
    // Three flags were ever curated and one of them came back.
    assert_eq!(metrics.recurrence_ppm, 333_333);
}
