//! Serving the scoped query adapter (MEDULLA-DECISION-ENGINE-R174.7.3.1): the
//! adapter a `ScopedAdapter` pointer activates is the one the read paths apply
//! to a `Discover` or `SemanticSearch` query vector, and a rollback serves the
//! base query again.

use super::learning_tests::{
    activate, embedded_graph, fit_request, judged_runs, pointer_state, top, GRAPH,
};
use super::retrieval_tests::learn;
use super::*;
use medulla_decision_types::decision::statistical::retrieval::{
    LearningRecorded, LearningWrite, PointerMovement, PointerRef,
};
use medulla_decision_types::decision::statistical::retrieval_adapter::{
    AdapterFitted, AdapterScope,
};
use medulla_server_ops::api::admin::served_adapter::{adapted_query, resolve_served, salt};

// spec: MEDULLA-DECISION-ENGINE-R174.7.3.1
#[tokio::test]
async fn discover_applies_the_resolved_adapter() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    // The read paths resolve the adapter through the installed decide seam, the
    // same route `Discover` and `SemanticSearch` take in the graph operations.
    assert!(medulla_server_ops::api::admin::install_decide_seam(
        &h.state.read().await.seams,
        Arc::new(super::super::DecideSeam),
    ));
    let core = embedded_graph(&h).await;
    let space = core.semantic_store.read().space().unwrap().digest.clone();
    judged_runs(&h, &space).await;
    let fitted: AdapterFitted = match learn(
        &h,
        "decider",
        LearningWrite::FitAdapter {
            request: Box::new(fit_request(&space)),
        },
    )
    .await
    .unwrap()
    {
        LearningRecorded::Fitted(fitted) => *fitted,
        other => panic!("fit: {other:?}"),
    };
    assert!(fitted.receipt.passed, "{:?}", fitted.receipt);

    let tenant = medulla_server_api::carrier::CarrierAuthority::from_verified(&verified())
        .unwrap()
        .tenant_scope()
        .to_string();
    let query = vec![1.0_f32, 0.9, 0.0];
    let served_query = || {
        adapted_query(
            &h.state,
            Some(&tenant),
            (GRAPH, core.as_ref()),
            query.clone(),
        )
    };
    let scoped = || PointerRef::ScopedAdapter {
        graph: GRAPH.to_string(),
        scope: AdapterScope::Graph,
    };

    // Fitted but not active: the base query is served, and the base order
    // prefers the negatives.
    assert_eq!(served_query().await, query);
    assert!(top(&core, &served_query().await).starts_with('n'));
    assert!(resolve_served(&h.state, Some(&tenant), GRAPH, &core)
        .await
        .is_none());

    // Activating through the scoped pointer re-orders the rows, and the cache
    // key is salted with the resolved adapter's digest.
    let moved = activate(&fitted.adapter_digest, &fitted.receipt_digest);
    let active = pointer_state(&h, scoped(), moved).await.unwrap();
    assert_eq!(active.target(), Some(fitted.adapter_digest.as_str()));
    let adapted = served_query().await;
    assert_ne!(adapted, query, "the resolved adapter re-aims the query");
    assert!(
        top(&core, &adapted).starts_with('p'),
        "the adapter re-orders the rows toward what was cited"
    );
    let resolved = resolve_served(&h.state, Some(&tenant), GRAPH, &core)
        .await
        .expect("the activated adapter resolves");
    assert_eq!(resolved.digest(), fitted.adapter_digest);
    let mut salted = Vec::new();
    salt(Some(&resolved), &mut salted);
    assert_eq!(salted, fitted.adapter_digest.as_bytes());

    // Another tenant's read never sees it.
    assert_eq!(
        adapted_query(
            &h.state,
            Some("tenant-other"),
            (GRAPH, core.as_ref()),
            query.clone()
        )
        .await,
        query
    );

    // Rollback serves the base order again.
    let rolled = pointer_state(&h, scoped(), PointerMovement::Rollback)
        .await
        .unwrap();
    assert!(rolled.active.is_none());
    assert_eq!(served_query().await, query);
    assert!(top(&core, &served_query().await).starts_with('n'));
}

