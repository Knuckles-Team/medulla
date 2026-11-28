// MEDULLA-DECISION-ENGINE-R869.2: the admission of `action_records` batches. The four refusals
// leave the graph unchanged, an approved money movement commits, and a repeat names the held node
// and writes nothing.

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_action_record::{ActionKind, ActionRecordItem, FactUseItem};
use medulla_types::contract::BoundedVec;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use {
    medulla_h_ingest_repository::source_ingestion::action_record::action_node_id,
    medulla_h_ingest_repository::source_ingestion::action_record::admit_actions,
    medulla_h_ingest_repository::source_ingestion::action_record::ActionReads,
};

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

/// The gate decision and its request as graph nodes joined by a `decisionRequest` edge.
fn add_gate_edge(core: &Arc<GraphCore>) {
    core.add_node("gate:1".to_string(), blob(&json!({"type": "GateDecision"})));
    core.add_node(
        "req:1".to_string(),
        blob(&json!({"type": "ApprovalRequest"})),
    );
    core.add_edge(
        "gate:1".to_string(),
        "req:1".to_string(),
        blob(&json!({"relationship": "decisionRequest"})),
    )
    .unwrap();
}

fn approved_gate() -> Vec<(&'static str, Value)> {
    vec![
        (
            "gate:1",
            json!({"type": "GateDecision", "gateOutcome": "approve"}),
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
    assert!(
        refused.starts_with("ACTION_FACT_REVISION_UNKNOWN:"),
        "{refused}"
    );

    // ACTION_APPROVAL_REQUIRED: a money movement with no approved gate decision.
    let unapproved = record(ActionKind::MoneyMovement, "k-3", Some(&fact));
    let refused = refusal_of(admit_actions(TENANT, &[unapproved], &reads(&core, vec![])));
    assert!(
        refused.starts_with("ACTION_APPROVAL_REQUIRED:"),
        "{refused}"
    );

    // An approved money movement commits: the record, one fact use, and its edges.
    // A decision with no `decisionRequest` edge in the graph is not approved.
    let mut paid = record(ActionKind::MoneyMovement, "k-4", Some(&fact));
    paid.approved_by = Some("gate:1".to_string());
    let refused = refusal_of(admit_actions(
        TENANT,
        &[paid.clone()],
        &reads(&core, approved_gate()),
    ));
    assert!(
        refused.starts_with("ACTION_APPROVAL_REQUIRED:"),
        "{refused}"
    );
    add_gate_edge(&core);
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
    let Method::AddNode {
        properties_msgpack, ..
    } = &admitted.methods[0]
    else {
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
    assert!(
        refused.starts_with("ACTION_IDEMPOTENCY_CONFLICT:"),
        "{refused}"
    );
}

// MEDULLA-DECISION-ENGINE-R869.3, R869.4: the why chain over the served ingest. Records are
// ingested through `SourceIngest`, the question is the stored query `why_action` called through
// `StoredQueryCall`, and the rows are read back as `kg:read` readers.
#[cfg(all(unix, feature = "shacl", feature = "security"))]
mod why_chain {
    use medulla_identity_core_types::identity::ShareScope;
    use medulla_protocol_types::protocol::Method;
    use medulla_served_test_harness::connector_pack_harness::{next_id, ok, Served, GRAPH, TENANT};
    use medulla_source_types::source_ingestion::SourceIngestionRequest;
    use medulla_source_types::stored_query::{
        RefreshPolicy, StoredCall, StoredParamType, StoredQuery, StoredQueryLanguage,
    };
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    const CONNECTOR: &str = "action-source";
    const STREAM: &str = "action";
    const FACT: &str = "stock:item-x";
    const GATE: &str = "gate:po-1";
    const REQUEST: &str = "req:po-1";

    /// The one UQL read of R869.3, published as a stored query with one `Text` parameter.
    const WHY_ACTION: &str = "MATCH (:ActionRecord) WHERE idempotencyKey = $key \
                              |> TRAVERSE -[*]->{1,8} WITH PROOF";

    fn why_action() -> StoredQuery {
        StoredQuery {
            name: "why_action".to_string(),
            version: 1,
            language: StoredQueryLanguage::Uql,
            text: WHY_ACTION.to_string(),
            params: [("key".to_string(), StoredParamType::String)].into(),
            materialize: false,
            refresh: RefreshPolicy::OnChange,
            author: String::new(),
            share: ShareScope::Tenant,
        }
    }

    async fn served() -> Served {
        Served::provisioned(GRAPH, CONNECTOR, &[(STREAM, "ResearchPaper")]).await
    }

    async fn put(served: &Served, id: &str, properties: Value) {
        let mut properties = properties;
        properties["tenant_id"] = json!(TENANT);
        let response = served
            .call_on(
                served.graph(),
                &format!("seed:{id}:{}", next_id()),
                Method::AddNode {
                    node_id: id.to_string(),
                    properties_msgpack: rmp_serde::to_vec_named(&properties).unwrap(),
                },
            )
            .await;
        assert!(response.error.is_none(), "seed {id}: {response:?}");
    }

    async fn link(served: &Served, source: &str, target: &str, relationship: &str) {
        let response = served
            .call_on(
                served.graph(),
                &format!("link:{source}:{target}:{}", next_id()),
                Method::AddEdge {
                    source_id: source.to_string(),
                    target_id: target.to_string(),
                    properties_msgpack: rmp_serde::to_vec_named(
                        &json!({"relationship": relationship}),
                    )
                    .unwrap(),
                },
            )
            .await;
        assert!(
            response.error.is_none(),
            "link {source}->{target}: {response:?}"
        );
    }

    /// The graph version now, and the `sha256:` digest of the node's blob at that version.
    async fn pin(served: &Served, node: &str) -> (u64, String) {
        let guard = served.state().read().await;
        let core = &guard
            .registry
            .get(served.graph())
            .expect("the ingest graph")
            .core;
        let revision = core.version();
        let blob = core
            .node_properties_at(node, revision)
            .expect("the revision is readable")
            .expect("the node exists at the revision");
        (
            revision,
            format!("sha256:{}", hex::encode(Sha256::digest(blob.as_slice()))),
        )
    }

    /// Ingest `items` as one `action_records` batch and assert it is accepted.
    async fn record(served: &Served, items: Vec<Value>) {
        static PAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let request: SourceIngestionRequest = serde_json::from_value(json!({
            "connector": CONNECTOR, "kind": "action_records", "mode": "delta",
            "strict_schema": false,
            "records": [], "relationships": [], "withdrawals": [],
            "extension": {"action_records": items},
            "provider_checkpoint": {
                "stream": STREAM,
                "position": {"page": PAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)},
                "watermark": "2026-11-13T00:00:00Z",
            },
            "expected_previous_checkpoint": Value::Null,
        }))
        .expect("a valid action_records request");
        let response = served
            .call_on(
                served.graph(),
                &format!("action:{}", next_id()),
                Method::SourceIngest {
                    request: Box::new(request),
                },
            )
            .await;
        assert!(response.error.is_none(), "SourceIngest: {response:?}");
    }

    /// The node id of the record with `key`, read back through UQL.
    async fn record_id(served: &Served, key: &str) -> String {
        let ids = served
            .uql_ids(&format!(
                "MATCH (:ActionRecord) WHERE idempotencyKey = '{key}'"
            ))
            .await;
        assert_eq!(ids.len(), 1, "one record holds key {key}: {ids:?}");
        ids.into_iter().next().unwrap()
    }

    /// The row ids `why_action` answers for `key`, as `principal`.
    async fn why(served: &Served, principal: &str, key: &str) -> Vec<String> {
        let response = served
            .verified_as(
                principal,
                TENANT,
                &["kg:read"],
                Method::StoredQueryCall {
                    name: "why_action".to_string(),
                    version: None,
                    args: [("key".to_string(), json!(key))].into(),
                },
            )
            .await;
        let call: StoredCall = ok("StoredQueryCall", response);
        call.rows
            .as_array()
            .expect("rows are an array")
            .iter()
            .map(|row| row["id"].as_str().expect("a row id").to_string())
            .collect()
    }

    async fn publish_why(served: &Served) {
        let response = served
            .call_on(
                served.graph(),
                "put-why-action",
                Method::StoredQueryPut {
                    query: why_action(),
                },
            )
            .await;
        assert!(response.error.is_none(), "StoredQueryPut: {response:?}");
    }

    /// The gate decision of an approved money movement and the request it decides.
    async fn seed_approval(served: &Served, action_kind: &str) {
        put(
            served,
            GATE,
            json!({"type": "GateDecision", "gateOutcome": "approve"}),
        )
        .await;
        put(
            served,
            REQUEST,
            json!({
                "type": "ApprovalRequest",
                "requestStatus": "approved",
                "requestActionKind": action_kind,
            }),
        )
        .await;
        link(served, GATE, REQUEST, "decisionRequest").await;
    }

    fn fact_use(node: &str, revision: u64, digest: &str) -> Value {
        json!({"node_id": node, "revision": revision, "content_digest": digest})
    }

    /// The `(type, properties)` of every answered row.
    async fn rows_of(
        served: &Served,
        ids: &[String],
    ) -> Vec<(String, serde_json::Map<String, Value>)> {
        let mut rows = Vec::new();
        for id in ids {
            let properties = served
                .properties(id)
                .await
                .expect("an answered row is readable");
            let kind = properties["type"].as_str().unwrap_or_default().to_string();
            rows.push((kind, properties));
        }
        rows
    }

    // spec: MEDULLA-DECISION-ENGINE-R869.3
    #[tokio::test]
    async fn why_chain_shows_facts_at_used_revision() {
        let served = served().await;
        put(&served, FACT, json!({"type": "StockLevel", "quantity": 3})).await;
        seed_approval(&served, "money_movement").await;
        let (revision, digest) = pin(&served, FACT).await;

        // An earlier record, then the money movement it caused; both used the fact at `revision`.
        record(
            &served,
            vec![json!({
                "idempotency_key": "why-a", "action_kind": "tool_call", "actor": "agent:planner",
                "used_facts": [fact_use(FACT, revision, &digest)],
                "outcome": "counted",
            })],
        )
        .await;
        let earlier = record_id(&served, "why-a").await;
        record(
            &served,
            vec![json!({
                "idempotency_key": "why-b", "action_kind": "money_movement",
                "actor": "agent:payer",
                "used_facts": [fact_use(FACT, revision, &digest)],
                "approved_by": GATE, "caused_by": earlier, "outcome": "paid",
            })],
        )
        .await;

        // The fact changes after the action used it.
        let changed = served
            .call_on(
                served.graph(),
                "change-fact",
                Method::BatchUpdate {
                    operations_msgpack: rmp_serde::to_vec_named(&vec![json!({
                        "op": "upsert_node", "id": FACT,
                        "properties": {"type": "StockLevel", "quantity": 0, "tenant_id": TENANT},
                    })])
                    .unwrap(),
                },
            )
            .await;
        assert!(changed.error.is_none(), "{changed:?}");
        let (_, current) = pin(&served, FACT).await;
        assert_ne!(current, digest, "the fact moved on");

        publish_why(&served).await;
        let ids = why(&served, "connector-pack-admin", "why-b").await;
        let rows = rows_of(&served, &ids).await;

        // Each of the two records' facts is its `FactUse` row at the revision and digest used;
        // the fact's current node is never reached.
        let uses: Vec<_> = rows.iter().filter(|(kind, _)| kind == "FactUse").collect();
        assert_eq!(
            uses.len(),
            2,
            "the record's and the earlier record's fact use"
        );
        for (_, properties) in &uses {
            assert_eq!(properties["factNode"], FACT);
            assert_eq!(properties["factRevision"], revision);
            assert_eq!(properties["factDigest"], digest.as_str());
        }
        assert!(!ids.iter().any(|id| id == FACT));
        assert!(
            ids.iter().any(|id| id == GATE),
            "the approver is on the chain"
        );
        assert!(
            ids.contains(&earlier),
            "the earlier record named by causedBy"
        );

        // A second principal who may not read the record gets no rows.
        let reader = "reader-without-access";
        served.register_reader(reader).await;
        let record_b = record_id(&served, "why-b").await;
        served.deny_row(&record_b, "someone-else").await;
        assert!(why(&served, reader, "why-b").await.is_empty());
    }

    // spec: MEDULLA-DECISION-ENGINE-R869.4
    #[tokio::test]
    async fn why_did_we_reorder_end_to_end() {
        let served = served().await;
        const RULE: &str = "rule:reorder";
        // The stock level, the reorder rule and the reorder evaluation the action used.
        put(
            &served,
            FACT,
            json!({"type": "StockLevel", "quantity": 2, "reorderPoint": 5}),
        )
        .await;
        put(
            &served,
            RULE,
            json!({"type": "ReorderRule", "ruleVersion": 3}),
        )
        .await;
        seed_approval(&served, "money_movement").await;
        let (stock_revision, stock_digest) = pin(&served, FACT).await;
        let (rule_revision, rule_digest) = pin(&served, RULE).await;
        assert!(rule_revision >= stock_revision);

        record(
            &served,
            vec![json!({
                "idempotency_key": "po-7", "action_kind": "money_movement",
                "actor": "agent:purchasing",
                "used_facts": [fact_use(FACT, stock_revision, &stock_digest)],
                "used_rules": [{"rule_id": RULE, "version": 3, "digest": rule_digest}],
                "approved_by": GATE, "outcome": "purchase_order:po-7",
            })],
        )
        .await;
        publish_why(&served).await;

        let ids = why(&served, "connector-pack-admin", "po-7").await;
        let rows = rows_of(&served, &ids).await;
        let fact = rows
            .iter()
            .find(|(kind, _)| kind == "FactUse")
            .expect("the stock fact use");
        assert_eq!(fact.1["factNode"], FACT);
        assert_eq!(fact.1["factRevision"], stock_revision);
        let rule = rows
            .iter()
            .find(|(kind, _)| kind == "RuleUse")
            .expect("the reorder rule use");
        assert_eq!(rule.1["ruleNode"], RULE);
        assert_eq!(rule.1["ruleDigest"], rule_digest.as_str());
        assert!(ids.iter().any(|id| id == GATE), "the approver");

        // The purchase order is the record's outcome.
        let record = served
            .properties(&record_id(&served, "po-7").await)
            .await
            .expect("the record");
        assert_eq!(record["actionOutcome"], "purchase_order:po-7");
    }
}
