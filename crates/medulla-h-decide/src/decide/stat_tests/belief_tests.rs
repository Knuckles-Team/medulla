//! Graph belief slices read through `BELIEF AS OF` (MEDULLA-DECISION-ENGINE-R088.1).
//!
//! A `Decide` over a graph candidate source reads each belief slice by
//! re-running the select-only candidate plan with a trailing `BELIEF AS OF t`
//! over the caller's RLS-filtered view. The slice's stored row digest equals
//! the digest of a direct query, and a candidate row absent at `t` makes the
//! slice an `UnknownFact` (no belief), never a different option set.

use super::scorer_tests::{evaluated, scorer_fit};
use super::*;

const GRAPH: &str = "kg-belief-slices";

fn tool_plan() -> medulla_types::wire::Plan {
    medulla_types::wire::Plan::new(vec![medulla_types::wire::Op::Scan {
        label: "Tool".to_string(),
    }])
}

/// The ids a direct `Scan Tool` then `BELIEF AS OF ts` query returns for the
/// caller, sorted.
async fn direct_ids(h: &Harness, ts: f64) -> Vec<String> {
    let view = super::super::stat_support::filtered_view(&h.state, "decider", GRAPH)
        .await
        .unwrap();
    let mut plan = tool_plan();
    plan.ops.push(medulla_types::wire::Op::BeliefAsOf { ts });
    let semantic = medulla_semantic_index::semantic::SemanticStore::new();
    let rows =
        medulla_exec::exec::execute(&plan, &medulla_exec::exec::PlanCtx::new(&view, &semantic))
            .unwrap();
    let mut ids = rows.ids();
    ids.sort_unstable();
    ids.dedup();
    ids
}

// spec: MEDULLA-DECISION-ENGINE-R088.1
#[tokio::test]
async fn graph_belief_slices_match_direct_belief_as_of_queries() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    {
        let mut guard = h.state.write().await;
        guard
            .registry
            .create_graph(
                GRAPH,
                medulla_protocol_types::protocol::GraphType::Team,
                None,
            )
            .unwrap();
        let core = guard.registry.get(GRAPH).unwrap().core.clone();
        // n-c enters the graph at transaction time 1_500: absent at 1_000.
        for (id, score, weight, tx_from) in [
            ("n-a", 0.9, 0.2, 100_u64),
            ("n-b", 0.1, 0.8, 100),
            ("n-c", 0.5, 0.5, 1_500),
        ] {
            let props = serde_json::json!({
                "type": "Tool", "score": score, "weight": weight, "tx_from": tx_from
            });
            core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
        }
    }
    let body = FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![number_feature("score"), number_feature("weight")]).unwrap(),
    };
    let schema_pin = h
        .publish_test_component(
            "schema-belief-graph",
            AgentComponentKind::FeatureSchema,
            "graph belief features",
            Some(&body),
            None,
        )
        .unwrap();
    let schema_digest = encode_body(&body).unwrap().content_digest;
    let graph_request = |head, policy, belief: Vec<u64>| {
        let mut request = request(&schema_pin, head, policy, QuestionSafety::Ordinary);
        request.candidates = CandidateSource::Graph {
            graph: GRAPH.to_string(),
            plan: Box::new(tool_plan()),
        };
        request.belief_as_of = BoundedVec::new(belief).unwrap();
        request
    };

    // The headless decision records the exact matrix the head is fitted on.
    let headless = decide(
        &h,
        graph_request(None, DecisionPolicyRef::Default, Vec::new()),
    )
    .await
    .unwrap();
    let FeatureMatrixRef::Inline {
        candidate_ids,
        values,
        ..
    } = &headless.records.as_slice()[0].inputs.feature_matrix
    else {
        panic!("inline matrix")
    };
    let fixture = RouteFixture {
        schema_pin: schema_pin.clone(),
        schema_digest: schema_digest.clone(),
        ids: candidate_ids.iter().cloned().collect(),
        values: values.as_slice().to_vec(),
    };
    assert_eq!(fixture.ids, ["n-a", "n-b", "n-c"]);
    let mut data = dataset(&schema_digest, &fixture.ids, &fixture.values, 400);
    data.feature_names = BoundedVec::new(vec!["score".to_string(), "weight".to_string()]).unwrap();

    let job: DecisionJobRecord = decode(
        super::super::jobs::handle_decision_fit(
            &h.state,
            2,
            &verified(),
            scorer_fit(&fixture, &data),
        )
        .await,
    )
    .unwrap();
    let DecisionJobOutput::Fit {
        draft,
        draft_sha256,
        draft_length,
        ..
    } = succeeded(&job).clone()
    else {
        panic!("fit output")
    };
    let passed = evaluated(&h, 3, (draft_sha256, draft_length), data).await;
    assert!(passed.passed, "gates: {:?}", passed.failed_gates.as_slice());
    let head_pin = h
        .publish_test_component(
            "head-belief-graph",
            AgentComponentKind::DecisionHead,
            "graph scorer",
            Some(draft.as_ref()),
            Some(passed.receipt_digest.clone()),
        )
        .unwrap();

    let policy_pin = h.publish_policy("policy-belief-explore", &ordinary_exploration_policy());
    let batch = decide(
        &h,
        graph_request(
            Some(head_pin),
            DecisionPolicyRef::Pinned {
                component: policy_pin,
            },
            vec![1_000, 2_000],
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let slices = record.inputs.belief_slices.as_slice();
    assert_eq!(slices.len(), 2);

    for slice in slices {
        let ids = direct_ids(&h, slice.as_of_ms as f64).await;
        let expected = medulla_query_types::solve_scalar::digest_text(
            "eg/decide-belief-slice-rows/v1",
            &(slice.as_of_ms, &ids),
        );
        assert_eq!(
            slice.rows_digest.as_deref(),
            Some(expected.as_str()),
            "slice at {} equals the direct BELIEF AS OF query",
            slice.as_of_ms
        );
    }
    // n-c is absent at 1_000: UnknownFact, so no matrix and no belief; at
    // 2_000 every candidate is present and the head reads the same three.
    assert!(slices[0].values.is_empty());
    assert_eq!(direct_ids(&h, 1_000.0).await, ["n-a", "n-b"]);
    assert!(record.belief.as_slice()[0].probabilities.is_none());
    assert_eq!(slices[1].values.len(), 6, "three options, two features");
    let present = record.belief.as_slice()[1]
        .probabilities
        .as_ref()
        .expect("the head reads the complete slice");
    assert_eq!(present.len(), 3, "a slice never adds or drops an option");
}
