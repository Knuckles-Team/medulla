//! The served `DecisionLog` `PolicyActivation` op (MEDULLA-ORCH-VOCAB-R806): a review is
//! recorded against the indexed candidates, and an activation names the receipt it rests on.
//! A receipt no review holds, another repository's and another tenant's all refuse before the
//! pointer moves.

use medulla_decision_types::decision::statistical::log::DecisionLogOp;
use medulla_decision_types::decision::statistical::retrieval::{LearningRecorded, LearningWrite};
use medulla_h_decide::decide::stat_test_support::{decode, Harness, TENANT};
use medulla_protocol_types::protocol::{Method, Request, Response};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::Digest256;
use medulla_types::policy_document::{
    PolicyActivated, PolicyBinding, PolicyChange, PolicyReview,
};
use serde_json::json;

const OTHER_TENANT: &str = "tenant-other";
/// The graph the served harness registers and the review is recorded in: the registered
/// commons graph constant, not a literal.
const GRAPH: &str = medulla_identity_types::tenancy::COMMONS_GRAPH;
const REPO: &str = "org/repo";
const PATH: &str = ".specify/memory/constitution.md";
const SCOPE: &str = "https://example.invalid/class/Repo";

fn sha256_hex(text: &str) -> String {
    Digest256::sha256(text.as_bytes()).to_hex()
}

async fn dispatch_as(h: &Harness, tenant: &str, method: Method) -> Response {
    let method = rmp_serde::from_slice(&rmp_serde::to_vec_named(&method).unwrap()).unwrap();
    Box::pin(
        medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
            &h.state,
            Request {
                id: 806,
                graph: GRAPH.into(),
                auth_token: String::new(),
                agent_id: Some("decider".into()),
                method,
                trace_id: None,
            },
            VerifiedRequestContext::verified_for_test_with_scopes(
                "decider",
                tenant,
                &["kg:read", "kg:write", "agent:decision-write", "agent:decision-read"],
            ),
        ),
    )
    .await
}

/// Index one candidate node of the set `set` into the request graph.
async fn put_candidate(h: &Harness, rule: &str, set: &str) {
    let properties = json!({
        "type": "PolicyCandidate",
        "tenant_id": TENANT,
        "repo": REPO,
        "path": PATH,
        "status": "proposed",
        "rule_id": format!("policy-{rule}"),
        "scope": SCOPE,
        "severity": "block",
        "clause_digest": sha256_hex(rule),
        "candidate_set_digest": set,
        "commit": "c0ffee",
        "blob_digest": "ab".repeat(32),
        "start_line": 3,
        "end_line": 3,
        "grammar_revision": 1,
    });
    let response = dispatch_as(
        h,
        TENANT,
        Method::AddNode {
            node_id: format!("policycandidate:{REPO}:policy-{rule}"),
            properties_msgpack: rmp_serde::to_vec_named(&properties).unwrap(),
        },
    )
    .await;
    assert!(response.error.is_none(), "seed {rule}: {response:?}");
}

fn review(set: &str, seed: &str, expected: Option<&str>, reviewer: &str) -> PolicyReview {
    PolicyReview {
        binding: PolicyBinding {
            repository: REPO.to_string(),
            path: PATH.to_string(),
            commit: "c0ffee".to_string(),
            blob_digest: "ab".repeat(32),
            candidate_set_digest: set.to_string(),
            review_receipt_digest: format!("sha256:{}", sha256_hex(seed)),
            // The generation of no invariants: the digest of the empty text.
            invariant_generation: format!("sha256:{}", sha256_hex("")),
            expected_policy_revision: expected.map(str::to_string),
        },
        author: "alice".to_string(),
        reviewer: reviewer.to_string(),
    }
}

async fn log(h: &Harness, tenant: &str, op: DecisionLogOp) -> Response {
    dispatch_as(h, tenant, Method::DecisionLog { op: Box::new(op) }).await
}

fn activation(
    tenant: &str,
    change: PolicyChange,
    source: &str,
    receipt: &str,
) -> DecisionLogOp {
    DecisionLogOp::PolicyActivation {
        tenant_id: tenant.to_string(),
        change,
        source: source.to_string(),
        receipt_digest: receipt.to_string(),
    }
}

