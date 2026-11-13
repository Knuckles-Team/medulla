//! Served acceptance of the `capability_designation` question family
//! (MEDULLA-DECISION-ENGINE-R172.7, R172.8): a `Decide` over the resident
//! capability index returns typed, ranked designations for the asking tenant
//! only, and every refusal is typed and writes nothing.

use super::*;
use crate::decide::stat_designation::{shared_capability_index, DesignationRuntime};
use medulla_decision_memory::memory::capability_index::Change;
use medulla_decision_types::decision::capability_designation::{
    CapabilityDesignation, CapabilityDesignationQuestion, CapabilityOutcomeApplied,
    CapabilityRecord, CapabilityReward, DesignationAsk, DesignationKnobs, DesignationWeights,
    EntityKind, QueryVector, SchemaProjection, DESIGNATION_PARAM, KNOBS as DESIGNATION_KNOBS,
    NEUTRAL_REWARD_Q32, Q32_ONE,
};
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::ResolutionKind;
use medulla_types::contract::{OpaqueId, TenantId};
use std::collections::BTreeSet;

const VERSION: &str = "emb-v1";
const OTHER_TENANT: &str = "tenant-other";

fn record(tenant: Option<&str>, id: &str, capabilities: &[&str]) -> CapabilityRecord {
    CapabilityRecord {
        tenant_id: tenant.map(|t| TenantId::new(t).unwrap()),
        entity_id: OpaqueId::new(id).unwrap(),
        entity_kind: EntityKind::Agent,
        capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
        ontology_type: None,
        policy_tags: BTreeSet::new(),
        swappable_with: BTreeSet::new(),
        embedding_version: VERSION.to_string(),
        dim: 3,
        record_revision: 1,
        applies_to: BTreeSet::new(),
        executes: BTreeSet::new(),
        pack: None,
    }
}

async fn seed(h: &Harness, rows: Vec<(CapabilityRecord, Vec<f32>)>) {
    let index = shared_capability_index(&*h.state.read().await);
    for (record, vector) in rows {
        index.upsert(record, vector).unwrap();
    }
}

fn weights(cosine: i64, reward: i64) -> DesignationWeights {
    DesignationWeights {
        cosine_q32: cosine,
        reward_q32: reward,
        prior_q32: 0,
    }
}

fn question(query: &[f32], k: u8, weights: DesignationWeights) -> CapabilityDesignationQuestion {
    CapabilityDesignationQuestion {
        tenant_id: TenantId::new(TENANT).unwrap(),
        query_vector: QueryVector::from_f32(VERSION, query).unwrap(),
        required_capabilities: BTreeSet::new(),
        required_policy_tags: BTreeSet::new(),
        k,
        weights,
        ontology_prior: None,
        target_classes: BTreeSet::new(),
    }
}

fn designation_request(
    schema: &ComponentDependency,
    question: CapabilityDesignationQuestion,
    policy: DecisionPolicyRef,
) -> DecideRequest {
    designation_request_on("__commons__", schema, question, policy)
}

fn designation_request_on(
    graph: &str,
    schema: &ComponentDependency,
    question: CapabilityDesignationQuestion,
    policy: DecisionPolicyRef,
) -> DecideRequest {
    let question_id = "capability.designation".to_string();
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: question_id.clone(),
            kind: QuestionKind::CapabilityDesignation,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![
                medulla_decision_types::decision::statistical::declared::DeclaredOption {
                    option_id: question_id,
                    classification: BoundedVec::default(),
                    numbers: BoundedVec::default(),
                    texts: BoundedVec::default(),
                },
            ])
            .unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy,
        params: BoundedVec::new(vec![TypedParam {
            name: DESIGNATION_PARAM.to_string(),
            value: TypedValue::CapabilityDesignation(Box::new(DesignationAsk {
                question,
                graph: graph.to_string(),
            })),
        }])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn answer_of(record: &StatisticalDecisionRecord) -> &CapabilityDesignation {
    record
        .answer
        .as_ref()
        .and_then(|answer| answer.capability_designation())
        .unwrap_or_else(|| panic!("a designation answer, got {:?}", record.outcome))
}

fn ids_of(answer: &CapabilityDesignation) -> Vec<&str> {
    answer
        .designations
        .iter()
        .map(|d| d.entity_id.as_str())
        .collect()
}

fn schema_pin(h: &Harness) -> ComponentDependency {
    h.publish_test_component(
        "schema-designation",
        AgentComponentKind::FeatureSchema,
        "designation question",
        Some(&score_schema()),
        None,
    )
    .unwrap()
}

fn score_schema() -> FeatureSchemaBody {
    FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![FeatureSpec {
            name: "score".to_string(),
            kind: FeatureKind::Number {
                key: "score".to_string(),
            },
            missing: MissingValue::Abstain,
        }])
        .unwrap(),
    }
}

fn fixture_rows() -> Vec<(CapabilityRecord, Vec<f32>)> {
    vec![
        (
            record(Some(TENANT), "agent-a", &["cap:x"]),
            vec![1.0, 0.0, 0.0],
        ),
        (
            record(Some(TENANT), "agent-b", &["cap:x"]),
            vec![0.95, 0.312_249_9, 0.0],
        ),
        (
            record(Some(OTHER_TENANT), "agent-c", &["cap:x"]),
            vec![1.0, 0.0, 0.0],
        ),
        (record(None, "agent-d", &["cap:x"]), vec![0.0, 1.0, 0.0]),
    ]
}

