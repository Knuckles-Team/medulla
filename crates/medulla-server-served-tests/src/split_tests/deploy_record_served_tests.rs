// MEDULLA-SDLC-R002.7, R002.8 and MEDULLA-BUSINESS-OPS-R004 through the served dispatch: the deploy
// record write and its two reads, and the approval gate question answered from the graph's
// approval rules. A foreign tenant's record and a missing one refuse alike, a retry replays, a
// different body is sealed out, and the gate answer follows the rule nodes of the graph.

use medulla_kernel::kernel::dispatch_support::with_engine_clock_offset;
use medulla_protocol_types::protocol::{Method, Response};
use medulla_source_types::approval_gate::{
    ApprovalGate, ApprovalGateAnswer, ApprovalGateRequest, GateClass,
};
use medulla_source_types::deploy_record::{
    DeployNodeOutcome, DeployNodeResult, DeployRecord, DeployRecordInput, DeployRecordPage,
    DeployRecordReceipt,
};
use serde_json::json;

use medulla_served_test_harness::connector_pack_harness::{
    ok, Served, GRAPH, OTHER_TENANT, TENANT,
};

const CONNECTOR: &str = "deploy-record-source";
const STREAM: &str = "paper";
const OTHER_PRINCIPAL: &str = "usr:other-tenant-deployer";
const WRITE: &[&str] = &["lifecycle:write"];
const READ: &[&str] = &["lifecycle:read"];
const DAY_MS: u64 = 86_400_000;

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}

fn body(plan: char) -> DeployRecordInput {
    DeployRecordInput {
        plan_digest: digest(plan),
        target: "production".to_string(),
        nodes: vec![
            DeployNodeOutcome {
                node: "web".to_string(),
                outcome: DeployNodeResult::Succeeded,
                code: None,
            },
            DeployNodeOutcome {
                node: "api".to_string(),
                outcome: DeployNodeResult::Failed,
                code: Some("STEP_TIMEOUT".to_string()),
            },
        ],
        image_digests: vec![digest('b'), digest('a')],
        approver: Some("usr:approver".to_string()),
        started_at_ms: Some(10),
        finished_at_ms: Some(20),
    }
}

pub(super) fn refusal_text(response: &Response) -> String {
    format!(
        "{} {}",
        response.error.clone().unwrap_or_default(),
        response.error_detail.clone().unwrap_or_default()
    )
}

fn put(record: DeployRecordInput) -> Method {
    Method::DeployRecordPut { record }
}

fn get(plan: char) -> Method {
    Method::DeployRecordGet {
        plan_digest: digest(plan),
    }
}

fn list(limit: Option<u32>, cursor: Option<String>) -> Method {
    Method::DeployRecordList { limit, cursor }
}

/// Admit `agent` as a regular identity with read, and optionally write, on the ingest graph, so a
/// refusal of its requests can only come from the handler and not from the graph's access check.
pub(super) async fn admit(served: &Served, agent: &str, writable: bool) {
    use {
        medulla_identity_core_types::acl::AgentIdentity,
        medulla_identity_core_types::acl::AgentRole, medulla_identity_core_types::acl::Grant,
        medulla_identity_core_types::acl::GrantEffect,
        medulla_identity_core_types::acl::RbacAction,
        medulla_identity_core_types::acl::ResourceSelector, medulla_identity_core_types::acl::Role,
    };
    let role = format!("deploy-record-{agent}");
    let mut state = served.state().write().await;
    state.isolation.add_role(Role::new(&role));
    let mut actions = vec![RbacAction::Read];
    if writable {
        actions.push(RbacAction::Write);
    }
    for action in actions {
        state.isolation.add_grant(Grant {
            role: role.clone(),
            resource: ResourceSelector::Graph(served.graph().to_string()),
            action,
            effect: GrantEffect::Allow,
        });
    }
    state.isolation.register_agent(AgentIdentity {
        agent_id: agent.to_string(),
        role: AgentRole::Agent,
        teams: Vec::new(),
        roles: vec![role],
    });
}

