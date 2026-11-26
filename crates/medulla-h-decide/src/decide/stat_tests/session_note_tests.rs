//! Session-note capture (MEDULLA-DECISION-ENGINE-R150.3, R150.4): a note is
//! staged only for a run whose stored receipt records success, a note without
//! a user is refused, and another user's note id answers as an absent note.
//! Every refusal stages nothing.

use super::*;
use medulla_decision_types::decision::statistical::run_credit::RunCreditReceipt;
use medulla_persistence_records::decision_jobs::encode_artifact;
use medulla_types::contract::Digest256;
use medulla_work_types::memory::SessionNoteBody;

use super::super::stat_executor::ExecutionContext;
use super::super::stat_generation_cycle::Reembedder;
use super::super::stat_log::LogReader;
use super::super::stat_run_credit::run_key;
use super::super::stat_session_note::record_note;
use super::super::stat_vectors::GraphVectors;

const DIMS: usize = 16;

/// The engine's hash embedder at a fixed width, as the configured one falls back to.
struct Hashed;

impl Reembedder for Hashed {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let hash = medulla_exec::exec::HashEmbedder::new(DIMS);
        texts.iter().map(|text| hash.embed(text)).collect()
    }
}

fn receipt(run_id: &str, success: bool) -> RunCreditReceipt {
    RunCreditReceipt {
        run_id: run_id.to_string(),
        decision_ids: BoundedVec::new(Vec::new()).unwrap(),
        outcome_digest: "ab".repeat(32),
        positive_count: u32::from(success),
        negative_count: u32::from(!success),
        success,
        steps: BoundedVec::new(Vec::new()).unwrap(),
        premises: BoundedVec::new(Vec::new()).unwrap(),
        receipt_digest: format!("{run_id}-digest"),
    }
}

fn note(id: &str, run: &str, user: &str) -> SessionNoteBody {
    SessionNoteBody {
        schema_version: 1,
        note_id: id.to_string(),
        run_id: run.to_string(),
        session_id: "session-1".to_string(),
        task_id: "task-1".to_string(),
        user_id: user.to_string(),
        input_digest: Digest256::from_bytes([1; 32]),
        output_digest: Digest256::from_bytes([2; 32]),
        source_revision: 5,
        content: format!("note {id} about the deploy"),
    }
}

fn reader(tenant: &str, principal: &str) -> LogReader {
    LogReader {
        tenant_id: tenant.to_string(),
        principal: principal.to_string(),
        roles: Vec::new(),
        retention: super::super::stat_retention::Retention::none(),
    }
}

fn ctx<'a>(h: &'a Harness, tenant: &'a str) -> ExecutionContext<'a> {
    ExecutionContext {
        store: &h.store,
        tenant_id: tenant,
        now_ms: 1_000,
        server_secret: b"",
        matched: &[],
        designation: None,
        reader: None,
        assistant_profile: None,
        observation_windows: None,
        type_digest: None,
        recommend_catalog: None,
    }
}

fn store_receipt(h: &Harness, tenant: &str, run: &str, success: bool) {
    h.store
        .put_decision_artifacts(
            tenant,
            &[(run_key(run), encode_artifact(&receipt(run, success)).unwrap())],
        )
        .unwrap();
}

async fn graph(h: &Harness) -> GraphVectors {
    let isolation = h.state.read().await.isolation.clone();
    GraphVectors::from_parts(
        std::sync::Arc::new(medulla_graph_core::graph::GraphCore::new()),
        isolation,
        "decider".to_string(),
    )
}

/// Verifies MEDULLA-DECISION-ENGINE-R150.3: Session-note capture and recall wiring.
// spec: MEDULLA-DECISION-ENGINE-R150.3
#[tokio::test]
async fn capture_requires_a_successful_run() {
    let h = Harness::new().await;
    store_receipt(&h, TENANT, "run-ok", true);
    store_receipt(&h, TENANT, "run-failed", false);
    store_receipt(&h, "tenant-other", "run-foreign", true);
    let graph = graph(&h).await;
    let ctx = ctx(&h, TENANT);
    let reader = reader(TENANT, "decider");
    let serve = |note: SessionNoteBody| {
        record_note(&ctx, &reader, TENANT, &graph, &Hashed, note)
    };

    let (stored, methods) = serve(note("n1", "run-ok", "user-a")).unwrap();
    assert!(stored.stored);
    assert_eq!(stored.run_receipt_digest, "run-ok-digest");
    assert!(!methods.is_empty(), "the note's rows are staged");

    // A failed run, an unknown run and another tenant's run all refuse the same way.
    for run in ["run-failed", "run-unknown", "run-foreign"] {
        let refused = serve(note("n2", run, "user-a")).unwrap_err();
        assert!(refused.starts_with("NOTE_RUN_NOT_SUCCESSFUL"), "{run}: {refused}");
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R150.4: Session-note refusals.
// spec: MEDULLA-DECISION-ENGINE-R150.4
#[tokio::test]
async fn unauthorized_refuses() {
    let h = Harness::new().await;
    store_receipt(&h, TENANT, "run-ok", true);
    let graph = graph(&h).await;
    let ctx = ctx(&h, TENANT);
    let reader = reader(TENANT, "decider");

    let refused = record_note(&ctx, &reader, TENANT, &graph, &Hashed, note("n1", "run-ok", " "))
        .unwrap_err();
    assert!(refused.starts_with("NOTE_USER_MISSING"), "{refused}");

    // A note id held by another user cannot be recorded over; the sidecar row
    // the first recording staged is what the second is judged against.
    let (_, methods) =
        record_note(&ctx, &reader, TENANT, &graph, &Hashed, note("n7", "run-ok", "user-a")).unwrap();
    for method in methods {
        if let medulla_protocol_types::protocol::Method::AddNode {
            node_id,
            properties_msgpack,
        } = method
        {
            if node_id.starts_with("sessionnote:") {
                graph.core().add_node(node_id, properties_msgpack);
            }
        }
    }
    let foreign = record_note(&ctx, &reader, TENANT, &graph, &Hashed, note("n7", "run-ok", "user-b"))
        .unwrap_err();
    assert!(foreign.starts_with("NOTE_UNAUTHORIZED"), "{foreign}");
    // The refusal names no owner, so it is the answer of a note that is not visible.
    assert!(!foreign.contains("user-a"), "{foreign}");
}
