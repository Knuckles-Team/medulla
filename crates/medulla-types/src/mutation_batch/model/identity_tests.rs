//! Scope identity tests that reach the batch envelope helpers.

mod tests {
    use crate::mutation_scope::*;

    fn incarnation(value: &str) -> IncarnationId {
        IncarnationId::new(value).unwrap()
    }

    #[test]
    fn malformed_names_are_rejected_without_normalization() {
        for value in ["", " graph", "graph ", ".", "..", "a/b", "a\\b", "a\0b"] {
            assert!(LogicalName::new(value).is_err(), "accepted {value:?}");
        }
        assert!(ScopeTenantId::new("tenant\nother").is_err());
        assert!(IncarnationId::new(" incarnation").is_err());
    }

    /// Regression: v0 scanned the flat `tenant`/`graph` strings for address- and
    /// path-shaped inline text in `ChangeEnvelope::validate_core_text_fields`. When v1
    /// made those fields typed, that scan stopped applying to them, and only the
    /// path-shaped half of the policy was reproduced by `validate_identifier` -- so an
    /// address-shaped tenant id would have been silently accepted where it had always
    /// been rejected. This pins the whole policy at the constructor.
    #[test]
    fn address_and_path_shaped_identities_are_rejected_by_construction() {
        for value in [
            "person@example.invalid",
            "/home/person",
            "/users/person",
            "/mnt/backup",
            "file://tenant",
        ] {
            assert!(
                ScopeTenantId::new(value).is_err(),
                "accepted tenant {value:?}"
            );
            assert!(LogicalName::new(value).is_err(), "accepted graph {value:?}");
        }
        // The policy must not over-reach into ordinary identities.
        assert!(ScopeTenantId::new("tenant-a").is_ok());
        assert!(LogicalName::new("graph-a").is_ok());
    }

    #[test]
    fn digest_separates_tenant_scope_kind_and_native_domain() {
        let graph = MutationScopeIdentity::graph(
            ScopeTenantId::new("tenant-a").unwrap(),
            LogicalName::new("shared").unwrap(),
            incarnation("incarnation-1"),
        );
        let other_tenant = MutationScopeIdentity::graph(
            ScopeTenantId::new("tenant-b").unwrap(),
            LogicalName::new("shared").unwrap(),
            incarnation("incarnation-1"),
        );
        let blob = MutationScopeIdentity::native(
            ScopeTenantId::new("tenant-a").unwrap(),
            DurabilityDomain::BlobStore,
            LogicalName::new("shared").unwrap(),
            incarnation("incarnation-1"),
        )
        .unwrap();
        let jobs = MutationScopeIdentity::native(
            ScopeTenantId::new("tenant-a").unwrap(),
            DurabilityDomain::AnalyticsJob,
            LogicalName::new("shared").unwrap(),
            incarnation("incarnation-1"),
        )
        .unwrap();
        let semantic = MutationScopeIdentity::native(
            ScopeTenantId::new("tenant-a").unwrap(),
            DurabilityDomain::SemanticIndex,
            LogicalName::new("shared").unwrap(),
            incarnation("incarnation-1"),
        )
        .unwrap();
        assert_ne!(graph.identity_digest(), other_tenant.identity_digest());
        assert_ne!(graph.identity_digest(), blob.identity_digest());
        assert_ne!(blob.identity_digest(), jobs.identity_digest());
        assert_ne!(jobs.identity_digest(), semantic.identity_digest());
    }

    #[test]
    fn persisted_digest_tampering_is_rejected() {
        let identity = MutationScopeIdentity::graph(
            ScopeTenantId::new("tenant-a").unwrap(),
            LogicalName::new("graph-a").unwrap(),
            incarnation("incarnation-1"),
        );
        let mut value = serde_json::to_value(identity).unwrap();
        value["identity_digest"][0] = serde_json::json!(255);
        assert!(serde_json::from_value::<MutationScopeIdentity>(value).is_err());
    }

    #[test]
    fn graph_domains_cannot_be_smuggled_into_native_scope() {
        let error = MutationScopeIdentity::native(
            ScopeTenantId::new("tenant-a").unwrap(),
            DurabilityDomain::GraphRows,
            LogicalName::new("graph-shaped-native").unwrap(),
            incarnation("incarnation-1"),
        )
        .unwrap_err();
        assert!(error.contains("cannot own a native scope"));
    }

