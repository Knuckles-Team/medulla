//! MEDULLA-DECISION-ENGINE-R134.110.3, R134.110.4, R134.110.7, R134.751: the
//! `ToolRelevance` family ranks the tenant's published tools through the tool
//! index and selects a covering subset inside the token budget. `Decide` writes
//! nothing; every refusal returns its code and no tools; another tenant's
//! tools and roles never take part.

use super::*;
use crate::decide::tool_index::ToolIndexCache;
use medulla_agent_types::agent_component::{AgentComponentDraft, AgentComponentFacts, ToolEffect};
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::tool_relevance::ToolAssignmentRequest;
use medulla_decision_types::decision::{AbstainReason, PremiseClass, ResolutionKind};
use medulla_persistence_records::agent_component::test_component_draft;

const FOREIGN: &str = "tenant-foreign";
const TASK: &str = "export the quarterly report";

fn tool_draft(
    tenant: &str,
    id: &str,
    name: &str,
    summary: &str,
    tokens: u32,
    capabilities: &[&str],
) -> AgentComponentDraft {
    let mut draft = test_component_draft(tenant, id);
    draft.summary = summary.to_string();
    draft.facts = AgentComponentFacts::Tool {
        effect: ToolEffect::Read,
        required_scopes: Vec::new(),
        input_schema_digest: None,
        output_schema_digest: None,
        read_only_hint: None,
        destructive_hint: None,
        idempotent_hint: None,
        open_world_hint: None,
        modalities: Default::default(),
        cost: None,
        latency_declared: None,
        side_effects: None,
        declared_by: None,
        schema_tokens: Some(tokens),
        read_only_unproven: false,
    };
    draft.declared_capabilities = capabilities.iter().map(|cap| cap.to_string()).collect();
    draft
        .attributes
        .insert("name".to_string(), name.to_string());
    draft
}

fn publish_tool(
    h: &Harness,
    tenant: &str,
    id: &str,
    name: &str,
    summary: &str,
) -> ComponentDependency {
    h.commit_in(
        tenant,
        tool_draft(tenant, id, name, summary, 100, &[]),
        None,
    )
    .unwrap()
}

/// Six tools of which one answers `TASK`.
fn publish_catalog(h: &Harness, tenant: &str) {
    for (id, name, summary) in [
        (
            "tool-export",
            "export report",
            "export the quarterly sales report to a file",
        ),
        (
            "tool-mail",
            "send email",
            "send an email message to a recipient",
        ),
        ("tool-weather", "weather lookup", "forecast for a city"),
        (
            "tool-calendar",
            "calendar entry",
            "create a meeting invitation",
        ),
        (
            "tool-translate",
            "translate text",
            "convert prose between languages",
        ),
        (
            "tool-resize",
            "resize image",
            "scale a picture to a new size",
        ),
    ] {
        publish_tool(h, tenant, id, name, summary);
    }
}

fn assignment(
    role: &str,
    top_k: u32,
    budget: u32,
    capabilities: &[&str],
    embedding: Option<Vec<i64>>,
) -> TypedParam {
    TypedParam {
        name: "tool_assignment".to_string(),
        value: TypedValue::ToolAssignment(Box::new(ToolAssignmentRequest {
            schema_version: 1,
            task_text: TASK.to_string(),
            task_embedding: embedding.map(|values| BoundedVec::new(values).unwrap()),
            agent_role_iri: role.to_string(),
            top_k,
            context_budget_tokens: budget,
            required_capabilities: BoundedVec::new(
                capabilities.iter().map(|cap| cap.to_string()).collect(),
            )
            .unwrap(),
        })),
    }
}

fn relevance_request(
    h: &Harness,
    tenant: &str,
    policy: DecisionPolicyRef,
    param: TypedParam,
) -> DecideRequest {
    let schema = publish_route_schema_in(h, tenant);
    let mut request = request(&schema, None, policy, QuestionSafety::Ordinary);
    request.tenant_id = tenant.to_string();
    request.question.kind = QuestionKind::ToolRelevance;
    request.params = BoundedVec::new(vec![param]).unwrap();
    request
}

fn ask_in(
    h: &Harness,
    tenant: &str,
    request: DecideRequest,
) -> Result<StatisticalDecisionRecord, String> {
    let ctx = super::super::stat_executor::ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: tenant,
        now_ms: 1_000,
        server_secret: b"decide-stat-test-secret",
        matched: &[],
    };
    super::super::stat_decide::decide_blocking(
        &ctx,
        &request,
        None,
        "decider".to_string(),
        std::time::Instant::now(),
    )
    .map(|batch| batch.records.as_slice()[0].clone())
}

fn ranked(record: &StatisticalDecisionRecord) -> Vec<String> {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => scores
            .iter()
            .map(|option| option.option_id.clone())
            .collect(),
        other => panic!("expected a ranking, got {other:?}"),
    }
}

