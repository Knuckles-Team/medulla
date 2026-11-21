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

/// Verifies MEDULLA-INCREMENTAL-VIEWS-R006: a persisted plan view reloads at boot
/// compiled and reseeded in incremental mode, and the operator-state row is rewritten.
// spec: MEDULLA-INCREMENTAL-VIEWS-R006
#[cfg(all(feature = "matview", feature = "redb"))]
#[tokio::test(flavor = "multi_thread")]
async fn restart_reloads_plan_view_in_incremental_mode() {
    use medulla_persistence::AsRedb;
    use medulla_protocol_types::protocol::GraphType;
    use medulla_query_types::wire::{Op, Plan};
    use medulla_server_cdc::matview::{encode_def, manager, Mode, PlanMatView};

    const VIEW: &str = "r006-restart-view";
    let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
    let dir = tempfile::tempdir().unwrap();
    let persist = dir.path().to_string_lossy().into_owned();
    let backend: Arc<dyn medulla_server_api::api::persistence::PersistenceBackend> = Arc::new(
        medulla_persistence::redb_backend::RedbBackend::open(persist.clone(), 64).unwrap(),
    );
    let mut state = medulla_server::seams_install::new_state_for_test(
        "r006-reload-secret", // sanitizer:ignore
        ServerState::test_isolation(CREATOR),
    );
    state.persist_dir = Some(persist);
    state.persistence = Some(backend.clone());
    let _ = state.registry.create_graph(GRAPH, GraphType::Global, None);
    let state = Arc::new(RwLock::new(state));

    // The durable definition a previous process left behind; the in-RAM index is empty.
    let view = PlanMatView {
        name: VIEW.to_string(),
        graph: GRAPH.to_string(),
        plan: Plan::new(vec![Op::Scan {
            label: "Doc".into(),
        }]),
    };
    let redb = backend.as_redb().expect("redb backend");
    redb.plan_matview_put(VIEW, encode_def(&view).unwrap())
        .await
        .unwrap();
    assert!(manager().mode(VIEW).is_none());

    let txn = medulla_server_ops::api::query_txn::txn_seam(&state)
        .await
        .expect("transaction seam installed");
    assert!(txn.reload_matviews(&state).await.unwrap() >= 1);

    assert_eq!(manager().mode(VIEW), Some(Mode::Incremental));
    assert!(manager().fallback_reason(VIEW).is_none());
    assert!(
        redb.matview_operator_state_scan()
            .unwrap()
            .iter()
            .any(|(name, _)| name == VIEW),
        "the operator-state row is rewritten by the reseed"
    );
}

/// Name of the plan view the served hot-path proof defines (the matview registry is process-wide).
#[cfg(all(feature = "matview", feature = "redb"))]
const VIEW: &str = "served_plan_view_r001_10";
#[cfg(all(feature = "matview", feature = "redb"))]
const COMMONS: &str = "__commons__";
#[cfg(all(feature = "matview", feature = "redb"))]
const ADDED: usize = 200;
#[cfg(all(feature = "matview", feature = "redb"))]
const CHANGED: usize = 50;

/// `Scan Doc` then `Filter year > 2000`.
#[cfg(all(feature = "matview", feature = "redb"))]
fn doc_plan() -> medulla_query_types::wire::Plan {
    use medulla_query_types::wire::{Op, Plan, Pred};
    Plan::new(vec![
        Op::Scan {
            label: "Doc".into(),
        },
        Op::Filter {
            preds: vec![Pred::GtNum {
                prop: "year".into(),
                n: 2000.0,
            }],
        },
    ])
}

#[cfg(all(feature = "matview", feature = "redb"))]
fn doc_props(year: u32) -> Vec<u8> {
    rmp_serde::to_vec_named(&serde_json::json!({ "type": "Doc", "year": year }))
        .expect("doc properties")
}

/// One served request through the dispatch path; the reply must be a success.
#[cfg(all(feature = "matview", feature = "redb"))]
async fn send(
    state: &Arc<RwLock<ServerState>>,
    id: u64,
    method: Method,
) -> medulla_protocol_types::protocol::Response {
    let reply = crate::tests::dispatch_on_heap(state, request(id, COMMONS, method)).await;
    assert!(
        reply.error.is_none(),
        "request {id} failed: {:?}, detail: {:?}",
        reply.error,
        reply.error_detail
    );
    reply
}

