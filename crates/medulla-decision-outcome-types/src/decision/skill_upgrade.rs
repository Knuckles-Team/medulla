//! Skill consumer-contract upgrade receipt (MEDULLA-DECISION-ENGINE-R167.1).

#[cfg(test)]
mod tests {
    use medulla_decision_types::decision::skill_upgrade_request::{
        SkillUpgradeReceipt, SKILL_UPGRADE_RECEIPT_SCHEMA_VERSION,
    };
    use medulla_types::contract::{Digest256, OpaqueId};

    fn receipt(migration: Option<&str>) -> SkillUpgradeReceipt {
        SkillUpgradeReceipt {
            schema_version: SKILL_UPGRADE_RECEIPT_SCHEMA_VERSION,
            previous_consumer_contract_digest: Digest256::from_bytes([1; 32]),
            new_consumer_contract_digest: Digest256::from_bytes([2; 32]),
            declared_migration_id: migration.map(|id| OpaqueId::new(id).unwrap()),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R167.1
    #[test]
    fn receipt_round_trips_with_and_without_migration() {
        for original in [receipt(Some("migration:skill-v2")), receipt(None)] {
            let bytes = rmp_serde::to_vec_named(&original).unwrap();
            let decoded = rmp_serde::from_slice::<SkillUpgradeReceipt>(&bytes).unwrap();
            assert_eq!(decoded, original);
            assert_ne!(
                decoded.previous_consumer_contract_digest,
                decoded.new_consumer_contract_digest
            );
        }
    }
}
