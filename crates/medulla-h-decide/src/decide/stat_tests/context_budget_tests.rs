//! MEDULLA-DECISION-ENGINE-R029.2.2: a served `ContextBudget` question reads a
//! promoted head's scores as relevance, places every source exactly once and
//! refuses malformed questions with registered codes.
//! MEDULLA-DECISION-ENGINE-R029.2: the partition is complete.

use super::*;
use medulla_decision_types::decision::statistical::declared::{
    DeclaredNumber, DeclaredOption, DeclaredText,
};
use std::collections::BTreeMap;

const A: &str = "tool-a-search";
const B: &str = "tool-b-files";
const C: &str = "tool-c-mail";

fn q32_tokens(tokens: i64) -> i64 {
    tokens << 32
}

/// One declared source with the same summary the route fixture publishes, so
/// the feature matrix is the one the head was fitted on.
fn source(
    id: &str,
    summary: &str,
    tokens: Option<i64>,
    subject: &str,
    class: &str,
) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::new(vec!["eg:capability/retrieval/web-search".to_string()])
            .unwrap(),
        numbers: BoundedVec::new(
            tokens
                .map(|t| {
                    vec![DeclaredNumber {
                        key: "context.tokens".to_string(),
                        q32: q32_tokens(t),
                    }]
                })
                .unwrap_or_default(),
        )
        .unwrap(),
        texts: BoundedVec::new(vec![
            DeclaredText {
                key: "context.authority_class".to_string(),
                text: class.to_string(),
            },
            DeclaredText {
                key: "context.subject".to_string(),
                text: subject.to_string(),
            },
            DeclaredText {
                key: "summary".to_string(),
                text: summary.to_string(),
            },
        ])
        .unwrap(),
    }
}

fn sources(tokens_a: i64) -> Vec<DeclaredOption> {
    vec![
        source(
            A,
            "web search engine for pages",
            Some(tokens_a),
            "web",
            "proof",
        ),
        source(B, "write files to disk", Some(40), "web", "claim"),
        source(C, "send an email message", Some(40), "mail", "claim"),
    ]
}

fn budget_request(
    schema: &ComponentDependency,
    head: Option<ComponentDependency>,
    options: Vec<DeclaredOption>,
    budget: Option<i64>,
    requested: Option<Vec<&str>>,
) -> DecideRequest {
    let mut params = Vec::new();
    if let Some(names) = requested {
        params.push(TypedParam {
            name: "context.requested_sources".to_string(),
            value: TypedValue::IriList(
                BoundedVec::new(names.into_iter().map(str::to_string).collect()).unwrap(),
            ),
        });
    }
    if let Some(budget) = budget {
        params.push(TypedParam {
            name: "context.token_budget".to_string(),
            value: TypedValue::Int(budget),
        });
    }
    params.push(TypedParam {
        name: "needs".to_string(),
        value: TypedValue::IriList(
            BoundedVec::new(vec!["eg:capability/retrieval".to_string()]).unwrap(),
        ),
    });
    params.push(TypedParam {
        name: "query".to_string(),
        value: TypedValue::Text("web search engine".to_string()),
    });
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "context.budget".to_string(),
            kind: QuestionKind::ContextBudget,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(options).unwrap(),
        },
        feature_schema: schema.clone(),
        head,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(params).unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