/// Rows the served `PlanMatViewGet` returns, sorted by id.
#[cfg(all(feature = "matview", feature = "redb"))]
async fn served_rows(state: &Arc<RwLock<ServerState>>, id: u64) -> Vec<(String, Option<f32>)> {
    let reply = send(state, id, Method::PlanMatViewGet { name: VIEW.into() }).await;
    let Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) = reply.result else {
        panic!("PlanMatViewGet returned a non-raw payload");
    };
    let mut rows: Vec<(String, Option<f32>)> = rmp_serde::from_slice(&bytes).expect("view rows");
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// The same plan recomputed over the graph as it stands now, sorted by id.
#[cfg(all(feature = "matview", feature = "redb"))]
async fn recomputed_rows(state: &Arc<RwLock<ServerState>>) -> Vec<(String, Option<f32>)> {
    let core = state
        .read()
        .await
        .registry
        .get(COMMONS)
        .expect("graph exists")
        .core
        .clone();
    let def = medulla_server_cdc::matview::PlanMatView {
        name: VIEW.into(),
        graph: COMMONS.into(),
        plan: doc_plan(),
    };
    let (snapshot, _) = core.analysis_snapshot_versioned();
    let semantic = core.semantic_store.read().clone();
    // The oracle's FILTER leg drives DataFusion through a current-thread runtime
    // (`medulla_sql::sql::exec_sql` builds one and `block_on`s it), which cannot nest
    // inside this `#[tokio::test]` reactor — run it off-reactor, exactly as the
    // served handler does.
    let rows = tokio::task::spawn_blocking(move || {
        medulla_server_cdc::matview::materialize(&def, &snapshot, &semantic)
    })
    .await
    .expect("recompute oracle task")
    .expect("recompute");
    let mut rows = rows;
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// Read the view after a write: it equals a recompute and still runs incrementally.
#[cfg(all(feature = "matview", feature = "redb"))]
async fn assert_view_equals_recompute(state: &Arc<RwLock<ServerState>>, id: u64) {
    let served = served_rows(state, id).await;
    assert_eq!(
        served,
        recomputed_rows(state).await,
        "read after request {id}"
    );
    assert_eq!(
        medulla_server_cdc::matview::manager().mode(VIEW),
        Some(medulla_server_cdc::matview::Mode::Incremental)
    );
}

/// The writes of the proof, in order: add `ADDED` docs, update `CHANGED`, remove `CHANGED`.
#[cfg(all(feature = "matview", feature = "redb"))]
fn doc_writes() -> Vec<Method> {
    let node = |i: usize| format!("d{i}");
    let add = (0..ADDED).map(|i| Method::AddNode {
        node_id: node(i),
        properties_msgpack: doc_props(1980 + (i % 40) as u32),
    });
    // An update is `AddNode` over an existing id: it replaces the properties.
    let update = (0..CHANGED).map(|i| Method::AddNode {
        node_id: node(i),
        properties_msgpack: doc_props(if i % 2 == 0 { 2015 } else { 1985 }),
    });
    let remove = (ADDED - CHANGED..ADDED).map(|i| Method::RemoveNode { node_id: node(i) });
    add.chain(update).chain(remove).collect()
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R001, MEDULLA-INCREMENTAL-VIEWS-R001.10
#[cfg(all(feature = "matview", feature = "redb"))]
#[tokio::test(flavor = "multi_thread")]
async fn plan_view_served_define_write_get_equals_recompute() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let state = test_state(&[CREATOR]);
    if state.read().await.registry.get(COMMONS).is_none() {
        create_test_graph(&state, COMMONS, 1).await;
    }
    let define = Method::PlanMatViewDefine {
        name: VIEW.into(),
        graph: COMMONS.into(),
        plan: doc_plan(),
    };
    send(&state, 2, define).await;
    assert_view_equals_recompute(&state, 3).await;
    for (step, write) in doc_writes().into_iter().enumerate() {
        let id = 10 + 2 * step as u64;
        send(&state, id, write).await;
        assert_view_equals_recompute(&state, id + 1).await;
    }
    let dropped = Method::PlanMatViewDrop { name: VIEW.into() };
    send(&state, 10_000, dropped).await;
}

/// Per-view ceiling and process budget the budget proof runs under (registry knobs
/// `view.state_bytes` and `views.memory_bytes`). The budget is the knob's minimum; the
/// ceiling sits between one view's size and the size of the largest.
#[cfg(all(feature = "matview", feature = "redb"))]
const BUDGET_VIEW_CEILING: u64 = 12 * 1024 * 1024;
#[cfg(all(feature = "matview", feature = "redb"))]
const BUDGET_MEMORY: u64 = 16 * 1024 * 1024;
/// Every node id is this long, so one maintained entry costs `48 + ID_LEN` bytes.
#[cfg(all(feature = "matview", feature = "redb"))]
const ID_LEN: usize = 3000;
/// Nodes each graph holds before its view is defined: two of the first two views together
/// pass the budget, the third alone passes the ceiling.
#[cfg(all(feature = "matview", feature = "redb"))]
const BUDGET_SEED: [(&str, &str, usize); 3] = [
    ("r007_5_graph_a", "r007_5_view_a", 3000),
    ("r007_5_graph_b", "r007_5_view_b", 3000),
    ("r007_5_graph_c", "r007_5_view_c", 4200),
];

#[cfg(all(feature = "matview", feature = "redb"))]
fn budget_node_id(graph: &str, i: usize) -> String {
    let mut id = format!("{graph}-{i:06}-");
    id.extend(std::iter::repeat_n('x', ID_LEN - id.len()));
    id
}

#[cfg(all(feature = "matview", feature = "redb"))]
fn scan_plan() -> medulla_query_types::wire::Plan {
    medulla_query_types::wire::Plan::new(vec![medulla_query_types::wire::Op::Scan {
        label: "Doc".into(),
    }])
}

/// One served request on `graph`; the reply must be a success.
#[cfg(all(feature = "matview", feature = "redb"))]
async fn send_in(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    id: u64,
    method: Method,
) -> medulla_protocol_types::protocol::Response {
    let reply = crate::tests::dispatch_on_heap(state, request(id, graph, method)).await;
    assert!(
        reply.error.is_none(),
        "request {id} on {graph} failed: {:?}, detail: {:?}",
        reply.error,
        reply.error_detail
    );
    reply
}

/// The rows `view` serves, and the rows a full recompute over `graph` gives, both sorted.
#[cfg(all(feature = "matview", feature = "redb"))]
async fn served_and_recomputed(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    view: &str,
    id: u64,
) -> (Vec<(String, Option<f32>)>, Vec<(String, Option<f32>)>) {
    let reply = send_in(
        state,
        graph,
        id,
        Method::PlanMatViewGet { name: view.into() },
    )
    .await;
    let Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) = reply.result else {
        panic!("PlanMatViewGet returned a non-raw payload");
    };
    let mut served: Vec<(String, Option<f32>)> = rmp_serde::from_slice(&bytes).expect("view rows");
    served.sort_by(|a, b| a.0.cmp(&b.0));
    let core = state
        .read()
        .await
        .registry
        .get(graph)
        .expect("graph exists")
        .core
        .clone();
    let def = medulla_server_cdc::matview::PlanMatView {
        name: view.into(),
        graph: graph.into(),
        plan: scan_plan(),
    };
    let (snapshot, _) = core.analysis_snapshot_versioned();
    let semantic = core.semantic_store.read().clone();
    // The recompute drives DataFusion through its own runtime; run it off-reactor.
    let mut recomputed = tokio::task::spawn_blocking(move || {
        medulla_server_cdc::matview::materialize(&def, &snapshot, &semantic)
    })
    .await
    .expect("recompute task")
    .expect("recompute");
    recomputed.sort_by(|a, b| a.0.cmp(&b.0));
    (served, recomputed)
}