    #[test]
    fn logical_and_physical_graph_keys_keep_authority_contracts_distinct() {
        let logical =
            MutationScopeIdentity::fixed_graph("tenant-a", "tenant:scope", "incarnation-1")
                .unwrap();
        let malformed =
            MutationScopeIdentity::fixed_graph("tenant-a", "tenant~3ascope", "incarnation-1")
                .unwrap();
        let physical =
            MutationScopeIdentity::fixed_graph("__shard__", "tenant~3ascope", "incarnation-1")
                .unwrap();
        let hash_key = format!("~h{}", "a".repeat(64));
        let hashed_physical =
            MutationScopeIdentity::fixed_graph("__shard__", &hash_key, "incarnation-1").unwrap();
        let slash_hash_key = format!("~2fh{}", "a".repeat(64));
        let escaped_hash_physical =
            MutationScopeIdentity::fixed_graph("__shard__", &slash_hash_key, "incarnation-1")
                .unwrap();

        let logical_scope = crate::mutation_batch::authority_scope_for(&logical).unwrap();
        assert!(
            crate::mutation_batch::authority_scope_for(&malformed).is_err(),
            "caller authority must reject an unsanitized physical spelling"
        );
        assert!(
            super::super::authority_scope_for_physical_graph(&malformed).is_err(),
            "physical conversion must remain reserved to the shard tenant"
        );
        let physical_scope = super::super::authority_scope_for_physical_graph(&physical).unwrap();
        let hashed_scope =
            super::super::authority_scope_for_physical_graph(&hashed_physical).unwrap();
        let escaped_hash_scope =
            super::super::authority_scope_for_physical_graph(&escaped_hash_physical).unwrap();
        let external_alias_scope = crate::authority::AuthorityScope {
            kind: crate::contract::ScopeKind::new("graph").unwrap(),
            scope_id: crate::contract::ResourceId::new("tenant/3ascope").unwrap(),
            tenant: Some(crate::contract::TenantId::new("tenant-a").unwrap()),
            parent_scope_ids: crate::contract::BoundedVec::new(vec![
                crate::contract::ResourceId::new("tenant-a").unwrap(),
            ])
            .unwrap(),
            graph_incarnation: None,
        };
        let replay_digest = |scope: crate::authority::AuthorityScope, tenant: &str| {
            crate::authority::OperationReplayIdentity {
                schema_version: crate::contract::ResourceId::new("operation-replay-identity.v1")
                    .unwrap(),
                protocol_id: crate::contract::ProtocolId::new(
                    crate::authority::AUTHORITY_PROTOCOL_V1,
                )
                .unwrap(),
                catalog_digest: crate::contract::Digest256::from_bytes([0; 32]),
                tenant: crate::contract::TenantId::new(tenant).unwrap(),
                actor: crate::contract::ActorId::new("actor-a").unwrap(),
                audience: crate::contract::AudienceId::new("eg").unwrap(),
                purpose_resource: Some(scope.scope_id.clone()),
                authority_scope: scope,
                operation: crate::contract::Operation::new("mutation").unwrap(),
                purpose_kind: crate::contract::PurposeKind::new("graph_write").unwrap(),
                method: crate::contract::MethodId::new("mutation.apply").unwrap(),
                method_schema_id: crate::contract::SchemaId::new("mutation-envelope.v1").unwrap(),
                method_schema_digest: crate::contract::Digest256::from_bytes([1; 32]),
                canonical_payload_digest: crate::contract::Digest256::from_bytes([2; 32]),
                policy_revision: crate::contract::PolicyRevision::new("policy:1").unwrap(),
                policy_epoch: 0,
                policy_digest: crate::contract::Digest256::from_bytes([3; 32]),
                idempotency_key: crate::contract::IdempotencyKey::new("idem:stable").unwrap(),
            }
            .digest()
            .unwrap()
        };

        assert_eq!(logical_scope.scope_id.as_str(), "tenant:scope");
        assert!(physical_scope.scope_id.as_str().starts_with("physical:"));
        assert_ne!(physical_scope, logical_scope);
        assert_ne!(physical_scope, external_alias_scope);
        assert_ne!(
            physical_scope.tenant,
            Some(crate::contract::TenantId::new("tenant-a").unwrap())
        );
        assert_ne!(hashed_scope, escaped_hash_scope);
        assert_ne!(hashed_scope.scope_id, escaped_hash_scope.scope_id);
        assert_ne!(
            physical_scope.digest().unwrap(),
            logical_scope.digest().unwrap()
        );
        assert_ne!(
            replay_digest(physical_scope.clone(), "__shard__"),
            replay_digest(external_alias_scope, "tenant-a")
        );
        assert_ne!(
            replay_digest(hashed_scope, "__shard__"),
            replay_digest(escaped_hash_scope, "__shard__")
        );
        assert_ne!(physical.identity_digest(), logical.identity_digest());
    }
}
