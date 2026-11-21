//! Graph intake served through `Decide` (MEDULLA-DECISION-ENGINE-R163.3,
//! R163.4): the family resolves each source against the caller's visible graph
//! and concludes a constraint naming each exact revision and digest. One source
//! that does not resolve refuses the whole request with its registered code, and
//! a node of another owner resolves as failed.

use super::*;
use medulla_decision_types::decision::graph_intake::{GraphReference, IntakeSource};
use medulla_decision_types::decision::{EvidenceClass, PremiseClass, PremiseProvenance, ResolutionKind};
use medulla_types::contract::Digest256;

const GRAPH: &str = "kg-graph-intake";
const AUTHORITY: &str = "graph:main";

fn put(core: &medulla_graph_core::graph::GraphCore, id: &str, props: serde_json::Value) {
    core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
}

async fn harness() -> (Harness, Digest256) {
    let h = Harness::with_isolation(reader_isolation("decider", &[GRAPH])).await;
    let digest = Digest256::sha256(b"paper body");
    {
        let mut guard = h.state.write().await;
        guard
            .registry
            .create_graph(
                GRAPH,
                medulla_protocol_types::protocol::GraphType::Team,
                None,
            )
            .unwrap();
        let core = guard.registry.get(GRAPH).unwrap().core.clone();
        let mine = |extra: serde_json::Value| {
            let mut props = extra;
            props["_owner"] = serde_json::json!("decider");
            props["_visibility"] = serde_json::json!("private");
            props
        };
        put(
            &core,
            AUTHORITY,
            mine(serde_json::json!({"type": "SourceAuthority"})),
        );
        put(
            &core,
            "paper:1",
            mine(serde_json::json!({
                "authority": AUTHORITY,
                "revision": 12,
                "content_digest": digest.to_hex(),
            })),
        );
        put(
            &core,
            "paper:norev",
            mine(serde_json::json!({"authority": AUTHORITY})),
        );
        // Another owner's node: hidden from the caller by row-level security.
        put(
            &core,
            "paper:theirs",
            serde_json::json!({
                "authority": AUTHORITY,
                "revision": 3,
                "content_digest": digest.to_hex(),
                "_owner": "other-agent",
                "_visibility": "private",
            }),
        );
    }
    (h, digest)
}

fn graph_source(authority: &str, query: &str, revision: u64, digest: Digest256) -> IntakeSource {
    IntakeSource::Graph {
        graph_ref: GraphReference {
            authority: authority.to_string(),
            query: query.to_string(),
            revision,
            content_digest: digest,
        },
    }
}

fn file_source(path: &str, digest: Digest256) -> IntakeSource {
    IntakeSource::Filesystem {
        path: path.to_string(),
        content_digest: digest,
    }
}

fn intake_request(schema: &ComponentDependency, sources: Vec<IntakeSource>) -> DecideRequest {
    declared_request(
        schema,
        TENANT,
        QuestionKind::GraphIntake,
        "resolve_sources",
        DecisionPolicyRef::Default,
        vec![
            TypedParam {
                name: "intake_sources".to_string(),
                value: TypedValue::IntakeSources(BoundedVec::new(sources).unwrap()),
            },
            TypedParam {
                name: "source_graph".to_string(),
                value: TypedValue::Text(GRAPH.to_string()),
            },
        ],
    )
}

