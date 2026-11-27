//! The engine scopes arbora's served operations need (MEDULLA-IDENTITY-R736): each is in the
//! one scope catalog as a user-class engine scope, a caller without it is refused at the scope
//! gate and a caller with it is admitted. Scopes that reuse an existing scope are proven to
//! resolve to it.

#![cfg(feature = "test-support")]

use medulla_capabilities::scopes::{scope_entry, ScopeRegistry};
use medulla_identity_types::identity::{ScopeClass, ScopeClassifier, UserKind};
use medulla_server_api::authority_context::VerifiedRequestContext;

const ADDED: [&str; 5] = [
    "skills:eval",
    "skills:author",
    "skills:review",
    "sessions:read",
    "runs:control",
];

// spec: MEDULLA-IDENTITY-R736
#[test]
fn each_added_scope_is_a_user_class_engine_scope_a_person_may_hold() {
    for scope in ADDED {
        let entry = scope_entry(scope).expect("the scope is in the catalog");
        assert_eq!(entry.class, ScopeClass::User, "{scope}");
        assert_eq!(entry.owner, "engine", "{scope}");
        assert!(entry.class.allows_kind(UserKind::Human), "{scope}");
        assert!(entry.class.allows_kind(UserKind::Service), "{scope}");
        assert_eq!(ScopeRegistry.approver_group_of(scope), None, "{scope}");
    }
}

// spec: MEDULLA-IDENTITY-R736
#[test]
fn a_caller_without_an_added_scope_is_refused_and_one_with_it_is_admitted() {
    for scope in ADDED {
        let without = VerifiedRequestContext::verified_for_test_with_scopes(
            "agent-1",
            "tenant-a",
            &["kg:read"],
        );
        assert!(
            !without.allows_action(scope),
            "{scope} must be refused without the grant"
        );
        let with =
            VerifiedRequestContext::verified_for_test_with_scopes("agent-1", "tenant-a", &[scope]);
        assert!(
            with.allows_action(scope),
            "{scope} must be admitted with the grant"
        );
        // A grant of one added scope admits no other.
        for other in ADDED.iter().filter(|other| **other != scope) {
            assert!(!with.allows_action(other), "{scope} must not admit {other}");
        }
    }
}

// spec: MEDULLA-IDENTITY-R736
#[test]
fn the_reused_scopes_are_registered_and_the_names_arbora_asked_for_are_not() {
    // `agents:read` and `prompts:read` are the agent library read, `agent:library-read`.
    assert!(scope_entry("agent:library-read").is_some());
    for asked in ["agents:read", "prompts:read"] {
        assert!(
            scope_entry(asked).is_none(),
            "{asked} must not become a second name"
        );
    }
}

// spec: MEDULLA-IDENTITY-R737
#[test]
fn telemetry_write_is_user_class_so_a_person_may_hold_it() {
    let entry = scope_entry("telemetry:write").expect("registered");
    assert_eq!(entry.class, ScopeClass::User);
    assert!(entry.class.allows_kind(UserKind::Human));
    let without = VerifiedRequestContext::verified_for_test_with_scopes(
        "agent-1",
        "tenant-a",
        &["telemetry:read"],
    );
    assert!(!without.allows_action("telemetry:write"));
}

// spec: MEDULLA-IDENTITY-R743
#[test]
fn webhook_manage_is_a_domain_scope_of_the_door_that_a_person_or_a_service_may_hold() {
    let entry = scope_entry("webhook:manage").expect("registered");
    assert_eq!(entry.class, ScopeClass::Domain);
    assert_eq!(entry.owner, "graph-os");
    assert!(entry.class.allows_kind(UserKind::Human));
    assert!(entry.class.allows_kind(UserKind::Service));
    assert_eq!(ScopeRegistry.approver_group_of("webhook:manage"), None);
    let without = VerifiedRequestContext::verified_for_test_with_scopes(
        "agent-1",
        "tenant-a",
        &["memory:read"],
    );
    assert!(!without.allows_action("webhook:manage"));
    let with = VerifiedRequestContext::verified_for_test_with_scopes(
        "agent-1",
        "tenant-a",
        &["webhook:manage"],
    );
    assert!(with.allows_action("webhook:manage"));
    assert!(!with.allows_action("memory:events"));
}
