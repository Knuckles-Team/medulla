//! Served acceptance of the `load_set` question family
//! (MEDULLA-DECISION-ENGINE-R176.3 to R176.5): a `Decide` over the resident
//! capability index returns a bounded, ordered set of tools, skills and agents
//! for the asking tenant only, every refusal is typed and names what is
//! missing, and every chosen component carries its own premise.

use super::*;
use crate::decide::stat_designation::{shared_capability_index, DesignationRuntime};
use crate::decide::stat_load_set::LoadSetRead;
use medulla_decision_outcome_types::decision::load_set::AgentCandidate;
use medulla_decision_types::decision::capability_designation::{
    CapabilityRecord, EntityKind, PackOrigin, SchemaProjection,
};
use medulla_decision_types::decision::load_set::{LoadSetAnswer, LoadSetQuestion, LOAD_SET_PARAM};
use medulla_decision_types::decision::load_set_knobs::KNOBS as LOAD_SET_KNOBS;
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance, ResolutionKind};
use medulla_types::contract::{OpaqueId, TenantId};
use std::collections::BTreeSet;

const VERSION: &str = "emb-v1";
const OTHER_TENANT: &str = "tenant-other";
const GRAPH_REVISION: u64 = 7;

fn pack(connector: &str, revision: u64) -> PackOrigin {
    PackOrigin {
        connector: connector.to_string(),
        binding_revision: revision,
        pack_digest: format!("sha256:{}", "ab".repeat(32)),
        entry_digest: format!("sha256:{}", "cd".repeat(32)),
    }
}

fn record(
    tenant: &str,
    id: &str,
    kind: EntityKind,
    capabilities: &[&str],
    pack: Option<PackOrigin>,
) -> CapabilityRecord {
    CapabilityRecord {
        tenant_id: Some(TenantId::new(tenant).unwrap()),
        entity_id: OpaqueId::new(id).unwrap(),
        entity_kind: kind,
        capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
        ontology_type: None,
        policy_tags: BTreeSet::new(),
        swappable_with: BTreeSet::new(),
        embedding_version: VERSION.to_string(),
        dim: 3,
        record_revision: 1,
        applies_to: BTreeSet::new(),
        executes: BTreeSet::new(),
        pack,
    }
}

/// Three issue tools and a mail tool of two connectors, an issue skill and a
/// tool of another tenant that would win every ranking.
fn fixture_rows() -> Vec<(CapabilityRecord, Vec<f32>)> {
    let issues = || Some(pack("issues-connector", 1));
    let mail = || Some(pack("mail-connector", 1));
    vec![
        (
            record(
                TENANT,
                "tool-a",
                EntityKind::Tool,
                &["cap:issues"],
                issues(),
            ),
            vec![1.0, 0.0, 0.0],
        ),
        (
            record(
                TENANT,
                "tool-b",
                EntityKind::Tool,
                &["cap:issues"],
                issues(),
            ),
            vec![0.95, 0.312_249_9, 0.0],
        ),
        (
            record(TENANT, "tool-c", EntityKind::Tool, &["cap:mail"], mail()),
            vec![0.6, 0.8, 0.0],
        ),
        (
            record(
                TENANT,
                "skill-a",
                EntityKind::Skill,
                &["cap:issues"],
                issues(),
            ),
            vec![0.9, 0.435_889_9, 0.0],
        ),
        (
            record(
                OTHER_TENANT,
                "tool-foreign",
                EntityKind::Tool,
                &["cap:issues"],
                Some(pack("foreign-connector", 1)),
            ),
            vec![1.0, 0.0, 0.0],
        ),
    ]
}

async fn seed(h: &Harness, rows: Vec<(CapabilityRecord, Vec<f32>)>) {
    let index = shared_capability_index(&*h.state.read().await);
    for (record, vector) in rows {
        index.upsert(record, vector).unwrap();
    }
}