// spec: MEDULLA-DECISION-ENGINE-R163.3
#[tokio::test]
async fn graph_intake_routes() {
    let (h, digest) = harness().await;
    let (schema, _) = publish_route_schema(&h);
    let file_digest = Digest256::sha256(b"a");

    let batch = decide(
        &h,
        intake_request(
            &schema,
            vec![
                graph_source(AUTHORITY, "paper:1", 12, digest),
                file_source("docs/a.md", file_digest),
            ],
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::GraphIntake);
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("resolved sources answer advisory, got {:?}", record.outcome);
    };
    assert_eq!(scores.as_slice()[0].option_id, "resolve_sources");

    // Each source is one premise with its exact revision and digest.
    let facts: Vec<&str> = record.premises.iter().map(|p| p.fact.as_str()).collect();
    assert!(facts.contains(&format!("intake:graph={AUTHORITY}/paper:1@12#{}", digest.to_hex()).as_str()));
    assert!(facts.contains(&format!("intake:file=docs/a.md#{}", file_digest.to_hex()).as_str()));
    assert!(record
        .premises
        .iter()
        .all(|premise| premise.class == PremiseClass::Claim));
    // The node content is never returned.
    assert!(!facts.iter().any(|fact| fact.contains("body")));

    // Another owner's node is not visible: the reference does not resolve.
    let error = decide(
        &h,
        intake_request(&schema, vec![graph_source(AUTHORITY, "paper:theirs", 3, digest)]),
    )
    .await
    .unwrap_err();
    assert!(
        error.starts_with("GRAPH_INTAKE_RESOLUTION_FAILED"),
        "{error}"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R163.4
#[tokio::test]
async fn missing_graph_data_refuses() {
    let (h, digest) = harness().await;
    let (schema, _) = publish_route_schema(&h);
    let refused = |sources: Vec<IntakeSource>, code: &'static str| {
        let (h, schema) = (&h, &schema);
        async move {
            let error = decide(h, intake_request(schema, sources))
                .await
                .expect_err("a refusal");
            assert!(error.starts_with(code), "{code} vs {error}");
        }
    };
    refused(
        vec![graph_source("graph:other", "paper:1", 12, digest)],
        "GRAPH_INTAKE_UNKNOWN_AUTHORITY",
    )
    .await;
    refused(
        vec![graph_source(AUTHORITY, "paper:norev", 1, digest)],
        "GRAPH_INTAKE_MISSING_REVISION",
    )
    .await;
    for stale in [
        graph_source(AUTHORITY, "paper:absent", 12, digest),
        graph_source(AUTHORITY, "paper:1", 11, digest),
        graph_source(AUTHORITY, "paper:1", 12, Digest256::sha256(b"other")),
    ] {
        refused(vec![stale], "GRAPH_INTAKE_RESOLUTION_FAILED").await;
    }
    // One refused source refuses the whole request.
    refused(
        vec![
            file_source("docs/a.md", Digest256::sha256(b"a")),
            graph_source("graph:other", "paper:1", 12, digest),
        ],
        "GRAPH_INTAKE_UNKNOWN_AUTHORITY",
    )
    .await;
}

// spec: MEDULLA-DECISION-ENGINE-R163.5
#[tokio::test]
async fn graph_intake_evidence_binds() {
    let (h, digest) = harness().await;
    let (schema, _) = publish_route_schema(&h);
    let file_digest = Digest256::sha256(b"a");
    let batch = decide(
        &h,
        intake_request(
            &schema,
            vec![
                graph_source(AUTHORITY, "paper:1", 12, digest),
                file_source("docs/a.md", file_digest),
            ],
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let graph_fact = format!("intake:graph={AUTHORITY}/paper:1@12#{}", digest.to_hex());
    let file_fact = format!("intake:file=docs/a.md#{}", file_digest.to_hex());
    let premise = |fact: &str| {
        record
            .premises
            .iter()
            .find(|premise| premise.fact == fact)
            .unwrap_or_else(|| panic!("no premise {fact}"))
    };
    // Both are Claims: resolution proves the stored revision and digest equal the
    // request's, not that the content is true.
    for fact in [&graph_fact, &file_fact] {
        assert_eq!(premise(fact).class, PremiseClass::Claim, "{fact}");
    }
    assert_eq!(
        premise(&graph_fact).provenance,
        PremiseProvenance::GraphRow {
            graph: GRAPH.to_string(),
            graph_version: 12,
        }
    );
    assert_eq!(
        premise(&file_fact).provenance,
        PremiseProvenance::Request {
            field: "intake_sources".to_string(),
        }
    );
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    // The node body is never returned.
    assert!(!record
        .premises
        .iter()
        .any(|premise| premise.fact.contains("body")));
}
