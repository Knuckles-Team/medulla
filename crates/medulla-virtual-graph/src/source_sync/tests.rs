use serde_json::{json, Value};

use super::*;

// spec: MEDULLA-SRC-AUTH-R012.3
#[test]
fn reconcile_covers_every_case() {
    use ConflictRule::{HoldForReview, SystemOfRecordWins};
    let wins = SyncAction::Conflict {
        kept: Some(Side::Graph),
        held: vec![Side::Source],
    };
    let hold = SyncAction::Conflict {
        kept: None,
        held: vec![Side::Graph, Side::Source],
    };
    // (base, graph, source, rule-independent expectation or per-rule)
    let rule_free = [
        (Some("a"), Some("a"), Some("a"), SyncAction::Unchanged),
        (Some("a"), Some("a"), Some("b"), SyncAction::ApplyInbound),
        (Some("a"), Some("b"), Some("a"), SyncAction::ProposeOutbound),
        (None, None, Some("a"), SyncAction::ApplyInbound),
        (None, Some("a"), None, SyncAction::ProposeOutbound),
        (Some("a"), Some("a"), None, SyncAction::ProposeOutbound),
        (Some("a"), Some("b"), Some("b"), SyncAction::Unchanged),
        (None, None, None, SyncAction::Unchanged),
    ];
    for rule in [SystemOfRecordWins, HoldForReview] {
        for (base, graph, source, expected) in &rule_free {
            assert_eq!(
                &reconcile(*base, *graph, *source, rule),
                expected,
                "{base:?} {graph:?} {source:?}"
            );
        }
    }
    // Both changed to different values.
    assert_eq!(
        reconcile(Some("a"), Some("b"), Some("c"), SystemOfRecordWins),
        wins
    );
    assert_eq!(
        reconcile(Some("a"), Some("b"), Some("c"), HoldForReview),
        hold
    );
    // The graph value is proposed outbound only under system_of_record_wins.
    assert!(wins.proposes_outbound());
    assert!(!hold.proposes_outbound());
    assert!(SyncAction::ProposeOutbound.proposes_outbound());
    assert!(!SyncAction::ApplyInbound.proposes_outbound());
    assert!(!SyncAction::Unchanged.proposes_outbound());
    // The rule literal set is closed.
    assert_eq!(ConflictRule::parse("hold_for_review"), Some(HoldForReview));
    assert_eq!(ConflictRule::parse("anything"), None);
}

struct Tickets;

impl SyncAdapter for Tickets {
    fn record_key(&self, record: &Value) -> Option<String> {
        record.get("id").and_then(Value::as_str).map(str::to_string)
    }
    fn record_digest(&self, record: &Value) -> String {
        record
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }
    fn inbound_mutations(&self, key: &str, record: &Value) -> Vec<SyncMutation> {
        let mut properties = serde_json::Map::new();
        properties.insert("title".to_string(), record["title"].clone());
        vec![SyncMutation {
            node_id: format!("ticket:{key}"),
            properties,
        }]
    }
    fn outbound_payload(&self, _key: &str, record: &Value) -> Value {
        record.clone()
    }
    fn graph_records(
        &self,
        nodes: &super::GraphNodes,
    ) -> std::collections::BTreeMap<String, Value> {
        nodes
            .iter()
            .filter_map(|(id, props)| {
                let key = id.strip_prefix("ticket:")?;
                Some((
                    key.to_string(),
                    serde_json::json!({"id": key, "title": props.get("title")?}),
                ))
            })
            .collect()
    }
}

fn outcome(key: &str, action: SyncAction, base: Option<&str>, source: Option<&str>) -> SyncOutcome {
    SyncOutcome {
        record_key: key.to_string(),
        action,
        base_digest: base.map(str::to_string),
        source_digest: source.map(str::to_string),
        source_revision: Some("r7".to_string()),
    }
}

