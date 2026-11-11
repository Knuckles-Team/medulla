//! Served engine-contract registry invariants (MEDULLA-CONTRACT-R054).

use medulla_capabilities::{catalog::catalog_json, contract_catalog_digest, method_policy_entries};

#[test]
fn served_descriptors_cover_all_fields() {
    let served: serde_json::Value =
        serde_json::from_str(&catalog_json()).expect("the served catalog is JSON");

    let methods = served["methods"].as_array().expect("methods array");
    assert!(!methods.is_empty());

    // The registry and the served rows name exactly the same methods.
    let registry: Vec<&str> = method_policy_entries().map(|(name, _, _)| name).collect();
    for row in methods {
        let id = row["id"].as_str().expect("method row carries an id");
        assert!(
            registry.contains(&id),
            "served row {id} has no registry row"
        );
    }
    for name in &registry {
        assert!(
            methods.iter().any(|row| row["id"] == *name),
            "registry row {name} is not served"
        );
    }

    // Every served row carries all of its contract fields, non-null, and its
    // authz_action names a registered scope.
    let scopes: Vec<&str> = served["scopes"]
        .as_array()
        .expect("scopes array")
        .iter()
        .map(|row| row["scope"].as_str().expect("scope row carries a scope"))
        .collect();
    for row in methods {
        for field in [
            "id",
            "domain",
            "is_wire_callable",
            "error_set",
            "policy",
            "replay_class",
            "consumer_profiles",
            "stability",
            "note",
        ] {
            assert!(
                row.get(field).is_some() && !row[field].is_null(),
                "{field} is missing or null on {}",
                row["id"]
            );
        }
        let policy = &row["policy"];
        for field in [
            "authz_action",
            "mutates",
            "idempotent",
            "audited",
            "emits_cdc",
            "durability_domain",
            "txn_participation",
        ] {
            assert!(
                policy.get(field).is_some() && !policy[field].is_null(),
                "policy.{field} is missing or null on {}",
                row["id"]
            );
        }
        assert!(
            scopes.contains(&policy["authz_action"].as_str().expect("authz_action")),
            "unregistered scope for {}",
            row["id"]
        );
    }

    let describe = methods
        .iter()
        .find(|row| row["id"] == "DescribeContract")
        .expect("the DescribeContract descriptor is served");
    // spec: MEDULLA-CONTRACT-R054
    assert_eq!(describe["policy"]["authz_action"], "contract:read");
    // spec: MEDULLA-CONTRACT-R054
    assert_eq!(describe["stability"], "stable");
    assert!(describe["is_wire_callable"].as_bool().expect("bool"));
    assert!(!describe["policy"]["mutates"].as_bool().expect("bool"));
    assert!(describe["consumer_profiles"]
        .as_array()
        .expect("profiles")
        .iter()
        .any(|p| p == "python"));

    // The user-class scope registry row is served.
    let scope_row = served["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["scope"] == "contract:read")
        .expect("contract:read is registered");
    // spec: MEDULLA-CONTRACT-R054
    assert_eq!(scope_row["class"], "user");

    // The digest is 64 lowercase hex and is the one the catalog pins.
    let digest = contract_catalog_digest();
    // spec: MEDULLA-CONTRACT-R054
    assert_eq!(digest.len(), 64);
    assert!(digest
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert_eq!(served["catalog_digest"], digest);
}
