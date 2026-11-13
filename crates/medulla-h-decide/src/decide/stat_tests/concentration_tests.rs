//! The `edit_concentration` family through the actual Decide path
//! (MEDULLA-DECISION-ENGINE-R173.3): the served verdict matches the SHACL
//! concentration shape over the same graph, and another tenant's edits are
//! never counted.

use std::time::Instant;

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, QuestionSafety, StatisticalDecisionRecord,
    StatisticalOutcome, StatisticalQuestion, TypedParam, TypedValue,
};
use medulla_decision_types::decision::DecisionPolicyRef;
use medulla_types::contract::BoundedVec;

use super::super::stat_executor::ExecutionContext;
use super::*;

const SHAPES: &str = include_str!("../../../../medulla-core/ontology/harness-v1.shapes.ttl");
const DIMENSION: &str = "dimension:tool-selection";
const CANDIDATE: &str = "edit:candidate";
const POLICY: &str = "policy:concentration";

/// One edit of the fixture graph: id, status, round and owning tenant.
struct Edit(&'static str, &'static str, u64, Option<&'static str>);

fn graph_view(edits: &[Edit], min_shipped: u64) -> medulla_graph_core::graph::GraphView {
    let core = medulla_graph_core::graph::GraphCore::new();
    let put = |id: &str, props: serde_json::Value| {
        core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
    };
    put(DIMENSION, serde_json::json!({}));
    put(
        POLICY,
        serde_json::json!({ "minShipped": min_shipped, "windowRounds": 5, "revision": 1 }),
    );
    for Edit(id, status, round, tenant) in edits {
        let mut props = serde_json::json!({
            "_harness_edit": true,
            "editStatus": status,
            "editRound": round,
        });
        if let Some(tenant) = tenant {
            props["tenant_id"] = serde_json::json!(tenant);
        }
        put(id, props);
        core.add_edge(
            id.to_string(),
            DIMENSION.to_string(),
            rmp_serde::to_vec_named(&serde_json::json!({ "relationship": "targetsDimension" }))
                .unwrap(),
        )
        .unwrap();
    }
    core.analysis_snapshot()
}

fn iri(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Iri(value.to_string()),
    }
}

fn request(pin: &ComponentDependency) -> DecideRequest {
    let option = |id: &str| DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::default(),
        texts: BoundedVec::default(),
    };
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "harness.concentration".to_string(),
            kind: QuestionKind::EditConcentration,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![option("action:keep"), option("action:revert")]).unwrap(),
        },
        feature_schema: pin.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![
            iri("candidate_edit", CANDIDATE),
            iri("concentration_policy", POLICY),
            iri("graph", "harness-edits"),
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn decide_edit(
    h: &Harness,
    pin: &ComponentDependency,
    view: &medulla_graph_core::graph::GraphView,
) -> StatisticalDecisionRecord {
    let ctx = ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: TENANT,
        now_ms: 1_000,
        server_secret: b"decide-stat-test-secret",
        matched: &[],
        designation: None,
    };
    super::super::stat_decide::decide_blocking(
        &ctx,
        &request(pin),
        Some(view),
        "decider".to_string(),
        Instant::now(),
    )
    .unwrap()
    .records
    .as_slice()[0]
        .clone()
}

/// The action option the record's advisory chose (score of one), and its count.
fn verdict(record: &StatisticalDecisionRecord) -> (String, String) {
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("expected an advisory, got {:?}", record.outcome);
    };
    let chosen = scores
        .iter()
        .find(|s| s.option_id.starts_with("action:") && s.score.value != 0)
        .expect("one action scores one");
    let count = scores
        .iter()
        .find(|s| s.option_id.starts_with("count:"))
        .expect("the count option is present");
    (chosen.option_id.clone(), count.option_id.clone())
}

/// The same edits as Turtle, for the SHACL shape.
fn turtle(edits: &[Edit], min_shipped: u64) -> String {
    let mut data = String::from(
        "@prefix core: <https://knuckles-team.github.io/ontology/core#> .\n\
         @prefix ex: <http://example.org/> .\n\
         ex:dim a core:HarnessDimension .\n",
    );
    data.push_str(&format!(
        "ex:policy a core:ConcentrationPolicy ; core:minShipped {min_shipped} ; \
         core:windowRounds 5 ; core:revision 1 .\n"
    ));
    data.push_str(
        "ex:candidate a core:HarnessEdit ; core:editStatus \"proposed\" ; \
         core:editRound 6 ; core:targetsDimension ex:dim .\n",
    );
    for (n, Edit(_, status, round, tenant)) in edits.iter().enumerate() {
        if tenant.is_some() {
            continue;
        }
        data.push_str(&format!(
            "ex:e{n} a core:HarnessEdit ; core:editStatus \"{status}\" ; \
             core:editRound {round} ; core:targetsDimension ex:dim .\n"
        ));
    }
    data
}

fn candidate() -> Edit {
    Edit(CANDIDATE, "proposed", 6, None)
}

async fn pinned(h: &Harness) -> ComponentDependency {
    publish_route_schema(h).0
}

/// Verifies MEDULLA-DECISION-ENGINE-R173.3: the served verdict equals the
/// SHACL shape's verdict over one graph.
// spec: MEDULLA-DECISION-ENGINE-R173.3
#[tokio::test]
async fn decide_matches_shacl() {
    let h = Harness::new().await;
    let pin = pinned(&h).await;
    // Window of round 6 with width 5 is rounds 2..=6; the round-1 edit is outside.
    let cases = [
        (
            vec![
                Edit("edit:a", "shipped", 4, None),
                Edit("edit:b", "shipped", 5, None),
            ],
            "action:revert",
            "count:3",
        ),
        (
            vec![
                Edit("edit:a", "shipped", 4, None),
                Edit("edit:b", "shipped", 1, None),
            ],
            "action:keep",
            "count:2",
        ),
        (vec![], "action:keep", "count:1"),
    ];
    for (mut edits, action, count) in cases {
        edits.push(candidate());
        let record = decide_edit(&h, &pin, &graph_view(&edits, 3));
        assert_eq!(verdict(&record), (action.to_string(), count.to_string()));
        let report = medulla_shacl::validate_turtle(SHAPES, &turtle(&edits, 3)).unwrap();
        assert_eq!(
            !report.conforms,
            action == "action:revert",
            "the shape and Decide disagree for {action}: {:?}",
            report.results
        );
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R173.3: another tenant's edits are not counted.
// spec: MEDULLA-DECISION-ENGINE-R173.3
#[tokio::test]
async fn other_tenant_edits_are_not_counted() {
    let h = Harness::new().await;
    let pin = pinned(&h).await;
    let edits = vec![
        Edit("edit:a", "shipped", 4, None),
        Edit("edit:foreign-1", "shipped", 5, Some("tenant-other")),
        Edit("edit:foreign-2", "shipped", 3, Some("tenant-other")),
        candidate(),
    ];
    let record = decide_edit(&h, &pin, &graph_view(&edits, 3));
    assert_eq!(
        verdict(&record),
        ("action:keep".to_string(), "count:2".to_string())
    );
}
