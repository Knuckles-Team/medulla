//! Skill tiers served through `Decide` kind `Promotion` and the load set
//! (MEDULLA-DECISION-ENGINE-R714.3 to R714.6).

use super::learning_tests::{activate, pointer_state};
use super::path_preference_tests::pins;
use super::promotion_tests::{commit, promotion_request};
use super::retrieval_tests::learn;
use super::*;
use medulla_decision_stats::decision::promotion::rollout_set_digest;
use medulla_decision_types::decision::codes::PROMOTION_EVAL_TOO_SMALL;
use medulla_decision_types::decision::held_out::RevisionSubject;
use medulla_decision_types::decision::promotion_tier::{
    SkillTier, TierReview, ALWAYS_ON_CAPACITY_REACHED, MEMBER_OF, TIER_DOWNGRADE_NEEDS_REVIEW,
    TIER_REVIEW_MISSING, TIER_SELF_REVIEW,
};
use medulla_decision_types::decision::skill_evolution::RolloutSet;
use medulla_decision_types::decision::statistical::retrieval::{LearningWrite, PointerRef};
use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};
use medulla_decision_types::skill_eval::{SuiteBand, SuiteReport};
use medulla_persistence_records::decision_jobs::encode_artifact;
use serde_json::json;

use super::super::stat_skill_tier::{read_tier, write_tier};

const SKILL: &str = "skill:triage@3";
const GRAPH: &str = "__commons__";
/// The caller of every promotion in this file: the engine reads its team membership and its
/// authorship from the graph under this agent id (MEDULLA-IDENTITY-R739).
const REVIEWER: &str = "decider";

/// A person that holds the review scope: its principal is its own effective agent, with no
/// delegation and no service prefix, so the engine binds it as the reviewer.
fn reviewer() -> VerifiedRequestContext {
    VerifiedRequestContext::from_verified_claims(
        medulla_identity_types::acl::RequestContextClaims {
            principal: REVIEWER.to_string(),
            tenant: TENANT.to_string(),
            audience: "medulla".to_string(),
            agent_id: REVIEWER.to_string(),
            roles: Vec::new(),
            scopes: vec!["kg:read".to_string(), "skills:review".to_string()],
            policy_version: "policy-test".to_string(),
            delegation: Vec::new(),
            node: None,
            priority: None,
            delegation_id: None,
        },
        format!("test:{REVIEWER}"),
    )
}

/// A promotion decided by a caller that holds the review scope.
async fn decide(h: &Harness, request: DecideRequest) -> Result<DecisionBatch, String> {
    decide_as(h, &reviewer(), request).await
}

async fn decide_as(
    h: &Harness,
    caller: &VerifiedRequestContext,
    request: DecideRequest,
) -> Result<DecisionBatch, String> {
    decode(super::super::statistical::handle_decide(&h.state, 1, caller, request).await)
}

fn subject(tier: Option<SkillTier>) -> RevisionSubject {
    RevisionSubject::Skill {
        skill_ref: SKILL.to_owned(),
        tier,
    }
}

fn skill_set(challenger: &str, champion: &str) -> RolloutSet {
    RolloutSet {
        program_ref: SKILL.to_owned(),
        challenger_ref: challenger.to_owned(),
        champion_ref: Some(champion.to_owned()),
        ..super::promotion_tests::rollout_set(150_000)
    }
}

fn review(tier: SkillTier, team: &str, approver: &str) -> TierReview {
    TierReview {
        tier,
        reviewer_team: team.to_owned(),
        approver_principal: approver.to_owned(),
        eval_report_ref: None,
    }
}

/// A promotion request with the tier review and the graph the membership is read from.
fn tiered_request(
    schema: &medulla_decision_types::decision::ComponentDependency,
    tier: SkillTier,
    receipt: &str,
    challenger: &str,
    review: Option<&TierReview>,
) -> DecideRequest {
    let mut request = promotion_request(schema, &subject(Some(tier)), receipt, challenger);
    let mut params: Vec<TypedParam> = request.params.as_slice().to_vec();
    params.push(TypedParam {
        name: "graph".to_owned(),
        value: TypedValue::Text(GRAPH.to_owned()),
    });
    if let Some(review) = review {
        params.push(TypedParam {
            name: "tier_review".to_owned(),
            value: TypedValue::Text(serde_json::to_string(review).unwrap()),
        });
    }
    request.params = BoundedVec::new(params).unwrap();
    request
}

