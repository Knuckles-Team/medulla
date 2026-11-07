//! MEDULLA-IDENTITY-R701: the delegation record of a WorkItem, written through
//! the identity store, read through `Method::GetDelegationRecord`, revoked, and
//! read from a foreign tenant.

use super::*;
use medulla_server_identity::dispatch::identity_store::resolution_context::record_tenant;

const OWNER: &str = "usr:owner";

fn admin() -> VerifiedRequestContext {
    context(BOOTSTRAP_PRINCIPAL, &[IDENTITY_ADMIN_SCOPE])
}

fn reader() -> VerifiedRequestContext {
    context("svc:arbora", &[IDENTITY_READ_SCOPE])
}

fn record_op(work_item_id: &str, tenant: &str) -> IdentityOp {
    IdentityOp::Delegation(DelegationOp::Record {
        request: DelegationRecordWrite {
            work_item_id: work_item_id.to_string(),
            record: DelegationRecord {
                tenant: tenant.to_string(),
                principal: OWNER.to_string(),
                tool: IDENTITY_ADMIN_SCOPE.to_string(),
                expires_at: u64::MAX,
                revoked_at: None,
            },
        },
    })
}

fn revoke_op(work_item_id: &str) -> IdentityOp {
    IdentityOp::Delegation(DelegationOp::Revoke {
        request: ObjectRef {
            id: work_item_id.to_string(),
        },
    })
}

async fn read(state: &Arc<RwLock<ServerState>>, work_item_id: &str) -> Response {
    send_method(
        state,
        reader(),
        Method::GetDelegationRecord {
            work_item_id: work_item_id.to_string(),
        },
    )
    .await
}

fn state_of(response: &Response) -> DelegationState {
    assert!(response.error.is_none(), "{:?}", response.error);
    match response.result.as_ref() {
        Some(ResultPayload::Json(value)) => {
            serde_json::from_value(value.clone()).expect("delegation state")
        }
        other => panic!("expected a JSON delegation state, got {other:?}"),
    }
}

// spec: MEDULLA-IDENTITY-R701
#[tokio::test]
async fn write_read_revoke_and_foreign_tenant_read() {
    let state = state();
    for (context, op) in [
        (broker(), initialize(PASSWORD)),
        (
            admin(),
            medulla_identity_types::identity_fixtures::identity::create_administrator_op(
                "owner", OWNER,
            ),
        ),
    ] {
        let response = send(&state, context, op).await;
        assert!(response.error.is_none(), "{:?}", response.error);
    }
    let tenant = record_tenant(
        &medulla_server_api::server_context::ServerContext::fixture(),
        &reader(),
    )
    .expect("verified tenant");

    for op in [
        record_op("wi_1", &tenant),
        record_op("wi_foreign", "tenant-foreign"),
    ] {
        let written = send(&state, admin(), op).await;
        assert!(written.error.is_none(), "{:?}", written.error);
    }

    let live = state_of(&read(&state, "wi_1").await);
    assert_eq!(live.standing, DelegationStanding::Active);
    assert_eq!(live.record.principal, OWNER);
    assert_eq!(live.record.revoked_at, None);
    assert!(live.principal_active && live.holds_tool_scope);

    let revoked = send(&state, admin(), revoke_op("wi_1")).await;
    assert!(revoked.error.is_none(), "{:?}", revoked.error);
    let after = state_of(&read(&state, "wi_1").await);
    assert_eq!(after.standing, DelegationStanding::Revoked);
    assert!(after.record.revoked_at.is_some());

    for unseen in ["wi_foreign", "wi_missing"] {
        let refused = read(&state, unseen).await;
        let error = refused.error.as_deref().unwrap_or_default();
        assert!(
            error.starts_with("DELEGATION_UNKNOWN"),
            "{unseen}: {refused:?}"
        );
    }
}
