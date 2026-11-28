//! Reward decomposition, served through `Decide` (MEDULLA-DECISION-ENGINE-R751.751,
//! R751.755): a trajectory reward splits over its steps and hierarchy, the
//! rewards sum exactly, and the record names the trajectory and its depth.

use super::kernel_family_support::{
    decide_as_other_tenant, facts, kernel_request, persisted_files,
};
use super::*;
use medulla_decision_types::decision::record::ResolutionKind;
use {
    medulla_decision_statistical_types::decision::reward::HtnNode,
    medulla_decision_statistical_types::decision::reward::RewardDecompositionRequest,
    medulla_decision_statistical_types::decision::reward::StepSignal,
    medulla_decision_statistical_types::decision::reward::TrajectoryReward,
};

const ONE: i64 = 1 << 32;

fn q(value: i64) -> QuantisedValue {
    QuantisedValue {
        scale: medulla_decision_statistical_types::decision::QuantScaleTag::Q32,
        value,
    }
}

fn trajectory(
    id: &str,
    reward: i64,
    steps: usize,
    hierarchy: Vec<HtnNode>,
    with_advantage: bool,
) -> RewardDecompositionRequest {
    RewardDecompositionRequest::new(
        id.to_string(),
        q(reward),
        (0..steps)
            .map(|i| StepSignal {
                // Progress stalls on every tenth step; every fifth is a cache hit.
                value_estimate: q((ONE / 2) * (i as i64 - i as i64 / 10) / steps as i64 + 1),
                cache_hit: i % 5 == 0,
            })
            .collect(),
        hierarchy,
        with_advantage,
        None,
    )
    .unwrap()
}

fn ask(schema: &ComponentDependency, body: RewardDecompositionRequest) -> DecideRequest {
    let id = body.trajectory_id.clone();
    kernel_request(
        schema,
        QuestionKind::RewardDecomposition,
        "reward_decomposition",
        TypedValue::RewardDecomposition(Box::new(body)),
        &[id.as_str()],
    )
}

fn reward_of(record: &StatisticalDecisionRecord) -> &TrajectoryReward {
    record
        .answer
        .as_ref()
        .and_then(|a| a.reward_decomposition())
        .expect("the family answers a trajectory reward")
}

/// Verifies MEDULLA-DECISION-ENGINE-R751.751 and R751.755: a 1024-step
/// trajectory is split in step order, the rewards sum to the total, the
/// hierarchy rolls up, and a second call returns the same answer.
// spec: MEDULLA-DECISION-ENGINE-R751.751
// spec: MEDULLA-DECISION-ENGINE-R751.755
#[tokio::test]
async fn served_decide_decomposes_a_trajectory_reward() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let hierarchy = vec![
        HtnNode {
            parent: None,
            first_step: 0,
            last_step: 1023,
        },
        HtnNode {
            parent: Some(0),
            first_step: 0,
            last_step: 511,
        },
        HtnNode {
            parent: Some(0),
            first_step: 512,
            last_step: 1023,
        },
    ];
    let first = decide(
        &h,
        ask(
            &schema,
            trajectory("traj-1", ONE / 2, 1024, hierarchy.clone(), true),
        ),
    )
    .await
    .unwrap();
    let record = &first.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::RewardDecomposition);
    let reward = reward_of(record);
    assert_eq!(reward.trajectory_id, "traj-1");
    assert_eq!(reward.step_rewards.len(), 1024);
    // The order of rewards is the order of steps.
    assert!(reward
        .step_rewards
        .iter()
        .enumerate()
        .all(|(i, r)| r.step as usize == i && r.advantage.is_some()));
    let total: i64 = reward.step_rewards.iter().map(|r| r.reward.value).sum();
    assert_eq!(
        total,
        ONE / 2,
        "the rewards sum exactly to the trajectory reward"
    );
    let nodes: Vec<i64> = reward.node_rewards.iter().map(|r| r.value).collect();
    assert_eq!(nodes[0], ONE / 2);
    assert_eq!(nodes[1] + nodes[2], ONE / 2);
    assert!(reward.keep_for_training, "1/2 lies strictly inside (0, 1)");
    assert!(matches!(
        record.outcome,
        StatisticalOutcome::Advisory { .. }
    ));
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);

    // A second call for the same trajectory returns the same answer.
    let again = decide(
        &h,
        ask(
            &schema,
            trajectory("traj-1", ONE / 2, 1024, hierarchy, true),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_vec(reward_of(&again.records.as_slice()[0])).unwrap(),
        serde_json::to_vec(reward).unwrap()
    );

    // A reward of exactly 1 is saturated: the trajectory is not kept.
    let saturated = decide(
        &h,
        ask(&schema, trajectory("traj-2", ONE, 8, Vec::new(), false)),
    )
    .await
    .unwrap();
    assert!(!reward_of(&saturated.records.as_slice()[0]).keep_for_training);
}