fn load_question(required: &[&str], max_tools: u16) -> LoadSetQuestion {
    LoadSetQuestion {
        job_text: "triage the issue queue".to_string(),
        job_embedding: vec![1.0, 0.0, 0.0],
        required_capabilities: required.iter().map(|c| c.to_string()).collect(),
        max_tools,
        max_agents: 4,
        graph: "__commons__".to_string(),
        embedding_version: VERSION.to_string(),
    }
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

fn schema_pin(h: &Harness) -> ComponentDependency {
    h.publish_test_component(
        "schema-load-set",
        AgentComponentKind::FeatureSchema,
        "load set question",
        Some(&score_schema()),
        None,
    )
    .unwrap()
}

fn load_set_request(schema: &ComponentDependency, question: LoadSetQuestion) -> DecideRequest {
    let question_id = "load.set".to_string();
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: question_id.clone(),
            kind: QuestionKind::LoadSet,
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
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![TypedParam {
            name: LOAD_SET_PARAM.to_string(),
            value: TypedValue::LoadSet(Box::new(question)),
        }])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn answer_of(record: &StatisticalDecisionRecord) -> &LoadSetAnswer {
    record
        .answer
        .as_ref()
        .and_then(|answer| answer.load_set())
        .unwrap_or_else(|| panic!("a load set answer, got {:?}", record.outcome))
}

fn runtime_of(
    index: std::sync::Arc<medulla_decision_memory::memory::capability_index::CapabilityIndex>,
    agents: Vec<AgentCandidate>,
) -> DesignationRuntime {
    let digest = format!("sha256:{}", "ab".repeat(32));
    DesignationRuntime {
        index,
        projection: SchemaProjection::new(&digest, Vec::<(String, String)>::new()),
        schema_digest: digest,
        schema_error: None,
        load_set: Some(LoadSetRead {
            agents,
            graph_revision: GRAPH_REVISION,
        }),
        usage_guidance: None,
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

fn agent(iri: &str, tools: &[&str]) -> AgentCandidate {
    AgentCandidate {
        agent_iri: iri.to_string(),
        prompt_iri: format!("{iri}-prompt"),
        model_profile: "profile-small".to_string(),
        tools: tools.iter().map(|t| t.to_string()).collect(),
        skills: BTreeSet::new(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R176.3
#[tokio::test]
async fn served_decide_reaches_load_set() {
    let h = Harness::new().await;
    let pin = schema_pin(&h);
    seed(&h, fixture_rows()).await;

    let batch = decide(&h, load_set_request(&pin, load_question(&[], 32)))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::LoadSet);
    assert_eq!(record.resolution_kind, ResolutionKind::Optimization);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let answer = answer_of(record);
    // Best cosine first; the skill is not a tool; the other tenant's tool,
    // which would rank first, is never read.
    assert_eq!(answer.tools, vec!["tool-a", "tool-b", "tool-c"]);
    assert_eq!(answer.skills, vec!["skill-a"]);
    assert!(answer.agents.is_empty(), "the commons graph holds no agent");
    assert!(!answer.tools.iter().any(|tool| tool == "tool-foreign"));

    // The bound holds: two tools of three.
    let batch = decide(&h, load_set_request(&pin, load_question(&[], 2)))
        .await
        .unwrap();
    assert_eq!(
        answer_of(&batch.records.as_slice()[0]).tools,
        vec!["tool-a", "tool-b"]
    );

    // A required capability puts the best tool of that capability in the set
    // even when the cap would cut it.
    let index = shared_capability_index(&*h.state.read().await);
    let sealed = decide_with_runtime(
        &h,
        &load_set_request(&pin, load_question(&["cap:mail"], 1)),
        runtime_of(index, Vec::new()),
    )
    .unwrap();
    assert_eq!(answer_of(&sealed).tools, vec!["tool-c"]);
}

// spec: MEDULLA-DECISION-ENGINE-R176.3
#[tokio::test]
async fn other_tenant_packs_never_enter_the_set() {
    let h = Harness::new().await;
    let pin = schema_pin(&h);
    seed(&h, fixture_rows()).await;
    let index = shared_capability_index(&*h.state.read().await);

    for required in [&[][..], &["cap:issues"][..]] {
        let sealed = decide_with_runtime(
            &h,
            &load_set_request(&pin, load_question(required, 32)),
            runtime_of(index.clone(), Vec::new()),
        )
        .unwrap();
        let answer = answer_of(&sealed);
        assert!(!answer.tools.iter().any(|tool| tool == "tool-foreign"));
        assert!(!sealed.premises.as_slice().iter().any(|premise| premise
            .fact
            .contains("tool-foreign")
            || matches!(
                &premise.provenance,
                PremiseProvenance::ConnectorPack { connector, .. }
                    if connector == "foreign-connector"
            )));
    }

    // Another tenant's caller cannot ask for this tenant's load set.
    let stranger = VerifiedRequestContext::verified_for_test_in_tenant("decider", OTHER_TENANT);
    let response = super::super::statistical::handle_decide(
        &h.state,
        1,
        &stranger,
        load_set_request(&pin, load_question(&[], 32)),
    )
    .await;
    assert!(
        response
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("ACCESS_DENIED")),
        "{:?}",
        response.error
    );
}

// spec: MEDULLA-DECISION-ENGINE-R176.4
#[tokio::test]
async fn load_set_refusals() {
    let h = Harness::new().await;
    let pin = schema_pin(&h);
    seed(&h, fixture_rows()).await;
    let index = shared_capability_index(&*h.state.read().await);
    let run = |question: LoadSetQuestion| {
        decide_with_runtime(
            &h,
            &load_set_request(&pin, question),
            runtime_of(index.clone(), Vec::new()),
        )
    };

    // PARAMETER_INVALID: no job text and no required capability.
    let mut empty = load_question(&[], 32);
    empty.job_text = "  ".to_string();
    let error = run(empty).unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID:"), "{error}");
    // PARAMETER_INVALID: more required capabilities than max_tools.
    let error = run(load_question(&["cap:issues", "cap:mail", "cap:x"], 2)).unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID:"), "{error}");
    // PARAMETER_INVALID: nothing to rank by without the job embedding.
    let mut no_embedding = load_question(&[], 32);
    no_embedding.job_embedding = Vec::new();
    let error = run(no_embedding).unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID:"), "{error}");

    // LOAD_SET_NO_DESIGNATED_COMPONENT: an abstention naming the capability,
    // and no partial set for the capabilities that do have a component.
    let sealed = run(load_question(&["cap:issues", "cap:missing"], 32)).unwrap();
    let StatisticalOutcome::Abstained { reasons } = &sealed.outcome else {
        panic!("an abstention, got {:?}", sealed.outcome);
    };
    let reason = &reasons.as_slice()[0];
    assert_eq!(reason.code().as_str(), "LOAD_SET_NO_DESIGNATED_COMPONENT");
    assert!(matches!(
        reason,
        medulla_decision_types::decision::AbstainReason::Family { detail, .. }
            if detail.contains("cap:missing")
    ));
    assert_eq!(sealed.resolution_kind, ResolutionKind::Abstention);
    assert!(sealed.answer.is_none(), "no partial set");

    // LOAD_SET_STALE_PACK_REVISION: the index holds a newer revision of the
    // pack a chosen tool came from.
    index
        .upsert(
            record(
                TENANT,
                "tool-new",
                EntityKind::Tool,
                &["cap:other"],
                Some(pack("issues-connector", 2)),
            ),
            vec![0.0, 0.0, 1.0],
        )
        .unwrap();
    let error = run(load_question(&["cap:issues"], 32)).unwrap_err();
    assert!(
        error.starts_with("LOAD_SET_STALE_PACK_REVISION:"),
        "{error}"
    );
    assert!(error.contains("issues-connector"), "{error}");
}

// spec: MEDULLA-DECISION-ENGINE-R176.5
#[tokio::test]
async fn premises_per_component() {
    let h = Harness::new().await;
    let pin = schema_pin(&h);
    seed(&h, fixture_rows()).await;
    let index = shared_capability_index(&*h.state.read().await);

    // The best tool of each of two required capabilities fits; the second
    // issue tool is cut: one premise per chosen tool, one excluded.
    let sealed = decide_with_runtime(
        &h,
        &load_set_request(&pin, load_question(&["cap:issues", "cap:mail"], 2)),
        runtime_of(index, vec![agent("agent-triage", &["tool-a"])]),
    )
    .unwrap();
    let answer = answer_of(&sealed);
    assert_eq!(answer.tools, vec!["tool-a", "tool-c"]);
    assert_eq!(answer.skills, vec!["skill-a"]);
    assert_eq!(answer.graph_revision, GRAPH_REVISION);
    assert_eq!(answer.agents.len(), 1);
    assert_eq!(answer.agents[0].agent_iri, "agent-triage");

    let premises = sealed.premises.as_slice();
    for (tool, connector, capability) in [
        ("tool-a", "issues-connector", "cap:issues"),
        ("tool-c", "mail-connector", "cap:mail"),
    ] {
        let own: Vec<_> = premises
            .iter()
            .filter(|premise| {
                premise
                    .fact
                    .starts_with(&format!("load_set:component={tool}/"))
            })
            .collect();
        assert_eq!(own.len(), 1, "one premise for {tool}");
        assert_eq!(own[0].class, PremiseClass::Claim);
        assert!(matches!(
            &own[0].provenance,
            PremiseProvenance::ConnectorPack { connector: c, pack_digest, .. }
                if c == connector
                    && *pack_digest == format!("sha256:{}", "ab".repeat(32))
        ));
        assert!(
            own[0].fact.contains(&format!("/{capability}/")),
            "{}",
            own[0].fact
        );
    }
    assert!(premises
        .iter()
        .any(|premise| premise.fact == "load_set:excluded=1"));
    assert!(premises
        .iter()
        .any(|premise| premise.fact == format!("load_set:graph_revision={GRAPH_REVISION}")));
    for spec in LOAD_SET_KNOBS {
        assert!(
            premises
                .iter()
                .any(|premise| premise.fact.starts_with(&format!("knob:{}=", spec.name))),
            "a premise for {}",
            spec.name
        );
    }
    // The answer carries the same premises it was sealed with.
    assert_eq!(answer.premises.len(), premises.len());
}