// --- scoped fit, activation, rollback, monitor and refusals ---------------

use super::super::stat_adapter::scoped_adapter_pointer;
use super::super::stat_pointer::{read_pointer, PointerMove, Qualified};
use super::learning_tests::{q16, space_of};
use super::retrieval_tests::{
    attest, clone_executed, evaluate_by, executed_template, outcome, template,
};
use medulla_decision_types::decision::jobs::RecordWindow;
use medulla_decision_types::decision::statistical::retrieval::QueryVector;
use medulla_decision_types::decision::statistical::retrieval_adapter::{
    AdapterFitRequest, AdapterMonitorRequest, AdapterSide, MonitorState, OutcomeSource,
};

/// The corpus class the template plan anchors on.
const CORPUS: &str = "Incident";

fn corpus() -> AdapterScope {
    AdapterScope::Corpus {
        corpus_iri: CORPUS.to_string(),
    }
}

/// Judged runs like `judged_runs`, each executing the typed plan template, so
/// the corpus scope covers them. `cited_axis` picks what the answers cite:
/// `p<i>` (what the adapter should learn to favour) or `n<i>` (degrading).
async fn scoped_runs(h: &Harness, space: &str, prefix: &str, count: usize, cite_p: bool) {
    let template_entry = executed_template(h).await;
    for i in 0..count {
        let record_id = format!("{prefix}-{i:03}");
        clone_executed(h, &template_entry, &record_id);
        let (n, p) = (format!("n{}", i % 40), format!("p{}", i % 40));
        let cited: &str = if cite_p { &p } else { &n };
        let mut attested = outcome(&record_id, &[&n, &p], &[cited]);
        if !cite_p {
            attested = outcome(&record_id, &[&p, &n], &[cited]);
        }
        attested.query = Some(QueryVector {
            space_digest: space.to_string(),
            q16: q16(&[1.0, 0.9 + (i % 40) as f64 * 0.001, 0.0]),
        });
        attested.path = Some(template("sha256:schema-a"));
        let recorded = learn(h, "decider", attest(attested)).await;
        assert!(recorded.is_ok(), "{recorded:?}");
        evaluate_by(h, "evaluator", &record_id, true).await;
    }
}

fn scoped_request(space: &str, scope: AdapterScope) -> AdapterFitRequest {
    AdapterFitRequest {
        scope,
        ..fit_request(space)
    }
}

async fn fit_scoped_adapter(
    h: &Harness,
    request: AdapterFitRequest,
) -> Result<AdapterFitted, String> {
    match learn(
        h,
        "decider",
        LearningWrite::FitAdapter {
            request: Box::new(request),
        },
    )
    .await?
    {
        LearningRecorded::Fitted(fitted) => Ok(*fitted),
        other => panic!("fit: {other:?}"),
    }
}

fn served_tenant() -> String {
    medulla_server_api::carrier::CarrierAuthority::from_verified(&verified())
        .unwrap()
        .tenant_scope()
        .to_string()
}