async fn record_set(h: &Harness, set: &RolloutSet) -> String {
    learn(h, "decider", LearningWrite::RecordRollouts { set: Box::new(set.clone()) })
        .await
        .unwrap();
    rollout_set_digest(set)
}

async fn graph_core(h: &Harness) -> std::sync::Arc<medulla_graph_core::graph::GraphCore> {
    h.state.read().await.registry.get(GRAPH).expect("graph exists").core.clone()
}

fn member(core: &medulla_graph_core::graph::GraphCore, principal: &str, team: &str) {
    for node in [principal, team] {
        core.add_node(
            node.to_string(),
            rmp_serde::to_vec_named(&json!({"type": "Entity"})).unwrap(),
        );
    }
    core.add_edge(
        principal.to_string(),
        team.to_string(),
        rmp_serde::to_vec_named(&json!({"relationship": MEMBER_OF})).unwrap(),
    )
    .unwrap();
}

fn facts(batch: &DecisionBatch) -> Vec<String> {
    batch.records.as_slice()[0].premises.iter().map(|p| p.fact.clone()).collect()
}

fn abstain_code(batch: &DecisionBatch) -> String {
    let StatisticalOutcome::Abstained { reasons } = &batch.records.as_slice()[0].outcome else {
        panic!("expected an abstention, got {:?}", batch.records.as_slice()[0].outcome)
    };
    reasons.as_slice()[0].code().as_str().to_owned()
}

/// Promote `challenger` on a decision that wins, commit it and move the pointer.
async fn promote_to(
    h: &Harness,
    schema: &medulla_decision_types::decision::ComponentDependency,
    tier: SkillTier,
    challenger: &str,
    champion: &str,
    review: &TierReview,
) -> DecisionBatch {
    let digest = record_set(h, &skill_set(challenger, champion)).await;
    let batch = decide(h, tiered_request(schema, tier, &digest, challenger, Some(review)))
        .await
        .unwrap();
    let record = batch.records.as_slice()[0].clone();
    commit(h, &record).await;
    pointer_state(h, PointerRef::Champion { subject: subject(None) }, activate(challenger, &record.record_id))
        .await
        .unwrap();
    batch
}

// spec: MEDULLA-DECISION-ENGINE-R714, MEDULLA-DECISION-ENGINE-R714.3, MEDULLA-DECISION-ENGINE-R714.6
#[tokio::test]
async fn a_skill_climbs_domain_core_and_always_on() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    let core = graph_core(&h).await;
    member(&core, REVIEWER, "support");
    member(&core, REVIEWER, "platform");
    let store = h.state.write().await.ensure_agent_library().unwrap();

    // Domain: a team approver.
    let domain = review(SkillTier::Domain, "support", "principal:ana");
    let batch = promote_to(&h, &pins.0, SkillTier::Domain, "revision:one", "revision:zero", &domain).await;
    assert!(facts(&batch).contains(&"tier:target=domain".to_owned()), "{:?}", facts(&batch));
    assert_eq!(read_tier(&store, TENANT, SKILL).unwrap(), Some(SkillTier::Domain));

    // Core: the platform team and a healthy suite report of enough cases.
    let core_review = review(SkillTier::Core, "platform", "principal:root");
    let digest = record_set(&h, &skill_set("revision:two", "revision:one")).await;
    let missing_report = decide(&h, tiered_request(&pins.0, SkillTier::Core, &digest, "revision:two", Some(&core_review)))
        .await
        .unwrap();
    assert_eq!(abstain_code(&missing_report), TIER_REVIEW_MISSING.as_str());
    let report = |cases: u64| SuiteReport {
        golden_ppm: 50_000,
        real_ppm: 800_000,
        judged_ppm: 150_000,
        pass_rate_ppm: 800_000,
        band: SuiteBand::Healthy,
        case_count: cases,
    };
    let put_report = |cases: u64| {
        let key = format!("skill-eval-report:{SKILL}");
        let current = store.decision_artifact(TENANT, &key).unwrap();
        store
            .swap_decision_artifact(TENANT, &key, current, encode_artifact(&report(cases)).unwrap())
            .unwrap();
    };
    put_report(5);
    let small = decide(&h, tiered_request(&pins.0, SkillTier::Core, &digest, "revision:two", Some(&core_review)))
        .await
        .unwrap();
    assert_eq!(abstain_code(&small), PROMOTION_EVAL_TOO_SMALL.as_str());
    put_report(40);
    promote_to(&h, &pins.0, SkillTier::Core, "revision:two", "revision:one", &core_review).await;
    assert_eq!(read_tier(&store, TENANT, SKILL).unwrap(), Some(SkillTier::Core));

    // AlwaysOn at count 99, then at 100.
    for n in 0..99 {
        write_tier(&store, TENANT, &format!("skill:other-{n}"), SkillTier::AlwaysOn).unwrap();
    }
    let always_on = review(SkillTier::AlwaysOn, "platform", "principal:root");
    let batch = promote_to(&h, &pins.0, SkillTier::AlwaysOn, "revision:three", "revision:two", &always_on).await;
    assert!(facts(&batch).contains(&"tier:always_on=99/100".to_owned()), "{:?}", facts(&batch));
    assert_eq!(read_tier(&store, TENANT, SKILL).unwrap(), Some(SkillTier::AlwaysOn));
    write_tier(&store, TENANT, "skill:other-99", SkillTier::AlwaysOn).unwrap();
    let digest = record_set(&h, &skill_set("revision:four", "revision:three")).await;
    let full = decide(&h, tiered_request(&pins.0, SkillTier::AlwaysOn, &digest, "revision:four", Some(&always_on)))
        .await
        .unwrap();
    assert_eq!(abstain_code(&full), ALWAYS_ON_CAPACITY_REACHED.as_str());
}

