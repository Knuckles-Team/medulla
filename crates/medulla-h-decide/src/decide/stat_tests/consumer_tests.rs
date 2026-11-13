//! The consumer surface (decide-consumers lane): caller-declared options
//! decide as claims under the declaring principal's visibility, and a logged
//! abstention takes an escalation's resolution as evidence of its own class
//! (EH-037) without ever gaining an option it did not hold.

use super::*;
use medulla_decision_types::{
    decision::statistical::declared::DeclaredNumber,
    decision::statistical::declared::DeclaredOption,
};
use medulla_decision_types::{
    decision::statistical::enrichment_schedule::COST_KEY,
    decision::statistical::enrichment_schedule::EXPECTED_VALUE_KEY,
};
use medulla_decision_types::{
    decision::statistical::log::AbstentionResolution,
    decision::statistical::log::AbstentionResolver, decision::statistical::log::StoredResolution,
};

fn declared(id: &str, score: i64) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::new(vec![DeclaredNumber {
            key: "score".to_string(),
            q32: score,
        }])
        .unwrap(),
        texts: BoundedVec::default(),
    }
}

/// One enrichment candidate's declared option: expected value and cost on
/// the Q32 scale, under the keys `candidates::declared_candidates` reads.
fn enrichment_option(id: &str, expected_value_q32: i64, cost_q32: i64) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        // check_declared requires fact keys strictly sorted; COST_KEY
        // ("enrichment.cost_q32") sorts before EXPECTED_VALUE_KEY
        // ("enrichment.expected_value_q32"), so it must come first here or
        // every call is refused PARAMETER_INVALID at the generic
        // check_declared gate before the MEDULLA-DECISION-ENGINE-R031-specific
        // negative-cost check ever runs.
        numbers: BoundedVec::new(vec![
            DeclaredNumber {
                key: COST_KEY.to_string(),
                q32: cost_q32,
            },
            DeclaredNumber {
                key: EXPECTED_VALUE_KEY.to_string(),
                q32: expected_value_q32,
            },
        ])
        .unwrap(),
        texts: BoundedVec::default(),
    }
}

fn score_schema() -> FeatureSchemaBody {
    FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![number_feature("score")]).unwrap(),
    }
}

