//! The `team_coordination` family through the actual Decide path
//! (MEDULLA-DECISION-ENGINE-R170.2): an available fresh channel selects the
//! primary channel, an unavailable one selects the team when a member is
//! explicitly bound, and a stale signal or an unbound team abstains.

use std::time::Instant;

use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, QuestionSafety, StatisticalDecisionRecord,
    StatisticalOutcome, StatisticalQuestion, TypedParam, TypedValue,
};
use medulla_decision_types::decision::team_coordination::ChannelSignal;
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, DecisionPolicyRef, PremiseClass, ResolutionKind,
};
use medulla_types::contract::{BoundedVec, OpaqueId};

use super::super::stat_executor::ExecutionContext;
use super::*;

const TEAM: &str = "team:ops";
const NOW_MS: u64 = 1_700_000_100_000;

/// The fixture graph: a team, its members as (id, bound-to team, tenant).
fn graph_view(
    team_tenant: Option<&str>,
    members: &[(&str, &str, Option<&str>)],
) -> medulla_graph_core::graph::GraphView {
    let core = medulla_graph_core::graph::GraphCore::new();
    let put = |id: &str, props: serde_json::Value| {
        core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
    };
    let mut team = serde_json::json!({ "_team": true });
    if let Some(tenant) = team_tenant {
        team["tenant_id"] = serde_json::json!(tenant);
    }
    put(TEAM, team);
    for (id, bound_to, tenant) in members {
        let mut props = serde_json::json!({ "_team_member": true, "boundToTeam": bound_to });
        if let Some(tenant) = tenant {
            props["tenant_id"] = serde_json::json!(tenant);
        }
        put(id, props);
    }
    core.analysis_snapshot()
}

fn request(pin: &ComponentDependency, available: bool, age_s: u64) -> DecideRequest {
    let option = |id: &str| DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::default(),
        texts: BoundedVec::default(),
    };
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "team.fallback".to_string(),
            kind: QuestionKind::TeamCoordination,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![option("primary_channel"), option("team_coordination")])
                .unwrap(),
        },
        feature_schema: pin.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![
            TypedParam {
                name: "graph".to_string(),
                value: TypedValue::Iri("team-graph".to_string()),
            },
            TypedParam {
                name: "team".to_string(),
                value: TypedValue::Iri(TEAM.to_string()),
            },
            TypedParam {
                name: "channel_signal".to_string(),
                value: TypedValue::ChannelSignal(ChannelSignal {
                    channel_id: OpaqueId::new("channel:slack-ops").unwrap(),
                    available,
                    observed_at_ms: NOW_MS - age_s * 1000,
                }),
            },
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn decide_team(
    h: &Harness,
    request: &DecideRequest,
    view: &medulla_graph_core::graph::GraphView,
) -> Result<StatisticalDecisionRecord, String> {
    let ctx = ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: TENANT,
        now_ms: NOW_MS,
        server_secret: b"decide-stat-test-secret",
        designation: None,
    };
    super::super::stat_decide::decide_blocking(
        &ctx,
        request,
        Some(view),
        "decider".to_string(),
        Instant::now(),
    )
    .map(|batch| batch.records.as_slice()[0].clone())
}

/// The option the advisory scored one.
fn chosen(record: &StatisticalDecisionRecord) -> String {
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("expected an advisory, got {:?}", record.outcome);
    };
    let ones: Vec<_> = scores.iter().filter(|s| s.score.value != 0).collect();
    assert_eq!(ones.len(), 1, "exactly one legal option");
    ones[0].option_id.clone()
}