/// Verifies MEDULLA-DECISION-ENGINE-R751.755: premises name the trajectory, the
/// step count, the hierarchy depth, the keep verdict and every knob read.
// spec: MEDULLA-DECISION-ENGINE-R751.755
#[tokio::test]
async fn premises_name_trajectory_and_depth() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let hierarchy = vec![
        HtnNode {
            parent: None,
            first_step: 0,
            last_step: 3,
        },
        HtnNode {
            parent: Some(0),
            first_step: 0,
            last_step: 1,
        },
        HtnNode {
            parent: Some(1),
            first_step: 0,
            last_step: 0,
        },
    ];
    let batch = decide(
        &h,
        ask(&schema, trajectory("traj-9", ONE / 4, 4, hierarchy, false)),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let facts = facts(record);
    for expected in [
        format!("reward:trajectory=traj-9/{}", ONE / 4),
        "reward:steps=4".to_string(),
        "reward:hierarchy_depth=3".to_string(),
        "reward:keep=true".to_string(),
        "knob:reward.difficulty_floor=0.010000".to_string(),
        "knob:reward.cache_tier_discount=0.500000".to_string(),
    ] {
        assert!(
            facts.contains(&expected.as_str()),
            "missing premise {expected}"
        );
    }
    assert_eq!(
        facts
            .iter()
            .filter(|f| f.starts_with("knob:reward.") || f.starts_with("knob:htn."))
            .count(),
        medulla_decision_statistical_types::decision::reward::KNOBS.len()
    );
}

/// Verifies MEDULLA-DECISION-ENGINE-R751.755: a request for another tenant is
/// refused before anything is read, and a candidate that is not the trajectory
/// id is refused.
// spec: MEDULLA-DECISION-ENGINE-R751.755
#[tokio::test]
async fn other_tenant_trajectory_is_not_read() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let refused = decide_as_other_tenant(
        &h,
        ask(&schema, trajectory("traj-1", ONE / 2, 4, Vec::new(), false)),
    )
    .await;
    assert!(refused.starts_with("ACCESS_DENIED"), "{refused}");
    let body = trajectory("traj-1", ONE / 2, 4, Vec::new(), false);
    let mismatched = kernel_request(
        &schema,
        QuestionKind::RewardDecomposition,
        "reward_decomposition",
        TypedValue::RewardDecomposition(Box::new(body)),
        &["another-id"],
    );
    let error = decide(&h, mismatched).await.unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
}

fn node(parent: Option<u32>, first_step: u32, last_step: u32) -> HtnNode {
    HtnNode {
        parent,
        first_step,
        last_step,
    }
}

async fn refused_with(h: &Harness, request: DecideRequest, code: &str) {
    let error = decide(h, request).await.unwrap_err();
    assert!(error.starts_with(code), "{code}: {error}");
}