// spec: MEDULLA-DECISION-ENGINE-R172.7
#[tokio::test]
async fn decide_routes_capability_designation() {
    let h = Harness::new().await;
    let pin = schema_pin(&h);
    seed(&h, fixture_rows()).await;

    // Cosine alone: the asking tenant's own records and the commons record,
    // best first; another tenant's record is never read.
    let batch = decide(
        &h,
        designation_request(
            &pin,
            question(&[1.0, 0.0, 0.0], 4, weights(Q32_ONE, 0)),
            DecisionPolicyRef::Default,
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::CapabilityDesignation);
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(record.evidence_class, EvidenceClass::Observation);
    let answer = answer_of(record);
    assert_eq!(ids_of(answer), vec!["agent-a", "agent-b", "agent-d"]);
    assert_eq!(answer.candidate_pool_size, 3);
    assert_eq!(answer.embedding_version, VERSION);
    assert!(answer.designations.iter().all(|d| d.eligibility.eligible));
    assert!(
        !ids_of(answer).contains(&"agent-c"),
        "another tenant's record"
    );
    let StatisticalOutcome::Advisory { scores, calibrated } = &record.outcome else {
        panic!("an advisory outcome, got {:?}", record.outcome);
    };
    assert!(!calibrated);
    assert_eq!(scores.len(), 3);

    // A learned reward moves the ranking when the question weighs it.
    let mut reward = CapabilityReward::neutral(
        TenantId::new(TENANT).unwrap(),
        OpaqueId::new("agent-b").unwrap(),
    );
    reward.reward_q32 = Q32_ONE;
    reward.outcome_count = 4;
    reward.updated_revision = 1;
    h.store.put_capability_reward(&reward).unwrap();
    let batch = decide(
        &h,
        designation_request(
            &pin,
            question(&[1.0, 0.0, 0.0], 2, weights(Q32_ONE / 2, Q32_ONE / 2)),
            DecisionPolicyRef::Default,
        ),
    )
    .await
    .unwrap();
    let answer = answer_of(&batch.records.as_slice()[0]);
    assert_eq!(ids_of(answer), vec!["agent-b", "agent-a"]);
    assert_eq!(answer.designations[0].reward_q32, Q32_ONE);
    assert_eq!(answer.designations[1].reward_q32, NEUTRAL_REWARD_Q32);

    // A k of zero returns an empty list, not a refusal.
    let batch = decide(
        &h,
        designation_request(
            &pin,
            question(&[1.0, 0.0, 0.0], 0, weights(Q32_ONE, 0)),
            DecisionPolicyRef::Default,
        ),
    )
    .await
    .unwrap();
    assert!(answer_of(&batch.records.as_slice()[0])
        .designations
        .is_empty());

    // Cross-tenant negative: another tenant's caller cannot ask for this
    // tenant's question, and a question naming another tenant is refused.
    let stranger = VerifiedRequestContext::verified_for_test_in_tenant("decider", OTHER_TENANT);
    let request = designation_request(
        &pin,
        question(&[1.0, 0.0, 0.0], 2, weights(Q32_ONE, 0)),
        DecisionPolicyRef::Default,
    );
    let response =
        super::super::statistical::handle_decide(&h.state, 1, &stranger, request.clone()).await;
    assert!(
        response
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("ACCESS_DENIED")),
        "{:?}",
        response.error
    );
    let mut foreign = request;
    let mut params = foreign.params.as_slice().to_vec();
    if let TypedValue::CapabilityDesignation(ask) = &mut params[0].value {
        ask.question.tenant_id = TenantId::new(OTHER_TENANT).unwrap();
    }
    foreign.params = BoundedVec::new(params).unwrap();
    let error = decide(&h, foreign).await.unwrap_err();
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
}

fn runtime_of(
    index: std::sync::Arc<medulla_decision_memory::memory::capability_index::CapabilityIndex>,
) -> DesignationRuntime {
    DesignationRuntime {
        index,
        projection: SchemaProjection::new(
            &format!("sha256:{}", "ab".repeat(32)),
            Vec::<(String, String)>::new(),
        ),
        schema_digest: format!("sha256:{}", "ab".repeat(32)),
        schema_error: None,
        load_set: None,
        usage_guidance: None,
        adapter: None,
    }
}

fn decide_with_runtime(
    h: &Harness,
    request: &DecideRequest,
    runtime: DesignationRuntime,
) -> Result<StatisticalDecisionRecord, String> {
    let ctx = crate::decide::stat_executor::ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: TENANT,
        now_ms: 1_000,
        server_secret: b"decide-stat-test-secret",
        matched: &[],
        designation: Some(&runtime),
        reader: None,
    };
    crate::decide::stat_decide::decide_blocking(
        &ctx,
        request,
        None,
        "decider".to_string(),
        std::time::Instant::now(),
    )
    .map(|batch| batch.records.as_slice()[0].clone())
}

fn epoch_of(index: &medulla_decision_memory::memory::capability_index::CapabilityIndex) -> u64 {
    index
        .visible(&TenantId::new(TENANT).unwrap())
        .iter()
        .map(|snapshot| snapshot.epoch())
        .sum()
}

