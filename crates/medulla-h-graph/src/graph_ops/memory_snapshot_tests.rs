//! Planned content of a memory snapshot sync (MEDULLA-DURABLE-KERNEL-R085.4):
//! the bytes the CAS receives before the graph commit are the bytes the commit
//! stores, read from the plan alone.

use super::memory_snapshot::{planned_content, plan_sync};
use medulla_graph_core::graph::GraphCore;
use medulla_graph_core::memory_snapshot as store;
use medulla_mutation_types::memory_sync::{
    DocId, RequestId, SyncAction, SyncOutcome, SyncRequest, SyncStamp,
};
use medulla_types::contract::Digest256;

const DOC: &str = "notes/plan.md";

fn stamp() -> SyncStamp {
    SyncStamp {
        actor: "writer".into(),
        policy_digest: Digest256::sha256(b"policy"),
        graph_incarnation: "incarnation-1".into(),
    }
}

fn submit(id: &str, expected: Option<u64>, content: &str) -> SyncRequest {
    SyncRequest {
        request_id: RequestId::new(id).unwrap(),
        doc_id: DocId::new(DOC).unwrap(),
        action: SyncAction::Submit {
            expected_revision: expected,
            content: content.into(),
            content_digest: Digest256::sha256(content.as_bytes()),
        },
        admit_receipt_digests: Vec::new(),
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R085.4
#[test]
fn planned_content_is_read_from_the_plan_before_the_commit() {
    let core = GraphCore::new();
    let doc = DocId::new(DOC).unwrap();
    let first = submit("p1", None, "one");
    let (outcome, plan) = plan_sync(&core, &first, &stamp());
    // Nothing is committed yet: the content comes from the plan alone.
    assert!(store::load_head(&core, &doc).is_none());
    assert_eq!(
        planned_content(&plan, &doc, &outcome),
        Some((1, "one".to_string()))
    );
    store::apply_plan(&core, &plan);

    // A competing save is retained as a candidate and its bytes are staged.
    let competing = submit("p2", None, "two");
    let (outcome, plan) = plan_sync(&core, &competing, &stamp());
    let SyncOutcome::ConflictRetained {
        candidate_revision, ..
    } = outcome
    else {
        panic!("a stale save is retained as a candidate: {outcome:?}");
    };
    assert_eq!(
        planned_content(&plan, &doc, &outcome),
        Some((candidate_revision, "two".to_string()))
    );

    // An outcome that stores no content stages nothing.
    let refused = SyncOutcome::Refused {
        code: medulla_mutation_types::memory_sync::MemorySyncRefusal::NotAvailable,
    };
    assert_eq!(planned_content(&plan, &doc, &refused), None);
}
