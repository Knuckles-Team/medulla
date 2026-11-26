//! Entity-resolution proposals through the served `Decide` handler
//! (MEDULLA-DECISION-ENGINE-R706.3): the `resolve_entity` family grows its
//! candidate pairs from the embedded neighbours of the declared pair sides,
//! only ever the asking tenant's; the reviews of earlier proposals come back
//! through `DecisionLog` `Evaluate` and `Aggregate` as per-tier accept counts,
//! and the accept-rate drift is recorded beside them.

use super::path_preference_tests::{pins, run, seed};
use super::resolve_entity_tests::{
    entity, pair_option, pair_request, publish_acting_head, publish_schema, two_pairs,
};
use super::*;
use medulla_decision_stats::decision::entity_resolution::proposal_option_id;
use medulla_decision_types::decision::entity_resolution::{ResolutionTier, TierEvaluation};
use medulla_decision_types::decision::statistical::declared::{DeclaredOption, DeclaredText};
use medulla_decision_types::decision::statistical::log::OutcomeAggregateRequest;
use medulla_decision_types::decision::statistical::{
    CandidateSource, QuestionKind, StatisticalOutcome, TypedParam,
};
use medulla_decision_types::decision::{AbstainReason, PremiseClass, PremiseProvenance};

const OTHER_TENANT: &str = "tenant-other";
const GRAPH: &str = "entities";

/// One entity node of `owner`, embedded by `vector`.
fn node(
    core: &medulla_graph_core::graph::GraphCore,
    name: &str,
    owner: &str,
    vector: Vec<f32>,
) -> String {
    let id = entity(name);
    // `tenant_id` is caller-writable and always claims the asking tenant;
    // only the row owner decides visibility.
    let props = serde_json::json!({
        "type": "Entity",
        "tenant_id": TENANT,
        "_owner": owner,
        "_visibility": "private",
    });
    core.add_node(id.clone(), rmp_serde::to_vec_named(&props).unwrap());
    core.semantic_store
        .write()
        .add_embedding(id.clone(), vector)
        .unwrap();
    id
}

/// The entity graph: the declared pair `a-left`/`a-right`, two of this
/// tenant's nodes (`near-1`, `near-2`) and one of another tenant that is the
/// nearest node to `a-left` of all and forges the asking tenant's `tenant_id`.
async fn entity_graph(h: &Harness) {
    let core = super::learning_tests::graph_with(h, GRAPH, "er-m", 3, Vec::new()).await;
    node(&core, "a-left", "decider", vec![1.0, 0.0, 0.0]);
    node(&core, "a-right", "decider", vec![0.0, -1.0, 0.0]);
    node(&core, "near-1", "decider", vec![0.3, 0.954, 0.0]);
    node(&core, "near-2", "decider", vec![0.2, 0.0, 0.98]);
    node(&core, "foreign", "other-agent", vec![1.0, 0.01, 0.0]);
}

fn with_graph(mut request: DecideRequest) -> DecideRequest {
    let mut params = request.params.as_slice().to_vec();
    params.push(TypedParam {
        name: "er.graph".to_string(),
        value: TypedValue::Text(GRAPH.to_string()),
    });
    request.params = BoundedVec::new(params).unwrap();
    request
}

fn facts(record: &StatisticalDecisionRecord, prefix: &str) -> Vec<String> {
    let mut found: Vec<String> = record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with(prefix))
        .map(|p| p.fact.clone())
        .collect();
    found.sort();
    found
}