// spec: MEDULLA-DECISION-ENGINE-R172.8
#[tokio::test]
async fn capability_designation_refusals() {
    let h = Harness::new().await;
    let pin = schema_pin(&h);
    seed(&h, fixture_rows()).await;
    let index = shared_capability_index(&*h.state.read().await);
    let base = || question(&[1.0, 0.0, 0.0], 2, weights(Q32_ONE, 0));
    let artifacts = |h: &Harness| {
        h.store
            .decision_artifacts_with_prefix(TENANT, "", 10_000)
            .unwrap()
            .len()
    };
    let rewards = |h: &Harness| h.store.capability_rewards(TENANT).unwrap();

    let mut cases: Vec<(&str, CapabilityDesignationQuestion)> = Vec::new();
    let mut other_version = base();
    other_version.query_vector = QueryVector::from_f32("emb-v2", &[1.0, 0.0, 0.0]).unwrap();
    cases.push(("DESIGNATION_EMBEDDING_VERSION_MISMATCH", other_version));
    let mut other_dim = base();
    other_dim.query_vector = QueryVector::from_f32(VERSION, &[1.0, 0.0]).unwrap();
    cases.push(("DESIGNATION_DIM_MISMATCH", other_dim));
    let mut empty = base();
    empty.query_vector = QueryVector {
        embedding_version: VERSION.to_string(),
        dim: 0,
        f32le_b64: String::new(),
    };
    cases.push(("DESIGNATION_EMPTY_QUERY_VECTOR", empty));
    let mut malformed = base();
    malformed.query_vector.f32le_b64 = "not base64 !!".to_string();
    cases.push(("DESIGNATION_MALFORMED_QUERY_VECTOR", malformed));
    let mut many_required = base();
    many_required.required_capabilities = (0..33).map(|i| format!("cap:{i:02}")).collect();
    cases.push(("DESIGNATION_TOO_MANY_REQUIRED_CAPABILITIES", many_required));
    let mut many_k = base();
    many_k.k = 65;
    cases.push(("DESIGNATION_TOO_MANY_CANDIDATES", many_k));
    let mut needs_schema = base();
    needs_schema.required_capabilities = ["cap:x".to_string()].into_iter().collect();

    let (epoch, count, held) = (epoch_of(&index), artifacts(&h), rewards(&h));
    for (code, question) in cases {
        let request = designation_request(&pin, question, DecisionPolicyRef::Default);
        let error = decide_with_runtime(&h, &request, runtime_of(index.clone())).unwrap_err();
        assert!(error.starts_with(&format!("{code}:")), "{code}: {error}");
        // The same refusal is the served one.
        let response =
            super::super::statistical::handle_decide(&h.state, 1, &verified(), request).await;
        assert_eq!(response.error.as_deref(), Some(code));
    }

    // The schema closure is unavailable: refused, never literal matching.
    let mut unavailable = runtime_of(index.clone());
    unavailable.schema_error = Some("graph not found".to_string());
    let request = designation_request(&pin, needs_schema.clone(), DecisionPolicyRef::Default);
    let error = decide_with_runtime(&h, &request, unavailable).unwrap_err();
    assert!(
        error.starts_with("DESIGNATION_CAPABILITY_SCHEMA_UNAVAILABLE:"),
        "{error}"
    );

    // A policy that bounds the shard below what the tenant shard holds.
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.knobs.insert(
        KnobName::new("designation.max_records_per_shard"),
        KnobValue::Integer(1000),
    );
    let bounded = DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-designation-bound", &policy),
    };
    let filler: Vec<Change> = (0..1001)
        .map(|n| {
            Change::Upsert(
                record(Some(TENANT), &format!("filler-{n:04}"), &["cap:filler"]),
                vec![0.0, 0.0, 1.0],
            )
        })
        .collect();
    index
        .shard(Some(&TenantId::new(TENANT).unwrap()))
        .unwrap()
        .apply(filler)
        .unwrap();
    let epoch_full = epoch_of(&index);
    let request = designation_request(&pin, base(), bounded);
    let error = decide_with_runtime(&h, &request, runtime_of(index.clone())).unwrap_err();
    assert!(
        error.starts_with("DESIGNATION_INDEX_CAPACITY_EXCEEDED:"),
        "{error}"
    );

    // No visible candidate covers a required capability: an abstention naming it.
    let mut uncovered = base();
    uncovered.required_capabilities = ["cap:missing".to_string()].into_iter().collect();
    let request = designation_request(&pin, uncovered, DecisionPolicyRef::Default);
    let sealed = decide_with_runtime(&h, &request, runtime_of(index.clone())).unwrap();
    let StatisticalOutcome::Abstained { reasons } = &sealed.outcome else {
        panic!("an abstention, got {:?}", sealed.outcome);
    };
    let reason = &reasons.as_slice()[0];
    assert_eq!(reason.code().as_str(), "DESIGNATION_NO_CAPABLE_CANDIDATE");
    assert!(matches!(
        reason,
        medulla_decision_types::decision::AbstainReason::Family { detail, .. } if detail == "cap:missing"
    ));
    assert_eq!(sealed.resolution_kind, ResolutionKind::Abstention);

    // Nothing changed: no reward, no index epoch, no decision record.
    assert_eq!(epoch_of(&index), epoch_full);
    assert_eq!(
        epoch_full - epoch,
        1,
        "only the filler batch advanced the epoch"
    );
    assert_eq!(artifacts(&h), count);
    assert_eq!(rewards(&h), held);
}

fn outcome_op(
    tenant: &str,
    entity: &str,
    decision: &str,
    success: Option<bool>,
    reward_q32: Option<i64>,
    sources: &[&str],
) -> DecisionLogOp {
    DecisionLogOp::CapabilityOutcome {
        tenant_id: tenant.to_string(),
        entity_id: entity.to_string(),
        success,
        reward_q32,
        decision_id: decision.to_string(),
        source_ids: BoundedVec::new(sources.iter().map(|s| s.to_string()).collect()).unwrap(),
        policy: DecisionPolicyRef::Default,
    }
}

async fn send_outcome(h: &Harness, op: DecisionLogOp) -> Result<CapabilityOutcomeApplied, String> {
    decode(super::log_tests::log_op(h, "decider", op).await)
}

/// `(1 - alpha) * old + alpha * observed` on Q32, computed independently of
/// the production function from the default knob value of 0.3.
fn ema_q32(old: i64, observed: i64) -> i64 {
    let alpha: i128 = (300_000i128 << 32) / 1_000_000;
    let one: i128 = i128::from(Q32_ONE);
    ((((one - alpha) * i128::from(old)) >> 32) + ((alpha * i128::from(observed)) >> 32)) as i64
}

