//! MEDULLA-IDENTITY-R742: a WorkItem that names an agent-run delegation runs only while the
//! delegation is active. The request boundary refuses `DELEGATION_INVALID` for a revoked one,
//! and leaves an id with no agent-run record of the caller's tenant ungated (no oracle).

use super::*;
use medulla_protocol_types::protocol::Request;
use medulla_result_types::epistemic_operations::{
    RequestContext, RequestContextAuthenticationMethod, RequestContextSchemaVersion,
};
use medulla_server_identity::dispatch::identity_store::resolution_context::record_tenant;
use medulla_server_router_arms::dispatch::request_boundary::authorization::{
    check_agent_run_delegation, DispatchAuthority,
};
use medulla_work_types::native_control::{NativeControlSchemaVersion, SubmitWorkItemRequest};
use std::collections::BTreeMap;

const OWNER: &str = "usr:owner";
const DELEGATION: &str = "delegation:run-1";

fn admin() -> VerifiedRequestContext {
    context(BOOTSTRAP_PRINCIPAL, &[IDENTITY_ADMIN_SCOPE])
}

fn submit(tenant: &str, delegation_id: &str) -> Request {
    let context = RequestContext {
        schema_version: RequestContextSchemaVersion::V2,
        request_id: "req-1".to_string(),
        subject_id: OWNER.to_string(),
        tenant_id: tenant.to_string(),
        agent_id: OWNER.to_string(),
        scopes: Vec::new(),
        audience: "medulla-test".to_string(),
        authentication_method: RequestContextAuthenticationMethod::LocalProcess,
        policy_version: "policy-test".to_string(),
        graph: "__commons__".to_string(),
        placement_epoch: None,
        trace_id: "trace-1".to_string(),
        issued_at_ms: 1,
        expires_at_ms: 2,
    };
    let request = SubmitWorkItemRequest {
        child_input: None,
        conversation: None,
        run_id: None,
        workflow: None,
        schema_version: NativeControlSchemaVersion::V1,
        context,
        work_item_id: Some("wi-run-1".to_string()),
        idempotency_key: "key-1".to_string(),
        command_digest: "sha256:0".to_string(),
        kind: "agent_turn".to_string(),
        priority: 0,
        depends_on: Vec::new(),
        input_ref: "input-1".to_string(),
        policy_digest: "sha256:1".to_string(),
        catalog_digest: "sha256:2".to_string(),
        model_digest: "sha256:3".to_string(),
        max_attempts: 3,
        deadline_unix: None,
        metadata: BTreeMap::from([(
            "delegation_id".to_string(),
            serde_json::Value::String(delegation_id.to_string()),
        )]),
        provenance_refs: Vec::new(),
        max_tenant_in_flight: 0,
    };
    Request {
        id: 7,
        graph: "__commons__".to_string(),
        auth_token: String::new(),
        agent_id: Some(OWNER.to_string()),
        method: Method::SubmitWorkItem { request },
        trace_id: None,
    }
}

async fn gate(state: &Arc<RwLock<ServerState>>, request: &Request) -> Result<(), Response> {
    check_agent_run_delegation(
        state,
        request,
        &context(OWNER, &[]),
        DispatchAuthority {
            state_machine_authorized: false,
            identity_bootstrap: false,
        },
    )
    .await
}

// spec: MEDULLA-IDENTITY-R742
#[tokio::test]
async fn a_revoked_agent_run_delegation_refuses_the_work_item_and_other_ids_stay_ungated() {
    let state = state();
    for (context, op) in [
        (broker(), initialize(PASSWORD)),
        (
            admin(),
            medulla_identity_core_types::identity_fixtures::identity::create_administrator_op(
                "owner", OWNER,
            ),
        ),
    ] {
        let response = send(&state, context, op).await;
        assert!(response.error.is_none(), "{:?}", response.error);
    }
    let tenant = record_tenant(
        &medulla_server_api::server_context::ServerContext::fixture(),
        &context(OWNER, &[]),
    )
    .expect("verified tenant");
    let record = IdentityOp::Delegation(DelegationOp::Record {
        request: DelegationRecordWrite {
            work_item_id: DELEGATION.to_string(),
            record: DelegationRecord {
                tenant: tenant.clone(),
                principal: OWNER.to_string(),
                expires_at: medulla_kernel::kernel::dispatch_support::authoritative_now_ms()
                    + 3_600_000,
                kind: DelegationKind::AgentRun,
                run_id: Some("run-1".to_string()),
                operations: [IDENTITY_ADMIN_SCOPE.to_string()].into_iter().collect(),
                ..DelegationRecord::default()
            },
        },
    });
    let written = send(&state, admin(), record).await;
    assert!(written.error.is_none(), "{:?}", written.error);

    assert!(gate(&state, &submit(&tenant, DELEGATION)).await.is_ok());

    let revoked = send(
        &state,
        admin(),
        IdentityOp::Delegation(DelegationOp::Revoke {
            request: ObjectRef {
                id: DELEGATION.to_string(),
            },
        }),
    )
    .await;
    assert!(revoked.error.is_none(), "{:?}", revoked.error);
    let refused = gate(&state, &submit(&tenant, DELEGATION))
        .await
        .expect_err("a revoked delegation refuses the item");
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("DELEGATION_INVALID")),
        "{refused:?}"
    );

    // Another tenant's id and an id with no record are ungated, exactly like each other.
    assert!(gate(&state, &submit("tenant-foreign", DELEGATION))
        .await
        .is_ok());
    assert!(gate(&state, &submit(&tenant, "delegation:no-such"))
        .await
        .is_ok());
}