/// Verifies MEDULLA-INCREMENTAL-VIEWS-R007.5: three views over three graphs run under a
/// tight memory budget and per-view ceiling; one spills, one demotes with
/// `STATE_BUDGET_EXCEEDED`, and every read equals a recompute.
// spec: MEDULLA-INCREMENTAL-VIEWS-R007.5
#[cfg(all(feature = "matview", feature = "redb"))]
#[tokio::test(flavor = "multi_thread")]
async fn state_budget_spills_then_demotes_and_answers_stay_correct() {
    use medulla_core::view_status::{snapshot, ViewMode};
    use medulla_server_api::knob_config;
    use medulla_server_cdc::matview::{manager, Mode};

    // The knobs are read once per process, so they are installed before the first
    // manager call; this test runs in its own process.
    let loaded = knob_config::load_knobs(
        &[
            format!("view.state_bytes={BUDGET_VIEW_CEILING}"),
            format!("views.memory_bytes={BUDGET_MEMORY}"),
        ],
        |_| None,
    )
    .expect("budget knobs load");
    knob_config::install(loaded);
    assert_eq!(knob_config::knobs().views_memory_bytes, BUDGET_MEMORY);
    assert_eq!(knob_config::knobs().view_state_bytes, BUDGET_VIEW_CEILING);

    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let state = test_state(&[CREATOR]);
    let props = rmp_serde::to_vec_named(&serde_json::json!({ "type": "Doc" })).expect("props");
    for (n, (graph, _, count)) in BUDGET_SEED.iter().enumerate() {
        create_test_graph(&state, graph, 1 + n as u64).await;
        let core = state
            .read()
            .await
            .registry
            .get(graph)
            .expect("graph exists")
            .core
            .clone();
        for i in 0..*count {
            core.add_node(budget_node_id(graph, i), props.clone());
        }
    }

    // The store backs the spill once the transaction seam has reloaded the views.
    let txn = medulla_server_ops::api::query_txn::txn_seam(&state)
        .await
        .expect("transaction seam installed");
    txn.reload_matviews(&state).await.expect("reload");
    for (n, (graph, view, _)) in BUDGET_SEED.iter().enumerate() {
        let define = Method::PlanMatViewDefine {
            name: (*view).into(),
            graph: (*graph).into(),
            plan: scan_plan(),
        };
        send_in(&state, graph, 10 + n as u64, define).await;
    }

    // Write to each graph in turn; read and check every view after every write.
    let mut request_id = 100;
    for round in 0..3usize {
        for (graph, _, count) in BUDGET_SEED.iter() {
            request_id += 1;
            let write = Method::AddNode {
                node_id: budget_node_id(graph, count + round),
                properties_msgpack: props.clone(),
            };
            send_in(&state, graph, request_id, write).await;
            for (read_graph, view, _) in BUDGET_SEED.iter() {
                request_id += 1;
                let (served, recomputed) =
                    served_and_recomputed(&state, read_graph, view, request_id).await;
                assert_eq!(
                    served, recomputed,
                    "view {view} after a write to {graph} (round {round})"
                );
            }
        }
    }

    let modes: Vec<Option<Mode>> = BUDGET_SEED
        .iter()
        .map(|(_, v, _)| manager().mode(v))
        .collect();
    let spilled = modes.iter().filter(|m| **m == Some(Mode::Spilled)).count();
    let recompute: Vec<&str> = BUDGET_SEED
        .iter()
        .filter(|(_, v, _)| manager().mode(v) == Some(Mode::Recompute))
        .map(|(_, v, _)| *v)
        .collect();
    assert_eq!(spilled, 1, "one view is spilled: {modes:?}");
    assert_eq!(
        recompute,
        vec!["r007_5_view_c"],
        "one view demoted: {modes:?}"
    );
    assert_eq!(
        manager().fallback_reason("r007_5_view_c").as_deref(),
        Some("STATE_BUDGET_EXCEEDED")
    );

    // The status board agrees with the manager.
    let board: Vec<_> = snapshot()
        .into_iter()
        .filter(|s| s.name.starts_with("r007_5_view_"))
        .collect();
    assert_eq!(
        board.iter().filter(|s| s.mode == ViewMode::Spilled).count(),
        1
    );
    let demoted: Vec<_> = board
        .iter()
        .filter(|s| s.reason_code.as_deref() == Some("STATE_BUDGET_EXCEEDED"))
        .collect();
    assert_eq!(demoted.len(), 1);
    assert_eq!(demoted[0].mode, ViewMode::Recompute);

    // Each drop is its own operation, so each carries its own request id: a reused id
    // is the same idempotency key and the second drop would refuse as a conflict.
    for (n, (_, view, _)) in BUDGET_SEED.iter().enumerate() {
        let dropped = Method::PlanMatViewDrop {
            name: (*view).into(),
        };
        send_in(&state, "r007_5_graph_a", 10_000 + n as u64, dropped).await;
    }
}

