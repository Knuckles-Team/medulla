//! Skill consumer-contract upgrade receipt (MEDULLA-DECISION-ENGINE-R167.1).

use serde::{Deserialize, Serialize};

use medulla_decision_types::decision::skill_upgrade_request::SkillUpgradeRequest;
use medulla_types::{contract::Digest256, contract::OpaqueId};

/// Current wire schema version of [`SkillUpgradeReceipt`].
pub const SKILL_UPGRADE_RECEIPT_SCHEMA_VERSION: u16 = 1;

/// Records the old and new consumer contracts and any declared migration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillUpgradeReceipt {
    pub schema_version: u16,
    pub previous_consumer_contract_digest: Digest256,
    pub new_consumer_contract_digest: Digest256,
    pub declared_migration_id: Option<OpaqueId>,
}

impl SkillUpgradeReceipt {
    /// The receipt of one upgrade request: both contract digests and the
    /// migration the author declared.
    pub fn of_request(request: &SkillUpgradeRequest) -> Result<Self, String> {
        request.validate()?;
        Ok(Self {
            schema_version: SKILL_UPGRADE_RECEIPT_SCHEMA_VERSION,
            previous_consumer_contract_digest: request.previous.digest()?,
            new_consumer_contract_digest: request.new.digest()?,
            declared_migration_id: request.declared_migration_id.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