// spec: MEDULLA-DECISION-ENGINE-R172.7.1
#[tokio::test]
async fn capability_outcome_updates_reward_and_refuses() {
    let h = Harness::new().await;
    seed(&h, fixture_rows()).await;
    let alpha = {
        let knobs = DesignationKnobs::from_policy(
            &medulla_decision_types::decision::DecisionPolicy::engine_default(),
        );
        knobs.reward_alpha_q32
    };
    assert_eq!(alpha, ((300_000i128 << 32) / 1_000_000) as i64);

    // A success moves the neutral reward by alpha and leaves a trace.
    let first = send_outcome(
        &h,
        outcome_op(
            TENANT,
            "agent-a",
            "decision-1",
            Some(true),
            None,
            &["record-1", "record-2"],
        ),
    )
    .await
    .unwrap();
    let want = ema_q32(NEUTRAL_REWARD_Q32, Q32_ONE);
    assert_eq!(first.reward.reward_q32, want);
    assert_eq!(first.reward.outcome_count, 1);
    assert_eq!(first.reward.generation, 0);
    assert!(!first.replayed);
    assert_eq!(first.trace.prior_reward_q32, NEUTRAL_REWARD_Q32);
    assert_eq!(first.trace.new_reward_q32, want);
    assert_eq!(first.trace.alpha_q32, alpha);
    assert_eq!(first.trace.generation, 0);
    assert_eq!(first.trace.revision, 1);
    assert_eq!(first.trace.decision_id, "decision-1");
    assert_eq!(first.trace.source_ids, vec!["record-1", "record-2"]);
    // Durable: the reward row holds it.
    let stored = h
        .store
        .capability_reward(TENANT, "agent-a")
        .unwrap()
        .unwrap();
    assert_eq!(stored, first.reward);

    // An explicit reward is clamped to [0, 1] and chains from the prior.
    let second = send_outcome(
        &h,
        outcome_op(
            TENANT,
            "agent-a",
            "decision-2",
            None,
            Some(Q32_ONE * 3),
            &[],
        ),
    )
    .await
    .unwrap();
    assert_eq!(second.trace.observed_q32, Q32_ONE);
    assert_eq!(second.trace.prior_reward_q32, want);
    assert_eq!(second.reward.reward_q32, ema_q32(want, Q32_ONE));
    assert_eq!(second.reward.outcome_count, 2);
    assert_eq!(second.trace.revision, 2);
    let failure = send_outcome(
        &h,
        outcome_op(
            TENANT,
            "agent-b",
            "decision-3",
            Some(false),
            None,
            &["record-3"],
        ),
    )
    .await
    .unwrap();
    assert_eq!(failure.reward.reward_q32, ema_q32(NEUTRAL_REWARD_Q32, 0));

    // A re-send of an applied outcome answers its stored trace and applies
    // nothing twice; the same decision with another outcome is a conflict.
    let rewards_before = h.store.capability_rewards(TENANT).unwrap();
    let replay = send_outcome(
        &h,
        outcome_op(
            TENANT,
            "agent-a",
            "decision-1",
            Some(true),
            None,
            &["record-1", "record-2"],
        ),
    )
    .await
    .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.trace, first.trace);
    assert_eq!(replay.reward, second.reward);
    let conflict = send_outcome(
        &h,
        outcome_op(
            TENANT,
            "agent-a",
            "decision-1",
            Some(false),
            None,
            &["record-1", "record-2"],
        ),
    )
    .await
    .unwrap_err();
    assert!(conflict.starts_with("IDEMPOTENCY_CONFLICT"), "{conflict}");
    assert_eq!(h.store.capability_rewards(TENANT).unwrap(), rewards_before);

    // Typed refusals, none of which writes.
    let artifacts = |h: &Harness| {
        h.store
            .decision_artifacts_with_prefix(TENANT, "", 10_000)
            .unwrap()
            .len()
    };
    let count = artifacts(&h);
    let cases: Vec<(&str, DecisionLogOp)> = vec![
        (
            "DESIGNATION_OUTCOME_VALUE_MISSING:",
            outcome_op(TENANT, "agent-a", "decision-4", None, None, &[]),
        ),
        (
            "DESIGNATION_OUTCOME_VALUE_CONFLICT:",
            outcome_op(
                TENANT,
                "agent-a",
                "decision-5",
                Some(true),
                Some(Q32_ONE),
                &[],
            ),
        ),
        (
            // Another tenant's entity is not readable by this tenant.
            "ACCESS_DENIED",
            outcome_op(TENANT, "agent-c", "decision-6", Some(true), None, &[]),
        ),
        (
            "ACCESS_DENIED",
            outcome_op(TENANT, "agent-missing", "decision-7", Some(true), None, &[]),
        ),
        (
            // The op names a tenant other than the verified caller's.
            "ACCESS_DENIED",
            outcome_op(OTHER_TENANT, "agent-a", "decision-8", Some(true), None, &[]),
        ),
    ];
    for (code, op) in cases {
        let error = send_outcome(&h, op).await.unwrap_err();
        assert!(error.starts_with(code), "{code}: {error}");
    }
    assert_eq!(h.store.capability_rewards(TENANT).unwrap(), rewards_before);
    assert_eq!(artifacts(&h), count);
    assert!(h.store.capability_rewards(OTHER_TENANT).unwrap().is_empty());
}

// spec: MEDULLA-DECISION-ENGINE-R172.9
#[tokio::test]
async fn premises_name_scores_and_knobs() {
    let h = Harness::new().await;
    let pin = schema_pin(&h);
    let mut rows = fixture_rows();
    rows.push((
        record(Some(TENANT), "agent-s", &["cap:sub"]),
        vec![0.9, 0.435_889_9, 0.0],
    ));
    seed(&h, rows).await;
    let index = shared_capability_index(&*h.state.read().await);
    let digest = format!("sha256:{}", "cd".repeat(32));
    let runtime = DesignationRuntime {
        index: index.clone(),
        projection: SchemaProjection::new(
            &digest,
            vec![("cap:sub".to_string(), "cap:super".to_string())],
        ),
        schema_digest: digest.clone(),
        schema_error: None,
        load_set: None,
        usage_guidance: None,
        adapter: None,
    };
    let mut reward = CapabilityReward::neutral(
        TenantId::new(TENANT).unwrap(),
        OpaqueId::new("agent-s").unwrap(),
    );
    reward.reward_q32 = Q32_ONE;
    reward.outcome_count = 2;
    reward.updated_revision = 2;
    h.store.put_capability_reward(&reward).unwrap();

    let mut ask = question(&[1.0, 0.0, 0.0], 4, weights(Q32_ONE / 2, Q32_ONE / 2));
    ask.required_capabilities = ["cap:super".to_string()].into_iter().collect();
    let request = designation_request(&pin, ask, DecisionPolicyRef::Default);
    let sealed = decide_with_runtime(&h, &request, runtime).unwrap();
    let answer = answer_of(&sealed);
    assert_eq!(ids_of(answer), vec!["agent-s"]);
    let premises = sealed.premises.as_slice();
    let has = |subject: &str, fact: &str| {
        premises
            .iter()
            .any(|p| p.subject == subject && p.fact == fact)
    };

    // One premise per designation naming every score term and the verdict.
    for d in &answer.designations {
        assert!(
            has(
                d.entity_id.as_str(),
                &format!(
                    "designation:cosine={};reward={};prior={};score={};eligible={}",
                    d.cosine_q32, d.reward_q32, d.prior_q32, d.score_q32, d.eligibility.eligible
                )
            ),
            "{d:?}"
        );
        assert_eq!(d.reward_q32, Q32_ONE);
        // One premise per subsumption path used.
        assert_eq!(
            d.eligibility.subsumption_paths,
            vec![vec!["cap:sub", "cap:super"]]
        );
        assert!(has(d.entity_id.as_str(), "subsumption:cap:sub>cap:super"));
    }
    // One knob premise per designation knob read, at the resolved value.
    let policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    for spec in DESIGNATION_KNOBS {
        assert!(
            has(
                "policy",
                &format!("knob:{}={}", spec.name, policy.knob(spec).render())
            ),
            "{}",
            spec.name
        );
    }
    // The index epoch, embedding version, schema digest and pool size.
    assert!(has(
        "capability_index",
        &format!("index_epoch={}", answer.index_epoch)
    ));
    assert!(has(
        "capability_index",
        &format!("embedding_version={VERSION}")
    ));
    assert!(has(
        "capability_index",
        &format!("candidate_pool_size={}", answer.candidate_pool_size)
    ));
    assert!(has("schema", &format!("schema_digest={digest}")));
    // Index facts are observations: the record states the weakest class.
    assert_eq!(sealed.evidence_class, EvidenceClass::Observation);

    // The outcome trace of the designated entity links the decision's record
    // id as its source and carries the prior and new reward, alpha and generation.
    let applied = send_outcome(
        &h,
        outcome_op(
            TENANT,
            "agent-s",
            "decision-evidence",
            Some(false),
            None,
            &[&sealed.record_id],
        ),
    )
    .await
    .unwrap();
    let alpha = DesignationKnobs::from_policy(&policy).reward_alpha_q32;
    assert_eq!(applied.trace.source_ids, vec![sealed.record_id.clone()]);
    assert_eq!(applied.trace.prior_reward_q32, Q32_ONE);
    assert_eq!(applied.trace.new_reward_q32, ema_q32(Q32_ONE, 0));
    assert_eq!(applied.trace.alpha_q32, alpha);
    assert_eq!(applied.trace.generation, reward.generation);
    assert_eq!(applied.reward.outcome_count, 3);
}