fn declared_request(schema: &ComponentDependency, options: Vec<DeclaredOption>) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "au.retrieval-plan".to_string(),
            kind: QuestionKind::RetrievalPlan,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(options).unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::default(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn resolution(
    record_id: &str,
    id: &str,
    option: &str,
    resolver: AbstentionResolver,
) -> DecisionLogOp {
    DecisionLogOp::Resolve {
        tenant_id: TENANT.to_string(),
        resolution: AbstentionResolution {
            record_id: record_id.to_string(),
            resolution_id: id.to_string(),
            option_id: option.to_string(),
            resolver,
        },
    }
}

async fn resolve(h: &Harness, op: DecisionLogOp) -> Result<StoredResolution, String> {
    decode(log_op(h, "decider", op).await)
}

pub(super) async fn declared_abstention(h: &Harness) -> StatisticalDecisionRecord {
    let schema_pin = h
        .publish_test_component(
            "schema-declared",
            AgentComponentKind::FeatureSchema,
            "declared-option features",
            Some(&score_schema()),
            None,
        )
        .unwrap();
    let options = vec![
        declared("plan-deep", 1 << 31),
        declared("plan-hyde", 1 << 32),
    ];
    let batch = decide(h, declared_request(&schema_pin, options))
        .await
        .unwrap();
    batch.records.as_slice()[0].clone()
}

async fn logged_abstention(h: &Harness) -> DecisionLogCommitted {
    let record = declared_abstention(h).await;
    let commit = DecisionLogOp::Commit {
        record: Box::new(record),
        evaluator: None,
    };
    decode(log_op(h, "decider", commit).await).unwrap()
}

/// MEDULLA-DECISION-ENGINE-R029: a retrieval-plan decision refuses a declared
/// option that claims the registry's reserved namespace but does not name a
/// registered plan, through the real served `Decide` path (`handle_decide`,
/// the same handler `Method::Decide` dispatches to) -- not just the typed
/// request's own unit check. The refusal is structural
/// (`candidates::declared_candidates`), so it happens before the feature
/// schema is ever resolved; this request's schema pin is intentionally
/// unpublished. An ordinary declared option outside the reserved namespace
/// (as `declared_abstention`'s "plan-deep"/"plan-hyde" fixtures use) is
/// unaffected -- see `declared_options_decide_as_claims_under_the_declaring_principal`.
#[tokio::test]
async fn an_unregistered_retrieval_plan_is_refused_through_decide() {
    let h = Harness::new().await;
    let unresolved_schema = ComponentDependency {
        component_id: "schema:unused".to_string(),
        kind: AgentComponentKind::FeatureSchema,
        definition_digest: format!("sha256:{}", "0".repeat(64)),
    };
    let response = super::super::statistical::handle_decide(
        &h.state,
        1,
        &verified(),
        declared_request(
            &unresolved_schema,
            vec![declared("retrieval-plan:vector-only", 0)],
        ),
    )
    .await;
    assert!(
        response.result.is_none(),
        "a refused option must not yield a decision"
    );
    assert_eq!(response.error.as_deref(), Some("PARAMETER_INVALID"));
    assert!(
        response.error_detail.as_deref().is_some_and(|detail| {
            detail.contains("UNSUPPORTED_RETRIEVAL_PLAN")
                && detail.contains("retrieval-plan:vector-only")
        }),
        "expected the registry refusal detail, got: {:?}",
        response.error_detail
    );
}

#[tokio::test]
async fn declared_options_decide_as_claims_under_the_declaring_principal() {
    let h = Harness::new().await;
    let record = declared_abstention(&h).await;
    assert!(matches!(
        record.outcome,
        StatisticalOutcome::Abstained { .. }
    ));
    assert!(matches!(
        record.candidate_source,
        medulla_decision_types::decision::CandidateSourceRecord::Declared { .. }
    ));
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let claims = record
        .premises
        .iter()
        .filter(|p| p.fact == "declared_option")
        .count();
    assert_eq!(claims, 2, "every declared option is a named claim premise");
    let FeatureMatrixRef::Inline { values, .. } = &record.inputs.feature_matrix else {
        panic!("inline matrix")
    };
    assert_eq!(values.as_slice(), [1 << 31, 1 << 32]);

    let commit = |r: &StatisticalDecisionRecord| DecisionLogOp::Commit {
        record: Box::new(r.clone()),
        evaluator: None,
    };
    let logged: DecisionLogCommitted = decode(log_op(&h, "decider", commit(&record)).await)
        .expect("an abstention is logged after verify-replay");
    let get = DecisionLogOp::Get {
        tenant_id: TENANT.to_string(),
        record_id: logged.record_id.clone(),
    };
    let mine: Option<DecisionLogEntry> = decode(log_op(&h, "decider", get.clone()).await).unwrap();
    assert!(mine.is_some());
    let theirs: Option<DecisionLogEntry> = decode(log_op(&h, "stranger", get).await).unwrap();
    assert!(
        theirs.is_none(),
        "a declared record is the declarer's alone"
    );

    let unsorted = vec![declared("plan-z", 1), declared("plan-a", 2)];
    let refused = decide(
        &h,
        declared_request(&record.inputs.feature_schema, unsorted),
    )
    .await
    .unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
}

// spec: MEDULLA-DECISION-ENGINE-R037
#[tokio::test]
async fn a_logged_abstention_takes_a_resolution_of_its_resolver_class() {
    let h = Harness::new().await;
    let logged = logged_abstention(&h).await;
    let id = logged.record_id.as_str();
    let model = || AbstentionResolver::Model {
        producer: "au-escalation".to_string(),
        prompt_digest: None,
    };

    let foreign = resolve(&h, resolution(id, "r-0", "plan-invented", model())).await;
    assert!(foreign.unwrap_err().starts_with("PARAMETER_INVALID"));

    let claim = resolve(&h, resolution(id, "r-1", "plan-hyde", model()))
        .await
        .unwrap();
    assert_eq!(
        claim.class,
        EvidenceClass::Claim,
        "a model's answer is a claim"
    );
    let again = resolve(&h, resolution(id, "r-1", "plan-hyde", model()))
        .await
        .unwrap();
    assert_eq!(again, claim, "an identical re-send replays");
    let conflict = resolve(&h, resolution(id, "r-1", "plan-deep", model())).await;
    assert!(conflict.unwrap_err().starts_with("IDEMPOTENCY_CONFLICT"));

    let human = resolve(
        &h,
        resolution(id, "r-2", "plan-deep", AbstentionResolver::Human),
    )
    .await
    .unwrap();
    assert_eq!(human.class, EvidenceClass::Observation);

    let evaluation = DecisionLogOp::Evaluate {
        tenant_id: TENANT.to_string(),
        evaluation: DecisionOutcomeEvaluation {
            record_id: id.to_string(),
            evaluation_id: "e-1".to_string(),
            class: EvidenceClass::Observation,
            selected_agent: "agent".to_string(),
            lease_holder: "worker".to_string(),
            fidelity: OutcomeFidelity::ToolCalls,
            success: Some(true),
            backbone: None,
        },
    };
    let refused = log_op(&h, "decider", evaluation).await;
    assert!(refused.result.is_none());
    assert!(
        refused
            .error_detail
            .as_deref()
            .is_some_and(|detail| detail.contains("executed no option")),
        "an abstention executed nothing an outcome could evaluate"
    );
}

/// EH-039: cost-aware routing reads observed L5 cost; with no L5 accounting
/// store no option carries it, and the decision abstains NAMING the missing
/// fact instead of reading it as zero.
// spec: MEDULLA-DECISION-ENGINE-R039.1
#[tokio::test]
async fn cost_routing_without_l5_accounting_abstains_naming_the_fact() {
    let h = Harness::new().await;
    let schema = FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![FeatureSpec {
            name: "l5.observed_cost".to_string(),
            kind: FeatureKind::Number {
                key: "l5.observed_cost".to_string(),
            },
            missing: MissingValue::Abstain,
        }])
        .unwrap(),
    };
    let schema_pin = h
        .publish_test_component(
            "schema-cost",
            AgentComponentKind::FeatureSchema,
            "cost routing features",
            Some(&schema),
            None,
        )
        .unwrap();
    let options = vec![declared("model-a", 1 << 32), declared("model-b", 1 << 33)];
    let batch = decide(&h, declared_request(&schema_pin, options))
        .await
        .unwrap();
    let StatisticalOutcome::Abstained { reasons } = &batch.records.as_slice()[0].outcome else {
        panic!("abstained")
    };
    assert_eq!(
        reasons.as_slice(),
        [
            medulla_decision_types::decision::AbstainReason::UnknownFact {
                component_id: "model-a".to_string(),
                field: "l5.observed_cost".to_string(),
            }
        ]
    );
}

