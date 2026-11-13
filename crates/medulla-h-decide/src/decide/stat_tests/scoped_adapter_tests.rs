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
