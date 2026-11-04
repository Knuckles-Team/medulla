//! The engine method catalog, computed from the descriptor rows in code.
//!
//! `OperationReplayIdentity` binds a method's `SchemaId` and a digest of that
//! method's contract row, and the whole identity is bound to the catalog's own
//! digest. Both are pure functions of [`crate::method_descriptors`]: the Rust
//! descriptor rows are the one source of truth, there is no generated file and
//! nothing on disk to drift. A change to any descriptor moves its row digest and
//! the catalog digest, and therefore every replay identity minted under it.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use medulla_types::contract::Digest256;

struct Catalog {
    rows: BTreeMap<&'static str, (&'static str, [u8; 32])>,
    digest: String,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut rows = BTreeMap::new();
        for descriptor in crate::method_descriptors() {
            let id = descriptor.id.as_str();
            let schema_id: &'static str =
                Box::leak(format!("medulla:method/{id}").into_boxed_str());
            let row = Digest256::sha256(format!("{descriptor:?}").as_bytes());
            rows.insert(id, (schema_id, *row.as_bytes()));
        }
        let mut all = Vec::new();
        for (id, (schema_id, digest)) in &rows {
            all.extend_from_slice(id.as_bytes());
            all.push(0);
            all.extend_from_slice(schema_id.as_bytes());
            all.push(0);
            all.extend_from_slice(digest);
        }
        let digest = Digest256::sha256(&all).to_hex();
        Catalog { rows, digest }
    })
}

/// The one digest a consumer pins: every method row, in method-id order.
pub fn contract_catalog_digest() -> &'static str {
    catalog().digest.as_str()
}

/// The `SchemaId` and row digest of one method.
///
/// `None` means the id names no contract row at all, which the caller must fail
/// closed on: an identity cannot claim a method the contract does not declare.
pub fn method_schema(method_id: &str) -> Option<(&'static str, [u8; 32])> {
    catalog().rows.get(method_id).copied()
}

/// The served method catalog as JSON (MEDULLA-CONTRACT-R057): every descriptor row and
/// every registered scope, computed from [`crate::method_descriptors`] and the scope
/// registry each call. Nothing is generated or written; consumers bind to this.
pub fn catalog_json() -> String {
    let methods: Vec<serde_json::Value> = crate::method_descriptors()
        .map(|d| {
            serde_json::json!({
                "id": d.id.as_str(),
                "domain": d.domain,
                "is_wire_callable": d.stability != crate::Stability::Internal
                    && !d.consumer_profiles.is_empty(),
                "error_set": d.error_set,
                "policy": {
                    "mutates": d.policy.mutates,
                    "durability_domain": format!("{:?}", d.policy.durability_domain),
                    "authz_action": d.policy.authz_action,
                    "idempotent": d.policy.idempotent,
                    "audited": d.policy.audited,
                    "emits_cdc": d.policy.emits_cdc,
                    "txn_participation": format!("{:?}", d.policy.txn_participation),
                },
                "replay_class": format!("{:?}", d.replay_class),
                "consumer_profiles": d
                    .consumer_profiles
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>(),
                "stability": d.stability.as_str(),
                "note": d.note,
            })
        })
        .collect();
    let scopes: Vec<serde_json::Value> = crate::scopes::SCOPES
        .iter()
        .map(|entry| {
            let group = crate::scopes::APPROVER_GROUPS
                .iter()
                .find(|(scope, _)| *scope == entry.scope)
                .map(|(_, group)| *group);
            serde_json::json!({
                "scope": entry.scope,
                "class": crate::scopes::class_name(entry.class),
                "owner": entry.owner,
                "approver_group": group,
            })
        })
        .collect();
    serde_json::json!({
        "catalog_digest": contract_catalog_digest(),
        "methods": methods,
        "scopes": scopes,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-CONTRACT-R057
    #[test]
    fn the_served_catalog_lists_every_method_under_a_registered_scope() {
        let catalog: serde_json::Value =
            serde_json::from_str(&catalog_json()).expect("catalog is JSON");
        let methods = catalog["methods"].as_array().expect("methods");
        assert_eq!(methods.len(), crate::method_descriptors().count());
        assert_eq!(catalog["catalog_digest"], contract_catalog_digest());
        let scopes: std::collections::BTreeSet<&str> = catalog["scopes"]
            .as_array()
            .expect("scopes")
            .iter()
            .map(|s| s["scope"].as_str().expect("scope name"))
            .collect();
        assert!(!scopes.is_empty());
        for method in methods {
            let action = method["policy"]["authz_action"].as_str().expect("action");
            assert!(
                scopes.contains(action),
                "{} names unregistered scope {action}",
                method["id"]
            );
        }
        assert!(methods.iter().any(|m| m["is_wire_callable"] == true));
    }

    #[test]
    fn every_descriptor_has_a_catalog_row() {
        for descriptor in crate::method_descriptors() {
            let (schema_id, _) = method_schema(descriptor.id.as_str())
                .unwrap_or_else(|| panic!("{} has no catalog row", descriptor.id.as_str()));
            assert_eq!(
                schema_id,
                format!("medulla:method/{}", descriptor.id.as_str())
            );
        }
    }

    #[test]
    fn an_undeclared_method_has_no_catalog_row() {
        assert!(method_schema("NoSuchMethod").is_none());
    }

    // spec: MEDULLA-TYPED-PACKS-R068
    #[test]
    fn the_catalog_digest_is_a_stable_sha256() {
        assert_eq!(contract_catalog_digest().len(), 64);
        assert_eq!(contract_catalog_digest(), contract_catalog_digest());
        assert!(Digest256::parse(contract_catalog_digest()).is_ok());
    }
}
