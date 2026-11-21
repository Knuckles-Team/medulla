//! Policy rule bundles (MEDULLA-IDENTITY-R702): compile, bind, refuse and project.

use super::*;
use crate::acl::{Grant, GrantEffect, RbacAction, ResourceSelector, Role};

fn upsert_group(group_id: &str) -> IdentityOp {
    IdentityOp::Access(AccessOp::UpsertGroup {
        request: GroupUpsert {
            group_id: group_id.to_string(),
            name: group_id.to_string(),
            roles: BTreeSet::new(),
            mfa_required: false,
        },
    })
}

fn join(group_id: &str, principal: &str) -> IdentityOp {
    IdentityOp::Access(AccessOp::ChangeMembership {
        request: GroupMembershipChange {
            group_id: group_id.to_string(),
            principal_id: principal.to_string(),
            change: BindingChange::Add,
        },
    })
}

fn rule(group: &str, action: RbacAction, resource: ResourceSelector) -> PolicyRule {
    PolicyRule {
        principal_group: group.to_string(),
        action,
        resource_scope: resource,
        effect: GrantEffect::Allow,
    }
}

fn graph_scope(graph: &str) -> PolicyBundleScope {
    PolicyBundleScope::Graph {
        graph: graph.to_string(),
    }
}

fn bundle(id: &str, scope: PolicyBundleScope, rules: Vec<PolicyRule>) -> PolicyRuleBundle {
    PolicyRuleBundle {
        schema_version: POLICY_BUNDLE_SCHEMA_VERSION,
        bundle_id: id.to_string(),
        scope,
        rules,
    }
}

fn editors_write_g(id: &str) -> PolicyRuleBundle {
    bundle(
        id,
        graph_scope("g"),
        vec![rule(
            "editors",
            RbacAction::Write,
            ResourceSelector::Graph("g".to_string()),
        )],
    )
}

fn validate(bundle: PolicyRuleBundle) -> IdentityOp {
    IdentityOp::Policy(PolicyBundleOp::Validate { bundle })
}

fn bind(bundle: PolicyRuleBundle) -> IdentityOp {
    IdentityOp::Policy(PolicyBundleOp::Bind { bundle })
}

fn unbind(scope: PolicyBundleScope) -> IdentityOp {
    IdentityOp::Policy(PolicyBundleOp::Unbind { scope })
}

/// A store with the groups `editors` and `ops`; the administrator belongs to `ops`.
fn store_with_groups() -> IdentityStore {
    let mut store = store_in(AuthMode::Local);
    for group in ["editors", "ops"] {
        apply_kept(&mut store, &upsert_group(group), &admin(), NOW).unwrap();
    }
    apply_kept(
        &mut store,
        &join("ops", BOOTSTRAP_PRINCIPAL),
        &admin(),
        NOW,
    )
    .unwrap();
    store
}