/// Write nodes into the ingest graph with one signed batch, as the harness administrator.
async fn seed(served: &Served, key: &str, operations: serde_json::Value) {
    let response = served
        .call_on(
            served.graph(),
            key,
            Method::BatchUpdate {
                operations_msgpack: rmp_serde::to_vec_named(&operations).expect("encode"),
            },
        )
        .await;
    assert!(response.error.is_none(), "seed {key}: {response:?}");
}

// spec: MEDULLA-SDLC-R002.7
#[tokio::test]
async fn a_record_is_written_once_replays_on_retry_and_refuses_a_different_body() {
    let served = Served::provisioned(GRAPH, CONNECTOR, &[(STREAM, "ResearchPaper")]).await;
    let first: DeployRecordReceipt = ok(
        "DeployRecordPut",
        served.verified(TENANT, WRITE, put(body('1'))).await,
    );
    assert!(!first.replayed);
    assert!(
        first.recorded_at_ms > 0,
        "the engine clock stamps the record"
    );
    assert!(
        !first.actor.is_empty(),
        "the verified principal is the actor"
    );
    assert!(first.node_id.starts_with("deploy_record:"));
    assert!(
        !first.node_id.contains(TENANT),
        "no tenant name in a node id"
    );

    // The same body again, a day later by the engine clock, replays the first write unchanged.
    let retry: DeployRecordReceipt = ok(
        "DeployRecordPut retry",
        with_engine_clock_offset(DAY_MS, served.verified(TENANT, WRITE, put(body('1')))).await,
    );
    assert!(retry.replayed);
    assert_eq!(
        (
            retry.recorded_at_ms,
            retry.actor.as_str(),
            retry.seal.as_str()
        ),
        (
            first.recorded_at_ms,
            first.actor.as_str(),
            first.seal.as_str()
        )
    );

    // The stored record reads back as written, in canonical order, with the sealed node behind it.
    let stored: DeployRecord = ok(
        "DeployRecordGet",
        served.verified(TENANT, READ, get('1')).await,
    );
    assert_eq!(stored.record.image_digests, vec![digest('a'), digest('b')]);
    assert_eq!(stored.record.nodes[0].node, "api");
    assert_eq!(stored.seal, first.seal);
    let node = served
        .properties(&first.node_id)
        .await
        .expect("the record node");
    assert_eq!(node["deploy_seal"], json!(first.seal));

    // A different body for the same plan digest is sealed out and changes nothing.
    let mut changed = body('1');
    changed.target = "staging".to_string();
    let refused = served.verified(TENANT, WRITE, put(changed)).await;
    assert!(
        refusal_text(&refused).contains("DEPLOY_RECORD_SEALED"),
        "{refused:?}"
    );
    let again: DeployRecord = ok(
        "DeployRecordGet after a refused overwrite",
        served.verified(TENANT, READ, get('1')).await,
    );
    assert_eq!(again, stored);

    // A body out of bounds refuses before anything is written.
    let mut bad = body('2');
    bad.plan_digest = "latest".to_string();
    let invalid = served.verified(TENANT, WRITE, put(bad)).await;
    assert!(
        refusal_text(&invalid).contains("DEPLOY_RECORD_INVALID"),
        "{invalid:?}"
    );
}