// spec: MEDULLA-DECISION-ENGINE-R174.7.3
#[tokio::test]
async fn scoped_fit_activation_and_rollback_move_the_pointer() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let core = embedded_graph(&h).await;
    let space = core.semantic_store.read().space().unwrap().digest.clone();
    scoped_runs(&h, &space, "rec", 40, true).await;
    let pointer = || PointerRef::ScopedAdapter {
        graph: GRAPH.to_string(),
        scope: corpus(),
    };

    // A fit for a corpus scope carries its scope, sources and lineage.
    let first = fit_scoped_adapter(&h, scoped_request(&space, corpus()))
        .await
        .unwrap();
    assert!(first.receipt.passed, "{:?}", first.receipt);
    assert_eq!(first.receipt.scope, corpus());
    assert_eq!(first.receipt.side, AdapterSide::Query);
    assert_eq!(first.receipt.revision, 1);
    assert_eq!(first.receipt.parent_digest, None);
    assert_eq!(first.receipt.sources.len(), 1);
    assert_eq!(
        first.receipt.sources[0].source,
        OutcomeSource::RetrievalOutcome
    );
    assert_eq!(first.receipt.sources[0].count, 40);
    assert_eq!(first.body.revision, 1);

    // The graph pointer's receipt cannot qualify a corpus pointer.
    let graph_fit = fit_scoped_adapter(&h, fit_request(&space)).await.unwrap();
    let wrong = pointer_state(
        &h,
        pointer(),
        activate(&graph_fit.adapter_digest, &graph_fit.receipt_digest),
    )
    .await;
    assert!(wrong.unwrap_err().starts_with("PARAMETER_INVALID"));

    // Activation moves only the scoped pointer.
    let active = pointer_state(
        &h,
        pointer(),
        activate(&first.adapter_digest, &first.receipt_digest),
    )
    .await
    .unwrap();
    assert_eq!(active.target(), Some(first.adapter_digest.as_str()));
    let store = &h.store;
    let key = scoped_adapter_pointer(GRAPH, &corpus());
    let (_, served) = read_pointer(store, &served_tenant(), &key).unwrap();
    assert_eq!(served.target(), Some(first.adapter_digest.as_str()));
    let (_, graph_pointer) = read_pointer(
        store,
        &served_tenant(),
        &scoped_adapter_pointer(GRAPH, &AdapterScope::Graph),
    )
    .unwrap();
    assert!(graph_pointer.active.is_none());

    // A refit names the active adapter as its parent.
    let refit = fit_scoped_adapter(&h, scoped_request(&space, corpus()))
        .await
        .unwrap();
    assert_eq!(
        refit.receipt.parent_digest.as_deref(),
        Some(first.adapter_digest.as_str())
    );
    assert_eq!(refit.receipt.revision, 2);
    assert_eq!(refit.body.parent_digest, refit.receipt.parent_digest);

    // Another tenant never sees the pointer.
    let (_, other) = read_pointer(store, "tenant-other", &key).unwrap();
    assert!(other.active.is_none());

    // Rollback returns to the base.
    let rolled = pointer_state(&h, pointer(), PointerMovement::Rollback)
        .await
        .unwrap();
    assert!(rolled.active.is_none());
}

// spec: MEDULLA-DECISION-ENGINE-R174.7.2.1
#[tokio::test]
async fn monitor_rolls_back_a_degrading_adapter_through_the_pointer() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let core = embedded_graph(&h).await;
    let space = core.semantic_store.read().space().unwrap().digest.clone();
    scoped_runs(&h, &space, "rec", 40, true).await;
    let pointer = || PointerRef::ScopedAdapter {
        graph: GRAPH.to_string(),
        scope: corpus(),
    };
    let fitted = fit_scoped_adapter(&h, scoped_request(&space, corpus()))
        .await
        .unwrap();
    pointer_state(
        &h,
        pointer(),
        activate(&fitted.adapter_digest, &fitted.receipt_digest),
    )
    .await
    .unwrap();
    async fn monitor(
        h: &Harness,
    ) -> medulla_decision_types::decision::statistical::retrieval_adapter::AdapterMonitored {
        let request = AdapterMonitorRequest {
            graph: GRAPH.to_string(),
            scope: corpus(),
            window: RecordWindow {
                from_ms: 0,
                to_ms: u64::MAX,
            },
        };
        let write = LearningWrite::MonitorAdapter {
            request: Box::new(request),
        };
        match learn(h, "decider", write).await.unwrap() {
            LearningRecorded::Monitored(monitored) => *monitored,
            other => panic!("monitor: {other:?}"),
        }
    }

    // Too few outcomes: keep watching, the pointer stays.
    let seen = monitor(&h).await;
    assert_ne!(seen.state, MonitorState::RolledBack);
    assert_eq!(seen.adapter_digest, fitted.adapter_digest);

    // Degrading outcomes push the win rate's upper bound under one half.
    scoped_runs(&h, &space, "deg", 120, false).await;
    let seen = monitor(&h).await;
    assert_eq!(seen.state, MonitorState::RolledBack, "{seen:?}");
    assert!(seen.win_rate_upper_q16 < 32_768);
    let (_, state) = read_pointer(
        &h.store,
        &served_tenant(),
        &scoped_adapter_pointer(GRAPH, &corpus()),
    )
    .unwrap();
    assert!(state.active.is_none(), "the pointer moved back to the base");
}