/// A signed host request that names `delegation_id`, with the claims of `base` otherwise.
fn claiming(base: &VerifiedRequestContext, delegation_id: &str) -> VerifiedRequestContext {
    let mut claims = base.claims().clone();
    claims.delegation_id = Some(delegation_id.to_string());
    VerifiedRequestContext::from_verified_claims(claims, "idem-delegated".to_string())
}

// spec: MEDULLA-IDENTITY-R744
#[tokio::test]
async fn an_envelope_naming_a_delegation_runs_as_its_principal_bounded_to_its_operations() {
    use medulla_server_router_arms::dispatch::request_boundary::delegated_actor::{
        resolve_delegated_actor, DELEGATE_SCOPE,
    };
    let state = state();
    for (context, op) in [
        (broker(), initialize(PASSWORD)),
        (
            admin(),
            medulla_identity_core_types::identity_fixtures::identity::create_administrator_op(
                "owner", OWNER,
            ),
        ),
    ] {
        let response = send(&state, context, op).await;
        assert!(response.error.is_none(), "{:?}", response.error);
    }
    let tenant = record_tenant(
        &medulla_server_api::server_context::ServerContext::fixture(),
        &context(OWNER, &[]),
    )
    .expect("verified tenant");
    let written = send(
        &state,
        admin(),
        IdentityOp::Delegation(DelegationOp::Record {
            request: DelegationRecordWrite {
                work_item_id: DELEGATION.to_string(),
                record: DelegationRecord {
                    tenant: tenant.clone(),
                    principal: OWNER.to_string(),
                    expires_at: medulla_kernel::kernel::dispatch_support::authoritative_now_ms()
                        + 3_600_000,
                    kind: DelegationKind::AgentRun,
                    run_id: Some("run-1".to_string()),
                    operations: [IDENTITY_ADMIN_SCOPE.to_string()].into_iter().collect(),
                    ..DelegationRecord::default()
                },
            },
        }),
    )
    .await;
    assert!(written.error.is_none(), "{:?}", written.error);
    let request = submit(&tenant, DELEGATION);
    let host = context("svc:host", &[DELEGATE_SCOPE, "graph:read"]);

    // A request that names no delegation is unchanged.
    let plain = resolve_delegated_actor(&state, &request, host.clone())
        .await
        .expect("no claim, no change");
    assert_eq!(plain.principal(), "svc:host");
    assert!(plain.acting_host().is_none());

    // A host without the delegation scope is refused before the record is read.
    let weak = claiming(&context("svc:weak", &["graph:read"]), DELEGATION);
    let denied = resolve_delegated_actor(&state, &request, weak)
        .await
        .expect_err("the host needs identity:delegate");
    assert!(
        denied
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("ACCESS_DENIED")),
        "{denied:?}"
    );

    // The claim runs the request as the principal, bounded to the delegation's operations.
    let acting = resolve_delegated_actor(&state, &request, claiming(&host, DELEGATION))
        .await
        .expect("an active delegation names its principal");
    assert_eq!(acting.principal(), OWNER);
    assert_eq!(acting.agent_id(), OWNER);
    assert_eq!(acting.tenant(), host.tenant());
    assert!(acting.allows_method(IDENTITY_ADMIN_SCOPE, true));
    assert!(
        !acting.allows_method("graph:read", false),
        "the host's own scopes do not carry over"
    );
    let recorded = acting.acting_host().expect("the host is recorded");
    assert_eq!(
        (recorded.principal.as_str(), recorded.agent_id.as_str()),
        ("svc:host", "svc:host")
    );

    // Every way the delegation is unusable refuses the same text.
    let same = |response: Response| response.error.unwrap_or_default();
    let unknown = same(
        resolve_delegated_actor(&state, &request, claiming(&host, "delegation:no-such"))
            .await
            .expect_err("no such record"),
    );
    assert!(unknown.starts_with("DELEGATION_INVALID"), "{unknown}");
    let foreign = {
        let mut claims = host.claims().clone();
        claims.tenant = "tenant-foreign".to_string();
        claims.delegation_id = Some(DELEGATION.to_string());
        let foreign = VerifiedRequestContext::from_verified_claims(claims, "idem-foreign".into());
        same(
            resolve_delegated_actor(&state, &request, foreign)
                .await
                .expect_err("another tenant's record is absent"),
        )
    };
    assert_eq!(foreign, unknown);
    let revoked = send(
        &state,
        admin(),
        IdentityOp::Delegation(DelegationOp::Revoke {
            request: ObjectRef {
                id: DELEGATION.to_string(),
            },
        }),
    )
    .await;
    assert!(revoked.error.is_none(), "{:?}", revoked.error);
    let lapsed = same(
        resolve_delegated_actor(&state, &request, claiming(&host, DELEGATION))
            .await
            .expect_err("a revoked delegation is unusable"),
    );
    assert_eq!(lapsed, unknown);
}
