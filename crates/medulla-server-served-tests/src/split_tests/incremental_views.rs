//! Served proofs for incremental view maintenance over ontologies: retracting one
//! subclass edge through `RemoveEdge` updates the opted-in closure by delete and
//! re-derive, never by a fresh rebuild.

use std::sync::Arc;
use std::time::Duration;

use medulla_protocol_types::protocol::Method;
use medulla_server_api::state::ServerState;
use medulla_server_cdc::cdc::CdcStateExt;
use medulla_server_cdc::reasoning_cascade::ReasoningCascade;
use tokio::sync::RwLock;

use super::medulla_wire_wire_tests::wired_catalog_tests::create_test_graph;
use super::medulla_wire_wire_tests::wired_catalog_tests::request;
use super::medulla_wire_wire_tests::wired_catalog_tests::test_state;
use super::medulla_wire_wire_tests::wired_catalog_tests::CREATOR;

const GRAPH: &str = "onto";
const SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const DEBOUNCE: Duration = Duration::from_millis(100);
const SETTLE: Duration = Duration::from_millis(400);
/// The spine of the hierarchy; each spine class carries `LEAVES` leaf subclasses.
const SPINE: usize = 100;
const LEAVES: usize = 99;

fn class(name: &str) -> String {
    format!("<http://ex/{name}>")
}

/// `(sub, sup)` edges of 10,000 classes: a spine `S0 ⊑ S1 ⊑ …` with leaves under each.
fn hierarchy_edges() -> Vec<(String, String)> {
    let spine = (1..SPINE).map(|i| (class(&format!("S{}", i - 1)), class(&format!("S{i}"))));
    let leaves = (0..SPINE).flat_map(|i| {
        (0..LEAVES).map(move |j| (class(&format!("L{i}_{j}")), class(&format!("S{i}"))))
    });
    spine.chain(leaves).collect()
}

/// Load the hierarchy straight into the graph core (the write path under test is the
/// retraction, not the 10,000-class load).
async fn load_hierarchy(state: &Arc<RwLock<ServerState>>) {
    let core = state
        .read()
        .await
        .registry
        .get(GRAPH)
        .expect("graph exists")
        .core
        .clone();
    let props = rmp_serde::to_vec_named(&serde_json::json!({ "relationship": SUBCLASS_OF }))
        .expect("edge properties");
    let empty = rmp_serde::to_vec_named(&serde_json::json!({})).expect("node properties");
    for (sub, sup) in hierarchy_edges() {
        core.add_node(sub.clone(), empty.clone());
        core.add_node(sup.clone(), empty.clone());
        core.add_edge(sub, sup, props.clone())
            .expect("subclass edge");
    }
}

fn armed_cascade() -> Arc<ReasoningCascade> {
    Arc::new(ReasoningCascade::new(
        [GRAPH.to_string()].into_iter().collect(),
        DEBOUNCE,
    ))
}

/// Let the debounce elapse and run the sweep; the number of graphs refreshed.
async fn sweep(cascade: &ReasoningCascade, state: &Arc<RwLock<ServerState>>) -> usize {
    tokio::time::sleep(SETTLE).await;
    cascade.refresh_due(state).await
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R011.7
#[tokio::test(flavor = "multi_thread")]
async fn ontology_retraction_updates_closure_incrementally() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let state = test_state(&[CREATOR]);
    create_test_graph(&state, GRAPH, 1).await;
    load_hierarchy(&state).await;
    let cascade = armed_cascade();
    state
        .read()
        .await
        .cdc_hub()
        .expect("cdc hub")
        .install_reasoning_cascade(cascade.clone());

    cascade.note_write(GRAPH);
    assert_eq!(sweep(&cascade, &state).await, 1);
    let first = cascade.refresh_count(GRAPH);
    let (s0, s1) = (class("S49"), class("S50"));
    let before = cascade.last_refresh(GRAPH).expect("first refresh");
    assert!(before.classification.entails_subclass(&s0, &class("S99")));

    let removal = Method::RemoveEdge {
        source_id: s0.clone(),
        target_id: s1.clone(),
    };
    let reply = medulla_server_router::dispatch::request_boundary::dispatch(
        &state,
        request(2, GRAPH, removal),
    )
    .await;
    assert!(
        reply.error.is_none(),
        "RemoveEdge failed: {:?}",
        reply.error
    );
    assert_eq!(sweep(&cascade, &state).await, 1);

    assert_eq!(cascade.refresh_count(GRAPH), first + 1);
    let after = cascade.last_refresh(GRAPH).expect("second refresh");
    assert!(
        after.retracted_triples > 0,
        "the sweep retracted, not rebuilt"
    );
    assert!(!after.classification.entails_subclass(&s0, &class("S99")));

    // The closure equals a fresh classification of the same graph.
    let fresh = armed_cascade();
    fresh.note_write(GRAPH);
    assert_eq!(sweep(&fresh, &state).await, 1);
    let expected = fresh.last_refresh(GRAPH).expect("fresh refresh");
    assert_eq!(
        after.classification.subsumers,
        expected.classification.subsumers
    );
}
