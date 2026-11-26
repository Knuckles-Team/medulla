//! MEDULLA-IDENTITY-R704.2: `Method::CheckResourceAccess` answers the resource grant
//! decision through the real request boundary, with a refusal reason as a reply.

use super::*;
use medulla_identity_types::acl::{
    AgentIdentity, AgentRole, ResourceAccessDecision, ResourceGrant, ResourceGrantSubject,
    ResourceProtocol,
};

const AGENT: &str = "agent-worker";
const ROLE: &str = "role:deployers";
const RESOURCE: &str = "github-mcp";
const NOW_FAR_MS: u64 = u64::MAX / 2;

fn checker() -> VerifiedRequestContext {
    context("svc:arbora", &["security:check"])
}

fn grant(subject: ResourceGrantSubject, operation: &str, expires_at_ms: u64) -> ResourceGrant {
    ResourceGrant::new(
        subject,
        RESOURCE.to_string(),
        ResourceProtocol::Mcp,
        operation.to_string(),
        vec!["kg:read".to_string()],
        1,
        expires_at_ms,
    )
    .expect("a valid grant")
}

async fn check(state: &Arc<RwLock<ServerState>>, operation: &str) -> ResourceAccessDecision {
    let response = send_method(
        state,
        checker(),
        Method::CheckResourceAccess {
            agent_id: AGENT.to_string(),
            resource_ref: RESOURCE.to_string(),
            protocol: ResourceProtocol::Mcp,
            operation: operation.to_string(),
        },
    )
    .await;
    assert!(response.error.is_none(), "{:?}", response.error);
    match response.result.as_ref() {
        Some(ResultPayload::Json(value)) => {
            serde_json::from_value(value.clone()).expect("resource access decision")
        }
        other => panic!("expected a JSON decision, got {other:?}"),
    }
}

// spec: MEDULLA-IDENTITY-R704.2
#[tokio::test]
async fn check_resource_access_default_denies() {
    let state = state();

    // No grant names the resource.
    let unknown = check(&state, "create_issue").await;
    assert!(!unknown.allow);
    assert_eq!(unknown.reason_code.as_deref(), Some("RESOURCE_UNKNOWN"));

    let role_grant = grant(
        ResourceGrantSubject::Role(ROLE.to_string()),
        "create_issue",
        NOW_FAR_MS,
    );
    let expired = grant(
        ResourceGrantSubject::Principal(AGENT.to_string()),
        "close_issue",
        2,
    );
    {
        let mut guard = state.write().await;
        guard
            .isolation
            .try_register_agent(AgentIdentity {
                agent_id: AGENT.to_string(),
                role: AgentRole::Agent,
                teams: Vec::new(),
                roles: vec![ROLE.to_string()],
            })
            .expect("register the agent");
        guard
            .isolation
            .try_grant_resource(role_grant.clone())
            .expect("record the role grant");
        guard
            .isolation
            .try_grant_resource(expired)
            .expect("record the expired grant");
    }

    // A grant to another operation does not allow this one.
    let missing = check(&state, "delete_repo").await;
    assert!(!missing.allow);
    assert_eq!(missing.reason_code.as_deref(), Some("RESOURCE_GRANT_MISSING"));

    // A role grant allows a member, with its scope ceiling and expiry.
    let allowed = check(&state, "create_issue").await;
    assert!(allowed.allow);
    assert_eq!(allowed.grant_id.as_deref(), Some(role_grant.grant_id.as_str()));
    assert_eq!(allowed.scope_ceiling, vec!["kg:read".to_string()]);
    assert_eq!(allowed.expires_at_ms, Some(NOW_FAR_MS));
    assert_eq!(allowed.reason_code, None);

    // An ended grant replies GRANT_EXPIRED.
    let ended = check(&state, "close_issue").await;
    assert!(!ended.allow);
    assert_eq!(ended.reason_code.as_deref(), Some("GRANT_EXPIRED"));

    // A revoke flips the allow to a missing grant and moves the revision.
    let revoked = state
        .write()
        .await
        .isolation
        .try_revoke_resource(&role_grant.grant_id)
        .expect("revoke");
    assert!(revoked);
    let after = check(&state, "create_issue").await;
    assert!(!after.allow);
    assert_eq!(after.reason_code.as_deref(), Some("RESOURCE_GRANT_MISSING"));
    assert!(after.revocation_revision >= allowed.revocation_revision);
}

/// One principal grant on `protocol` allows exactly that protocol and no other, through the
/// served handler.
async fn protocol_grant_allows_only_its_protocol(protocol: ResourceProtocol, resource: &str) {
    let state = state();
    let granted = ResourceGrant::new(
        ResourceGrantSubject::Principal(AGENT.to_string()),
        resource.to_string(),
        protocol,
        "connect".to_string(),
        vec!["kg:read".to_string()],
        1,
        NOW_FAR_MS,
    )
    .expect("a valid grant");
    state
        .write()
        .await
        .isolation
        .try_grant_resource(granted.clone())
        .expect("record the grant");
    let ask = |asked: ResourceProtocol| {
        let state = state.clone();
        let resource = resource.to_string();
        async move {
            let response = send_method(
                &state,
                checker(),
                Method::CheckResourceAccess {
                    agent_id: AGENT.to_string(),
                    resource_ref: resource,
                    protocol: asked,
                    operation: "connect".to_string(),
                },
            )
            .await;
            assert!(response.error.is_none(), "{:?}", response.error);
            match response.result.as_ref() {
                Some(ResultPayload::Json(value)) => {
                    serde_json::from_value::<ResourceAccessDecision>(value.clone())
                        .expect("resource access decision")
                }
                other => panic!("expected a JSON decision, got {other:?}"),
            }
        }
    };
    let allowed = ask(protocol).await;
    assert!(allowed.allow);
    assert_eq!(allowed.grant_id.as_deref(), Some(granted.grant_id.as_str()));
    let other = if protocol == ResourceProtocol::Mcp {
        ResourceProtocol::A2a
    } else {
        ResourceProtocol::Mcp
    };
    let refused = ask(other).await;
    assert!(!refused.allow);
    assert_eq!(refused.reason_code.as_deref(), Some("RESOURCE_GRANT_MISSING"));
}

// spec: MEDULLA-IDENTITY-R704.3
#[tokio::test]
async fn check_resource_access_ssh_protocol() {
    protocol_grant_allows_only_its_protocol(ResourceProtocol::Ssh, "bastion-host").await;
}

// spec: MEDULLA-IDENTITY-R704.4
#[tokio::test]
async fn check_resource_access_database_protocol() {
    protocol_grant_allows_only_its_protocol(ResourceProtocol::Database, "orders-db").await;
}

// spec: MEDULLA-IDENTITY-R704.5
#[tokio::test]
async fn check_resource_access_web_protocol() {
    protocol_grant_allows_only_its_protocol(ResourceProtocol::Web, "wiki-site").await;
}

// spec: MEDULLA-IDENTITY-R704.6
#[tokio::test]
async fn check_resource_access_rdp_protocol() {
    protocol_grant_allows_only_its_protocol(ResourceProtocol::Rdp, "build-desktop").await;
}