/// Verifies MEDULLA-DECISION-ENGINE-R751.752 (reward half): every reward refusal
/// code is returned as itself, is registered, and writes nothing.
// spec: MEDULLA-DECISION-ENGINE-R751.752
#[tokio::test]
async fn reward_refusals() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let dir = std::path::PathBuf::from(h.state.read().await.persist_dir.clone().unwrap());
    decide(
        &h,
        ask(&schema, trajectory("t", ONE / 2, 4, Vec::new(), false)),
    )
    .await
    .unwrap();
    let before = persisted_files(&dir);

    // REWARD_HTN_TOO_DEEP: a chain of 9 levels is above the default 8.
    let chain: Vec<HtnNode> = (0..9u32).map(|i| node(i.checked_sub(1), 0, 3)).collect();
    refused_with(
        &h,
        ask(&schema, trajectory("t", ONE / 2, 4, chain.clone(), false)),
        "REWARD_HTN_TOO_DEEP",
    )
    .await;
    // The depth bound is the pinned knob: a hierarchy of two levels fails a pin of 1.
    let mut policy = medulla_decision_statistical_types::decision::DecisionPolicy::engine_default();
    policy.knobs.insert(
        medulla_decision_statistical_types::decision::knobs::KnobName::new("htn.max_depth"),
        medulla_decision_statistical_types::decision::knobs::KnobValue::Integer(1),
    );
    policy.knobs.insert(
        medulla_decision_statistical_types::decision::knobs::KnobName::new("reward.max_steps"),
        medulla_decision_statistical_types::decision::knobs::KnobValue::Integer(3),
    );
    let pin = h.publish_policy("policy-reward", &policy);
    let mut pinned = ask(
        &schema,
        trajectory(
            "t",
            ONE / 2,
            2,
            vec![node(None, 0, 1), node(Some(0), 0, 0)],
            false,
        ),
    );
    pinned.policy = DecisionPolicyRef::Pinned {
        component: pin.clone(),
    };
    refused_with(&h, pinned, "REWARD_HTN_TOO_DEEP").await;

    // REWARD_TOO_MANY_STEPS: four steps against a pinned bound of three.
    let mut many = ask(&schema, trajectory("t", ONE / 2, 4, Vec::new(), false));
    many.policy = DecisionPolicyRef::Pinned { component: pin };
    refused_with(&h, many, "REWARD_TOO_MANY_STEPS").await;

    // REWARD_HTN_INVALID: a parent that is not below its child, and a child
    // range outside its parent's.
    refused_with(
        &h,
        ask(
            &schema,
            trajectory(
                "t",
                ONE / 2,
                4,
                vec![node(Some(1), 0, 3), node(None, 0, 3)],
                false,
            ),
        ),
        "REWARD_HTN_INVALID",
    )
    .await;
    refused_with(
        &h,
        ask(
            &schema,
            trajectory(
                "t",
                ONE / 2,
                4,
                vec![node(None, 0, 1), node(Some(0), 0, 3)],
                false,
            ),
        ),
        "REWARD_HTN_INVALID",
    )
    .await;

    // REWARD_SUM_MISMATCH: proposed rewards that miss the total.
    let mut proposed = trajectory("t", ONE / 2, 2, Vec::new(), false);
    proposed.proposed_rewards = Some(BoundedVec::new(vec![q(ONE / 8), q(ONE / 8)]).unwrap());
    refused_with(&h, ask(&schema, proposed), "REWARD_SUM_MISMATCH").await;

    // REWARD_ADVANTAGE_UNDEFINED: one step has no peer, equal rewards no spread.
    refused_with(
        &h,
        ask(&schema, trajectory("t", ONE / 2, 1, Vec::new(), true)),
        "REWARD_ADVANTAGE_UNDEFINED",
    )
    .await;
    let mut flat = trajectory("t", ONE / 2, 2, Vec::new(), true);
    flat.steps = BoundedVec::new(
        (1..=2)
            .map(|i| StepSignal {
                value_estimate: q(i * (ONE / 4)),
                cache_hit: false,
            })
            .collect(),
    )
    .unwrap();
    refused_with(&h, ask(&schema, flat), "REWARD_ADVANTAGE_UNDEFINED").await;

    // A refusal wrote nothing, and every code is registered.
    assert_eq!(persisted_files(&dir), before);
    for code in medulla_decision_statistical_types::decision::reward::REFUSALS {
        assert_eq!(
            medulla_decision_types::decision::code_registry::resolve(code.as_str()),
            Some(*code)
        );
    }
}