fn abstain_codes(record: &StatisticalDecisionRecord) -> Vec<String> {
    match &record.outcome {
        StatisticalOutcome::Abstained { reasons } => reasons
            .iter()
            .map(|reason| match reason {
                AbstainReason::Family { code, .. } => code.as_str().to_string(),
                other => panic!("unexpected abstain reason {other:?}"),
            })
            .collect(),
        other => panic!("expected an abstention, got {other:?}"),
    }
}

fn pinned_max_tools(h: &Harness, max: i64) -> DecisionPolicyRef {
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.knobs.insert(
        KnobName::new("tool_subset.max_tools"),
        KnobValue::Integer(max),
    );
    DecisionPolicyRef::Pinned {
        component: h.publish_policy(&format!("policy-max-tools-{max}"), &policy),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R134.110.3
#[tokio::test]
async fn catalog_change_reorders_the_ranking() {
    let h = Harness::new().await;
    publish_catalog(&h, TENANT);
    // Another tenant's better match never appears in this tenant's ranking.
    publish_tool(
        &h,
        FOREIGN,
        "tool-foreign",
        "quarterly report export",
        "export the quarterly report export report",
    );
    let ask = |h: &Harness| {
        ask_in(
            h,
            TENANT,
            relevance_request(
                h,
                TENANT,
                DecisionPolicyRef::Default,
                assignment("", 10, 1000, &[], None),
            ),
        )
        .unwrap()
    };
    let before = ask(&h);
    assert_eq!(ranked(&before), vec!["tool-export".to_string()]);
    assert_eq!(before.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(before.evidence_class, EvidenceClass::Claim);
    let cache = ToolIndexCache::of(&h.store);
    assert_eq!(cache.rebuilds(), 1);
    // The same generation reuses the cached index.
    assert_eq!(ranked(&ask(&h)), vec!["tool-export".to_string()]);
    assert_eq!(cache.rebuilds(), 1);

    // A better match published after the first answer reorders the ranking.
    publish_tool(
        &h,
        TENANT,
        "tool-quarterly",
        "quarterly report export",
        "export the quarterly report export report",
    );
    let after = ask(&h);
    assert_eq!(
        ranked(&after),
        vec!["tool-quarterly".to_string(), "tool-export".to_string()]
    );
    assert_eq!(after.evidence_class, EvidenceClass::Claim);
    assert_eq!(cache.rebuilds(), 2);
    // Each returned tool carries its Claim premise and the catalog digest is
    // a Definition premise (R134.110.5 evidence the family seals).
    let facts: Vec<&str> = after
        .premises
        .iter()
        .map(|premise| premise.fact.as_str())
        .collect();
    assert!(facts
        .iter()
        .any(|fact| fact.starts_with("tool_rank:tool=tool-quarterly;")
            && fact.contains("fused_rank=1")));
    assert!(after.premises.iter().any(|premise| {
        premise.class == PremiseClass::Definition && premise.fact.starts_with("catalog_digest=")
    }));
}

// spec: MEDULLA-DECISION-ENGINE-R134.110.4
#[tokio::test]
async fn each_refusal_returns_its_code_and_no_tools() {
    let h = Harness::new().await;
    let default = || DecisionPolicyRef::Default;
    let ask = |h: &Harness, tenant: &str, policy: DecisionPolicyRef, param: TypedParam| {
        ask_in(h, tenant, relevance_request(h, tenant, policy, param))
    };

    // EMPTY_CATALOG: the tenant holds no tool.
    let record = ask(&h, TENANT, default(), assignment("", 10, 1000, &[], None)).unwrap();
    assert_eq!(abstain_codes(&record), vec!["EMPTY_CATALOG".to_string()]);

    publish_catalog(&h, TENANT);
    // BUDGET_BELOW_MINIMUM: every tool costs 100 tokens.
    let record = ask(&h, TENANT, default(), assignment("", 10, 99, &[], None)).unwrap();
    assert_eq!(
        abstain_codes(&record),
        vec!["BUDGET_BELOW_MINIMUM".to_string()]
    );

    // UNKNOWN_ROLE: a toolset of another tenant is no role here.
    h.commit_in(
        FOREIGN,
        {
            let mut draft = test_component_draft(FOREIGN, "role-foreign");
            draft.kind = AgentComponentKind::Toolset;
            draft.facts = AgentComponentFacts::Toolset {
                transport: medulla_agent_types::agent_component::ToolsetTransport::Function,
            };
            draft
        },
        None,
    )
    .unwrap();
    for role in ["role-foreign", "role-nobody"] {
        let error = ask(&h, TENANT, default(), assignment(role, 10, 1000, &[], None)).unwrap_err();
        assert!(error.contains("UNKNOWN_ROLE"), "{error}");
    }

    // EMBEDDING_DIMENSION_MISMATCH: a tool embedding of 2 components against a
    // task embedding of 3.
    let mut draft = tool_draft(
        TENANT,
        "tool-embedded",
        "embedded export",
        "export report",
        100,
        &[],
    );
    draft
        .attributes
        .insert("embedding_q32".to_string(), "4294967296,0".to_string());
    h.commit(draft, None).unwrap();
    let error = ask(
        &h,
        TENANT,
        default(),
        assignment("", 10, 1000, &[], Some(vec![1 << 32, 0, 0])),
    )
    .unwrap_err();
    assert!(error.contains("EMBEDDING_DIMENSION_MISMATCH"), "{error}");

    // CATALOG_TOO_LARGE: one tool above the pinned bound.
    let policy = pinned_max_tools(&h, 100);
    for index in 0..95 {
        publish_tool(
            &h,
            TENANT,
            &format!("tool-fill-{index}"),
            "filler",
            "filler tool",
        );
    }
    // 6 + 1 + 95 = 102 visible tools against a bound of 100.
    let error = ask(&h, TENANT, policy, assignment("", 10, 1000, &[], None)).unwrap_err();
    assert!(error.contains("CATALOG_TOO_LARGE"), "{error}");

    // The refusals wrote nothing: no decision-log row of any kind exists.
    assert_eq!(
        h.store
            .decision_artifacts_with_prefix(TENANT, "run_record/", 100)
            .unwrap()
            .len(),
        0
    );
}

// spec: MEDULLA-DECISION-ENGINE-R134.751
#[tokio::test]
async fn catalog_above_the_cap_refuses_and_cap_size_is_served() {
    let h = Harness::new().await;
    // The smallest bound the knob allows keeps the fixture small; the nightly
    // bench group `decide/tool_relevance_cap` serves the default cap of 10000.
    let pinned = pinned_max_tools(&h, 100);
    let policy = || pinned.clone();
    publish_catalog(&h, TENANT);
    for index in 0..94 {
        publish_tool(
            &h,
            TENANT,
            &format!("tool-fill-{index}"),
            "filler",
            "filler tool",
        );
    }
    // Exactly `tool_subset.max_tools` (100) tools are served and ranked.
    let record = ask_in(
        &h,
        TENANT,
        relevance_request(&h, TENANT, policy(), assignment("", 10, 1000, &[], None)),
    )
    .unwrap();
    assert_eq!(ranked(&record), vec!["tool-export".to_string()]);
    // One tool more is refused with CATALOG_TOO_LARGE.
    publish_tool(&h, TENANT, "tool-fill-over", "filler", "filler tool");
    let error = ask_in(
        &h,
        TENANT,
        relevance_request(&h, TENANT, policy(), assignment("", 10, 1000, &[], None)),
    )
    .unwrap_err();
    assert!(error.contains("CATALOG_TOO_LARGE"), "{error}");
}

// Covers the retired and republished cases of R134.110.7. The connector-pack
// `Withdrawn` case needs a pack import fixture and is not bound yet, so this
// test carries no `spec:` marker.
#[tokio::test]
async fn index_follows_library_generation_and_hides_invisible_tools() {
    let h = Harness::new().await;
    let ask = |h: &Harness| {
        ask_in(
            h,
            TENANT,
            relevance_request(
                h,
                TENANT,
                DecisionPolicyRef::Default,
                assignment("", 10, 1000, &[], None),
            ),
        )
        .unwrap()
    };
    let generation = |h: &Harness| -> u64 {
        let index = ToolIndexCache::of(&h.store)
            .get(&h.store, TENANT, 10_000)
            .unwrap();
        index.generation
    };
    publish_catalog(&h, TENANT);
    // Another tenant's tool changes neither this tenant's generation nor result.
    publish_tool(
        &h,
        FOREIGN,
        "tool-foreign",
        "quarterly report export",
        "export the quarterly report export report",
    );
    let first = generation(&h);
    assert_eq!(first, 6, "six tools at revision 1");
    assert_eq!(ranked(&ask(&h)), vec!["tool-export".to_string()]);

    // A retired tool leaves the ranking and the catalog digest; the generation
    // still grows because the tombstone is a new revision.
    let digest_before = ToolIndexCache::of(&h.store)
        .get(&h.store, TENANT, 10_000)
        .unwrap()
        .catalog_digest
        .clone();
    h.retire_in(TENANT, "tool-export", 1).unwrap();
    let retired = generation(&h);
    assert!(retired > first);
    let record = ask(&h);
    assert_eq!(abstain_codes(&record), vec!["NO_RELEVANT_TOOL".to_string()]);
    let digest_after = ToolIndexCache::of(&h.store)
        .get(&h.store, TENANT, 10_000)
        .unwrap()
        .catalog_digest
        .clone();
    assert_ne!(digest_before, digest_after);

    // Republishing the capability under a new id raises the generation again
    // and the tool ranks.
    publish_tool(
        &h,
        TENANT,
        "tool-export-v2",
        "export report",
        "export the quarterly sales report to a file",
    );
    assert!(generation(&h) > retired);
    assert_eq!(ranked(&ask(&h)), vec!["tool-export-v2".to_string()]);
    let rebuilds = ToolIndexCache::of(&h.store).rebuilds();
    let _ = ask(&h);
    assert_eq!(
        ToolIndexCache::of(&h.store).rebuilds(),
        rebuilds,
        "no rebuild at an unchanged generation"
    );
}