// spec: MEDULLA-SDLC-R002.8
#[tokio::test]
async fn list_pages_newest_first_by_a_tenant_bound_cursor() {
    let served = Served::provisioned(GRAPH, CONNECTOR, &[(STREAM, "ResearchPaper")]).await;
    for (plan, offset) in [('1', 0), ('2', DAY_MS), ('3', 2 * DAY_MS)] {
        let receipt: DeployRecordReceipt = ok(
            "DeployRecordPut",
            with_engine_clock_offset(offset, served.verified(TENANT, WRITE, put(body(plan)))).await,
        );
        assert!(!receipt.replayed);
    }
    let mut seen = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page: DeployRecordPage = ok(
            "DeployRecordList",
            served
                .verified(TENANT, READ, list(Some(2), cursor.clone()))
                .await,
        );
        seen.extend(page.records.iter().map(|row| row.plan_digest.clone()));
        pages += 1;
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, vec![digest('3'), digest('2'), digest('1')]);
    assert_eq!(pages, 2);

    // A cursor minted for this tenant does not resume another tenant's listing, and a limit
    // outside its bound is refused.
    let minted = ok::<DeployRecordPage>(
        "DeployRecordList first page",
        served.verified(TENANT, READ, list(Some(1), None)).await,
    )
    .next_cursor
    .expect("two records remain");
    admit(&served, OTHER_PRINCIPAL, false).await;
    let transplanted = served
        .verified_as(
            OTHER_PRINCIPAL,
            OTHER_TENANT,
            READ,
            list(None, Some(minted)),
        )
        .await;
    assert!(
        refusal_text(&transplanted).contains("DEPLOY_RECORD_INVALID"),
        "{transplanted:?}"
    );
    let zero = served.verified(TENANT, READ, list(Some(0), None)).await;
    assert!(
        refusal_text(&zero).contains("DEPLOY_RECORD_INVALID"),
        "{zero:?}"
    );
}

// spec: MEDULLA-SDLC-R002.8
#[tokio::test]
async fn another_tenant_sees_no_record_and_refuses_exactly_like_a_missing_one() {
    let served = Served::provisioned(GRAPH, CONNECTOR, &[(STREAM, "ResearchPaper")]).await;
    admit(&served, OTHER_PRINCIPAL, true).await;
    let _: DeployRecordReceipt = ok(
        "DeployRecordPut",
        served.verified(TENANT, WRITE, put(body('1'))).await,
    );

    let foreign = served
        .verified_as(OTHER_PRINCIPAL, OTHER_TENANT, READ, get('1'))
        .await;
    let missing = served
        .verified_as(OTHER_PRINCIPAL, OTHER_TENANT, READ, get('9'))
        .await;
    let foreign_text = refusal_text(&foreign);
    assert!(
        foreign_text.contains("DEPLOY_RECORD_NOT_FOUND"),
        "{foreign_text}"
    );
    assert_eq!(
        foreign_text,
        refusal_text(&missing),
        "a foreign record and a missing one must refuse alike"
    );
    let page: DeployRecordPage = ok(
        "DeployRecordList as the other tenant",
        served
            .verified_as(OTHER_PRINCIPAL, OTHER_TENANT, READ, list(None, None))
            .await,
    );
    assert!(page.records.is_empty(), "{page:?}");

    // The other tenant writes its own record under the same plan digest: the key is the tenant
    // and the digest, so the first tenant's record does not seal it out.
    let mut other_body = body('1');
    other_body.target = "other-environment".to_string();
    let theirs: DeployRecordReceipt = ok(
        "DeployRecordPut as the other tenant",
        served
            .verified_as(OTHER_PRINCIPAL, OTHER_TENANT, WRITE, put(other_body))
            .await,
    );
    assert!(!theirs.replayed);
    let ours: DeployRecord = ok(
        "DeployRecordGet as the first tenant",
        served.verified(TENANT, READ, get('1')).await,
    );
    assert_eq!(ours.record.target, "production");
}

// spec: MEDULLA-SDLC-R002.7
#[tokio::test]
async fn the_write_needs_the_lifecycle_write_scope_and_the_reads_the_read_scope() {
    let served = Served::provisioned(GRAPH, CONNECTOR, &[(STREAM, "ResearchPaper")]).await;
    let by_reader = served.verified(TENANT, READ, put(body('1'))).await;
    assert!(
        by_reader.error.is_some(),
        "a read scope must not write: {by_reader:?}"
    );
    let by_writer = served.verified(TENANT, WRITE, get('1')).await;
    assert!(
        by_writer.error.is_some(),
        "a write scope must not read: {by_writer:?}"
    );
    let listed = served.verified(TENANT, WRITE, list(None, None)).await;
    assert!(
        listed.error.is_some(),
        "a write scope must not list: {listed:?}"
    );
}