// spec: MEDULLA-DECISION-ENGINE-R174.7.4
#[tokio::test]
async fn scoped_adapter_refusals() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let core = embedded_graph(&h).await;
    let space = core.semantic_store.read().space().unwrap().digest.clone();
    scoped_runs(&h, &space, "rec", 40, true).await;
    let pointer_of = |scope: AdapterScope| PointerRef::ScopedAdapter {
        graph: GRAPH.to_string(),
        scope,
    };
    let key = scoped_adapter_pointer(GRAPH, &corpus());
    let unchanged = |h: &Harness| {
        read_pointer(&h.store, &served_tenant(), &key)
            .unwrap()
            .1
            .active
            .clone()
    };

    // A caller without admin:decision-head.
    let write = LearningWrite::FitAdapter {
        request: Box::new(scoped_request(&space, corpus())),
    };
    let denied = learn(&h, "stranger", write).await.unwrap_err();
    assert!(denied.starts_with("ACCESS_DENIED"), "{denied}");

    // A scope no judged retrieval belongs to.
    let unknown = AdapterScope::Corpus {
        corpus_iri: "NoSuchCorpus".to_string(),
    };
    let refused = fit_scoped_adapter(&h, scoped_request(&space, unknown))
        .await
        .unwrap_err();
    assert!(refused.starts_with("ADAPTER_SCOPE_UNKNOWN"), "{refused}");

    // Fewer held-out outcomes than the evaluation needs.
    let mut few = scoped_request(&space, corpus());
    few.min_eval_items = 10_000;
    let refused = fit_scoped_adapter(&h, few).await.unwrap_err();
    assert!(
        refused.starts_with("ADAPTER_INSUFFICIENT_OUTCOMES"),
        "{refused}"
    );

    // A space digest that is not the graph's.
    let other_space = space_of("2", 3);
    let refused = fit_scoped_adapter(&h, scoped_request(&other_space, corpus()))
        .await
        .unwrap_err();
    assert!(refused.starts_with("ADAPTER_SPACE_MISMATCH"), "{refused}");

    // Rank times dimension above the resident-bytes budget.
    let wide_space = space_of("1", 1400);
    let wide_rows = vec![("w".to_string(), super::learning_tests::axis(1400, 0, 1.0))];
    super::learning_tests::graph_with(&h, "kg-wide", "1", 1400, wide_rows).await;
    let mut wide = scoped_request(&wide_space, corpus());
    wide.graph = "kg-wide".to_string();
    wide.rank = 8;
    let refused = fit_scoped_adapter(&h, wide).await.unwrap_err();
    assert!(refused.starts_with("ADAPTER_MEMORY_BUDGET"), "{refused}");

    // Too many active adapters: the 65th activation is refused and moves nothing.
    let fitted = fit_scoped_adapter(&h, scoped_request(&space, corpus()))
        .await
        .unwrap();
    for i in 0..64 {
        let at = PointerMove {
            tenant: &served_tenant(),
            key: format!("adapter:filler-{i}"),
            principal: "filler",
            now_ms: 1,
        };
        let target = Qualified {
            target: format!("sha256:filler-{i}"),
            receipt_digest: "sha256:r".to_string(),
        };
        super::super::stat_pointer::activate(&h.store, &at, &target).unwrap();
    }
    let refused = pointer_state(
        &h,
        pointer_of(corpus()),
        activate(&fitted.adapter_digest, &fitted.receipt_digest),
    )
    .await
    .unwrap_err();
    assert!(refused.starts_with("ADAPTER_TOO_MANY"), "{refused}");
    assert!(
        unchanged(&h).is_none(),
        "a refused activation moves no pointer"
    );

    // A receipt that names another adapter moves no pointer either.
    let failing = pointer_state(
        &h,
        pointer_of(corpus()),
        activate(&fitted.adapter_digest, "sha256:not-the-receipt"),
    )
    .await
    .unwrap_err();
    assert!(
        failing.starts_with("EVALUATION_RECEIPT_MISMATCH"),
        "{failing}"
    );
    assert!(unchanged(&h).is_none());
}