const ONT: &str = "https://knuckles-team.github.io/ontology/core#";

fn class(name: &str) -> String {
    format!("{ONT}{name}")
}

fn pack_runtime(
    index: std::sync::Arc<medulla_decision_memory::memory::capability_index::CapabilityIndex>,
) -> DesignationRuntime {
    let digest = format!("sha256:{}", "ef".repeat(32));
    DesignationRuntime {
        index,
        projection: SchemaProjection::new(
            &digest,
            vec![
                (class("EquityPosition"), class("Position")),
                (class("ValueAtRisk"), class("RiskMethod")),
            ],
        )
        .with_classes([
            class("Position"),
            class("EquityPosition"),
            class("RiskMethod"),
        ]),
        schema_digest: digest,
        schema_error: None,
        load_set: None,
        usage_guidance: None,
    }
}

fn pack_record(id: &str, applies: &[&str], executes: &[&str], revision: u64) -> CapabilityRecord {
    let mut record = record(Some(TENANT), id, &[]);
    record.entity_kind = EntityKind::Tool;
    record.applies_to = applies.iter().map(|c| class(c)).collect();
    record.executes = executes.iter().map(|c| class(c)).collect();
    record.record_revision = 7;
    record.pack = Some(
        medulla_decision_types::decision::capability_designation::PackOrigin {
            connector: "finance".to_string(),
            binding_revision: revision,
            pack_digest: format!("sha256:{}", "12".repeat(32)),
            entry_digest: format!("sha256:{}", "34".repeat(32)),
        },
    );
    record
}

fn target_request(h: &Harness, targets: &[&str]) -> DecideRequest {
    let mut ask = question(&[1.0, 0.0, 0.0], 3, weights(Q32_ONE, 0));
    ask.target_classes = targets.iter().map(|c| class(c)).collect();
    designation_request(&schema_pin(h), ask, DecisionPolicyRef::Default)
}