fn abstention_code(record: &StatisticalDecisionRecord) -> String {
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("expected an abstention, got {:?}", record.outcome);
    };
    match &reasons.as_slice()[0] {
        AbstainReason::Family { code, .. } => code.as_str().to_string(),
        other => panic!("expected a family reason, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R170.2
#[tokio::test]
async fn unavailable_channel_selects_team() {
    let h = Harness::new().await;
    let pin = publish_route_schema(&h).0;
    let bound = graph_view(None, &[("member:a", TEAM, None)]);

    // An available channel with a fresh signal: the primary channel, even
    // though a team with a bound member exists.
    let available = decide_team(&h, &request(&pin, true, 5), &bound).unwrap();
    assert_eq!(chosen(&available), "primary_channel");
    assert_eq!(available.resolution_kind, ResolutionKind::Constraint);

    // An unavailable channel with a bound member: the team.
    let down = decide_team(&h, &request(&pin, false, 5), &bound).unwrap();
    assert_eq!(chosen(&down), "team_coordination");
    assert_eq!(down.resolution_kind, ResolutionKind::Constraint);

    // An unavailable channel is a team choice whatever the signal's age; the
    // freshness limit only gates the primary channel.
    let old_down = decide_team(&h, &request(&pin, false, 3_000), &bound).unwrap();
    assert_eq!(chosen(&old_down), "team_coordination");

    // An available channel whose signal is older than team.signal_max_age
    // (30 s) is not legal, and the team is not legal either: abstain.
    let stale = decide_team(&h, &request(&pin, true, 31), &bound).unwrap();
    assert_eq!(abstention_code(&stale), "TEAM_SIGNAL_STALE");
    // Exactly at the limit the signal is still fresh.
    let edge = decide_team(&h, &request(&pin, true, 30), &bound).unwrap();
    assert_eq!(chosen(&edge), "primary_channel");

    // The freshness limit is a decision-policy knob: a pinned policy that
    // raises it to 60 s makes the 31 s signal fresh, and one that lowers it to
    // 10 s makes a 20 s signal stale; the sealed premise names the pinned value.
    let pinned = |name: &str, seconds: i64| {
        let mut policy = DecisionPolicy::engine_default();
        policy.knobs.insert(
            KnobName::new("team.signal_max_age"),
            KnobValue::Integer(seconds),
        );
        DecisionPolicyRef::Pinned {
            component: h.publish_policy(name, &policy),
        }
    };
    let mut relaxed = request(&pin, true, 31);
    relaxed.policy = pinned("team-relaxed", 60);
    let relaxed = decide_team(&h, &relaxed, &bound).unwrap();
    assert_eq!(chosen(&relaxed), "primary_channel");
    assert!(relaxed
        .premises
        .iter()
        .any(|p| p.fact == "knob:team.signal_max_age=60"));
    let mut strict = request(&pin, true, 20);
    strict.policy = pinned("team-strict", 10);
    let strict = decide_team(&h, &strict, &bound).unwrap();
    assert_eq!(abstention_code(&strict), "TEAM_SIGNAL_STALE");

    // The team needs one explicitly bound, visible member: a member naming
    // another team, a member of another tenant, and no member all abstain.
    for members in [
        vec![("member:a", "team:other", None)],
        vec![("member:a", TEAM, Some("tenant-other"))],
        vec![],
    ] {
        let view = graph_view(None, &members);
        let record = decide_team(&h, &request(&pin, false, 5), &view).unwrap();
        assert_eq!(abstention_code(&record), "TEAM_NO_BOUND_MEMBER");
    }
    // A team node of another tenant is not visible.
    let foreign = graph_view(Some("tenant-other"), &[("member:a", TEAM, None)]);
    let record = decide_team(&h, &request(&pin, false, 5), &foreign).unwrap();
    assert_eq!(abstention_code(&record), "TEAM_NO_BOUND_MEMBER");

    // The record seals the observation premises.
    let facts: Vec<&str> = down.premises.iter().map(|p| p.fact.as_str()).collect();
    assert!(facts.contains(&"team.channel_available=false"));
    assert!(facts.contains(&"knob:team.signal_max_age=30"));
    assert!(facts.contains(&"team.bound_member:member:a"));
    assert!(down
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("team."))
        .all(|p| p.class == PremiseClass::Observation));
}

// spec: MEDULLA-DECISION-ENGINE-R170.2
#[tokio::test]
async fn malformed_team_questions_are_refused() {
    let h = Harness::new().await;
    let pin = publish_route_schema(&h).0;
    let view = graph_view(None, &[("member:a", TEAM, None)]);
    // A declared set other than the two options is refused, not abstained.
    let mut wrong = request(&pin, true, 1);
    if let CandidateSource::Declared { options } = &mut wrong.candidates {
        let mut all = options.as_slice().to_vec();
        all[1].option_id = "pager".to_string();
        *options = BoundedVec::new(all).unwrap();
    }
    assert!(decide_team(&h, &wrong, &view).is_err());
    // A question with no channel signal is refused.
    let mut missing = request(&pin, true, 1);
    missing.params = BoundedVec::new(missing.params.as_slice()[..2].to_vec()).unwrap();
    assert!(decide_team(&h, &missing, &view)
        .unwrap_err()
        .contains("channel_signal"));
}