/// One served `CdcRead` of `feed` on `graph`, decoded; the reply must be a success.
#[cfg(all(feature = "matview", feature = "redb"))]
async fn read_feed(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    id: u64,
    feed: &str,
    from_seq: u64,
) -> medulla_query_types::wire::CdcReadResult {
    let reply = send_in(
        state,
        graph,
        id,
        Method::CdcRead {
            graph: feed.into(),
            from_seq,
            limit: 0,
        },
    )
    .await;
    let Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) = reply.result else {
        panic!("CdcRead returned a non-raw payload");
    };
    rmp_serde::from_slice(&bytes).expect("CdcRead result")
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R010.3
#[cfg(all(feature = "matview", feature = "redb"))]
#[tokio::test(flavor = "multi_thread")]
async fn view_feed_gap_then_snapshot_resume() {
    const FEED_GRAPH: &str = "r010_3_graph";
    const FEED_VIEW: &str = "r010_3_view";
    let feed = format!("view:{FEED_VIEW}");

    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let state = test_state(&[CREATOR]);
    create_test_graph(&state, FEED_GRAPH, 1).await;
    let define = Method::PlanMatViewDefine {
        name: FEED_VIEW.into(),
        graph: FEED_GRAPH.into(),
        plan: doc_plan(),
    };
    send_in(&state, FEED_GRAPH, 2, define).await;
    for i in 0..3u64 {
        let write = Method::AddNode {
            node_id: format!("feed-doc-{i}"),
            properties_msgpack: doc_props(2010 + i as u32),
        };
        send_in(&state, FEED_GRAPH, 10 + i, write).await;
    }

    // The feed answers the rows the view gained, in order, from the start.
    let first = read_feed(&state, FEED_GRAPH, 20, &feed, 0).await;
    assert!(!first.gap);
    assert_eq!(first.events.len(), 3);
    let head_seq = first.head_seq;
    assert_eq!(head_seq, 3);

    // A cursor outside the retained window answers a gap with the current window.
    let behind = read_feed(&state, FEED_GRAPH, 21, &feed, head_seq + 1_000).await;
    assert!(behind.gap);
    assert!(behind.events.is_empty());
    assert_eq!(behind.head_seq, head_seq);

    // The subscriber re-seeds from the snapshot, then resumes at the head it read before it.
    let snapshot = send_in(
        &state,
        FEED_GRAPH,
        22,
        Method::PlanMatViewGet {
            name: FEED_VIEW.into(),
        },
    )
    .await;
    let Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) = snapshot.result else {
        panic!("PlanMatViewGet returned a non-raw payload");
    };
    let rows: Vec<(String, Option<f32>)> = rmp_serde::from_slice(&bytes).expect("view rows");
    assert_eq!(rows.len(), 3);
    let write = Method::AddNode {
        node_id: "feed-doc-3".into(),
        properties_msgpack: doc_props(2020),
    };
    send_in(&state, FEED_GRAPH, 23, write).await;
    let resumed = read_feed(&state, FEED_GRAPH, 24, &feed, head_seq).await;
    assert!(!resumed.gap);
    assert_eq!(resumed.events.len(), 1);
    assert_eq!(resumed.events[0].node_id, "feed-doc-3");

    // A feed of a view that is not defined is refused with its code.
    let unknown = crate::tests::dispatch_on_heap(
        &state,
        request(
            25,
            FEED_GRAPH,
            Method::CdcRead {
                graph: "view:r010_3_missing".into(),
                from_seq: 0,
                limit: 0,
            },
        ),
    )
    .await;
    assert!(
        unknown
            .refusal_text()
            .is_some_and(|error| error.contains("MATVIEW_UNKNOWN")),
        "unexpected reply: {:?}",
        unknown.error
    );

    let dropped = Method::PlanMatViewDrop {
        name: FEED_VIEW.into(),
    };
    send_in(&state, FEED_GRAPH, 26, dropped).await;
}