// spec: MEDULLA-ORCH-VOCAB-R806
#[tokio::test]
async fn activation_requires_a_matching_receipt() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let set = {
        let mut digests = vec![sha256_hex("no-secrets"), sha256_hex("no-force-push")];
        digests.sort();
        sha256_hex(&digests.join("\n"))
    };
    put_candidate(&h, "no-secrets", &set).await;
    put_candidate(&h, "no-force-push", &set).await;
    let reviewed = review(&set, "served", None, "bob");
    let receipt = reviewed.binding.review_receipt_digest.clone();

    // No review holds the receipt: the activation refuses and nothing moves.
    let missing: Result<PolicyActivated, String> = decode(
        log(&h, TENANT, activation(TENANT, PolicyChange::Create, REPO, &receipt)).await,
    );
    assert!(missing.unwrap_err().starts_with("POLICY_REVIEW_MISSING"));

    // The review is recorded once against the indexed candidates.
    let recorded: LearningRecorded = decode(
        log(
            &h,
            TENANT,
            DecisionLogOp::Learn {
                tenant_id: TENANT.to_string(),
                write: LearningWrite::RecordPolicyReview {
                    graph: GRAPH.to_string(),
                    review: Box::new(reviewed.clone()),
                },
            },
        )
        .await,
    )
    .expect("the review records");
    assert!(matches!(recorded, LearningRecorded::PolicyReview(_)));

    // Another tenant holds no such receipt; another repository's source does not match it;
    // a create that names a replaced revision, and an update that names none, are refused.
    let foreign: Result<PolicyActivated, String> = decode(
        log(
            &h,
            OTHER_TENANT,
            activation(OTHER_TENANT, PolicyChange::Create, REPO, &receipt),
        )
        .await,
    );
    assert!(foreign.unwrap_err().starts_with("POLICY_REVIEW_MISSING"));
    let wrong_source: Result<PolicyActivated, String> = decode(
        log(&h, TENANT, activation(TENANT, PolicyChange::Create, "org/other", &receipt)).await,
    );
    assert!(wrong_source.unwrap_err().starts_with("POLICY_REVIEW_MISMATCH"));
    let update_without_revision: Result<PolicyActivated, String> = decode(
        log(&h, TENANT, activation(TENANT, PolicyChange::Update, REPO, &receipt)).await,
    );
    assert!(update_without_revision
        .unwrap_err()
        .starts_with("POLICY_REVIEW_MISMATCH"));

    // The matching receipt activates the set.
    let activated: PolicyActivated = decode(
        log(&h, TENANT, activation(TENANT, PolicyChange::Create, REPO, &receipt)).await,
    )
    .expect("the reviewed set activates");
    assert_eq!(activated.scope_ref, format!("policy:{REPO}"));
    assert_eq!(activated.revision.as_deref(), Some(format!("policyset:{set}").as_str()));
    assert_eq!(activated.entities.len(), 2);
    assert!(activated
        .entities
        .iter()
        .all(|entity| entity.entity_id.starts_with(&format!("policycandidate:{REPO}:"))));

    // A review by the author alone is not recorded as an approval the gate admits.
    let selfie = review(&set, "self", Some(&format!("policyset:{set}")), "alice");
    let _: LearningRecorded = decode(
        log(
            &h,
            TENANT,
            DecisionLogOp::Learn {
                tenant_id: TENANT.to_string(),
                write: LearningWrite::RecordPolicyReview {
                    graph: GRAPH.to_string(),
                    review: Box::new(selfie.clone()),
                },
            },
        )
        .await,
    )
    .expect("the review records");
    let refused: Result<PolicyActivated, String> = decode(
        log(
            &h,
            TENANT,
            activation(
                TENANT,
                PolicyChange::Update,
                REPO,
                &selfie.binding.review_receipt_digest,
            ),
        )
        .await,
    );
    assert!(refused.unwrap_err().starts_with("REVIEW_NOT_INDEPENDENT"));

    // Retiring the active set withdraws it, behind a review made against that revision.
    let retiring = review(&set, "retire", Some(&format!("policyset:{set}")), "bob");
    let _: LearningRecorded = decode(
        log(
            &h,
            TENANT,
            DecisionLogOp::Learn {
                tenant_id: TENANT.to_string(),
                write: LearningWrite::RecordPolicyReview {
                    graph: GRAPH.to_string(),
                    review: Box::new(retiring.clone()),
                },
            },
        )
        .await,
    )
    .expect("the review records");
    let retired: PolicyActivated = decode(
        log(
            &h,
            TENANT,
            activation(
                TENANT,
                PolicyChange::Retire,
                REPO,
                &retiring.binding.review_receipt_digest,
            ),
        )
        .await,
    )
    .expect("the active set retires");
    assert_eq!(retired.revision, None);
}