/// MEDULLA-DECISION-ENGINE-R030: an ingestion-lane decision refuses a declared
/// option outside the fixed lane registry, through the real served `Decide`
/// path (`handle_decide`, the same handler `Method::Decide` dispatches to;
/// see `src/server/dispatch/router/decision_plane.rs`) -- not just the
/// typed request's own unit check. The refusal is structural
/// (`candidates::declared_candidates`), so it happens before the feature
/// schema is ever resolved; this request's schema pin is intentionally
/// unpublished.
#[tokio::test]
async fn an_unregistered_ingestion_lane_is_refused_through_decide() {
    let h = Harness::new().await;
    let unresolved_schema = ComponentDependency {
        component_id: "schema:unused".to_string(),
        kind: AgentComponentKind::FeatureSchema,
        definition_digest: format!("sha256:{}", "0".repeat(64)),
    };
    let request = DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "au.ingestion-lane".to_string(),
            kind: QuestionKind::IngestionLane,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![declared("ingestion-lane:glacial", 0)]).unwrap(),
        },
        feature_schema: unresolved_schema,
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::default(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    };
    let error = decide(&h, request).await.expect_err("must be refused");
    assert!(error.contains("UNSUPPORTED_INGESTION_LANE"), "got: {error}");
}

/// MEDULLA-DECISION-ENGINE-R031: an enrichment-schedule decision refuses a
/// declared candidate with a negative cost, through the real served
/// `Decide` path (`handle_decide`, the same handler `Method::Decide`
/// dispatches to) -- not just the typed request's own unit check. The
/// refusal is structural (`candidates::declared_candidates`), so it
/// happens before the feature schema is ever resolved; this request's
/// schema pin is intentionally unpublished.
#[tokio::test]
async fn a_negative_enrichment_cost_is_refused_through_decide() {
    let h = Harness::new().await;
    let unresolved_schema = ComponentDependency {
        component_id: "schema:unused".to_string(),
        kind: AgentComponentKind::FeatureSchema,
        definition_digest: format!("sha256:{}", "0".repeat(64)),
    };
    let request = DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "au.enrichment-schedule".to_string(),
            kind: QuestionKind::EnrichmentSchedule,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![enrichment_option("work-a", 10, -1)]).unwrap(),
        },
        feature_schema: unresolved_schema,
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::default(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    };
    let error = decide(&h, request).await.expect_err("must be refused");
    assert!(
        error.contains("UNSUPPORTED_ENRICHMENT_COST"),
        "got: {error}"
    );
}