// spec: MEDULLA-INCREMENTAL-VIEWS-R010.4
#[cfg(all(feature = "matview", feature = "redb"))]
#[tokio::test(flavor = "multi_thread")]
async fn view_delta_feed_matches_result_difference() {
    use std::collections::BTreeMap;

    use medulla_query_types::wire::CdcKind;

    const DELTA_GRAPH: &str = "r010_4_graph";
    const DELTA_VIEW: &str = "r010_4_view";
    const DOCS: u64 = 20;
    const CHANGES: u64 = 100;
    let feed = format!("view:{DELTA_VIEW}");

    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let state = test_state(&[CREATOR]);
    create_test_graph(&state, DELTA_GRAPH, 1).await;
    let define = Method::PlanMatViewDefine {
        name: DELTA_VIEW.into(),
        graph: DELTA_GRAPH.into(),
        plan: doc_plan(),
    };
    send_in(&state, DELTA_GRAPH, 2, define).await;

    // Subscribe at seq 0, then apply 100 pseudo-random changes: each one adds or replaces
    // a document with a year on either side of the filter, or removes one. After every
    // change the rows received since the last read, folded as `+1` for an insert and `-1`
    // for a delete, must equal the view the served `PlanMatViewGet` answers.
    let first = read_feed(&state, DELTA_GRAPH, 3, &feed, 0).await;
    assert!(!first.gap);
    assert!(first.events.is_empty());
    let mut cursor = first.head_seq;
    let mut folded: BTreeMap<String, i64> = BTreeMap::new();
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for step in 0..CHANGES {
        let draw = next();
        let node_id = format!("delta-doc-{}", draw % DOCS);
        let write = if (draw >> 8) % 4 == 0 {
            Method::RemoveNode {
                node_id: node_id.clone(),
            }
        } else {
            Method::AddNode {
                node_id: node_id.clone(),
                properties_msgpack: doc_props(1990 + ((draw >> 16) % 30) as u32),
            }
        };
        let id = 100 + 4 * step;
        // A removal of an absent document is refused; the view is unchanged then.
        let reply = crate::tests::dispatch_on_heap(&state, request(id, DELTA_GRAPH, write)).await;
        let _ = reply;

        let read = read_feed(&state, DELTA_GRAPH, id + 1, &feed, cursor).await;
        assert!(!read.gap, "change {step}: the cursor stays inside the ring");
        for event in &read.events {
            assert_eq!(event.table, DELTA_VIEW);
            match event.kind {
                CdcKind::InsertRow => *folded.entry(event.node_id.clone()).or_insert(0) += 1,
                CdcKind::DeleteRow => *folded.entry(event.node_id.clone()).or_insert(0) -= 1,
                other => panic!("a view feed carries row changes only, got {other:?}"),
            }
        }
        cursor = read.head_seq;

        let snapshot = send_in(
            &state,
            DELTA_GRAPH,
            id + 2,
            Method::PlanMatViewGet {
                name: DELTA_VIEW.into(),
            },
        )
        .await;
        let Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) = snapshot.result
        else {
            panic!("PlanMatViewGet returned a non-raw payload");
        };
        let rows: Vec<(String, Option<f32>)> = rmp_serde::from_slice(&bytes).expect("view rows");
        let mut served: Vec<String> = rows.into_iter().map(|(row_id, _)| row_id).collect();
        served.sort();
        let multiset: Vec<String> = folded
            .iter()
            .flat_map(|(row_id, count)| {
                assert!(
                    (0..=1).contains(count),
                    "change {step}: row {row_id} folded to {count}"
                );
                std::iter::repeat_n(row_id.clone(), *count as usize)
            })
            .collect();
        assert_eq!(multiset, served, "change {step}: the feed folds to the view");
    }

    let dropped = Method::PlanMatViewDrop {
        name: DELTA_VIEW.into(),
    };
    send_in(&state, DELTA_GRAPH, 10_000, dropped).await;
}