// --- index-side adapters materialized by the generation cycle -------------

use super::super::stat_executor::ExecutionContext;
use super::super::stat_generation::Generations;
use super::super::stat_generation_cycle::{run_cycle, CycleRun, Reembedder};
use super::super::stat_log::LogReader;
use super::super::stat_vectors::graph_vectors;
use super::generation_cycle_tests::{with_ledger_class, CELL};
use super::learning_tests::graph_with;
use medulla_decision_stats::decision::adapter::AdapterKernel;
use medulla_decision_types::decision::statistical::retrieval_generation::{
    GenerationCycleRequest, GenerationCycleState, GenerationEvalItem, GenerationEvalRequest,
    IndexAdapterRef, ModelRef,
};
use medulla_mutation_types::capacity_lease::CapacityResourceClass;
use std::sync::atomic::{AtomicUsize, Ordering};

const INDEX_SHADOW: &str = "kg-index-2";
const INDEX_CELL: &str = CELL;
/// Judged runs the generation receipt replays.
const INDEX_ITEMS: usize = 5;

/// An embedder that only counts: an index-side cycle must never reach it.
struct EndpointCalls(AtomicUsize);

impl Reembedder for EndpointCalls {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        self.0.fetch_add(texts.len(), Ordering::SeqCst);
        Err("the embedding endpoint was called".to_string())
    }
}

/// The active generation (distinct vectors: `p<i>` leans on axis 1, `n<i>` on
/// axis 0, each nudged by `i`), judged runs against it, a shadow generation
/// that has its nodes and no embeddings, and an index-side adapter fitted on
/// the runs. Returns the fitted adapter and the replayed items.
async fn index_setup(h: &Harness) -> (AdapterFitted, Vec<GenerationEvalItem>) {
    let rows = (0..40).flat_map(|i| {
        let nudge = 0.0005 * i as f32;
        [
            (format!("p{i}"), vec![0.0, 1.0, nudge]),
            (format!("n{i}"), vec![1.0, 0.0, nudge]),
        ]
    });
    graph_with(h, GRAPH, "1", 3, rows.collect()).await;
    let shadow = graph_with(h, INDEX_SHADOW, "2", 3, Vec::new()).await;
    for i in 0..40 {
        for id in [format!("p{i}"), format!("n{i}")] {
            let props = serde_json::json!({"type": "Doc", "text": id});
            shadow.add_node(id, rmp_serde::to_vec_named(&props).unwrap());
        }
    }
    let space = space_of("1", 3);
    judged_runs(h, &space).await;
    let request = AdapterFitRequest {
        side: AdapterSide::Index,
        ..fit_request(&space)
    };
    let fitted = fit_scoped_adapter(h, request).await.unwrap();
    assert!(fitted.receipt.passed, "{:?}", fitted.receipt);
    assert_eq!(fitted.receipt.side, AdapterSide::Index);
    assert_eq!(fitted.body.side, AdapterSide::Index);
    // The shadow index is queried with the BASE query: the adapter lives in
    // the stored vectors.
    let items = (0..INDEX_ITEMS)
        .map(|i| {
            let query = q16(&[1.0, 0.9 + i as f64 * 0.001, 0.0]);
            GenerationEvalItem {
                record_id: format!("rec-{i:02}"),
                active_q16: query.clone(),
                shadow_q16: query,
            }
        })
        .collect();
    (fitted, items)
}