// spec: MEDULLA-DECISION-ENGINE-R714.5
#[tokio::test]
async fn a_review_that_does_not_qualify_is_refused_and_a_downgrade_needs_an_approver() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    let core = graph_core(&h).await;
    let store = h.state.write().await.ensure_agent_library().unwrap();
    let digest = record_set(&h, &skill_set("revision:one", "revision:zero")).await;

    // No review, and a reviewer who is in no team, both lack the receipt.
    let none = decide(&h, tiered_request(&pins.0, SkillTier::Domain, &digest, "revision:one", None))
        .await
        .unwrap();
    assert_eq!(abstain_code(&none), TIER_REVIEW_MISSING.as_str());
    let stranger = review(SkillTier::Domain, "support", "principal:eve");
    let outsider = decide(&h, tiered_request(&pins.0, SkillTier::Domain, &digest, "revision:one", Some(&stranger)))
        .await
        .unwrap();
    assert_eq!(abstain_code(&outsider), TIER_REVIEW_MISSING.as_str());

    // A skill that holds Core moves down to Domain only with an approver.
    member(&core, REVIEWER, "support");
    write_tier(&store, TENANT, SKILL, SkillTier::Core).unwrap();
    let down = decide(&h, tiered_request(&pins.0, SkillTier::Domain, &digest, "revision:one", None))
        .await
        .unwrap();
    assert_eq!(abstain_code(&down), TIER_DOWNGRADE_NEEDS_REVIEW.as_str());
    let approved = review(SkillTier::Domain, "support", "principal:ana");
    let down = decide(&h, tiered_request(&pins.0, SkillTier::Domain, &digest, "revision:one", Some(&approved)))
        .await
        .unwrap();
    assert!(facts(&down).contains(&"tier:target=domain".to_owned()));
}

// spec: MEDULLA-IDENTITY-R739
#[tokio::test]
async fn a_tiered_promotion_needs_the_review_scope_and_the_reviewer_is_the_verified_caller() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    let core = graph_core(&h).await;
    member(&core, REVIEWER, "support");
    member(&core, "principal:root", "platform");
    let digest = record_set(&h, &skill_set("revision:one", "revision:zero")).await;
    let domain = review(SkillTier::Domain, "support", "principal:ana");

    // Without the scope the request is refused before anything is read.
    let refused = decide_as(
        &h,
        &verified(),
        tiered_request(&pins.0, SkillTier::Domain, &digest, "revision:one", Some(&domain)),
    )
    .await
    .expect_err("the review scope is required");
    assert!(refused.contains("TIER_REVIEW_SCOPE_REQUIRED"), "{refused}");

    // The review names a platform member as its approver and a forged `tier_reviewer` names the
    // same one, but the reviewer is the verified caller, who is in the support team only: a Core
    // review is no receipt.
    let mut forged = tiered_request(
        &pins.0,
        SkillTier::Core,
        &digest,
        "revision:one",
        Some(&review(SkillTier::Core, "platform", "principal:root")),
    );
    let mut params = forged.params.as_slice().to_vec();
    params.push(TypedParam {
        name: "tier_reviewer".to_owned(),
        value: TypedValue::Text("principal:root".to_owned()),
    });
    forged.params = BoundedVec::new(params).unwrap();
    let batch = decide(&h, forged).await.unwrap();
    assert_eq!(abstain_code(&batch), TIER_REVIEW_MISSING.as_str());
}