/// MEDULLA-DECISION-ENGINE-R035: `graph.decide()` returns a valid typed choice
/// and its evidence class for declared candidates representative of EVERY
/// routing category the requirement enumerates (model, prompt, skill,
/// tool, harness mode, account mode, sandbox configuration), through the
/// SAME generic `QuestionKind::Route`/declared-candidate mechanism.
///
/// This is a contract test, not a new refusal: the wire protocol carries
/// no field naming WHICH category a `Route` request is choosing among (all
/// 7 share one `QuestionKind`), so there is nothing for a chokepoint to
/// gate on the way `IngestionLane`/`EnrichmentSchedule` can. `graph.decide()`
/// already answers every category through the one mechanism the
/// requirement asks for; this proves it for all 7, closing R035 on the
/// contract-test reading of its acceptance criterion rather than inventing
/// a wire field. With no head published, the deterministic ladder cannot
/// act, so an Ordinary exploration policy supplies the typed choice
/// (`Explored`); its evidence class is always `Claim` (never stronger than
/// a claim for a draw, not a calibrated act).
// spec: MEDULLA-DECISION-ENGINE-R035
#[tokio::test]
async fn graph_decide_answers_every_execution_routing_category() {
    use medulla_decision_types::decision::statistical::execution_routing::ExecutionRoutingCategory;

    let h = Harness::new().await;
    let schema_pin = h
        .publish_test_component(
            "schema-execution-routing",
            AgentComponentKind::FeatureSchema,
            "execution routing features",
            Some(&score_schema()),
            None,
        )
        .unwrap();
    let policy_pin = h.publish_policy(
        "policy-execution-routing-explore",
        &ordinary_exploration_policy(),
    );

    for category in ExecutionRoutingCategory::ALL {
        let (a, b) = match category {
            ExecutionRoutingCategory::Model => ("model:claude", "model:gpt"),
            ExecutionRoutingCategory::Prompt => ("prompt:concise", "prompt:verbose"),
            ExecutionRoutingCategory::Skill => ("skill:search", "skill:summarize"),
            ExecutionRoutingCategory::Tool => ("tool:calculator", "tool:web-search"),
            ExecutionRoutingCategory::HarnessMode => ("harness:autonomous", "harness:guided"),
            ExecutionRoutingCategory::AccountMode => ("account:elevated", "account:standard"),
            ExecutionRoutingCategory::SandboxConfiguration => ("sandbox:relaxed", "sandbox:strict"),
        };
        let request = DecideRequest {
            tenant_id: TENANT.to_string(),
            question: StatisticalQuestion {
                question_id: "route.tools".to_string(),
                kind: QuestionKind::Route,
                safety: QuestionSafety::Ordinary,
            },
            candidates: CandidateSource::Declared {
                options: BoundedVec::new(vec![declared(a, 0), declared(b, 1)]).unwrap(),
            },
            feature_schema: schema_pin.clone(),
            head: None,
            policy: DecisionPolicyRef::Pinned {
                component: policy_pin.clone(),
            },
            params: BoundedVec::default(),
            max_records: None,
            belief_as_of: BoundedVec::default(),
        };
        let batch = decide(&h, request)
            .await
            .unwrap_or_else(|error| panic!("{category:?}: {error}"));
        let record = &batch.records.as_slice()[0];
        assert!(
            matches!(record.outcome, StatisticalOutcome::Explored { .. }),
            "{category:?}: expected a typed choice, got {:?}",
            record.outcome
        );
        assert_eq!(record.evidence_class, EvidenceClass::Claim, "{category:?}");
    }
}