fn index_cycle(fitted: &AdapterFitted, items: Vec<GenerationEvalItem>) -> GenerationCycleRequest {
    GenerationCycleRequest {
        model_ref: ModelRef {
            id: "m".to_string(),
            revision: "2".to_string(),
            dimension: 3,
        },
        evaluation: GenerationEvalRequest {
            logical: GRAPH.to_string(),
            active_graph: GRAPH.to_string(),
            active_space: space_of("1", 3),
            shadow_graph: INDEX_SHADOW.to_string(),
            shadow_space: space_of("2", 3),
            items: BoundedVec::new(items).unwrap(),
            top_k: 5,
            min_eval_items: 5,
            max_score_psi_q16: 64 << 16,
        },
        capacity_cell: INDEX_CELL.to_string(),
        max_batches: None,
        index_adapter: Some(IndexAdapterRef {
            adapter_digest: fitted.adapter_digest.clone(),
            receipt_digest: fitted.receipt_digest.clone(),
        }),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R174.7.3.2
#[tokio::test]
async fn index_adapter_materializes_without_an_endpoint_call() {
    #[cfg(any(feature = "security", feature = "raft"))]
    let _env_guard = medulla_kernel::crypto::acquire_test_env_read_lock().await;

    // Served: the cycle is submitted through `DecisionLog` and the adapter is
    // read from the Agent Library under the carrier tenant.
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let ledger = tempfile::tempdir().unwrap();
    with_ledger_class(&h, ledger.path(), CapacityResourceClass::Cpu).await;
    let (fitted, items) = index_setup(&h).await;
    let kernel = AdapterKernel::of(&fitted.body).unwrap();
    let request = index_cycle(&fitted, items.clone());

    // An index-side adapter is never activated on an adapter pointer.
    let scoped = PointerRef::ScopedAdapter {
        graph: GRAPH.to_string(),
        scope: AdapterScope::Graph,
    };
    let refused = pointer_state(
        &h,
        scoped,
        activate(&fitted.adapter_digest, &fitted.receipt_digest),
    )
    .await
    .unwrap_err();
    assert!(refused.contains("generation cycle"), "{refused}");

    // A cycle naming an adapter another receipt qualified is refused.
    let mut wrong = request.clone();
    wrong.index_adapter.as_mut().unwrap().receipt_digest = "sha256:other".to_string();
    let refused = match learn(
        &h,
        "decider",
        LearningWrite::RunGenerationCycle {
            request: Box::new(wrong),
        },
    )
    .await
    {
        Err(error) => error,
        Ok(other) => panic!("a mismatched receipt is refused: {other:?}"),
    };
    assert!(
        refused.starts_with("EVALUATION_RECEIPT_MISMATCH"),
        "{refused}"
    );

    let done = match learn(
        &h,
        "decider",
        LearningWrite::RunGenerationCycle {
            request: Box::new(request.clone()),
        },
    )
    .await
    .unwrap()
    {
        LearningRecorded::Cycle(job) => job,
        other => panic!("cycle: {other:?}"),
    };
    assert_eq!(done.cycle.state(), GenerationCycleState::Activated);
    assert_eq!(done.reembedded_nodes, 80, "every node was materialized");
    let receipt = done
        .receipt
        .as_ref()
        .expect("an activated cycle holds its receipt");
    assert_eq!(receipt.pointer_after.as_deref(), Some(INDEX_SHADOW));

    // Each shadow vector is the adapter applied to the stored active vector.
    let (active, shadow) = {
        let guard = h.state.read().await;
        (
            guard.registry.get(GRAPH).unwrap().core.clone(),
            guard.registry.get(INDEX_SHADOW).unwrap().core.clone(),
        )
    };
    for id in ["p0", "p7", "n3", "n39"] {
        let stored = active.semantic_store.read().get_embedding(id).unwrap();
        let expected = kernel.apply(&stored).unwrap();
        let got = shadow.semantic_store.read().get_embedding(id).unwrap();
        assert_eq!(got, expected, "{id}");
    }
    // The base query ranks the cited unit first in the materialized index and
    // not in the active one.
    let base = [1.0_f32, 0.9, 0.0];
    assert!(top(&active, &base).starts_with('n'));
    assert!(top(&shadow, &base).starts_with('p'));

    // Direct: a second state runs the same cycle with an embedder that counts
    // its calls, and none reach it.
    let h2 = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let (fitted2, items2) = index_setup(&h2).await;
    let request2 = index_cycle(&fitted2, items2);
    let kernel2 = AdapterKernel::of(&fitted2.body).unwrap();
    let verified = VerifiedRequestContext::verified_for_test_in_tenant("decider", TENANT);
    let reader = LogReader::of(&verified);
    let active = graph_vectors(&h2.state, "decider", GRAPH).await.unwrap();
    let shadow = graph_vectors(&h2.state, "decider", INDEX_SHADOW)
        .await
        .unwrap();
    let gens = Generations {
        active: &active,
        shadow: &shadow,
    };
    let ctx = ExecutionContext {
        store: &h2.store,
        tenant_id: TENANT,
        now_ms: 1_000,
        server_secret: b"cycle-test-secret",
        matched: &[],
        designation: None,
        reader: None,
    };
    let endpoint = EndpointCalls(AtomicUsize::new(0));
    let run = CycleRun {
        ctx: &ctx,
        reader: &reader,
        gens: &gens,
        embedder: &endpoint,
        index: Some(&kernel2),
        tenant: TENANT,
        lease_id: None,
        batch_nodes: 30,
    };
    let LearningRecorded::Cycle(job) = run_cycle(&run, &request2).unwrap() else {
        panic!("a cycle answers with its job");
    };
    assert_eq!(job.cycle.state(), GenerationCycleState::Activated);
    assert_eq!(job.batches, 3, "80 nodes in batches of 30");
    assert_eq!(
        endpoint.0.load(Ordering::SeqCst),
        0,
        "an index-side adapter materializes with zero embedding calls"
    );

    // The cycle and its adapter must agree: a cycle that names an adapter but
    // runs without its kernel (or the reverse) is refused.
    let mut plain = request2.clone();
    plain.index_adapter = None;
    plain.model_ref.revision = "3".to_string();
    let error = run_cycle(&run, &plain).unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
}

// --- evidence: fit receipt, rollback receipt and the Discover verdict field ---

use medulla_decision_types::decision::statistical::retrieval_adapter::AdapterMonitored;
use medulla_persistence_records::decision_jobs::decode_artifact;
use medulla_protocol_types::protocol::{Method, ResultPayload};

/// Serve `Discover` through the graph operations and return the verdict's adapter digest.
async fn verdict_adapter(h: &Harness, core: &Arc<medulla_graph_core::graph::GraphCore>) -> String {
    let authority = {
        let state = h.state.read().await;
        medulla_server_api::carrier::GraphReadAuthority::from_verified(
            &verified(),
            &state.isolation,
        )
        .unwrap()
    };
    let response = medulla_h_graph::graph_ops::try_handle(
        &h.state,
        7,
        None,
        GRAPH,
        &authority,
        core.clone(),
        Method::Discover {
            keywords: vec!["p1".into()],
            query_embedding: vec![1.0, 0.9, 0.0],
            k: 5,
            quality_policy_ref: None,
            hops: 0,
            query_text: None,
            mode: Default::default(),
            as_of: None,
        },
    )
    .await;
    assert!(response.error.is_none(), "{:?}", response.error);
    match response.result.expect("a Discover result") {
        ResultPayload::Json(serde_json::Value::Object(envelope)) => envelope["verdict"]
            ["adapter_digest"]
            .as_str()
            .expect("the verdict names an adapter digest")
            .to_string(),
        other => panic!("expected the Discover envelope, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R174.7.5
#[tokio::test]
async fn fit_and_rollback_receipts_and_verdict_field() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    assert!(medulla_server_ops::api::admin::install_decide_seam(
        &h.state.read().await.seams,
        Arc::new(super::super::DecideSeam),
    ));
    let core = embedded_graph(&h).await;
    let space = core.semantic_store.read().space().unwrap().digest.clone();
    scoped_runs(&h, &space, "rec", 40, true).await;

    // The fit receipt of a corpus scope holds scope, side and each source with its count.
    let corpus_fit = fit_scoped_adapter(&h, scoped_request(&space, corpus()))
        .await
        .unwrap();
    assert_eq!(corpus_fit.receipt.scope, corpus());
    assert_eq!(corpus_fit.receipt.side, AdapterSide::Query);
    assert_eq!(corpus_fit.receipt.sources.len(), 1);
    assert_eq!(
        corpus_fit.receipt.sources[0].source,
        OutcomeSource::RetrievalOutcome
    );
    assert_eq!(corpus_fit.receipt.sources[0].count, 40);

    // The graph-scope fit holds the evidence of the keep rule.
    let fitted = fit_scoped_adapter(&h, scoped_request(&space, AdapterScope::Graph))
        .await
        .unwrap();
    let receipt = &fitted.receipt;
    assert!(receipt.passed, "{receipt:?}");
    assert_eq!(receipt.scope, AdapterScope::Graph);
    assert_eq!(receipt.side, AdapterSide::Query);
    assert_eq!(receipt.sources[0].count, 40);
    assert!(!receipt.training_digest.is_empty());
    assert!(
        receipt.adapted_mrr_q16 > receipt.base_mrr_q16,
        "{receipt:?}"
    );
    assert!(receipt.win_rate_lower_q16 > 32_768, "{receipt:?}");
    assert!(receipt.win_rate_upper_q16 >= receipt.win_rate_lower_q16);
    assert_eq!(receipt.parent_digest, None);
    assert_eq!(receipt.revision, 1);
    assert_eq!(receipt.adapter_digest, fitted.adapter_digest);

    // With no adapter active the served verdict names the base space.
    assert_eq!(verdict_adapter(&h, &core).await, "base");

    // Activation: the verdict names the applied adapter digest.
    let pointer = || PointerRef::ScopedAdapter {
        graph: GRAPH.to_string(),
        scope: AdapterScope::Graph,
    };
    pointer_state(
        &h,
        pointer(),
        activate(&fitted.adapter_digest, &fitted.receipt_digest),
    )
    .await
    .unwrap();
    assert_eq!(verdict_adapter(&h, &core).await, fitted.adapter_digest);

    // Degrading outcomes roll the adapter back and leave a rollback receipt.
    scoped_runs(&h, &space, "deg", 120, false).await;
    let window = RecordWindow {
        from_ms: 0,
        to_ms: u64::MAX,
    };
    let write = LearningWrite::MonitorAdapter {
        request: Box::new(AdapterMonitorRequest {
            graph: GRAPH.to_string(),
            scope: AdapterScope::Graph,
            window,
        }),
    };
    let monitored = match learn(&h, "decider", write).await.unwrap() {
        LearningRecorded::Monitored(monitored) => *monitored,
        other => panic!("monitor: {other:?}"),
    };
    assert_eq!(monitored.state, MonitorState::RolledBack, "{monitored:?}");
    let prefix = format!("adapter-monitor:{}:", fitted.adapter_digest);
    let stored = h
        .store
        .decision_artifacts_with_prefix(&served_tenant(), &prefix, 8)
        .unwrap();
    assert_eq!(stored.len(), 1, "one rollback receipt");
    let rollback: AdapterMonitored = decode_artifact(&stored[0].1, "rollback receipt").unwrap();
    assert_eq!(rollback, monitored);
    assert_eq!(rollback.window, window);
    assert!(rollback.win_rate_upper_q16 < 32_768);
    assert!(rollback.losses > rollback.wins);
    assert!(rollback.outcomes >= rollback.min_outcomes);

    // After the rollback the verdict names the base space again.
    assert_eq!(verdict_adapter(&h, &core).await, "base");
}
