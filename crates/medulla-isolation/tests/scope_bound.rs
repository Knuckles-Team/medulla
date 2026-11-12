//! The stored run-scope bound (MEDULLA-IDENTITY-R008.4 / R008.5): the bound is
//! read from the principal's `:RunScopePolicy` node, never from the plan, and a
//! plan that names a tool outside it is refused by name.

use medulla_graph_core::graph::GraphCore;
use medulla_isolation::isolation::{
    enforce_scope, load_scope_bound, run_scope_policy_iri, ScopeBoundError, ScopeRefusal,
    SCOPE_EXCEEDS_POLICY,
};

const PRINCIPAL: &str = "worker-1";
const QUERY: &str = "http://ex.org/tools/query";
const ERASE: &str = "http://ex.org/tools/erase";

fn store_policy(core: &GraphCore, principal: &str, props: serde_json::Value) {
    let blob = rmp_serde::to_vec_named(&props).expect("policy props encode");
    core.add_node(run_scope_policy_iri(principal), blob);
}

fn query_only_core() -> GraphCore {
    let core = GraphCore::new();
    store_policy(
        &core,
        PRINCIPAL,
        serde_json::json!({
            "type": "RunScopePolicy",
            "allowed_tools": [QUERY],
            "allowed_graphs": ["__commons__"],
        }),
    );
    core
}

// spec: MEDULLA-IDENTITY-R008.4
#[test]
fn plan_scope_field_is_ignored() {
    let core = query_only_core();
    // The plan body claims a wider scope; only the stored node counts.
    let plan_claimed_scope = [QUERY.to_string(), ERASE.to_string()];
    let bound = load_scope_bound(&core, PRINCIPAL).expect("stored bound loads");
    assert_eq!(bound.policy_iri(), run_scope_policy_iri(PRINCIPAL));
    assert_eq!(bound.allowed_tools, vec![QUERY.to_string()]);
    assert_eq!(bound.allowed_graphs, vec!["__commons__".to_string()]);
    assert!(
        enforce_scope(&core, PRINCIPAL, &plan_claimed_scope).is_err(),
        "a plan cannot widen the stored bound"
    );
    assert!(enforce_scope(&core, PRINCIPAL, &[QUERY.to_string()]).is_ok());
}

// spec: MEDULLA-IDENTITY-R008.4
#[test]
fn missing_policy_node_is_empty_bound() {
    let core = query_only_core();
    let bound = load_scope_bound(&core, "other-principal").expect("missing node is empty bound");
    assert!(bound.allowed_tools.is_empty());
    assert!(bound.allowed_graphs.is_empty());
    assert!(matches!(
        enforce_scope(&core, "other-principal", &[QUERY.to_string()]),
        Err(ScopeRefusal::ExceedsPolicy { .. })
    ));
    store_policy(
        &core,
        "malformed",
        serde_json::json!({"type": "RunScopePolicy", "allowed_tools": "not-a-list"}),
    );
    assert!(matches!(
        load_scope_bound(&core, "malformed"),
        Err(ScopeBoundError::PolicyUnreadable { .. })
    ));
}

// spec: MEDULLA-IDENTITY-R008.5
#[test]
fn excess_plan_is_refused_and_audited() {
    let core = query_only_core();
    let refusal = enforce_scope(&core, PRINCIPAL, &[QUERY.to_string(), ERASE.to_string()])
        .expect_err("an out-of-bound tool must be refused");
    assert_eq!(refusal.code(), SCOPE_EXCEEDS_POLICY);
    assert_eq!(
        refusal,
        ScopeRefusal::ExceedsPolicy {
            tool_iri: ERASE.to_string(),
            policy_iri: run_scope_policy_iri(PRINCIPAL),
        }
    );
    let message = refusal.to_string();
    assert!(message.starts_with(SCOPE_EXCEEDS_POLICY));
    assert!(message.contains(ERASE) && message.contains(&run_scope_policy_iri(PRINCIPAL)));
    // The audit event the served path writes before the refusal returns is
    // one entry: class `scope_exceeds_policy`, the tenant, the subject tool
    // IRI and the bound's policy IRI - the same two IRIs the refusal names -
    // and no second entry of the class.
    let audit = medulla_isolation::isolation::scope_refusal_audit_line(&refusal, "tenant-a");
    assert_eq!(
        audit,
        format!(
            "scope_exceeds_policy|tenant=tenant-a|tool={ERASE}|policy={}",
            run_scope_policy_iri(PRINCIPAL)
        )
    );
    assert_eq!(audit.matches("scope_exceeds_policy").count(), 1);
    assert!(audit.contains(ERASE) && audit.contains(&run_scope_policy_iri(PRINCIPAL)));
}