/// Independent evaluations of the committed assembly `record_id` by "evaluator",
/// one per `outcomes` entry (EH-012; reused by the EH-523 slate split tests).
pub(super) async fn evaluate_assembly(
    h: &Harness,
    record_id: &str,
    outcomes: &[bool],
) -> Vec<StoredEvaluation> {
    use medulla_persistence_records::decision_record::tests as v1;
    let evaluator = VerifiedRequestContext::verified_for_test_in_tenant("evaluator", v1::TENANT);
    let mut stored = Vec::new();
    for (n, success) in outcomes.iter().enumerate() {
        let op = DecisionLogOp::Evaluate {
            tenant_id: v1::TENANT.to_string(),
            evaluation: DecisionOutcomeEvaluation {
                record_id: record_id.to_string(),
                evaluation_id: format!("e-{n}"),
                class: EvidenceClass::Observation,
                selected_agent: "agent".to_string(),
                lease_holder: "worker".to_string(),
                fidelity: OutcomeFidelity::FullStep,
                success: Some(*success),
                backbone: None,
            },
        };
        let response = super::super::log::handle_decision_log(&h.state, 30, &evaluator, op).await;
        stored.push(decode(response).expect("a committed assembly is evaluable"));
    }
    stored
}

/// EH-012: an independent evaluation of a committed ASSEMBLY joins its v1
/// record, and the aggregate credits the whole slate (`slate:<graph digest>`
/// under the `assembly` question) -- never its components.
#[tokio::test]
async fn an_assembly_outcome_is_credited_to_its_slate() {
    use medulla_persistence_records::decision_record::tests as v1;

    let h = Harness::new().await;
    v1::seed_library(&h.store);
    let record = v1::decided(&h.store).record;
    let digest = record.inputs.catalog_digest.clone();
    h.store
        .commit_decision_record(
            v1::commit_context(&h.store, "slate-commit", 1),
            &record,
            &digest,
        )
        .unwrap();
    let medulla_decision_types::decision::DecisionOutcome::Solved { graph_digest, .. } =
        &record.outcome
    else {
        panic!("solved")
    };
    let evaluator = VerifiedRequestContext::verified_for_test_in_tenant("evaluator", v1::TENANT);
    let outcomes: Vec<bool> = (0..10).map(|n| n % 2 == 0).collect();
    for stored in evaluate_assembly(&h, &record.record_id, &outcomes).await {
        assert_eq!(stored.producer, evaluator.principal_persistence_id());
    }
    let aggregate = DecisionLogOp::Aggregate {
        request: OutcomeAggregateRequest {
            tenant_id: v1::TENANT.to_string(),
            question_id: Some("assembly".to_string()),
            window: window(),
            attribution: None,
            transfer: None,
        },
    };
    let rows: OutcomeAggregate =
        decode(super::super::log::handle_decision_log(&h.state, 31, &evaluator, aggregate).await)
            .unwrap();
    let slate = format!("slate:{graph_digest}");
    let row = rows
        .rows
        .iter()
        .find(|r| r.option_id == slate)
        .expect("the slate is credited");
    assert_eq!((row.trials, row.successes), (10, 5));
    assert!(
        rows.rows.iter().all(|r| r.option_id.starts_with("slate:")),
        "components are never credited individually"
    );
}