/// Fit, evaluate and publish the scorer head over the route fixture.
async fn promoted_head(h: &Harness, fixture: &RouteFixture) -> ComponentDependency {
    let data = dataset(&fixture.schema_digest, &fixture.ids, &fixture.values, 400);
    let job: DecisionJobRecord = decode(
        super::super::jobs::handle_decision_fit(
            &h.state,
            2,
            &verified(),
            super::scorer_tests::scorer_fit(fixture, &data),
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
    let passed = super::scorer_tests::evaluated(h, 4, (draft_sha256, draft_length), data).await;
    assert!(passed.passed, "gates: {:?}", passed.failed_gates.as_slice());
    h.publish_test_component(
        "head-context",
        AgentComponentKind::DecisionHead,
        "context relevance head",
        Some(draft.as_ref()),
        Some(passed.receipt_digest.clone()),
    )
    .unwrap()
}

/// `context_source:<id>=<state>` premises of a record, by source id.
fn partition_of(record: &StatisticalDecisionRecord) -> BTreeMap<String, Vec<String>> {
    let mut by_source: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for premise in record.premises.iter() {
        if let Some(rest) = premise.fact.strip_prefix("context_source:") {
            if let Some((id, state)) = rest.split_once('=') {
                by_source
                    .entry(id.to_string())
                    .or_default()
                    .push(state.to_string());
            }
        }
    }
    by_source
}

fn selected_of(record: &StatisticalDecisionRecord) -> Vec<String> {
    let StatisticalOutcome::Advisory { scores, calibrated } = &record.outcome else {
        panic!(
            "expected the selected sources as advisory scores: {:?}",
            record.outcome
        )
    };
    assert!(!calibrated);
    scores.iter().map(|s| s.option_id.clone()).collect()
}

// spec: MEDULLA-DECISION-ENGINE-R029.2
// spec: MEDULLA-DECISION-ENGINE-R029.2.1
#[tokio::test]
async fn every_source_is_selected_or_omitted_once() {
    let h = Harness::new().await;
    let fixture = route_fixture(&h).await;
    let head = promoted_head(&h, &fixture).await;

    // Plenty of budget and one named source the caller cannot read.
    let batch = decide(
        &h,
        budget_request(
            &fixture.schema_pin,
            Some(head.clone()),
            sources(30),
            Some(1_000),
            Some(vec![A, "ghost-source"]),
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::ContextBudget);
    let partition = partition_of(record);
    let mut expected: Vec<&str> = vec![A, B, C, "ghost-source"];
    expected.sort();
    assert_eq!(
        partition.keys().map(String::as_str).collect::<Vec<_>>(),
        expected,
        "every visible source and the named unreadable one appear"
    );
    for (id, states) in &partition {
        assert_eq!(states.len(), 1, "{id} is placed exactly once: {states:?}");
    }
    assert_eq!(partition["ghost-source"], vec!["unauthorized"]);
    let selected = selected_of(record);
    for id in &selected {
        assert_eq!(partition[id], vec!["selected"]);
    }
    for (id, states) in &partition {
        assert_eq!(
            states[0] == "selected",
            selected.contains(id),
            "the premises and the advisory scores agree on {id}"
        );
    }
    assert!(
        selected.contains(&A.to_string()),
        "the head's best source is selected"
    );

    // A budget below the best source's cost: it is omitted over budget, and
    // the total of what is selected never passes the budget.
    let tight = decide(
        &h,
        budget_request(
            &fixture.schema_pin,
            Some(head),
            sources(500),
            Some(100),
            None,
        ),
    )
    .await
    .unwrap();
    let record = &tight.records.as_slice()[0];
    let partition = partition_of(record);
    assert_eq!(partition.len(), 3);
    assert_eq!(partition[A], vec!["over_budget"]);
    let selected = selected_of(record);
    assert!(!selected.contains(&A.to_string()));
    assert!(selected.len() * 40 <= 100);
}

// spec: MEDULLA-DECISION-ENGINE-R029.2.2
#[tokio::test]
async fn refusals_and_unauthorized_sources_leak_nothing() {
    let h = Harness::new().await;
    let fixture = route_fixture(&h).await;
    let head = promoted_head(&h, &fixture).await;
    let ask = |options: Vec<DeclaredOption>, budget: Option<i64>, requested: Option<Vec<&str>>| {
        budget_request(
            &fixture.schema_pin,
            Some(head.clone()),
            options,
            budget,
            requested,
        )
    };

    // CONTEXT_BUDGET_MISSING: no budget, and a budget of zero.
    for budget in [None, Some(0)] {
        let error = decide(&h, ask(sources(30), budget, None))
            .await
            .unwrap_err();
        assert!(error.starts_with("CONTEXT_BUDGET_MISSING"), "{error}");
    }
    // CONTEXT_TOKENS_UNKNOWN: a source with no token cost.
    let mut unknown = sources(30);
    unknown[1] = source(B, "write files to disk", None, "web", "claim");
    let error = decide(&h, ask(unknown, Some(100), None)).await.unwrap_err();
    assert!(error.starts_with("CONTEXT_TOKENS_UNKNOWN"), "{error}");
    assert!(error.contains(B), "{error}");
    // CONTEXT_SOURCE_DUPLICATE: a declared source twice, a name requested twice.
    let mut twice = sources(30);
    twice.insert(
        1,
        source(A, "web search engine for pages", Some(30), "web", "proof"),
    );
    let error = decide(&h, ask(twice, Some(100), None)).await.unwrap_err();
    assert!(error.starts_with("CONTEXT_SOURCE_DUPLICATE"), "{error}");
    let error = decide(&h, ask(sources(30), Some(100), Some(vec![A, A])))
        .await
        .unwrap_err();
    assert!(error.starts_with("CONTEXT_SOURCE_DUPLICATE"), "{error}");

    // A named source the caller cannot read is unauthorized and carries the
    // caller's own string only; nothing else is named, and the counts of the
    // visible sources are the same as without it.
    let plain = decide(&h, ask(sources(30), Some(1_000), None))
        .await
        .unwrap();
    let named = decide(
        &h,
        ask(sources(30), Some(1_000), Some(vec!["other-tenant-doc"])),
    )
    .await
    .unwrap();
    let (plain, named) = (&plain.records.as_slice()[0], &named.records.as_slice()[0]);
    let (plain_parts, named_parts) = (partition_of(plain), partition_of(named));
    assert_eq!(named_parts["other-tenant-doc"], vec!["unauthorized"]);
    let visible = |parts: &BTreeMap<String, Vec<String>>| {
        parts
            .iter()
            .filter(|(id, _)| id.as_str() != "other-tenant-doc" && id.as_str() != "context_sources")
            .map(|(id, state)| (id.clone(), state.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(visible(&plain_parts), visible(&named_parts));
    assert_eq!(selected_of(plain), selected_of(named));
    assert_eq!(
        named_parts.len(),
        plain_parts.len() + 1,
        "only the caller's own named string is added"
    );
}