// spec: MEDULLA-DECISION-ENGINE-R172.12.4
#[tokio::test]
async fn pack_target_refusals() {
    let h = Harness::new().await;
// --- the active adapter re-aims the task embedding before rank ------------

use super::learning_tests::{
    activate, embedded_graph, fit_request, judged_runs, pointer_state, GRAPH,
};
use medulla_decision_types::decision::statistical::retrieval::{PointerMovement, PointerRef};
use medulla_decision_types::decision::statistical::retrieval_adapter::{
    AdapterDirection, QueryAdapterBody,
};
use medulla_server_ops::api::admin::served_adapter::ServedAdapter;

/// One direction at 45 degrees between axes 0 and 1 with gain 1/2: a query
/// along axis 0 is pulled toward axis 1.
fn tilt_body() -> QueryAdapterBody {
    let unit = (std::f64::consts::FRAC_1_SQRT_2 * f64::from(1_u32 << 30)).round() as i32;
    QueryAdapterBody {
        schema_version: 1,
        space_digest: format!("sha256:{}", "ef".repeat(32)),
        dimensions: 3,
        directions: BoundedVec::new(vec![AdapterDirection {
            gain_q16: 1 << 15,
            unit_q30: BoundedVec::new(vec![unit, unit, 0]).unwrap(),
        }])
        .unwrap(),
        training_digest: format!("sha256:{}", "01".repeat(32)),
        n_training: 40,
        revision: 1,
        parent_digest: None,
    }
}

fn applied_tilt() -> crate::decide::stat_designation::AppliedAdapter {
    let digest = format!("sha256:{}", "aa".repeat(32));
    crate::decide::stat_designation::AppliedAdapter {
        served: std::sync::Arc::new(ServedAdapter::for_body(&digest, &tilt_body()).unwrap()),
        revision: 7,
    }
}

fn ranked_ids(record: &StatisticalDecisionRecord) -> Vec<String> {
    ids_of(answer_of(record))
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn adapter_premises(record: &StatisticalDecisionRecord) -> Vec<String> {
    record
        .premises
        .as_slice()
        .iter()
        .filter(|premise| premise.fact.starts_with("adapter:"))
        .map(|premise| premise.fact.clone())
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R172.13
#[tokio::test]
async fn capability_adapter_applies_before_rank() {
    let h = Harness::new().await;
    let pin = schema_pin(&h);
    seed(
        &h,
        vec![
            (
                pack_record("tool-var", &["EquityPosition"], &["ValueAtRisk"], 5),
                vec![1.0, 0.0, 0.0],
            ),
            (
                pack_record("tool-old", &["Position"], &[], 4),
                vec![0.0, 1.0, 0.0],
                record(Some(TENANT), "agent-n", &["cap:x"]),
                vec![1.0, 0.0, 0.0],
            ),
            (
                record(Some(TENANT), "agent-m", &["cap:x"]),
                vec![0.8, 0.6, 0.0],
            ),
        ],
    )
    .await;
    let index = shared_capability_index(&*h.state.read().await);
    let artifacts = |h: &Harness| {
        h.store
            .decision_artifacts_with_prefix(TENANT, "", 10_000)
            .unwrap()
            .len()
    };
    let count = artifacts(&h);
    let held = h.store.capability_rewards(TENANT).unwrap();
    let epoch = epoch_of(&index);

    // An unknown target class refuses and names the class.
    let request = target_request(&h, &["Nowhere"]);
    let error = decide_with_runtime(&h, &request, pack_runtime(index.clone())).unwrap_err();
    assert!(
        error.starts_with("DESIGNATION_UNKNOWN_TARGET_CLASS:") && error.contains("Nowhere"),
        "{error}"
    );

    // A known class no record applies to or executes abstains and names it.
    let request = target_request(&h, &["RiskMethod"]);
    let none = decide_with_runtime(&h, &request, pack_runtime(index.clone()));
    // The var tool executes a subclass of RiskMethod, so the class is served;
    // a class with no component at all is the abstention case.
    assert!(none.is_ok());
    let mut lonely = pack_runtime(index.clone());
    lonely.projection =
        SchemaProjection::new(&lonely.schema_digest, Vec::<(String, String)>::new())
            .with_classes([class("Orphan")]);
    let request = target_request(&h, &["Orphan"]);
    let sealed = decide_with_runtime(&h, &request, lonely).unwrap();
    let StatisticalOutcome::Abstained { reasons } = &sealed.outcome else {
        panic!("an abstention, got {:?}", sealed.outcome);
    };
    let reason = &reasons.as_slice()[0];
    assert_eq!(
        reason.code().as_str(),
        "DESIGNATION_NO_DESIGNATED_COMPONENT"
    );
    assert!(matches!(
        reason,
        medulla_decision_types::decision::AbstainReason::Family { detail, .. }
            if detail.contains(&class("Orphan"))
    ));
    assert_eq!(sealed.resolution_kind, ResolutionKind::Abstention);

    // A record from an older pack revision than the index holds refuses.
    let request = target_request(&h, &["Position"]);
    let error = decide_with_runtime(&h, &request, pack_runtime(index.clone())).unwrap_err();
    assert!(
        error.starts_with("DESIGNATION_STALE_PACK_REVISION:") && error.contains("tool-old"),
        "{error}"
    );

    // Nothing was written by any refusal.
    assert_eq!(epoch_of(&index), epoch);
    assert_eq!(artifacts(&h), count);
    assert_eq!(h.store.capability_rewards(TENANT).unwrap(), held);
}

// spec: MEDULLA-DECISION-ENGINE-R172.12.5
#[tokio::test]
async fn premises_name_subclass_path_and_pack() {
    let h = Harness::new().await;
    seed(
        &h,
        vec![(
            pack_record("tool-var", &["EquityPosition"], &["ValueAtRisk"], 5),
            vec![1.0, 0.0, 0.0],
        )],
    )
    .await;
    let index = shared_capability_index(&*h.state.read().await);
    let request = target_request(&h, &["Position"]);
    let sealed = decide_with_runtime(&h, &request, pack_runtime(index)).unwrap();
    let answer = answer_of(&sealed);
    assert_eq!(ids_of(answer), vec!["tool-var"]);
    let designated = &answer.designations[0];
    assert_eq!(
        designated.eligibility.target_paths,
        vec![vec![class("EquityPosition"), class("Position")]]
    );
    assert_eq!(
        designated.executes,
        [class("ValueAtRisk")].into_iter().collect()
    );
    let pack_digest = format!("sha256:{}", "12".repeat(32));
    let expected = format!(
        "target_designation:component=tool-var;path={}>{};pack_digest={pack_digest};graph_revision=7",
        class("EquityPosition"),
        class("Position")
    );
    let premise = sealed
        .premises
        .as_slice()
        .iter()
        .find(|p| p.fact == expected)
        .unwrap_or_else(|| panic!("no premise {expected}"));
    assert_eq!(premise.subject, "tool-var");
    assert_eq!(
        premise.class,
        medulla_decision_types::decision::PremiseClass::Claim
    );
    assert!(matches!(
        &premise.provenance,
        medulla_decision_types::decision::PremiseProvenance::ConnectorPack {
            connector, binding_revision: 5, pack_digest: digest, ..
        } if connector == "finance" && *digest == pack_digest
    ));
    // Pack facts are claims: the sealed record says so.
    assert_eq!(sealed.evidence_class, EvidenceClass::Claim);
}

// ---- R172.7.2: the index follows committed Agent Library changes ----

/// Embeds `alpha` texts on the first axis and everything else on the second,
/// at the dimension of the commons graph's active generation.
struct AxisEmbedder;

impl super::super::stat_generation_cycle::Reembedder for AxisEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Ok(texts
            .iter()
            .map(|text| {
                if text.contains("alpha") {
                    vec![1.0, 0.0, 0.0]
                } else {
                    vec![0.0, 1.0, 0.0]
                }
            })
            .collect())
    }
}

fn publish_tool(h: &Harness, tenant: &str, id: &str, summary: &str) {
    let mut draft = medulla_persistence_records::agent_component::test_component_draft(tenant, id);
    draft.kind = AgentComponentKind::Tool;
    draft.summary = summary.to_string();
    let nonce = h.nonce.get();
    h.nonce.set(nonce + 1);
    h.store
        .publish_component(
            medulla_agent_types::agent_component::AgentComponentPublishRequest {
                context: medulla_persistence_records::agent_fixtures::mutation_context(
                    &h.store,
                    tenant,
                    &format!("publish-{id}-{nonce}"),
                    nonce,
                    0,
                    "agent-component:publish",
                ),
                component: draft,
                evaluation_receipt_digest: None,
            },
        )
        .unwrap();
}

fn retire_tool(h: &Harness, tenant: &str, id: &str) {
    let nonce = h.nonce.get();
    h.nonce.set(nonce + 1);
    h.store
        .retire_component(
            medulla_agent_types::agent_component::AgentComponentRetireRequest {
                context: medulla_persistence_records::agent_fixtures::mutation_context(
                    &h.store,
                    tenant,
                    &format!("retire-{id}-{nonce}"),
                    nonce,
                    0,
                    "agent-component:retire",
                ),
                component_id: id.to_string(),
            },
        )
        .unwrap();
}

/// Run the feed over `tenant`'s outbox until it is drained.
async fn feed(h: &Harness, tenant: &str) {
    let embedder: std::sync::Arc<dyn super::super::stat_generation_cycle::Reembedder> =
        std::sync::Arc::new(AxisEmbedder);
    let subscribed = std::sync::atomic::AtomicBool::new(false);
    while crate::decide::capability_feed::sweep(&h.state, tenant, &embedder, &subscribed)
        .await
        .unwrap()
    {}
}

/// The entities the asking tenant's next question designates.
async fn designated(h: &Harness, version: &str) -> Vec<String> {
    let mut ask = question(&[1.0, 0.0, 0.0], 8, weights(Q32_ONE, 0));
    ask.query_vector = QueryVector::from_f32(version, &[1.0, 0.0, 0.0]).unwrap();
    let request = designation_request(&schema_pin(h), ask, DecisionPolicyRef::Default);
    match decide(h, request).await {
        Ok(batch) => batch.records.as_slice()[0]
            .answer
            .as_ref()
            .and_then(|answer| answer.capability_designation())
            .map(|answer| {
                answer
                    .designations
                    .iter()
                    .map(|d| d.entity_id.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R172.7.2
#[tokio::test]
async fn committed_library_write_is_visible_to_next_question() {
    let h = Harness::new().await;
    // The commons graph's active generation is the embedding space of the feed.
    super::learning_tests::graph_with(&h, "__commons__", "1", 3, Vec::new()).await;
    let version = super::learning_tests::space_of("1", 3);
    assert!(designated(&h, &version).await.is_empty());

    publish_tool(&h, TENANT, "tool-alpha", "alpha search");
    publish_tool(&h, OTHER_TENANT, "tool-beta-other", "alpha lookup");
    feed(&h, TENANT).await;
    feed(&h, OTHER_TENANT).await;

    // The committed write is visible to the next question, embedded under the
    // active generation, and the other tenant's component is not.
    assert_eq!(
        designated(&h, &version).await,
        vec!["tool-alpha".to_string()]
    );
    let index = shared_capability_index(&*h.state.read().await);
    let held = index
        .shard(Some(&TenantId::new(TENANT).unwrap()))
        .unwrap()
        .snapshot();
    let record = held.record(&OpaqueId::new("tool-alpha").unwrap()).unwrap();
    assert_eq!(
        (record.embedding_version.as_str(), record.dim),
        (version.as_str(), 3)
    );

    // A withdrawn component leaves the index at the next question.
    retire_tool(&h, TENANT, "tool-alpha");
    feed(&h, TENANT).await;
    assert!(designated(&h, &version).await.is_empty());
    let other = index
        .shard(Some(&TenantId::new(OTHER_TENANT).unwrap()))
        .unwrap()
        .snapshot();
    assert!(other
        .record(&OpaqueId::new("tool-beta-other").unwrap())
        .is_some());
}

// spec: MEDULLA-DECISION-ENGINE-R172.7.2
#[tokio::test]
async fn feed_without_an_active_generation_indexes_nothing() {
    let h = Harness::new().await;
    publish_tool(&h, TENANT, "tool-alpha", "alpha search");
    feed(&h, TENANT).await;
    let index = shared_capability_index(&*h.state.read().await);
    let live = index
        .shard(Some(&TenantId::new(TENANT).unwrap()))
        .map(|shard| shard.snapshot().live_count())
        .unwrap_or(0);
    assert_eq!(live, 0, "no embedding generation: nothing is indexed");
    let request = designation_request(
        &pin,
        question(&[1.0, 0.0, 0.0], 2, weights(Q32_ONE, 0)),
        DecisionPolicyRef::Default,
    );

    // No adapter: the base vector ranks, and no adapter premise is recorded.
    let base = decide_with_runtime(&h, &request, runtime_of(index.clone())).unwrap();
    assert_eq!(ranked_ids(&base), vec!["agent-n", "agent-m"]);
    assert!(adapter_premises(&base).is_empty());

    // The active adapter pulls the task embedding toward axis 1 before the
    // index is read, so the order changes, and the premise names the adapter
    // and the revision of its activation.
    let applied = applied_tilt();
    let mut runtime = runtime_of(index.clone());
    runtime.adapter = Some(applied.clone());
    let adapted = decide_with_runtime(&h, &request, runtime).unwrap();
    assert_eq!(ranked_ids(&adapted), vec!["agent-m", "agent-n"]);
    assert_eq!(adapter_premises(&adapted), vec![applied.fact()]);
    assert!(applied.fact().ends_with("@7"));

    // The adapted score is the cosine of the adapted, renormalised vector.
    let moved = crate::decide::family::capability_designation::adapted_query(
        &{
            let mut r = runtime_of(index.clone());
            r.adapter = Some(applied);
            r
        },
        vec![1.0, 0.0, 0.0],
    );
    let norm = moved.iter().map(|v| v * v).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-5, "unit length again: {norm}");
    let expected_cosine = f64::from(0.8 * moved[0] + 0.6 * moved[1]);
    let scored = answer_of(&adapted).designations[0].cosine_q32;
    let got = scored as f64 / Q32_ONE as f64;
    assert!(
        (got - expected_cosine).abs() < 1e-3,
        "{got} vs {expected_cosine}"
    );

    // A query of another width is served unadapted (the base order).
    let mut runtime = runtime_of(index);
    runtime.adapter = Some(applied_tilt());
    let narrow =
        crate::decide::family::capability_designation::adapted_query(&runtime, vec![1.0, 0.0]);
    assert_eq!(narrow, vec![1.0, 0.0]);
}

// spec: MEDULLA-DECISION-ENGINE-R172.13
#[tokio::test]
async fn designation_journey() {
    // The served path: an adapter fitted from cited outcomes and activated
    // through the pointer re-aims the designation query; a rollback serves the
    // base vector again.
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let pin = schema_pin(&h);
    let core = embedded_graph(&h).await;
    let space = core.semantic_store.read().space().unwrap().digest.clone();
    judged_runs(&h, &space).await;
    seed(
        &h,
        vec![
            (
                record(Some(TENANT), "agent-n", &["cap:x"]),
                vec![1.0, 0.0, 0.0],
            ),
            (
                record(Some(TENANT), "agent-p", &["cap:x"]),
                vec![0.0, 1.0, 0.0],
            ),
        ],
    )
    .await;
    let ask = || {
        designation_request_on(
            GRAPH,
            &pin,
            question(&[1.0, 0.9, 0.0], 2, weights(Q32_ONE, 0)),
            DecisionPolicyRef::Default,
        )
    };
    let base = decide(&h, ask()).await.unwrap();
    let base = &base.records.as_slice()[0];
    assert_eq!(ranked_ids(base), vec!["agent-n", "agent-p"]);
    assert!(adapter_premises(base).is_empty());

    let fitted = match super::retrieval_tests::learn(
        &h,
        "decider",
        medulla_decision_types::decision::statistical::retrieval::LearningWrite::FitAdapter {
            request: Box::new(fit_request(&space)),
        },
    )
    .await
    .unwrap()
    {
        medulla_decision_types::decision::statistical::retrieval::LearningRecorded::Fitted(f) => *f,
        other => panic!("fit: {other:?}"),
    };
    assert!(fitted.receipt.passed, "{:?}", fitted.receipt);
    let pointer = || PointerRef::Adapter {
        graph: GRAPH.to_string(),
    };
    pointer_state(
        &h,
        pointer(),
        activate(&fitted.adapter_digest, &fitted.receipt_digest),
    )
    .await
    .unwrap();

    let adapted = decide(&h, ask()).await.unwrap();
    let adapted = &adapted.records.as_slice()[0];
    assert_eq!(ranked_ids(adapted), vec!["agent-p", "agent-n"]);
    let facts = adapter_premises(adapted);
    assert_eq!(facts.len(), 1);
    assert!(
        facts[0].starts_with(&format!("adapter:{}@", fitted.adapter_digest)),
        "{facts:?}"
    );

    pointer_state(&h, pointer(), PointerMovement::Rollback)
        .await
        .unwrap();
    let rolled = decide(&h, ask()).await.unwrap();
    let rolled = &rolled.records.as_slice()[0];
    assert_eq!(ranked_ids(rolled), vec!["agent-n", "agent-p"]);
    assert!(adapter_premises(rolled).is_empty());
}

/// A runtime whose schema holds `cap:x` with `children` direct subclasses.
fn wide_runtime(
    index: std::sync::Arc<medulla_decision_memory::memory::capability_index::CapabilityIndex>,
    children: usize,
) -> DesignationRuntime {
    let digest = format!("sha256:{}", "cd".repeat(32));
    let edges: Vec<(String, String)> = (0..children)
        .map(|n| (format!("cap:sub{n:03}"), "cap:x".to_string()))
        .collect();
    DesignationRuntime {
        index,
        projection: SchemaProjection::new(&digest, edges),
        schema_digest: digest,
        schema_error: None,
        load_set: None,
        usage_guidance: None,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R650.2.3
#[tokio::test]
async fn closure_projection_is_served_and_bounded_by_its_knob() {
    let h = Harness::new().await;
    let pin = schema_pin(&h);
    seed(&h, fixture_rows()).await;
    let index = shared_capability_index(&*h.state.read().await);
    let mut asked = question(&[1.0, 0.0, 0.0], 2, weights(Q32_ONE, 0));
    asked.required_capabilities = ["cap:x".to_string()].into_iter().collect();

    // The closure of a required capability is the capability and every class
    // below it, sorted, at the question's schema digest.
    let request = designation_request(&pin, asked.clone(), DecisionPolicyRef::Default);
    let sealed = decide_with_runtime(&h, &request, wide_runtime(index.clone(), 70)).unwrap();
    let closure = &answer_of(&sealed).closures[0];
    assert_eq!(closure.capability_id.as_str(), "cap:x");
    assert_eq!(closure.members.len(), 71);
    assert!(closure.members.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(
        closure.ontology_digest,
        format!("sha256:{}", "cd".repeat(32))
    );

    // A policy whose `designation.closure_max_members` is below the closure
    // refuses with the registered code and writes nothing.
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.knobs.insert(
        KnobName::new("designation.closure_max_members"),
        KnobValue::Integer(64),
    );
    let bounded = DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-designation-closure", &policy),
    };
    let artifacts = || {
        h.store
            .decision_artifacts_with_prefix(TENANT, "", 10_000)
            .unwrap()
            .len()
    };
    let before = artifacts();
    let request = designation_request(&pin, asked.clone(), bounded.clone());
    let error = decide_with_runtime(&h, &request, wide_runtime(index.clone(), 70)).unwrap_err();
    assert!(
        error.starts_with("DESIGNATION_CLOSURE_TOO_LARGE:"),
        "{error}"
    );
    assert_eq!(artifacts(), before);
    // A closure at the bound is served.
    let sealed = decide_with_runtime(
        &h,
        &designation_request(&pin, asked, bounded),
        wide_runtime(index, 63),
    )
    .unwrap();
    assert_eq!(answer_of(&sealed).closures[0].members.len(), 64);
}

// spec: MEDULLA-DECISION-ENGINE-R650.4
#[tokio::test]
async fn descriptor_refusals_and_tenant_isolation() {
    use medulla_decision_types::decision::capability_designation::REFUSALS;
    // Both publication codes are in the family refusal table.
    for code in [
        "DESCRIPTOR_SIDE_EFFECT_MISSING",
        "DESCRIPTOR_APPROVAL_CLASS_MISSING",
    ] {
        assert!(REFUSALS.iter().any(|c| c.as_str() == code), "{code}");
    }
    // The publication side (pack projection raising both codes and writing no
    // descriptor) is proven beside it in the pack projection crate:
    // `a_descriptor_without_a_side_effect_or_approval_class_is_refused`.

    // Tenant isolation: the index holds only another tenant's records. The
    // asking tenant gets no designation and nothing names the foreign row.
    let h = Harness::new().await;
    let pin = schema_pin(&h);
    seed(
        &h,
        vec![(
            record(Some(OTHER_TENANT), "agent-foreign", &["cap:x"]),
            vec![1.0, 0.0, 0.0],
        )],
    )
    .await;
    let index = shared_capability_index(&*h.state.read().await);
    for required in [vec![], vec!["cap:x".to_string()]] {
        let mut asked = question(&[1.0, 0.0, 0.0], 4, weights(Q32_ONE, 0));
        asked.required_capabilities = required.into_iter().collect();
        let request = designation_request(&pin, asked, DecisionPolicyRef::Default);
        let sealed = decide_with_runtime(&h, &request, runtime_of(index.clone())).unwrap();
        assert!(sealed
            .answer
            .as_ref()
            .and_then(|answer| answer.capability_designation())
            .is_none_or(|answer| answer.designations.is_empty()));
        let wire = serde_json::to_string(&sealed).unwrap();
        assert!(!wire.contains("agent-foreign"), "{wire}");
    }
    // A question that names another tenant is refused, never answered.
    let mut foreign = designation_request(
        &pin,
        question(&[1.0, 0.0, 0.0], 4, weights(Q32_ONE, 0)),
        DecisionPolicyRef::Default,
    );
    let mut params = foreign.params.as_slice().to_vec();
    if let TypedValue::CapabilityDesignation(ask) = &mut params[0].value {
        ask.question.tenant_id = TenantId::new(OTHER_TENANT).unwrap();
    }
    foreign.params = BoundedVec::new(params).unwrap();
    let error = decide_with_runtime(&h, &foreign, runtime_of(index)).unwrap_err();
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
}