/// The UQL `DECISIONS` source as `who` sees it.
#[cfg(feature = "query")]
fn uql_decisions(h: &Harness, who: &str) -> Vec<String> {
    use super::super::stat_view::DecisionViews;
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(who, TENANT);
    let authority =
        medulla_server_api::carrier::CarrierAuthority::from_verified(&verified).unwrap();
    let views = DecisionViews::of(h.store.clone(), &authority);
    let graph = medulla_graph_core::graph::GraphCore::new().analysis_snapshot();
    let semantic = medulla_semantic_index::semantic::SemanticStore::new();
    let plan = medulla_uql::parse("DECISIONS WHERE outcome = 'abstained'").unwrap();
    let ctx = medulla_exec::exec::PlanCtx::new(&graph, &semantic).with_decisions(&views);
    medulla_exec::exec::execute(&plan, &ctx).unwrap().ids()
}

/// The relation names the decision views materialize for `who`.
#[cfg(feature = "query")]
fn view_names(h: &Harness, who: &str) -> Vec<String> {
    use medulla_server_auth::sql_catalog_acl::ReadOnlyRelations;
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(who, TENANT);
    let authority =
        medulla_server_api::carrier::CarrierAuthority::from_verified(&verified).unwrap();
    super::super::stat_view::DecisionViews::of(h.store.clone(), &authority)
        .materialize()
        .unwrap()
        .into_iter()
        .map(|(schema, _)| schema.name)
        .collect()
}

/// EH-066: the decision record views — the `decisions`/`decision_evaluations`/
/// `decision_resolutions` relations every served SQL surface projects, and the UQL
/// `DECISIONS` source — answer only from what the caller may read.
#[cfg(feature = "query")]
// spec: MEDULLA-DECISION-ENGINE-R064
#[tokio::test]
async fn decision_record_views_answer_only_from_visible_rows() {
    use super::retrieval_tests::sql;
    use serde_json::json;

    let h = Harness::new().await;
    let logged = logged_abstention(&h).await;
    let human = AbstentionResolver::Human;
    resolve(&h, resolution(&logged.record_id, "r-1", "plan-deep", human))
        .await
        .unwrap();
    let mine = "SELECT outcome, source, evidence_class FROM decisions";
    assert_eq!(
        sql(&h, "decider", mine).await,
        vec![vec![json!("abstained"), json!("declared"), json!("claim")]]
    );
    let joined =
        "SELECT r.option_id FROM decision_resolutions r JOIN decisions d USING (record_id)";
    assert_eq!(
        sql(&h, "decider", joined).await,
        vec![vec![json!("plan-deep")]]
    );
    for table in medulla_sql_lang::sql::classify::READ_ONLY_RELATION_NAMES {
        let theirs = sql(&h, "stranger", &format!("SELECT count(*) FROM {table}")).await;
        assert_eq!(
            theirs,
            vec![vec![json!(0)]],
            "{table}: another principal's declared record is not even counted"
        );
    }
    assert_eq!(
        view_names(&h, "decider"),
        medulla_sql_lang::sql::classify::READ_ONLY_RELATION_NAMES
    );

    // UQL `DECISIONS`: the same visibility through the plan source.
    assert_eq!(uql_decisions(&h, "decider"), vec![logged.record_id.clone()]);
    assert!(uql_decisions(&h, "stranger").is_empty());
}

mod topology_question {
    //! The standalone topology `Decide` question (MEDULLA-DECISION-ENGINE-R111.1).

