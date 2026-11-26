//! MEDULLA-ORCH-VOCAB-R811.7: the competency question "what can happen next to X", asked as two
//! served reads. The record's state text comes from `GetNodeProperties`; the machine description
//! is read with one `Method::Sparql` over `__commons__` whose statement text is the only one
//! `next_events_sparql` produces.

#![cfg(all(feature = "server", feature = "security", feature = "sparql"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_h_query::query::state_machine_next::next_events_sparql;
use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload, SparqlResult};

const SECRET: &str = "state-machine-next-served-secret";
const GRAPH: &str = "next-events";
const MACHINES: &str = include_str!("../crates/medulla-core/ontology/state-machines-v1.ttl");
const WORK_ITEM_MACHINE: &str = "https://knuckles-team.github.io/ontology/core#WorkItemMachine";

async fn send(state: &test_support::SharedState, id: u64, graph: &str, method: Method) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, graph, method)).await
}

async fn ok(state: &test_support::SharedState, id: u64, graph: &str, method: Method) -> Response {
    let response = send(state, id, graph, method).await;
    assert!(
        response.error.is_none(),
        "{:?} {:?}",
        response.error,
        response.error_detail
    );
    response
}

/// The engine with one data graph and the machine description in `__commons__`.
async fn served() -> test_support::SharedState {
    common::configure_authority();
    let state = test_support::durable_state(SECRET, common::current_isolation());
    ok(
        &state,
        1,
        GRAPH,
        Method::CreateGraph {
            graph_name: GRAPH.to_string(),
            graph_type: GraphType::Global,
        },
    )
    .await;
    ok(
        &state,
        2,
        "__commons__",
        Method::AddTriples {
            turtle: MACHINES.to_string(),
            ntriples: String::new(),
        },
    )
    .await;
    state
}

/// One seeded work item in `status`.
async fn seed_work_item(state: &test_support::SharedState, id: u64, node: &str, status: &str) {
    let props = serde_json::json!({
        "node_type": "WorkItem",
        "status": status,
        "kind": "next-events",
    });
    ok(
        state,
        id,
        GRAPH,
        Method::AddNode {
            node_id: node.to_string(),
            properties_msgpack: test_support::json_bytes(props),
        },
    )
    .await;
}

/// The `status` text of `node`, read with `GetNodeProperties`.
async fn status_of(state: &test_support::SharedState, id: u64, node: &str) -> String {
    let response = ok(
        state,
        id,
        GRAPH,
        Method::GetNodeProperties {
            node_id: node.to_string(),
        },
    )
    .await;
    let Some(ResultPayload::Raw(bytes)) = response.result else {
        panic!("properties of {node} did not come back");
    };
    let props: serde_json::Value = rmp_serde::from_slice(&bytes).expect("decode properties");
    props["status"].as_str().expect("a status").to_string()
}

/// One `Method::Sparql` against `__commons__` with the given statement text.
async fn sparql(state: &test_support::SharedState, id: u64, query: String) -> Vec<Vec<Option<String>>> {
    let response = ok(
        state,
        id,
        "__commons__",
        Method::Sparql {
            query,
            base_iri: String::new(),
            type_convention: String::new(),
            explain: false,
            requires_packs: Vec::new(),
            graph_media_type: None,
        },
    )
    .await;
    let result: SparqlResult = match response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed rows"),
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).expect("typed rows"),
        other => panic!("unexpected payload {other:?}"),
    };
    result.rows
}

/// A cell without the angle brackets or quotes a term may be rendered with.
fn plain(cell: &Option<String>) -> String {
    cell.as_deref()
        .unwrap_or_default()
        .trim_matches(|c| c == '<' || c == '>' || c == '"')
        .to_string()
}

// spec: MEDULLA-ORCH-VOCAB-R811.7
#[tokio::test(flavor = "multi_thread")]
async fn ready_work_item_lists_claim_and_cancel() {
    let state = served().await;
    seed_work_item(&state, 3, "wi-ready", "ready").await;
    let status = status_of(&state, 4, "wi-ready").await;
    let rows = sparql(
        &state,
        5,
        next_events_sparql(WORK_ITEM_MACHINE, &status).expect("a valid statement"),
    )
    .await;
    // Columns: edge, event, guard, to; ordered by edge id.
    let edges: Vec<String> = rows.iter().map(|row| plain(&row[0])).collect();
    assert_eq!(edges, vec!["W02", "W16"], "{rows:?}");
    assert!(plain(&rows[0][1]).ends_with("WorkItemMachine_Event_Claim"));
    assert!(plain(&rows[0][2]).ends_with("WorkItemMachine_Guard_RetryDueAttemptLeft"));
    assert_eq!(plain(&rows[0][3]), "leased");
    assert!(plain(&rows[1][1]).ends_with("WorkItemMachine_Event_Cancel"));
    assert!(rows[1][2].is_none(), "the cancel edge has no guard");
    assert_eq!(plain(&rows[1][3]), "cancelled");
}

// spec: MEDULLA-ORCH-VOCAB-R811.7
#[tokio::test(flavor = "multi_thread")]
async fn terminal_work_item_lists_no_event() {
    let state = served().await;
    seed_work_item(&state, 3, "wi-done", "succeeded").await;
    let status = status_of(&state, 4, "wi-done").await;
    let rows = sparql(
        &state,
        5,
        next_events_sparql(WORK_ITEM_MACHINE, &status).expect("a valid statement"),
    )
    .await;
    assert!(rows.is_empty(), "{rows:?}");
}

// spec: MEDULLA-ORCH-VOCAB-R811.7
#[tokio::test(flavor = "multi_thread")]
async fn every_medulla_machine_answers_next_events() {
    let state = served().await;
    let prefix = format!("PREFIX : <{}> ", medulla_h_query::query::state_machine_next::CORE_NAMESPACE);
    let machines = sparql(
        &state,
        3,
        format!("{prefix}SELECT ?m WHERE {{ ?m a :StateMachine }}"),
    )
    .await;
    let initial = sparql(
        &state,
        4,
        format!(
            "{prefix}SELECT ?m ?stored WHERE {{ ?m a :StateMachine ; :initialState ?s . \
             ?s :storedForm ?stored }}"
        ),
    )
    .await;
    assert!(!machines.is_empty(), "the machines are served");
    assert_eq!(
        initial.len(),
        machines.len(),
        "every machine states an initial state with its stored text"
    );
    let mut id = 10;
    for row in &initial {
        let (machine, stored) = (plain(&row[0]), plain(&row[1]));
        let next = sparql(
            &state,
            id,
            next_events_sparql(&machine, &stored).expect("a valid statement"),
        )
        .await;
        id += 1;
        if next.is_empty() {
            // Terminal-only: the initial state is a terminal state.
            let terminal = sparql(
                &state,
                id,
                format!(
                    "{prefix}SELECT ?s WHERE {{ <{machine}> :initialState ?s . ?s a ?c . \
                     ?c <http://www.w3.org/2000/01/rdf-schema#subClassOf> :TerminalState }}"
                ),
            )
            .await;
            id += 1;
            assert!(
                !terminal.is_empty(),
                "{machine} neither lists an event from '{stored}' nor starts terminal"
            );
        }
    }
}
