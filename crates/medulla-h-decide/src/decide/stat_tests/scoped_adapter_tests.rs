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
    let monitor = |h: &Harness| {
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
        async move {
            match learn(h, "decider", write).await.unwrap() {
                LearningRecorded::Monitored(monitored) => *monitored,
                other => panic!("monitor: {other:?}"),
            }
        }
    };

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
