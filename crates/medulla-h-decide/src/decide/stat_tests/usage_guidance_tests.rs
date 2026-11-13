//! Served acceptance of the `usage_guidance` question family
//! (MEDULLA-DECISION-ENGINE-R177.3 to R177.5): a `Decide` over the pack facts
//! of a connector returns the steps of a job class in `:precedes` order, every
//! refusal is typed and names what is missing, and every step cites the
//! triples and outcome counts it rests on.

use super::*;
use crate::decide::stat_designation::{shared_capability_index, DesignationRuntime};
use crate::decide::stat_usage_guidance::{facts_of, Fact, Object, UsageGuidanceRead};
use medulla_decision_stats::decision::usage_guidance::ToolRate;
use medulla_decision_types::decision::capability_designation::SchemaProjection;
use medulla_decision_types::decision::usage_guidance::{
    UsageGuidance, USAGE_GUIDANCE_CONNECTOR_PARAM, USAGE_GUIDANCE_CYCLIC_STEPS,
    USAGE_GUIDANCE_GRAPH_PARAM, USAGE_GUIDANCE_JOB_CLASS_PARAM, USAGE_GUIDANCE_NO_STEPS,
    USAGE_GUIDANCE_UNKNOWN_CONNECTOR,
};
use medulla_decision_types::decision::{PremiseClass, ResolutionKind};
use std::collections::BTreeMap;

const CP: &str = medulla_types::ontology_iri!("connector-pack#");
const CORE: &str = medulla_types::ontology_iri!("core#");
const COMMENT: &str = "http://www.w3.org/2000/01/rdf-schema#comment";
const CONNECTOR: &str = "mcp-server://notes";
const JOB: &str = "urn:job:triage";
const REVISION: u64 = 11;

fn iri(subject: &str, predicate: String, object: &str) -> Fact {
    Fact {
        subject: subject.to_string(),
        predicate,
        object: Object::Iri(object.to_string()),
    }
}

fn cp(name: &str) -> String {
    format!("{CP}{name}")
}

fn core(name: &str) -> String {
    format!("{CORE}{name}")
}

/// A notes connector whose job class has three steps: find and read may swap,
/// and both precede label. `extra` adds or replaces edges.
fn pack(extra: Vec<Fact>) -> Vec<Fact> {
    let mut out = vec![
        iri(CONNECTOR, cp("providesFeature"), "urn:feature:notes"),
        iri("urn:feature:notes", cp("realizedBy"), "tool://notes/find"),
        iri("urn:feature:notes", cp("realizedBy"), "tool://notes/read"),
        iri("urn:feature:notes", cp("realizedBy"), "tool://notes/label"),
        iri(
            "urn:feature:notes",
            cp("realizedBy"),
            "prompt://notes-triage",
        ),
        iri("tool://notes/find", cp("providesCapability"), "cap:find"),
        iri("tool://notes/read", cp("providesCapability"), "cap:read"),
        iri("tool://notes/label", cp("providesCapability"), "cap:label"),
        iri("tool://notes/find", cp("operatesOn"), "urn:class:note"),
        iri("prompt://notes-triage", cp("appliesTo"), "urn:class:note"),
        iri(JOB, core("hasStep"), "urn:step:find"),
        iri(JOB, core("hasStep"), "urn:step:read"),
        iri(JOB, core("hasStep"), "urn:step:label"),
        iri("urn:step:find", core("needsSkill"), "skill://find"),
        iri("urn:step:read", core("needsSkill"), "skill://read"),
        iri("urn:step:label", core("needsSkill"), "skill://label"),
        iri("skill://find", cp("providesCapability"), "cap:find"),
        iri("skill://read", cp("providesCapability"), "cap:read"),
        iri("skill://label", cp("providesCapability"), "cap:label"),
        iri("urn:step:find", core("precedes"), "urn:step:label"),
        iri("urn:step:read", core("precedes"), "urn:step:label"),
        Fact {
            subject: "skill://label".to_string(),
            predicate: COMMENT.to_string(),
            object: Object::Literal("never label a note the reporter can see".to_string()),
        },
    ];
    out.extend(extra);
    out
}

fn tool_id(name: &str) -> String {
    format!("mcp:notes/tool/{name}")
}

