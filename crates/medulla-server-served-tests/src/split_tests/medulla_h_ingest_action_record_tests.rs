// MEDULLA-DECISION-ENGINE-R869.2: the admission of `action_records` batches. The four refusals
// leave the graph unchanged, an approved money movement commits, and a repeat names the held node
// and writes nothing.

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_h_ingest::source_ingestion::action_record::{
    action_node_id, admit_actions, ActionReads,
};
use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_action_record::{ActionKind, ActionRecordItem, FactUseItem};
use medulla_types::contract::BoundedVec;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const TENANT: &str = "tenant-a";
const FACT: &str = "fact:invoice-7";

fn blob(value: &Value) -> Vec<u8> {
    rmp_serde::to_vec_named(value).unwrap()
}

/// A graph holding one fact node, and the pinned use of it at the current version.
fn graph_with_fact() -> (Arc<GraphCore>, FactUseItem) {
    let core = Arc::new(GraphCore::new());
    let bytes = blob(&json!({"type": "Invoice", "amount": 500}));
    core.add_node(FACT.to_string(), bytes.clone());
    let fact = FactUseItem {
        node_id: FACT.to_string(),
        revision: core.version(),
        content_digest: format!("sha256:{}", hex::encode(Sha256::digest(&bytes))),
    };
    (core, fact)
}

fn record(kind: ActionKind, key: &str, fact: Option<&FactUseItem>) -> ActionRecordItem {
    ActionRecordItem {
        idempotency_key: key.to_string(),
        action_kind: kind,
        actor: "agent:payer".to_string(),
        used_facts: BoundedVec::new(fact.cloned().into_iter().collect()).unwrap(),
        used_rules: BoundedVec::default(),
        approved_by: None,
        caused_by: None,
        outcome: Some("paid".to_string()),
        no_inputs: None,
    }
}

fn reads(core: &Arc<GraphCore>, held: Vec<(&str, Value)>) -> ActionReads {
    ActionReads {
        held: held
            .into_iter()
            .map(|(id, properties)| (id.to_string(), properties))
            .collect::<BTreeMap<_, _>>(),
        core: core.clone(),
    }
}

fn refusal_of(result: Result<impl std::fmt::Debug, String>) -> String {
    result.unwrap_err()
}

fn approved_gate() -> Vec<(&'static str, Value)> {
    vec![
        (
            "gate:1",
            json!({"type": "GateDecision", "gateOutcome": "approve", "decisionRequest": "req:1"}),
        ),
        (
            "req:1",
            json!({
                "type": "ApprovalRequest",
                "requestStatus": "approved",
                "requestActionKind": "money_movement",
            }),
        ),
    ]
}

// spec: MEDULLA-DECISION-ENGINE-R869.2
#[test]
fn refusals_and_idempotent_replay() {
    let (core, fact) = graph_with_fact();

    // ACTION_RECORD_INVALID: no fact and no stated reason.
    let bare = record(ActionKind::ToolCall, "k-1", None);
    let refused = refusal_of(admit_actions(TENANT, &[bare], &reads(&core, vec![])));
    assert!(refused.starts_with("ACTION_RECORD_INVALID:"), "{refused}");

    // ACTION_FACT_REVISION_UNKNOWN: a digest the node never had.
    let mut forged = fact.clone();
    forged.content_digest = format!("sha256:{}", "0".repeat(64));
    let refused = refusal_of(admit_actions(
        TENANT,
        &[record(ActionKind::ToolCall, "k-2", Some(&forged))],
        &reads(&core, vec![]),
    ));
    assert!(refused.starts_with("ACTION_FACT_REVISION_UNKNOWN:"), "{refused}");

    // ACTION_APPROVAL_REQUIRED: a money movement with no approved gate decision.
    let unapproved = record(ActionKind::MoneyMovement, "k-3", Some(&fact));
    let refused = refusal_of(admit_actions(TENANT, &[unapproved], &reads(&core, vec![])));
    assert!(refused.starts_with("ACTION_APPROVAL_REQUIRED:"), "{refused}");

    // An approved money movement commits: the record, one fact use, and its edges.
    let mut paid = record(ActionKind::MoneyMovement, "k-4", Some(&fact));
    paid.approved_by = Some("gate:1".to_string());
    let admitted = admit_actions(TENANT, &[paid.clone()], &reads(&core, approved_gate())).unwrap();
    let id = action_node_id(TENANT, "k-4").unwrap();
    assert_eq!(admitted.record_ids, vec![id.clone()]);
    assert_eq!(admitted.node_ids, vec![id.clone(), format!("{id}:fact:0")]);
    let edges = admitted
        .methods
        .iter()
        .filter(|method| matches!(method, Method::AddEdge { .. }))
        .count();
    assert_eq!(edges, 2, "usedFact and approvedBy");

    // Sending it again with the held node returns the same id and writes nothing.
    let Method::AddNode { properties_msgpack, .. } = &admitted.methods[0] else {
        panic!("the record node is written first");
    };
    let held_record: Value = rmp_serde::from_slice(properties_msgpack).unwrap();
    let mut held = approved_gate();
    held.push((id.as_str(), held_record.clone()));
    let again = admit_actions(TENANT, &[paid.clone()], &reads(&core, held.clone())).unwrap();
    assert_eq!(again.record_ids, vec![id.clone()]);
    assert!(again.methods.is_empty() && again.node_ids.is_empty());

    // ACTION_IDEMPOTENCY_CONFLICT: the same key with another body.
    let mut changed = paid;
    changed.outcome = Some("refunded".to_string());
    let refused = refusal_of(admit_actions(TENANT, &[changed], &reads(&core, held)));
    assert!(refused.starts_with("ACTION_IDEMPOTENCY_CONFLICT:"), "{refused}");
}