/// Verifies MEDULLA-DECISION-ENGINE-R706.3: the family adds the asking
/// tenant's neighbours as pair proposals and never another tenant's node, the
/// answer stays the exact pair behind the conformal guarantee, reviews of
/// earlier proposals join through `DecisionLog` and come back per tier, and a
/// caller cannot supply the engine's review params.
// spec: MEDULLA-DECISION-ENGINE-R706.3
#[tokio::test]
async fn resolve_entity_uses_tenant_scoped_candidates() {
    // A provisioned, non-system reader: row-level security hides the other owner's node.
    let h = Harness::with_isolation(reader_isolation("decider", &[GRAPH])).await;
    entity_graph(&h).await;
    let (schema, digest) = publish_schema(&h);
    let head = publish_acting_head(&h, &digest, "head-er-proposals", 0.4);
    let ask = || {
        with_graph(pair_request(
            &schema,
            Some(&head),
            DecisionPolicyRef::Default,
            two_pairs(),
        ))
    };

    // Proposals: every unseen pair of a declared side and a neighbour of the
    // asking tenant; the nearest node overall belongs to another tenant.
    let batch = decide(&h, ask()).await.unwrap();
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Acted {
        option_id,
        prediction_set,
        ..
    } = &record.outcome
    else {
        panic!("the exact pair still acts: {:?}", record.outcome)
    };
    assert_eq!(option_id, "pair-a");
    assert_eq!(prediction_set.as_slice(), ["pair-a"]);
    let pair = |l: &str, r: &str| format!("entity_pair:{}={}", entity(l), entity(r));
    let mut expected = vec![
        pair("a-left", "a-right"),
        pair("b-left", "b-right"),
        pair("a-left", "near-1"),
        pair("a-left", "near-2"),
        pair("a-right", "near-1"),
        pair("a-right", "near-2"),
    ];
    expected.sort();
    assert_eq!(facts(record, "entity_pair:"), expected);
    assert!(
        !record.premises.iter().any(|p| p.fact.contains("foreign")),
        "another tenant's node never appears: {:?}",
        record.premises
    );

    // Without a graph the question weighs only its declared pairs.
    let plain = decide(
        &h,
        pair_request(
            &schema,
            Some(&head),
            DecisionPolicyRef::Default,
            two_pairs(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        facts(&plain.records.as_slice()[0], "entity_pair:"),
        vec![pair("a-left", "a-right"), pair("b-left", "b-right")]
    );

    // The engine's review params are never the caller's.
    let mut forged = ask();
    let mut params = forged.params.as_slice().to_vec();
    params.push(TypedParam {
        name: "er.review.accepted:embedding".to_string(),
        value: TypedValue::Int(1_000),
    });
    forged.params = BoundedVec::new(params).unwrap();
    let refused = decide(&h, forged).await.unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");

    // No reviews yet: no tier evaluation is recorded.
    assert!(facts(record, "er:tier_evaluation:").is_empty());

    // Reviews join through DecisionLog Evaluate: ten of the embedding tier's
    // proposals (seven accepted) and ten of the exact tier's (eight accepted).
    let reviewers = pins(&h, TENANT);
    let embedding = proposal_option_id(
        ResolutionTier::Embedding,
        &entity("a-left"),
        &entity("near-1"),
    );
    let exact = proposal_option_id(ResolutionTier::Exact, &entity("a-left"), &entity("a-right"));
    seed(&h, TENANT, &reviewers, &embedding, &run(7, 3)).await;
    seed(&h, TENANT, &reviewers, &exact, &run(8, 2)).await;

    let reviewed = decide(&h, ask()).await.unwrap();
    let reviewed = &reviewed.records.as_slice()[0];
    assert_eq!(
        facts(reviewed, "er:tier_evaluation:"),
        vec![
            "er:tier_evaluation:embedding=7/3/700000".to_string(),
            "er:tier_evaluation:exact=8/2/800000".to_string(),
        ]
    );
    // Too few reviews for a drift comparison: no drift premise.
    assert!(facts(reviewed, "er:drift:").is_empty());

    // The same counts come back from DecisionLog Aggregate per option.
    let aggregate: medulla_decision_types::decision::statistical::log::OutcomeAggregate = decode(
        super::log_tests::log_op(
            &h,
            "decider",
            DecisionLogOp::Aggregate {
                request: OutcomeAggregateRequest {
                    tenant_id: TENANT.to_string(),
                    question_id: None,
                    window: window(),
                    attribution: None,
                    corrections: None,
                    transfer: None,
                },
            },
        )
        .await,
    )
    .unwrap();
    let tier_of = |option: &str, tier: ResolutionTier| {
        let row = aggregate
            .rows
            .iter()
            .find(|row| row.option_id == option)
            .unwrap_or_else(|| panic!("no aggregate row for {option}"));
        TierEvaluation::new(
            tier,
            u32::try_from(row.successes).unwrap(),
            u32::try_from(row.trials - row.successes).unwrap(),
        )
    };
    assert_eq!(
        tier_of(&embedding, ResolutionTier::Embedding),
        TierEvaluation::new(ResolutionTier::Embedding, 7, 3)
    );
    assert_eq!(
        tier_of(&exact, ResolutionTier::Exact),
        TierEvaluation::new(ResolutionTier::Exact, 8, 2)
    );

    // Another tenant's reviews never count here.
    let theirs = pins(&h, OTHER_TENANT);
    seed(&h, OTHER_TENANT, &theirs, &embedding, &run(0, 30)).await;
    let again = decide(&h, ask()).await.unwrap();
    assert_eq!(
        facts(&again.records.as_slice()[0], "er:tier_evaluation:"),
        facts(reviewed, "er:tier_evaluation:")
    );
}

/// Verifies MEDULLA-DECISION-ENGINE-R706.3: the drift check of the accept rate
/// runs in the family, over the reviews of every tier, and its state is
/// recorded beside the proposals.
// spec: MEDULLA-DECISION-ENGINE-R706.3
#[tokio::test]
async fn review_drift_is_recorded_with_the_proposals() {
    // A provisioned, non-system reader: row-level security hides the other owner's node.
    let h = Harness::with_isolation(reader_isolation("decider", &[GRAPH])).await;
    entity_graph(&h).await;
    let (schema, digest) = publish_schema(&h);
    let head = publish_acting_head(&h, &digest, "head-er-drift", 0.4);
    let reviewers = pins(&h, TENANT);
    let option = proposal_option_id(
        ResolutionTier::Embedding,
        &entity("a-left"),
        &entity("near-1"),
    );
    // The window before: half accepted; the last window: all accepted.
    let window = usize::try_from(medulla_server_api::knob_config::knobs().er_drift_window).unwrap();
    let mut outcomes = run(window / 2, window / 2);
    outcomes.extend(run(window, 0));
    seed(&h, TENANT, &reviewers, &option, &outcomes).await;

    let batch = decide(
        &h,
        with_graph(pair_request(
            &schema,
            Some(&head),
            DecisionPolicyRef::Default,
            vec![pair_option(
                "pair-a",
                &entity("a-left"),
                &entity("a-right"),
                3 << 32,
            )],
        )),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(
        facts(record, "er:drift:"),
        vec!["er:drift:drifted=500000".to_string()]
    );
    let accepted = window / 2 + window;
    assert_eq!(
        facts(record, "er:tier_evaluation:"),
        vec![format!(
            "er:tier_evaluation:embedding={accepted}/{}/{}",
            window / 2,
            accepted as u64 * 1_000_000 / (window as u64 * 2)
        )]
    );
}

fn abstain_code(record: &StatisticalDecisionRecord) -> String {
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("expected an abstention: {:?}", record.outcome)
    };
    match reasons.as_slice() {
        [AbstainReason::Family { family, code, .. }] => {
            assert_eq!(*family, QuestionKind::ResolveEntity);
            code.as_str().to_string()
        }
        other => panic!("expected one family reason: {other:?}"),
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R706.4: a pair joining a node the tenant
/// holds with one it does not refuses `ER_CROSS_TENANT` (another tenant's node
/// and an absent node answer alike), a top proposal under the head's act
/// threshold abstains `ER_BELOW_THRESHOLD`, a probabilistic proposal abstains
/// `ER_DRIFT_HOLD` while the accept rate has drifted, and an acted
/// probabilistic result is a `Claim` premise `er:hypothesis:<proposal_id>`.
// spec: MEDULLA-DECISION-ENGINE-R706.4
#[tokio::test]
async fn er_refusals_and_hypothesis_marking() {
    // A provisioned, non-system reader: row-level security hides the other owner's node.
    let h = Harness::with_isolation(reader_isolation("decider", &[GRAPH])).await;
    entity_graph(&h).await;
    let (schema, digest) = publish_schema(&h);
    let head = publish_acting_head(&h, &digest, "head-er-refusals", 0.4);
    let ask = |options: Vec<DeclaredOption>| {
        with_graph(pair_request(
            &schema,
            Some(&head),
            DecisionPolicyRef::Default,
            options,
        ))
    };

    // Cross tenant: the same refusal for another tenant's node and an absent one.
    let foreign = decide(
        &h,
        ask(vec![pair_option(
            "pair-x",
            &entity("a-left"),
            &entity("foreign"),
            3 << 32,
        )]),
    )
    .await
    .unwrap_err();
    let absent = decide(
        &h,
        ask(vec![pair_option(
            "pair-x",
            &entity("a-left"),
            &entity("ghost"),
            3 << 32,
        )]),
    )
    .await
    .unwrap_err();
    assert!(foreign.starts_with("ER_CROSS_TENANT"), "{foreign}");
    assert_eq!(
        foreign, absent,
        "an absent node reads like another tenant's"
    );

    // Below the act threshold: two equal proposals leave the top under 0.6.
    let embedding = proposal_option_id(
        ResolutionTier::Embedding,
        &entity("a-left"),
        &entity("a-right"),
    );
    let below = decide(
        &h,
        ask(vec![
            pair_option(&embedding, &entity("a-left"), &entity("a-right"), 0),
            pair_option("pair-b", &entity("b-left"), &entity("b-right"), 0),
        ]),
    )
    .await
    .unwrap();
    assert_eq!(
        abstain_code(&below.records.as_slice()[0]),
        "ER_BELOW_THRESHOLD"
    );

    // A probabilistic proposal that acts is a hypothesis: a Claim premise.
    let probabilistic = proposal_option_id(
        ResolutionTier::Probabilistic,
        &entity("a-left"),
        &entity("a-right"),
    );
    let ask_probabilistic = || {
        ask(vec![
            pair_option(
                &probabilistic,
                &entity("a-left"),
                &entity("a-right"),
                3 << 32,
            ),
            pair_option("pair-b", &entity("b-left"), &entity("b-right"), 0),
        ])
    };
    let acted = decide(&h, ask_probabilistic()).await.unwrap();
    let acted = &acted.records.as_slice()[0];
    assert!(
        matches!(&acted.outcome, StatisticalOutcome::Acted { option_id, .. } if *option_id == probabilistic),
        "{:?}",
        acted.outcome
    );
    let hypothesis = format!("er:hypothesis:{probabilistic}");
    let marked: Vec<_> = acted
        .premises
        .iter()
        .filter(|p| p.fact == hypothesis)
        .collect();
    assert_eq!(marked.len(), 1);
    assert_eq!(marked[0].class, PremiseClass::Claim);

    // While the accept rate has drifted the probabilistic proposal is held.
    let reviewers = pins(&h, TENANT);
    let window = usize::try_from(medulla_server_api::knob_config::knobs().er_drift_window).unwrap();
    let mut outcomes = run(window / 2, window / 2);
    outcomes.extend(run(window, 0));
    seed(&h, TENANT, &reviewers, &embedding, &outcomes).await;
    let held = decide(&h, ask_probabilistic()).await.unwrap();
    assert_eq!(abstain_code(&held.records.as_slice()[0]), "ER_DRIFT_HOLD");
}

/// Verifies MEDULLA-DECISION-ENGINE-R706.5: every proposal records its model,
/// its content hash and its tier as `Claim` premises with request provenance
/// `candidates`; the exact pair adds none of them, and the caller cannot
/// supply the recorded texts.
// spec: MEDULLA-DECISION-ENGINE-R706.5
#[tokio::test]
async fn proposal_records_model_hash_and_tier() {
    // A provisioned, non-system reader: row-level security hides the other owner's node.
    let h = Harness::with_isolation(reader_isolation("decider", &[GRAPH])).await;
    entity_graph(&h).await;
    let (schema, digest) = publish_schema(&h);
    let head = publish_acting_head(&h, &digest, "head-er-evidence", 0.4);
    let request = with_graph(pair_request(
        &schema,
        Some(&head),
        DecisionPolicyRef::Default,
        two_pairs(),
    ));
    let batch = decide(&h, request.clone()).await.unwrap();
    let record = &batch.records.as_slice()[0];

    let near = proposal_option_id(
        ResolutionTier::Embedding,
        &entity("a-left"),
        &entity("near-1"),
    );
    for name in ["tier", "model", "content_hash"] {
        let found: Vec<_> = record
            .premises
            .iter()
            .filter(|p| p.fact.starts_with(&format!("er:{name}:{near}=")))
            .collect();
        assert_eq!(found.len(), 1, "one {name} premise for {near}");
        assert_eq!(found[0].class, PremiseClass::Claim);
        assert_eq!(
            found[0].provenance,
            PremiseProvenance::Request {
                field: "candidates".to_string()
            }
        );
    }
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact == format!("er:tier:{near}=embedding")));
    let hash = record
        .premises
        .iter()
        .find_map(|p| p.fact.strip_prefix(&format!("er:content_hash:{near}=")))
        .unwrap();
    assert!(medulla_decision_types::decision::entity_resolution::is_content_hash(hash));
    // One proposal per generated option; the declared exact pairs add none.
    let proposals = record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("er:tier:"))
        .count();
    assert_eq!(proposals, 4);
    assert!(!record
        .premises
        .iter()
        .any(|p| p.fact.starts_with("er:model:pair-")));

    // The caller never supplies what the engine records.
    let mut forged = request;
    let CandidateSource::Declared { options } = &forged.candidates else {
        unreachable!()
    };
    let mut options = options.as_slice().to_vec();
    let mut texts = options[0].texts.as_slice().to_vec();
    texts.push(DeclaredText {
        key: "er.model".to_string(),
        text: "forged".to_string(),
    });
    texts.sort_by(|a, b| a.key.cmp(&b.key));
    options[0].texts = BoundedVec::new(texts).unwrap();
    forged.candidates = CandidateSource::Declared {
        options: BoundedVec::new(options).unwrap(),
    };
    let refused = decide(&h, forged).await.unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
}

// spec: MEDULLA-DECISION-ENGINE-R706.1
// spec: MEDULLA-DECISION-ENGINE-R706.6
#[tokio::test]
async fn cross_tenant_candidates_never_returned() {
    // Nodes of two tenants are embedded; the other tenant's node is the
    // nearest to a declared side and forges the asking tenant's `tenant_id`.
    let h = Harness::with_isolation(reader_isolation("decider", &[GRAPH])).await;
    entity_graph(&h).await;
    let (schema, digest) = publish_schema(&h);
    let head = publish_acting_head(&h, &digest, "head-er-cross-tenant", 0.4);
    let batch = decide(
        &h,
        with_graph(pair_request(
            &schema,
            Some(&head),
            DecisionPolicyRef::Default,
            two_pairs(),
        )),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let foreign = entity("foreign");

    // No premise, option or prediction names the other tenant's node.
    assert!(
        !record
            .premises
            .iter()
            .any(|premise| premise.fact.contains(&foreign) || premise.subject.contains(&foreign)),
        "another tenant's node never appears: {:?}",
        record.premises
    );
    let StatisticalOutcome::Acted {
        option_id,
        prediction_set,
        ..
    } = &record.outcome
    else {
        panic!("the exact pair still acts: {:?}", record.outcome)
    };
    assert!(!option_id.contains(&foreign));
    assert!(prediction_set.iter().all(|option| !option.contains(&foreign)));

    // The asking tenant's own neighbours are proposed in its place.
    let proposed = facts(record, "entity_pair:");
    assert!(proposed.contains(&format!(
        "entity_pair:{}={}",
        entity("a-left"),
        entity("near-1")
    )));
    assert!(proposed.iter().all(|fact| !fact.contains(&foreign)));
}