// spec: MEDULLA-IDENTITY-R739
#[tokio::test]
async fn platform_or_team_review_is_decided_by_the_reviewers_graph_role() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    let core = graph_core(&h).await;
    let store = h.state.write().await.ensure_agent_library().unwrap();
    let digest = record_set(&h, &skill_set("revision:one", "revision:zero")).await;
    let put_report = || {
        let key = format!("skill-eval-report:{SKILL}");
        let current = store.decision_artifact(TENANT, &key).unwrap();
        let report = SuiteReport {
            golden_ppm: 50_000,
            real_ppm: 800_000,
            judged_ppm: 150_000,
            pass_rate_ppm: 800_000,
            band: SuiteBand::Healthy,
            case_count: 40,
        };
        store
            .swap_decision_artifact(TENANT, &key, current, encode_artifact(&report).unwrap())
            .unwrap();
    };
    put_report();
    write_tier(&store, TENANT, SKILL, SkillTier::Domain).unwrap();

    // A team member claims the platform team for a Core review: the claim decides nothing.
    member(&core, REVIEWER, "support");
    let claim = review(SkillTier::Core, "platform", "principal:ana");
    let team_only = decide(&h, tiered_request(&pins.0, SkillTier::Core, &digest, "revision:one", Some(&claim)))
        .await
        .unwrap();
    assert_eq!(abstain_code(&team_only), TIER_REVIEW_MISSING.as_str());

    // The same team member reviews for a Domain skill by its team and the review is accepted.
    let domain = review(SkillTier::Domain, "platform", "principal:ana");
    let team_review = decide(&h, tiered_request(&pins.0, SkillTier::Domain, &digest, "revision:one", Some(&domain)))
        .await
        .unwrap();
    assert!(facts(&team_review).contains(&"tier:target=domain".to_owned()));

    // Once the graph shows the reviewer in the platform team, the Core review is accepted.
    member(&core, REVIEWER, "platform");
    let platform = decide(&h, tiered_request(&pins.0, SkillTier::Core, &digest, "revision:one", Some(&claim)))
        .await
        .unwrap();
    assert!(facts(&platform).contains(&"tier:target=core".to_owned()), "{:?}", facts(&platform));
}

// spec: MEDULLA-IDENTITY-R739
#[tokio::test]
async fn the_author_cannot_review_its_own_revision_and_another_reviewer_can() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    let core = graph_core(&h).await;
    member(&core, REVIEWER, "support");
    let digest = record_set(&h, &skill_set("revision:one", "revision:zero")).await;
    let domain = review(SkillTier::Domain, "support", "principal:ana");
    let put_skill = |owner: &str| {
        core.add_node(
            SKILL.to_string(),
            rmp_serde::to_vec_named(
                &json!({"type": "Skill", "tenant_id": TENANT, "_owner": owner, "_visibility": "public"}),
            )
            .unwrap(),
        );
    };

    // The reviewer wrote the revision: refused, with the code of the two-person rule.
    put_skill(REVIEWER);
    let own = decide(&h, tiered_request(&pins.0, SkillTier::Domain, &digest, "revision:one", Some(&domain)))
        .await
        .unwrap();
    assert_eq!(abstain_code(&own), TIER_SELF_REVIEW.as_str());

    // Another principal wrote it: the same reviewer is accepted.
    put_skill("someone-else");
    let other = decide(&h, tiered_request(&pins.0, SkillTier::Domain, &digest, "revision:one", Some(&domain)))
        .await
        .unwrap();
    assert!(facts(&other).contains(&"tier:target=domain".to_owned()));
}
