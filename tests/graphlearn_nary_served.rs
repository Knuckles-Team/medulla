//! Served n-ary link prediction (MEDULLA-FEDERATED-QUERY-R086.100.3, R086.100.4): the real
//! `dispatch()` path of `Method::GraphLearnPredictNary` against a row-level-security graph.
//! Alice owns a dense community of `Person` nodes; bob owns nothing there. The model is fitted
//! through `GraphLearnFit`, then an unseen entity is wired into the community.

#![cfg(all(feature = "graphlearn", feature = "security"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use serde_json::json;

use medulla_protocol_types::protocol::{
    GraphLearnParams, GraphSource, Method, Request, Response, ResultPayload,
};
use medulla_server_router::dispatch::request_boundary::dispatch;
use medulla_types::acl::AgentRole;
use medulla_types::graphlearn_nary::NaryRequest;

const SECRET: &str = "served-graphlearn-nary-secret";
const COMMONS_USER_ROLE: &str = "commons-user";
const COMMUNITY: usize = 5;

fn isolation() -> medulla_isolation::isolation::IsolationLayer {
    let mut isolation = common::current_isolation();
    use medulla_types::acl::{Grant, GrantEffect, RbacAction, ResourceSelector, Role};
    isolation.add_role(Role::new(COMMONS_USER_ROLE));
    for action in [RbacAction::Read, RbacAction::Write] {
        isolation.add_grant(Grant {
            role: COMMONS_USER_ROLE.to_string(),
            resource: ResourceSelector::Graph("__commons__".to_string()),
            action,
            effect: GrantEffect::Allow,
        });
    }
    isolation.add_role(Role::new(common::ROOT_ADMIN_ROLE));
    isolation.add_grant(Grant {
        role: common::ROOT_ADMIN_ROLE.to_string(),
        resource: ResourceSelector::Graph("__admin__".to_string()),
        action: RbacAction::Admin,
        effect: GrantEffect::Allow,
    });
    isolation
}

fn req_as(id: u64, agent: &str, method: Method) -> Request {
    common::signed_request_as(SECRET, id, "__commons__", agent, method)
}

fn pack(v: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&v).unwrap()
}

async fn system(state: &test_support::SharedState, id: u64, method: Method) {
    let r = Box::pin(dispatch(
        state,
        common::signed_request(SECRET, id, "__commons__", method),
    ))
    .await;
    assert!(r.error.is_none(), "op {id} failed: {:?}", r.error);
}

fn register(id: u64, actor: &str, agent: &str, role: AgentRole, roles: &[&str]) -> Request {
    common::signed_register_identity_request(common::SignedRegisterIdentity {
        secret: SECRET,
        id,
        graph: "__commons__",
        actor,
        registered_agent: agent,
        role,
        teams: Vec::new(),
        roles: roles.iter().map(|r| r.to_string()).collect(),
    })
}

fn source() -> GraphSource {
    GraphSource {
        node_label: "Person".into(),
        direction: "any".into(),
        relation: None,
        limit: 0,
    }
}

async fn add_person(state: &test_support::SharedState, id: u64, name: &str) {
    system(
        state,
        id,
        Method::AddNode {
            node_id: name.into(),
            properties_msgpack: pack(json!({
                "type": "Person", "_owner": "alice", "_visibility": "private",
            })),
        },
    )
    .await;
}

async fn add_edge(state: &test_support::SharedState, id: u64, a: &str, b: &str) {
    system(
        state,
        id,
        Method::AddEdge {
            source_id: a.into(),
            target_id: b.into(),
            properties_msgpack: pack(json!({"relationship": "KNOWS"})),
        },
    )
    .await;
}