    use super::*;
    use medulla_agent_types::agent_fixtures::topology::{
        facts, requirements, FAN_OUT_JOIN, INDEPENDENT_SUBTASKS,
    };
    use medulla_agent_types::agent_topology::{
        CapacityHeadroom, SchemaAuthority, TopologyAdmission, TopologyInputs, TopologyPlan,
    };
    use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
    use medulla_decision_types::decision::statistical::topology::{
        TopologyQuestion, TOPOLOGY_PARAM, TOPOLOGY_SCHEMA_UNAVAILABLE,
    };
    use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};
    use medulla_decision_types::decision::{AbstainReason, EvidenceClass, ResolutionKind};
    use medulla_mutation_types::capacity_lease::CapacityResourceClass;
    use medulla_persistence_records::agent_graph::tests::publish_topology_template;
    use medulla_server_api::authority_context::VerifiedRequestContext;

    fn read(available: u64) -> TopologyInputs {
        TopologyInputs {
            schema_digest: format!("sha256:{}", "ab".repeat(32)),
            admissions: BoundedVec::new(vec![TopologyAdmission {
                task_class: INDEPENDENT_SUBTASKS.into(),
                topology_class: FAN_OUT_JOIN.into(),
                admit_class: format!("{FAN_OUT_JOIN}Admission"),
                source_key: "swarm-topology".into(),
                authority: SchemaAuthority::Admin,
                axioms: BoundedVec::default(),
            }])
            .unwrap(),
            verify_required_by: BoundedVec::default(),
            headroom: BoundedVec::new(vec![CapacityHeadroom {
                cell_id: "cell-llm".into(),
                class: CapacityResourceClass::LlmGenerator,
                available,
                epoch: 1,
            }])
            .unwrap(),
        }
    }

    fn request(
        schema: &ComponentDependency,
        question: TopologyQuestion,
        tenant: &str,
    ) -> DecideRequest {
        let option = DeclaredOption {
            option_id: question.template.graph_id.clone(),
            classification: BoundedVec::default(),
            numbers: BoundedVec::default(),
            texts: BoundedVec::default(),
        };
        DecideRequest {
            tenant_id: tenant.to_string(),
            question: StatisticalQuestion {
                question_id: "swarm.topology".to_string(),
                kind: QuestionKind::Topology,
                safety: QuestionSafety::Ordinary,
            },
            candidates: CandidateSource::Declared {
                options: BoundedVec::new(vec![option]).unwrap(),
            },
            feature_schema: schema.clone(),
            head: None,
            policy: DecisionPolicyRef::Default,
            params: BoundedVec::new(vec![TypedParam {
                name: TOPOLOGY_PARAM.to_string(),
                value: TypedValue::Topology(Box::new(question)),
            }])
            .unwrap(),
            max_records: None,
            belief_as_of: BoundedVec::default(),
        }
    }

    fn decide(h: &Harness, request: &DecideRequest) -> Result<StatisticalDecisionRecord, String> {
        let ctx = crate::decide::stat_executor::ExecutionContext {
            store: h.store.as_ref(),
            tenant_id: TENANT,
            now_ms: 1_000,
            server_secret: b"decide-stat-test-secret",
            matched: &[],
            designation: None,
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

    fn plan_of(record: &StatisticalDecisionRecord) -> TopologyPlan {
        match record.answer.as_ref() {
            Some(FamilyAnswer::Topology(plan)) => (**plan).clone(),
            other => panic!("a topology answer, got {other:?}"),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R111.1
    #[tokio::test]
    async fn a_standalone_topology_decide_question_returns_a_typed_decision_record() {
        let h = Harness::new().await;
        let mut topology = facts();
        let mut slots = topology.slots.as_slice().to_vec();
        slots[0].node_id = "research".into();
        slots[1].node_id = "write".into();
        topology.slots = BoundedVec::new(slots).unwrap();
        let template = publish_topology_template(&h.store, TENANT, "swarm-template", topology);
        // The seeded fixtures own the low nonces; the schema pin takes a fresh one.
        h.nonce.set(60);
        let schema_pin = h
            .publish_test_component(
                "schema-topology",
                AgentComponentKind::FeatureSchema,
                "topology question",
                Some(&score_schema()),
                None,
            )
            .unwrap();
        let ask = |requirements, read: Option<TopologyInputs>| TopologyQuestion {
            graph: "__commons__".into(),
            template: template.clone(),
            requirements,
            read,
        };

        // Three subtasks on a roomy cell: a proven plan, sealed in the record.
        let record = decide(
            &h,
            &request(&schema_pin, ask(requirements(3), Some(read(8))), TENANT),
        )
        .expect("the topology question is answered");
        let plan = plan_of(&record);
        let widths: Vec<u8> = plan.slots.iter().map(|slot| slot.width).collect();
        assert_eq!(widths, vec![1, 3], "the fan-out slot is sized to demand");
        assert_eq!(record.resolution_kind, ResolutionKind::Optimization);
        // The weakest premise is the publisher's topology claim, so the record is a claim.
        assert_eq!(record.evidence_class, EvidenceClass::Claim);
        let facts_of: Vec<&str> = record.premises.iter().map(|p| p.fact.as_str()).collect();
        for expected in [
            "topology",
            "headroom:llm_generator",
            "topology_cap:max_width=8",
            "topology_cap:max_depth=2",
            "topology_cap:max_rounds=4",
        ] {
            assert!(facts_of.contains(&expected), "{expected} in {facts_of:?}");
        }
        assert!(facts_of.iter().any(|f| f.starts_with("plan_digest:")));
        assert!(
            plan.lease.per_cell.iter().any(|cell| cell.amount > 0),
            "the plan names the lease it would take, and takes none"
        );
        let again = decide(
            &h,
            &request(&schema_pin, ask(requirements(3), Some(read(8))), TENANT),
        )
        .unwrap();
        assert_eq!(record.record_digest, again.record_digest, "replays exactly");

        // Headroom below demand abstains naming the lease row, never a plan.
        let starved = decide(
            &h,
            &request(&schema_pin, ask(requirements(3), Some(read(1))), TENANT),
        )
        .unwrap();
        assert_eq!(starved.resolution_kind, ResolutionKind::Abstention);
        assert!(starved.answer.is_none());
        let StatisticalOutcome::Abstained { reasons } = &starved.outcome else {
            panic!("an abstention, got {:?}", starved.outcome);
        };
        assert!(
            reasons.iter().any(|reason| matches!(
                reason,
                AbstainReason::Infeasible { constraints }
                    if constraints.iter().any(|label| label.starts_with("lease:cell-llm"))
            )),
            "{reasons:?}"
        );

        // A missing schema digest is refused by its code.
        let mut unread = read(8);
        unread.schema_digest = String::new();
        let refused = decide(
            &h,
            &request(&schema_pin, ask(requirements(3), Some(unread)), TENANT),
        )
        .unwrap_err();
        assert!(
            refused.starts_with(TOPOLOGY_SCHEMA_UNAVAILABLE),
            "{refused}"
        );

        // The engine fills the read: a caller-supplied one is refused at the served door,
        // and a graph that cannot be read is TOPOLOGY_SCHEMA_UNAVAILABLE.
        let verified = VerifiedRequestContext::verified_for_test_in_tenant("decider", TENANT);
        let supplied = request(&schema_pin, ask(requirements(3), Some(read(8))), TENANT);
        let response =
            crate::decide::stat_decide::handle_decide(&h.state, 1, &verified, supplied).await;
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("filled by the engine")),
            "{response:?}"
        );
        let missing_graph = request(&schema_pin, ask(requirements(3), None), TENANT);
        let response =
            crate::decide::stat_decide::handle_decide(&h.state, 2, &verified, missing_graph).await;
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with(TOPOLOGY_SCHEMA_UNAVAILABLE)),
            "{response:?}"
        );

        // Cross-tenant negative: another tenant's caller is refused before anything is read.
        let stranger =
            VerifiedRequestContext::verified_for_test_in_tenant("decider", "tenant-other");
        let foreign = request(&schema_pin, ask(requirements(3), None), TENANT);
        let response =
            crate::decide::stat_decide::handle_decide(&h.state, 3, &stranger, foreign).await;
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("ACCESS_DENIED")),
            "{response:?}"
        );
    }
}