fn read_of(triples: Vec<Fact>, rates: &[(&str, u64, u64)]) -> UsageGuidanceRead {
    UsageGuidanceRead {
        facts: facts_of(CONNECTOR, JOB, &triples),
        rates: rates
            .iter()
            .map(|(name, successes, trials)| {
                (
                    tool_id(name),
                    ToolRate {
                        successes: *successes,
                        trials: *trials,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>(),
        graph_revision: REVISION,
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
        "schema-usage-guidance",
        AgentComponentKind::FeatureSchema,
        "usage guidance question",
        Some(&score_schema()),
        None,
    )
    .unwrap()
}

fn guidance_request(
    schema: &ComponentDependency,
    connector: &str,
    job_class: &str,
) -> DecideRequest {
    let question_id = "usage.guidance".to_string();
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: question_id.clone(),
            kind: QuestionKind::UsageGuidance,
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
        params: BoundedVec::new(vec![
            TypedParam {
                name: USAGE_GUIDANCE_CONNECTOR_PARAM.to_string(),
                value: TypedValue::Iri(connector.to_string()),
            },
            TypedParam {
                name: USAGE_GUIDANCE_GRAPH_PARAM.to_string(),
                value: TypedValue::Text("__commons__".to_string()),
            },
            TypedParam {
                name: USAGE_GUIDANCE_JOB_CLASS_PARAM.to_string(),
                value: TypedValue::Iri(job_class.to_string()),
            },
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn runtime_of(h: &Harness, read: UsageGuidanceRead) -> DesignationRuntime {
    let index = futures_lite_block(shared_index(h));
    DesignationRuntime {
        index,
        projection: SchemaProjection::default(),
        schema_digest: String::new(),
        schema_error: None,
        load_set: None,
        usage_guidance: Some(read),
    }
}

async fn shared_index(
    h: &Harness,
) -> std::sync::Arc<medulla_decision_memory::memory::capability_index::CapabilityIndex> {
    shared_capability_index(&*h.state.read().await)
}

fn futures_lite_block<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
}

fn decide_with_read(
    h: &Harness,
    request: &DecideRequest,
    read: UsageGuidanceRead,
) -> Result<StatisticalDecisionRecord, String> {
    let runtime = runtime_of(h, read);
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

fn guidance_of(record: &StatisticalDecisionRecord) -> &UsageGuidance {
    record
        .answer
        .as_ref()
        .and_then(|answer| answer.usage_guidance())
        .unwrap_or_else(|| panic!("a usage guidance answer, got {:?}", record.outcome))
}

// spec: MEDULLA-DECISION-ENGINE-R177.3
#[tokio::test(flavor = "multi_thread")]
async fn served_decide_reaches_guidance() {
    let h = Harness::new().await;
    let pin = schema_pin(&h);
    let request = guidance_request(&pin, CONNECTOR, JOB);

    // No outcomes: the steps that may swap are ordered by tool id, the
    // resolution is a constraint and the evidence a claim.
    let sealed = decide_with_read(&h, &request, read_of(pack(vec![]), &[])).unwrap();
    assert_eq!(sealed.question.kind, QuestionKind::UsageGuidance);
    assert_eq!(sealed.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(sealed.evidence_class, EvidenceClass::Claim);
    let guidance = guidance_of(&sealed);
    let tools: Vec<&str> = guidance.steps.iter().map(|s| s.tool.as_str()).collect();
    assert_eq!(
        tools,
        vec![tool_id("find"), tool_id("read"), tool_id("label")]
    );
    assert_eq!(guidance.connector, CONNECTOR);
    assert_eq!(guidance.job_class, JOB);
    assert_eq!(guidance.graph_revision, REVISION);
    assert_eq!(guidance.prompts, vec!["prompt://notes-triage".to_string()]);
    assert_eq!(
        guidance.cautions,
        vec!["never label a note the reporter can see".to_string()]
    );
    assert!(guidance
        .conditions
        .contains(&"capability:cap:find".to_string()));
    assert!(guidance
        .conditions
        .contains(&"operates_on:urn:class:note".to_string()));
    assert_eq!(guidance.steps[2].inputs_from.len(), 2);
    assert!(guidance.steps[0].precedes == vec![tool_id("label")]);

    // Enough outcomes and a better rate put read before find: a statistical
    // resolution on an observation.
    let rated = read_of(pack(vec![]), &[("find", 1, 10), ("read", 9, 10)]);
    let sealed = decide_with_read(&h, &request, rated).unwrap();
    assert_eq!(sealed.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(sealed.evidence_class, EvidenceClass::Observation);
    let tools: Vec<&str> = guidance_of(&sealed)
        .steps
        .iter()
        .map(|s| s.tool.as_str())
        .collect();
    assert_eq!(
        tools,
        vec![tool_id("read"), tool_id("find"), tool_id("label")]
    );
}

// spec: MEDULLA-DECISION-ENGINE-R177.4
#[tokio::test(flavor = "multi_thread")]
async fn guidance_refusals() {
    let h = Harness::new().await;
    let pin = schema_pin(&h);

    // An unknown connector is a typed refusal naming it.
    let error = decide_with_read(
        &h,
        &guidance_request(&pin, "mcp-server://ghost", JOB),
        read_of(Vec::new(), &[]),
    )
    .unwrap_err();
    assert!(
        error.starts_with(USAGE_GUIDANCE_UNKNOWN_CONNECTOR.as_str()),
        "{error}"
    );

    // A job class with no steps is an abstention with its own code.
    let sealed = decide_with_read(
        &h,
        &guidance_request(&pin, CONNECTOR, "urn:job:unknown"),
        UsageGuidanceRead {
            facts: facts_of(CONNECTOR, "urn:job:unknown", &pack(vec![])),
            rates: BTreeMap::new(),
            graph_revision: REVISION,
        },
    )
    .unwrap();
    assert_eq!(sealed.resolution_kind, ResolutionKind::Abstention);
    assert!(
        format!("{:?}", sealed.outcome).contains(USAGE_GUIDANCE_NO_STEPS.as_str()),
        "{:?}",
        sealed.outcome
    );

    // A cycle is refused, naming the steps on it.
    let cyclic = pack(vec![iri(
        "urn:step:label",
        core("precedes"),
        "urn:step:find",
    )]);
    let error = decide_with_read(
        &h,
        &guidance_request(&pin, CONNECTOR, JOB),
        read_of(cyclic, &[]),
    )
    .unwrap_err();
    assert!(
        error.starts_with(USAGE_GUIDANCE_CYCLIC_STEPS.as_str()),
        "{error}"
    );
    assert!(
        error.contains("urn:step:find") && error.contains("urn:step:label"),
        "{error}"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R177.5
#[tokio::test(flavor = "multi_thread")]
async fn steps_cite_triples() {
    let h = Harness::new().await;
    let pin = schema_pin(&h);
    let rated = read_of(pack(vec![]), &[("find", 1, 10), ("read", 9, 10)]);
    let sealed = decide_with_read(&h, &guidance_request(&pin, CONNECTOR, JOB), rated).unwrap();
    let facts: Vec<(&str, PremiseClass)> = sealed
        .premises
        .as_slice()
        .iter()
        .map(|premise| (premise.fact.as_str(), premise.class))
        .collect();
    let has = |fact: String, class: PremiseClass| facts.contains(&(fact.as_str(), class));
    // Every step cites the triples that placed it.
    for edge in [
        format!("guidance:triple={JOB}/{}/urn:step:find", core("hasStep")),
        format!(
            "guidance:triple=urn:step:find/{}/urn:step:label",
            core("precedes")
        ),
        format!(
            "guidance:triple=urn:step:read/{}/skill://read",
            core("needsSkill")
        ),
    ] {
        assert!(has(edge.clone(), PremiseClass::Claim), "missing {edge}");
    }
    // Outcome counts are observations of the tool.
    assert!(has(
        format!("guidance:outcomes={}/9/10", tool_id("read")),
        PremiseClass::Observation
    ));
    // Static skill text enters as a declared claim.
    assert!(sealed.premises.as_slice().iter().any(
        |premise| premise.fact.starts_with("declared:") && premise.class == PremiseClass::Claim
    ));
    // The knob the ordering read is recorded.
    assert!(sealed
        .premises
        .as_slice()
        .iter()
        .any(|premise| premise.fact.starts_with("knob:usage.min_trials=")));
}