/// Two 5-cliques `a0..a4` and `c0..c4` fitted by alice; `bob-private` belongs to bob.
/// Returns the fitted model blob. `unseen` is wired in after the fit.
async fn seed(state: &test_support::SharedState) -> serde_json::Value {
    let mut id = 1u64;
    let mut next = || {
        id += 1;
        id
    };
    for prefix in ["a", "c"] {
        for i in 0..COMMUNITY {
            add_person(state, next(), &format!("{prefix}{i}")).await;
        }
        for i in 0..COMMUNITY {
            for j in (i + 1)..COMMUNITY {
                add_edge(
                    state,
                    next(),
                    &format!("{prefix}{i}"),
                    &format!("{prefix}{j}"),
                )
                .await;
            }
        }
    }
    system(
        state,
        next(),
        Method::AddNode {
            node_id: "bob-private".into(),
            properties_msgpack: pack(json!({
                "type": "Robot", "_owner": "bob", "_visibility": "private",
            })),
        },
    )
    .await;
    let r = Box::pin(dispatch(
        state,
        register(
            900,
            common::TEST_AGENT,
            "root",
            AgentRole::Agent,
            &[common::ROOT_ADMIN_ROLE],
        ),
    ))
    .await;
    assert!(r.error.is_none(), "root registration failed: {:?}", r.error);
    for (i, agent) in [(901u64, "alice"), (902, "bob")] {
        let r = Box::pin(dispatch(
            state,
            register(i, "root", agent, AgentRole::Agent, &[COMMONS_USER_ROLE]),
        ))
        .await;
        assert!(
            r.error.is_none(),
            "RegisterIdentity {agent} failed: {:?}",
            r.error
        );
    }
    let fit = Box::pin(dispatch(
        state,
        req_as(
            950,
            "alice",
            Method::GraphLearnFit {
                source: source(),
                params: GraphLearnParams::default(),
                writeback: false,
            },
        ),
    ))
    .await;
    let model = json_of(&fit)["model"].clone();
    // The unseen entity, wired into the first community after the fit.
    add_person(state, 960, "unseen").await;
    for (n, peer) in ["a0", "a1", "a2", "a3"].iter().enumerate() {
        add_edge(state, 961 + n as u64, "unseen", peer).await;
    }
    model
}

fn json_of(r: &Response) -> serde_json::Value {
    assert!(
        r.error.is_none(),
        "dispatch error: {:?} {:?}",
        r.error,
        r.error_detail
    );
    match &r.result {
        Some(ResultPayload::Json(v)) => v.clone(),
        other => panic!("expected Json result, got {other:?}"),
    }
}

fn nary(
    id: u64,
    agent: &str,
    model: &serde_json::Value,
    candidates: Vec<Vec<&str>>,
    expected_graph_revision: Option<u64>,
) -> Request {
    req_as(
        id,
        agent,
        Method::GraphLearnPredictNary {
            request: NaryRequest {
                model: model.clone(),
                candidates: candidates
                    .into_iter()
                    .map(|t| t.into_iter().map(String::from).collect())
                    .collect(),
                relation_constraints: vec![],
                top_k: 0,
            },
            source: source(),
            expected_graph_revision,
        },
    )
}

fn refusal_code(r: &Response) -> &str {
    r.error.as_deref().expect("a refusal")
}

async fn node_count(state: &test_support::SharedState, id: u64) -> serde_json::Value {
    let r = Box::pin(dispatch(
        state,
        common::signed_request(SECRET, id, "__commons__", Method::NodeCount),
    ))
    .await;
    serde_json::to_value(&r.result).unwrap()
}

