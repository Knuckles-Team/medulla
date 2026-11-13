//! Production owner-open acceptance for the ConnectorPack catalog.

use super::*;
use medulla_types::contract::ResourceId;

// spec: MEDULLA-DECISION-ENGINE-R073.3
#[test]
fn fresh_open_initializes_all_pack_tables_and_reopens_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(AGENT_LIBRARY_FILE);
    assert!(!path.exists());
    for _ in 0..2 {
        let store = AgentLibraryStore::open(dir.path().to_str().unwrap()).unwrap();
        let read = store.read().unwrap();
        assert_eq!(
            medulla_storage::owner_table_names(medulla_storage::OwnerLayout::AgentLibrary).len(),
            22
        );
        macro_rules! empty_table {
            ($table:ident) => {
                assert_eq!(
                    read.open_owner_table(medulla_storage::$table)
                        .unwrap()
                        .len()
                        .unwrap(),
                    0,
                    stringify!($table)
                );
            };
        }
        empty_table!(CONNECTOR_PACK_HEADS);
        empty_table!(CONNECTOR_PACK_MEMBERS);
        empty_table!(CONNECTOR_PACK_IMPORTS);
        empty_table!(CONNECTOR_PACK_BODY_HOLDERS);
        empty_table!(CONNECTOR_PACK_BINDINGS);
        empty_table!(CAPABILITY_REWARD);
        let status = store
            .connector_pack_status("tenant-a", &ResourceId::new("fresh-pack").unwrap())
            .unwrap();
        assert!(status.head.is_none());
        assert!(status.importer.is_none());
        assert_eq!(
            status.members,
            medulla_catalog_types::connector_pack::PackMemberCounts::default()
        );
        assert!(path.exists());
    }
}

// spec: MEDULLA-DECISION-ENGINE-R073.3
#[test]
fn production_open_refuses_each_genuine_predecessor_without_writing() {
    let predecessors =
        medulla_storage::layout_predecessors(medulla_storage::OwnerLayout::AgentLibrary);
    assert!(!predecessors.is_empty());
    for predecessor in predecessors {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(AGENT_LIBRARY_FILE);
        medulla_storage::create_predecessor_owner_file(
            &path,
            PhysicalStoreIdentity::new(AGENT_LIBRARY_PHYSICAL_STORE).unwrap(),
            predecessor,
        )
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        for _ in 0..2 {
            let error = AgentLibraryStore::open(dir.path().to_str().unwrap()).unwrap_err();
            assert!(error.starts_with(&predecessor.error_code()), "{error}");
            assert!(error.contains(predecessor.label), "{error}");
            assert!(error.ends_with(&predecessor.operator_step()), "{error}");
            assert_eq!(
                std::fs::read(&path).unwrap(),
                before,
                "refusal wrote the owner file"
            );
        }
    }
}

// spec: MEDULLA-DECISION-ENGINE-R172.5.1
#[test]
fn capability_reward_survives_reopen() {
    use medulla_decision_types::decision::capability_designation::{CapabilityReward, Q32_ONE};
    use medulla_types::contract::{OpaqueId, TenantId};
    let dir = tempfile::tempdir().unwrap();
    let tenant = TenantId::new("tenant-a").unwrap();
    let entity = OpaqueId::new("agent-1").unwrap();
    let mut reward = CapabilityReward::neutral(tenant.clone(), entity.clone());
    reward.reward_q32 = Q32_ONE * 3 / 4;
    reward.outcome_count = 5;
    reward.generation = 2;
    reward.updated_revision = 9;
    {
        let store = AgentLibraryStore::open(dir.path().to_str().unwrap()).unwrap();
        assert!(store
            .capability_reward("tenant-a", "agent-1")
            .unwrap()
            .is_none());
        store.put_capability_reward(&reward).unwrap();
        // Another tenant's entity of the same id is a different row.
        let other = CapabilityReward::neutral(TenantId::new("tenant-b").unwrap(), entity.clone());
        store.put_capability_reward(&other).unwrap();
    }
    let store = AgentLibraryStore::open(dir.path().to_str().unwrap()).unwrap();
    assert_eq!(
        store.capability_reward("tenant-a", "agent-1").unwrap(),
        Some(reward.clone())
    );
    let rewards = store.capability_rewards("tenant-a").unwrap();
    assert_eq!(rewards, vec![reward.clone()]);
    // A staler write never replaces a newer durable reward.
    let mut stale = reward.clone();
    stale.updated_revision = 3;
    stale.reward_q32 = 0;
    let error = store.put_capability_reward(&stale).unwrap_err();
    assert!(error.starts_with(super::CAPABILITY_REWARD_STALE), "{error}");
    assert_eq!(
        store.capability_reward("tenant-a", "agent-1").unwrap(),
        Some(reward)
    );
}

// spec: MEDULLA-DECISION-ENGINE-R650.2.2
#[test]
fn power_digests_follow_the_tenant_ledger_and_never_another_tenants() {
    use medulla_decision_types::decision::capability_designation::CapabilityReward;
    use medulla_types::contract::{OpaqueId, TenantId};
    let dir = tempfile::tempdir().unwrap();
    let store = AgentLibraryStore::open(dir.path().to_str().unwrap()).unwrap();
    let (registry, empty) = store.power_digests("tenant-a", 1000).unwrap();
    for digest in [&registry, &empty] {
        assert_eq!(digest.strip_prefix("sha256:").unwrap().len(), 64);
    }
    let mut reward = CapabilityReward::neutral(
        TenantId::new("tenant-a").unwrap(),
        OpaqueId::new("agent-1").unwrap(),
    );
    reward.updated_revision = 1;
    store.put_capability_reward(&reward).unwrap();
    let (_, one) = store.power_digests("tenant-a", 1000).unwrap();
    assert_ne!(one, empty, "a reward row moves the ledger digest");
    // Another tenant's reward never moves this tenant's digest.
    let foreign = CapabilityReward::neutral(
        TenantId::new("tenant-b").unwrap(),
        OpaqueId::new("agent-2").unwrap(),
    );
    store.put_capability_reward(&foreign).unwrap();
    assert_eq!(store.power_digests("tenant-a", 1000).unwrap().1, one);
    assert_eq!(store.power_digests("tenant-a", 1000).unwrap().0, registry);
}
