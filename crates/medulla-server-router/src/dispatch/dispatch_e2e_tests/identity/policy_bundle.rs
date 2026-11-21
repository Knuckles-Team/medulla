//! MEDULLA-IDENTITY-R702: a policy rule bundle bound through the served identity op reaches
//! the one access boundary. A member of the rule's group may write the graph, a non-member
//! may not, and `Unbind` removes the access.

use super::*;
use medulla_identity_types::acl::{GrantEffect, RbacAction, ResourceSelector};
use medulla_isolation::isolation::AccessLevel;
use medulla_protocol_types::protocol::GraphType;
use medulla_server_auth::access::check_graph_access;

const MEMBER: &str = "usr:member";
const OUTSIDER: &str = "usr:outsider";
const GRAPH: &str = "g";

fn admin() -> VerifiedRequestContext {
    context(BOOTSTRAP_PRINCIPAL, &[IDENTITY_ADMIN_SCOPE])
}

/// An engine whose policy store is durable (the access boundary reads the durable authority)
/// and whose System identity already bootstrapped.
fn durable_state(dir: &std::path::Path) -> Arc<RwLock<ServerState>> {
    let authority = medulla_kernel::store_authority::process_authority();
    let mut layer = medulla_isolation::isolation::IsolationLayer::with_persist_dir(
        dir,
        authority.as_ref(),
        authority.principal(),
        &authority.proof(),
    )
    .expect("open a durable policy store");
    layer
        .try_bootstrap_system_identity(medulla_identity_types::acl::AgentIdentity {
            agent_id: "engine-root".to_string(),
            role: medulla_identity_types::acl::AgentRole::System,
            teams: Vec::new(),
            roles: Vec::new(),
        })
        .expect("bootstrap the System identity");
    Arc::new(RwLock::new(
        super::super::test_support::new_state_for_test("dispatch-test-secret", layer),
    ))
}

fn editors_group() -> IdentityOp {
    IdentityOp::Access(AccessOp::UpsertGroup {
        request: GroupUpsert {
            group_id: "editors".to_string(),
            name: "editors".to_string(),
            roles: Default::default(),
            mfa_required: false,
        },
    })
}

fn user(username: &str, principal: &str, groups: &[&str]) -> IdentityOp {
    IdentityOp::User(UserOp::Create {
        request: CreateUserRequest {
            username: username.to_string(),
            kind: UserKind::Human,
            principal_id: Some(principal.to_string()),
            display_name: None,
            email: None,
            roles: Default::default(),
            groups: groups.iter().map(|group| group.to_string()).collect(),
            password: Secret::default(),
            must_change: false,
        },
    })
}

fn bundle(id: &str) -> PolicyRuleBundle {
    PolicyRuleBundle {
        schema_version: POLICY_BUNDLE_SCHEMA_VERSION,
        bundle_id: id.to_string(),
        scope: PolicyBundleScope::Graph {
            graph: GRAPH.to_string(),
        },
        rules: vec![PolicyRule {
            principal_group: "editors".to_string(),
            action: RbacAction::Write,
            resource_scope: ResourceSelector::Graph(GRAPH.to_string()),
            effect: GrantEffect::Allow,
        }],
    }
}

async fn writable(state: &Arc<RwLock<ServerState>>, principal: &str) -> bool {
    let guard = state.read().await;
    check_graph_access(
        &guard.isolation,
        Some(principal),
        GRAPH,
        GraphType::Global,
        None,
        AccessLevel::Write,
    )
    .is_ok()
}

// spec: MEDULLA-IDENTITY-R702
#[tokio::test]
async fn bundle_compiles_enforces_and_refuses_both_codes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = durable_state(dir.path());
    for (context, op) in [
        (broker(), initialize(PASSWORD)),
        (admin(), editors_group()),
        (admin(), user("member", MEMBER, &["editors"])),
        (admin(), user("outsider", OUTSIDER, &[])),
    ] {
        let response = send(&state, context, op).await;
        assert!(response.error.is_none(), "{:?}", response.error);
    }
    assert!(!writable(&state, MEMBER).await, "no bundle yet");

    // Validate compiles and writes nothing.
    let validated = send(
        &state,
        admin(),
        IdentityOp::Policy(PolicyBundleOp::Validate {
            bundle: bundle("b1"),
        }),
    )
    .await;
    assert!(validated.error.is_none(), "{:?}", validated.error);
    assert!(!writable(&state, MEMBER).await, "validate binds nothing");

    // Bind: the member writes the graph, the outsider does not.
    let bound = send(
        &state,
        admin(),
        IdentityOp::Policy(PolicyBundleOp::Bind {
            bundle: bundle("b1"),
        }),
    )
    .await;
    assert!(bound.error.is_none(), "{:?}", bound.error);
    assert!(writable(&state, MEMBER).await);
    assert!(!writable(&state, OUTSIDER).await);

    // A second bundle on the bound scope refuses and changes nothing.
    let refused = send(
        &state,
        admin(),
        IdentityOp::Policy(PolicyBundleOp::Bind {
            bundle: bundle("b2"),
        }),
    )
    .await;
    assert!(
        refused
            .error
            .as_deref()
            .unwrap_or("")
            .contains("POLICY_SCOPE_BOUND"),
        "{:?}",
        refused.error
    );
    assert!(writable(&state, MEMBER).await);

    // A bundle that gives the binding administrator `Admin` through its own group refuses.
    let membership = send(
        &state,
        admin(),
        IdentityOp::Access(AccessOp::ChangeMembership {
            request: GroupMembershipChange {
                group_id: "editors".to_string(),
                principal_id: BOOTSTRAP_PRINCIPAL.to_string(),
                change: BindingChange::Add,
            },
        }),
    )
    .await;
    assert!(membership.error.is_none(), "{:?}", membership.error);
    let mut escalating = bundle("b3");
    escalating.scope = PolicyBundleScope::Tenant;
    escalating.rules[0].action = RbacAction::Admin;
    escalating.rules[0].resource_scope = ResourceSelector::All;
    let refused = send(
        &state,
        admin(),
        IdentityOp::Policy(PolicyBundleOp::Bind {
            bundle: escalating,
        }),
    )
    .await;
    assert!(
        refused
            .error
            .as_deref()
            .unwrap_or("")
            .contains("POLICY_SELF_AUTHORIZATION"),
        "{:?}",
        refused.error
    );

    // Unbind removes the access.
    let unbound = send(
        &state,
        admin(),
        IdentityOp::Policy(PolicyBundleOp::Unbind {
            scope: PolicyBundleScope::Graph {
                graph: GRAPH.to_string(),
            },
        }),
    )
    .await;
    assert!(unbound.error.is_none(), "{:?}", unbound.error);
    assert!(!writable(&state, MEMBER).await);
}