// spec: MEDULLA-FEDERATED-QUERY-R086.100.3
#[tokio::test]
async fn refusals_and_unchanged_graph_revision() {
    let (persist_dir, persistence) = common::tempdir_persistence();
    let state = test_support::state_with(SECRET, isolation(), persist_dir, persistence);
    let model = seed(&state).await;

    // A valid call pins the revision the refusals must leave unchanged.
    let ok = Box::pin(dispatch(
        &state,
        nary(
            1000,
            "alice",
            &model,
            vec![vec!["a0", "unseen", "a1"]],
            None,
        ),
    ))
    .await;
    let before = json_of(&ok);
    let revision = before["hypotheses"][0]["graph_revision"]
        .as_u64()
        .expect("revision");
    let nodes_before = node_count(&state, 1001).await;

    let too_many: Vec<Vec<&str>> = vec![vec!["a0", "a1", "a2"]; 1001];
    let cases: Vec<(&str, Request)> = vec![
        (
            "NARY_ARITY_INVALID",
            nary(1010, "alice", &model, vec![vec!["a0", "a1"]], None),
        ),
        (
            "NARY_CONSTRAINT_INVALID",
            nary(1011, "alice", &model, too_many, None),
        ),
        (
            "NARY_SOURCE_UNRESOLVED",
            nary(1012, "alice", &model, vec![vec!["a0", "ghost", "a1"]], None),
        ),
        (
            "NARY_SOURCE_UNAUTHORIZED",
            nary(
                1013,
                "alice",
                &model,
                vec![vec!["a0", "bob-private", "a1"]],
                None,
            ),
        ),
        (
            "NARY_REVISION_STALE",
            nary(
                1014,
                "alice",
                &model,
                vec![vec!["a0", "a1", "a2"]],
                Some(revision + 1_000),
            ),
        ),
    ];
    for (code, request) in cases {
        let r = Box::pin(dispatch(&state, request)).await;
        assert_eq!(refusal_code(&r), code, "detail: {:?}", r.error_detail);
        assert!(r.result.is_none(), "{code} must carry no hypotheses");
    }

    let after = Box::pin(dispatch(
        &state,
        nary(
            1020,
            "alice",
            &model,
            vec![vec!["a0", "unseen", "a1"]],
            Some(revision),
        ),
    ))
    .await;
    assert_eq!(
        json_of(&after)["hypotheses"][0]["graph_revision"].as_u64(),
        Some(revision)
    );
    assert_eq!(
        node_count(&state, 1021).await,
        nodes_before,
        "refusals never write"
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R086.100.4
#[tokio::test]
async fn nary_hypotheses_are_read_only_and_ordered() {
    let (persist_dir, persistence) = common::tempdir_persistence();
    let state = test_support::state_with(SECRET, isolation(), persist_dir, persistence);
    let model = seed(&state).await;

    let binary = |id| {
        req_as(
            id,
            "alice",
            Method::GraphLearnPredict {
                model: model.clone(),
                source: source(),
                candidate_pairs: vec![("a0".into(), "c0".into())],
                top_k: 5,
                writeback: false,
            },
        )
    };
    let binary_before = json_of(&Box::pin(dispatch(&state, binary(2000))).await);
    let nodes_before = node_count(&state, 2001).await;

    let served = Box::pin(dispatch(
        &state,
        nary(
            2002,
            "alice",
            &model,
            vec![
                vec!["a0", "unseen", "a1"],
                vec!["a1", "a0", "a2"],
                vec!["a0", "c0", "c1"],
            ],
            None,
        ),
    ))
    .await;
    let out = json_of(&served);
    let hypotheses = out["hypotheses"].as_array().expect("hypotheses");
    assert!(
        hypotheses.len() >= 2,
        "intra-community tuples are admitted: {out}"
    );
    let scores: Vec<u64> = hypotheses
        .iter()
        .map(|h| h["score_q16"].as_u64().unwrap())
        .collect();
    assert!(
        scores.windows(2).all(|w| w[0] >= w[1]),
        "descending score order: {scores:?}"
    );
    let revision = hypotheses[0]["graph_revision"].as_u64().unwrap();
    for h in hypotheses {
        assert_eq!(h["graph_revision"].as_u64(), Some(revision));
        assert!(h["model_revision"]
            .as_str()
            .unwrap()
            .starts_with("kanlink:"));
        assert!(h["policy_revision"].is_u64());
        let tuple: Vec<&str> = h["tuple"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap())
            .collect();
        let class = h["evidence_class"].as_str().unwrap();
        let expected = if tuple.contains(&"unseen") {
            "inductive_inference"
        } else {
            "observed_neighbourhood"
        };
        assert_eq!(class, expected, "tuple {tuple:?}");
    }
    assert!(
        hypotheses
            .iter()
            .any(|h| h["tuple"] == json!(["a0", "unseen", "a1"])),
        "the unseen entity is scored in request member order: {out}"
    );

    // Swapping source order changes the digest.
    let swapped = json_of(
        &Box::pin(dispatch(
            &state,
            nary(
                2003,
                "alice",
                &model,
                vec![vec!["a1", "unseen", "a0"]],
                None,
            ),
        ))
        .await,
    );
    let original = json_of(
        &Box::pin(dispatch(
            &state,
            nary(
                2004,
                "alice",
                &model,
                vec![vec!["a0", "unseen", "a1"]],
                None,
            ),
        ))
        .await,
    );
    assert_ne!(swapped["request_digest"], original["request_digest"]);

    // The binary path and the graph are unchanged.
    assert_eq!(
        json_of(&Box::pin(dispatch(&state, binary(2005))).await),
        binary_before
    );
    assert_eq!(node_count(&state, 2006).await, nodes_before);

    // A second tenant (bob) gets no candidates.
    let other = Box::pin(dispatch(
        &state,
        nary(2007, "bob", &model, vec![vec!["a0", "unseen", "a1"]], None),
    ))
    .await;
    assert_eq!(refusal_code(&other), "NARY_SOURCE_UNAUTHORIZED");
    assert!(other.result.is_none());
}