fn rule(id: &str, tenant: &str, operation: &str, quorum: u64, role: &str) -> serde_json::Value {
    json!({"op": "upsert_node", "id": id, "properties": {
        "type": "ApprovalRule",
        "tenant_id": tenant,
        "approvalActionKind": operation,
        "approvalCeilingMinor": 0,
        "approvalCurrency": "USD",
        "approvalQuorum": quorum,
        "approvalRequiredRole": role,
        "approvalGateClass": "publishing",
        "approvalExpiryMs": 3_600_000,
    }})
}

fn ask(operation: &str) -> Method {
    Method::ApprovalGateResolve {
        request: ApprovalGateRequest {
            operation: operation.to_string(),
            amount_minor: None,
            currency: None,
        },
    }
}

fn answer(response: Response) -> ApprovalGateAnswer {
    ok("ApprovalGateResolve", response)
}

// spec: MEDULLA-BUSINESS-OPS-R004
#[tokio::test]
async fn the_gate_answers_from_the_rule_nodes_and_follows_a_change_to_them() {
    let served = Served::provisioned(GRAPH, CONNECTOR, &[(STREAM, "ResearchPaper")]).await;
    seed(
        &served,
        "rules-1",
        json!([
            rule(
                "rule:promote",
                TENANT,
                "deploy.promotions.apply",
                1,
                "release-manager"
            ),
            rule("rule:filing", TENANT, "legal.filings.submit", 2, "counsel"),
            rule("rule:waived", TENANT, "notes.drafts.save", 0, "owner"),
            // A rule of another tenant never answers for this one.
            rule(
                "rule:foreign",
                OTHER_TENANT,
                "money.transfer",
                2,
                "treasurer"
            ),
        ]),
    )
    .await;
    let policy = ["policy:read"];

    let single = answer(
        served
            .verified(TENANT, &policy, ask("deploy.promotions.apply"))
            .await,
    );
    assert_eq!(single.gate, ApprovalGate::Single);
    assert_eq!(single.gate_class, Some(GateClass::Publishing));
    assert_eq!(single.approver_roles, vec!["release-manager".to_string()]);
    assert_eq!(single.expires_after_ms, Some(3_600_000));
    assert_eq!(single.rule_ids, vec!["rule:promote".to_string()]);
    let two = answer(
        served
            .verified(TENANT, &policy, ask("legal.filings.submit"))
            .await,
    );
    assert_eq!(two.gate, ApprovalGate::TwoPerson);
    let none = answer(
        served
            .verified(TENANT, &policy, ask("notes.drafts.save"))
            .await,
    );
    assert_eq!(none.gate, ApprovalGate::NoGate);

    // No rule covers it, or only another tenant's rule does: unresolved, never none.
    for operation in ["unknown.operation", "money.transfer"] {
        let open = answer(served.verified(TENANT, &policy, ask(operation)).await);
        assert_eq!(open.gate, ApprovalGate::Unresolved, "{operation}");
        assert!(open.rule_ids.is_empty() && open.approver_roles.is_empty());
    }

    // The graph is the authority: raising the quorum changes the next answer, with a new digest.
    seed(
        &served,
        "rules-2",
        json!([rule(
            "rule:promote",
            TENANT,
            "deploy.promotions.apply",
            2,
            "release-manager"
        )]),
    )
    .await;
    let raised = answer(
        served
            .verified(TENANT, &policy, ask("deploy.promotions.apply"))
            .await,
    );
    assert_eq!(raised.gate, ApprovalGate::TwoPerson);
    assert_ne!(raised.policy_digest, single.policy_digest);

    // A request out of bounds refuses, and so does a caller without a read scope.
    let blank = served.verified(TENANT, &policy, ask("  ")).await;
    assert!(
        refusal_text(&blank).contains("APPROVAL_GATE_INVALID"),
        "{blank:?}"
    );
    let unscoped = served
        .verified(TENANT, &["lifecycle:write"], ask("deploy.promotions.apply"))
        .await;
    assert!(unscoped.error.is_some(), "{unscoped:?}");
}