// spec: MEDULLA-SRC-AUTH-R012.4
#[test]
fn state_rides_the_inbound_batch() {
    let adapter = Tickets;
    let record = json!({"id": "T-1", "title": "new"});
    let key = adapter.record_key(&record).unwrap();
    let digest = adapter.record_digest(&record);
    let inbound = adapter.inbound_mutations(&key, &record);
    let outcomes = [
        outcome("T-1", SyncAction::ApplyInbound, Some("old"), Some(&digest)),
        outcome("T-2", SyncAction::ProposeOutbound, Some("x"), Some("y")),
        outcome("T-3", SyncAction::Unchanged, Some("x"), Some("x")),
        outcome(
            "T-4",
            SyncAction::Conflict {
                kept: None,
                held: vec![Side::Graph, Side::Source],
            },
            Some("x"),
            Some("z"),
        ),
        outcome(
            "T-5",
            SyncAction::Conflict {
                kept: Some(Side::Graph),
                held: vec![Side::Source],
            },
            None,
            Some("z"),
        ),
    ];
    let state = sync_state_mutations("conn:tickets", "kg:Ticket", &outcomes, 42);
    // Applied, proposed and graph-kept conflict write state; unchanged and held do not.
    assert_eq!(state.len(), 3);
    let batch = sync_batch(inbound.clone(), state.clone());
    assert_eq!(batch.len(), inbound.len() + state.len());
    assert_eq!(&batch[..inbound.len()], &inbound[..]);
    // The applied record's base is the source digest it took.
    let applied = &state[0].properties;
    assert_eq!(applied["baseDigest"], json!("new"));
    assert_eq!(applied["type"], json!(SYNC_STATE_CLASS));
    assert_eq!(applied["syncConnection"], json!("conn:tickets"));
    assert_eq!(applied["syncClass"], json!("kg:Ticket"));
    assert_eq!(applied["syncRecordKey"], json!("T-1"));
    assert_eq!(applied["sourceRevision"], json!("r7"));
    assert_eq!(applied["syncedAtMs"], json!(42));
    // A proposed record keeps its earlier base; one never synced has none.
    assert_eq!(state[1].properties["baseDigest"], json!("x"));
    assert!(!state[2].properties.contains_key("baseDigest"));
    // The node is keyed by connection, class and record key.
    let again = sync_state_mutations("conn:tickets", "kg:Ticket", &outcomes[..1], 99);
    assert_eq!(again[0].node_id, state[0].node_id);
    let other_class = sync_state_mutations("conn:tickets", "kg:Other", &outcomes[..1], 42);
    assert_ne!(other_class[0].node_id, state[0].node_id);
    let other_conn = sync_state_mutations("conn:other", "kg:Ticket", &outcomes[..1], 42);
    assert_ne!(other_conn[0].node_id, state[0].node_id);
}

fn ticket_node(key: &str, title: &str) -> (String, serde_json::Map<String, Value>) {
    let mut props = serde_json::Map::new();
    props.insert("title".to_string(), Value::String(title.to_string()));
    (format!("ticket:{key}"), props)
}

// spec: MEDULLA-SRC-AUTH-R012.6
#[test]
fn plan_sync_applies_proposes_and_holds_in_one_batch() {
    let mut nodes = super::GraphNodes::new();
    // Graph-only change: base "old", graph "new", source "old".
    nodes.extend([ticket_node("g", "new"), ticket_node("same", "x")]);
    let state = sync_state_mutations(
        "conn",
        "cls",
        &[
            outcome("g", SyncAction::ApplyInbound, None, Some("old")),
            outcome("s", SyncAction::ApplyInbound, None, Some("old")),
        ],
        1,
    );
    for m in state {
        nodes.insert(m.node_id, m.properties);
    }
    nodes.insert(ticket_node("s", "old").0, ticket_node("s", "old").1);
    let source = vec![
        serde_json::json!({"id": "g", "title": "old"}),
        serde_json::json!({"id": "s", "title": "changed"}),
        serde_json::json!({"id": "same", "title": "x"}),
    ];
    let plan = super::plan_sync(
        &Tickets,
        "conn",
        "cls",
        ConflictRule::SystemOfRecordWins,
        "system_of_record_wins",
        &source,
        &nodes,
        2,
    )
    .unwrap();
    assert_eq!(plan.report.unchanged, 1);
    assert_eq!(plan.report.applied, 1);
    assert_eq!(plan.report.proposed, 1);
    assert_eq!(plan.report.outbound[0].record_key, "g");
    // The inbound change and its state share the one batch.
    assert!(plan.batch.iter().any(|m| m.node_id == "ticket:s"));
    assert!(plan
        .batch
        .iter()
        .any(|m| m.properties.get("syncRecordKey") == Some(&Value::String("s".into()))));
}

// spec: MEDULLA-SRC-AUTH-R012.6
#[test]
fn plan_sync_refuses_duplicate_and_keyless_source_records() {
    let nodes = super::GraphNodes::new();
    let dup = vec![
        serde_json::json!({"id": "a", "title": "1"}),
        serde_json::json!({"id": "a", "title": "2"}),
    ];
    let run = |records: &[Value]| {
        super::plan_sync(
            &Tickets,
            "c",
            "k",
            ConflictRule::HoldForReview,
            "hold_for_review",
            records,
            &nodes,
            0,
        )
    };
    assert!(run(&dup).unwrap_err().contains("two records"));
    assert!(run(&[serde_json::json!({"title": "x"})])
        .unwrap_err()
        .contains("no key"));
}