// spec: MEDULLA-IDENTITY-R702
#[test]
fn bundle_compiles_and_refuses_both_codes() {
    let mut store = store_with_groups();
    let alice = create(&mut store, "alice", UserKind::Human).unwrap();
    let bob = create(&mut store, "bob", UserKind::Human).unwrap();
    apply_kept(&mut store, &join("editors", &alice), &admin(), NOW).unwrap();

    // Validate names the `idm:policy:` role and the grant of each rule and writes nothing.
    let before = store.clone();
    let reply = apply_kept(&mut store, &validate(editors_write_g("b1")), &admin(), NOW).unwrap();
    assert_eq!(
        reply,
        IdentityReply::PolicyCompiled {
            roles: vec![Role::new("idm:policy:b1:editors")],
            grants: vec![Grant {
                role: "idm:policy:b1:editors".to_string(),
                resource: ResourceSelector::Graph("g".to_string()),
                action: RbacAction::Write,
                effect: GrantEffect::Allow,
            }],
        }
    );
    assert_eq!(store, before);

    // Bind records the bundle under its scope key and projects roles, grants and members.
    assert_eq!(
        apply_kept(&mut store, &bind(editors_write_g("b1")), &admin(), NOW).unwrap(),
        IdentityReply::Done { changed: true }
    );
    assert_eq!(store.policy_bundles().len(), 1);
    assert!(store.policy_bundles().contains_key("graph:g"));
    let projection = store.rbac_projection();
    assert!(projection
        .roles
        .iter()
        .any(|role| role.name == "idm:policy:b1:editors"));
    assert!(projection.grants.iter().any(|grant| {
        grant.role == "idm:policy:b1:editors"
            && grant.action == RbacAction::Write
            && grant.resource == ResourceSelector::Graph("g".to_string())
    }));
    assert!(projection.identities[&alice]
        .roles
        .contains(&"idm:policy:b1:editors".to_string()));
    assert!(!projection.identities[&bob]
        .roles
        .contains(&"idm:policy:b1:editors".to_string()));

    // A second bundle on a bound scope refuses POLICY_SCOPE_BOUND and the store is unchanged.
    let bound = store.clone();
    assert_eq!(
        apply_kept(&mut store, &bind(editors_write_g("b2")), &admin(), NOW),
        Err(IdentityRefusal::PolicyScopeBound)
    );
    assert_eq!(IdentityRefusal::PolicyScopeBound.as_str(), "POLICY_SCOPE_BOUND");
    assert_eq!(store, bound);

    // The same bundle id replaces its own binding.
    let widened = bundle(
        "b1",
        graph_scope("g"),
        vec![
            rule(
                "editors",
                RbacAction::Write,
                ResourceSelector::Graph("g".to_string()),
            ),
            rule(
                "editors",
                RbacAction::Read,
                ResourceSelector::Graph("g".to_string()),
            ),
        ],
    );
    assert_eq!(
        apply_kept(&mut store, &bind(widened.clone()), &admin(), NOW).unwrap(),
        IdentityReply::Done { changed: true }
    );
    assert_eq!(store.policy_bundles()["graph:g"], widened);

    // A bundle that would raise its own binder's authority refuses POLICY_SELF_AUTHORIZATION.
    let self_admin = bundle(
        "root",
        PolicyBundleScope::Tenant,
        vec![rule("ops", RbacAction::Admin, ResourceSelector::All)],
    );
    let bound = store.clone();
    assert_eq!(
        apply_kept(&mut store, &bind(self_admin.clone()), &admin(), NOW),
        Err(IdentityRefusal::PolicySelfAuthorization)
    );
    assert_eq!(
        IdentityRefusal::PolicySelfAuthorization.as_str(),
        "POLICY_SELF_AUTHORIZATION"
    );
    assert_eq!(store, bound);
    // The nearest accepted input: the same rule for a group the actor is not in.
    let others_admin = bundle(
        "root",
        PolicyBundleScope::Tenant,
        vec![rule("editors", RbacAction::Admin, ResourceSelector::All)],
    );
    assert!(apply_kept(&mut store, &bind(others_admin), &admin(), NOW).is_ok());

    // Unbind frees the scope, so a different bundle id binds there.
    apply_kept(&mut store, &unbind(graph_scope("g")), &admin(), NOW).unwrap();
    assert!(!store.policy_bundles().contains_key("graph:g"));
    assert!(apply_kept(&mut store, &bind(editors_write_g("b2")), &admin(), NOW).is_ok());
    assert_eq!(
        apply_kept(&mut store, &unbind(graph_scope("absent")), &admin(), NOW),
        Err(IdentityRefusal::NotFound)
    );
}

// spec: MEDULLA-IDENTITY-R702
#[test]
fn a_bundle_naming_an_unknown_group_or_a_foreign_resource_is_refused() {
    let mut store = store_with_groups();
    let unknown_group = bundle(
        "b",
        PolicyBundleScope::Tenant,
        vec![rule("nobody", RbacAction::Read, ResourceSelector::All)],
    );
    assert_eq!(
        apply_kept(&mut store, &bind(unknown_group), &admin(), NOW),
        Err(IdentityRefusal::InvalidRequest)
    );
    // Under a graph scope a rule must name that graph or a label.
    for resource in [
        ResourceSelector::All,
        ResourceSelector::Pattern("g*".to_string()),
        ResourceSelector::Graph("other".to_string()),
    ] {
        let wrong = bundle(
            "b",
            graph_scope("g"),
            vec![rule("editors", RbacAction::Read, resource)],
        );
        assert_eq!(
            apply_kept(&mut store, &bind(wrong), &admin(), NOW),
            Err(IdentityRefusal::InvalidRequest)
        );
    }
    let label = bundle(
        "b",
        graph_scope("g"),
        vec![rule(
            "editors",
            RbacAction::Read,
            ResourceSelector::Label("Doc".to_string()),
        )],
    );
    assert!(apply_kept(&mut store, &bind(label), &admin(), NOW).is_ok());
    // A bundle needs one to 256 rules and a valid identifier.
    for invalid in [
        bundle("empty", PolicyBundleScope::Tenant, Vec::new()),
        bundle(
            "Upper Case",
            PolicyBundleScope::Tenant,
            vec![rule("editors", RbacAction::Read, ResourceSelector::All)],
        ),
    ] {
        assert_eq!(
            apply_kept(&mut store, &validate(invalid), &admin(), NOW),
            Err(IdentityRefusal::InvalidRequest)
        );
    }
}
