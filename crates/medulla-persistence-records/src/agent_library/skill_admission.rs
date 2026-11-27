//! Durable skill upgrade admissions (MEDULLA-TYPED-PACKS-R100.101).
//!
//! One admission row per tenant, skill component and revision, written through the Agent
//! Library control-plane write path so it survives restart. The judgement is the pure
//! rule in `medulla-catalog-types`; this file only numbers the revision, keeps the
//! admitted record and reads it back. A refused upgrade writes nothing.

use medulla_catalog_source_types::skill_admission::SkillAdmission;
use medulla_storage::DECISION_ARTIFACTS;
use redb::ReadableTable;

use super::AgentLibraryStore;

/// The control-plane event name of an admission write.
const ADMISSION_EVENT: &str = "skill_admission";

fn head_key(component_id: &str) -> String {
    format!("skill-admission/{component_id}/head")
}

fn row_key(component_id: &str, revision: u64) -> String {
    format!("skill-admission/{component_id}/{revision:020}")
}

fn decode(bytes: &[u8]) -> Result<SkillAdmission, String> {
    serde_json::from_slice(bytes)
        .map_err(|error| format!("CORRUPT_SKILL_ADMISSION: invalid admission row: {error}"))
}

fn decode_head(bytes: &[u8]) -> Result<u64, String> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| "CORRUPT_SKILL_ADMISSION: invalid admission head".to_string())
}

impl AgentLibraryStore {
    /// Judge and store the next admission of `component_id`. `judge` receives the
    /// revision the admission would take and answers the admission, or the typed refusal
    /// string; a refusal leaves the store untouched. The head and the row are written in
    /// one owner transaction, so two concurrent admissions never take the same revision.
    pub fn admit_skill_upgrade_row<F>(
        &self,
        tenant_id: &str,
        component_id: &str,
        judge: F,
    ) -> Result<SkillAdmission, String>
    where
        F: FnOnce(u64) -> Result<SkillAdmission, String>,
    {
        let mut admitted: Option<SkillAdmission> = None;
        self.maintain_control_rows(tenant_id, ADMISSION_EVENT, |owner| {
            let mut table = owner.open_table(DECISION_ARTIFACTS)?;
            let head = head_key(component_id);
            let previous = table
                .get((tenant_id, head.as_str()))
                .map_err(|error| error.to_string())?
                .map(|row| decode_head(row.value()))
                .transpose()?
                .unwrap_or(0);
            let revision = previous + 1;
            let admission = judge(revision)?;
            let bytes = serde_json::to_vec(&admission).map_err(|error| error.to_string())?;
            let key = row_key(component_id, revision);
            table
                .insert((tenant_id, key.as_str()), bytes.as_slice())
                .map_err(|error| error.to_string())?;
            table
                .insert((tenant_id, head.as_str()), revision.to_string().as_bytes())
                .map_err(|error| error.to_string())?;
            admitted = Some(admission);
            Ok(())
        })?;
        admitted.ok_or_else(|| {
            "skill admission was answered as a mutation replay without applying".to_string()
        })
    }

    /// The stored admission of `component_id` at `revision`, when one was taken.
    pub fn skill_admission(
        &self,
        tenant_id: &str,
        component_id: &str,
        revision: u64,
    ) -> Result<Option<SkillAdmission>, String> {
        let read = self.read()?;
        let table = read.open_owner_table(DECISION_ARTIFACTS)?;
        let key = row_key(component_id, revision);
        table
            .get((tenant_id, key.as_str()))
            .map_err(|error| error.to_string())?
            .map(|row| decode(row.value()))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::contract::{Digest256, OpaqueId};
    use {
        medulla_catalog_source_types::consumer_contract::ConsumerContractSchema,
        medulla_catalog_source_types::consumer_contract::ContractField,
    };
    use {
        medulla_catalog_source_types::skill_admission::admit_skill_upgrade,
        medulla_catalog_source_types::skill_admission::SkillAdmissionRefusal,
        medulla_catalog_source_types::skill_admission::SkillUpgradeAdmissionInput,
    };

    fn input(revision: u64) -> SkillUpgradeAdmissionInput {
        let field = |type_name: &str| {
            ConsumerContractSchema::new(vec![ContractField {
                name: "a".into(),
                type_name: type_name.into(),
                required: true,
            }])
            .unwrap()
        };
        SkillUpgradeAdmissionInput {
            previous_package_digest: Digest256::from_bytes([1; 32]),
            new_package_digest: Digest256::from_bytes([2; 32]),
            previous_contract: field("int"),
            new_contract: field("int"),
            cohort: vec![OpaqueId::new("consumer:one").unwrap()],
            migration: None,
            admission_revision: revision,
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R100.101
    #[test]
    fn admissions_take_consecutive_revisions_and_a_refusal_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentLibraryStore::open(dir.path().to_str().unwrap()).unwrap();
        let judge = |revision: u64| -> Result<SkillAdmission, String> {
            admit_skill_upgrade(&input(revision)).map_err(|_| "refused".to_string())
        };
        let first = store
            .admit_skill_upgrade_row("tenant-a", "skill:demo", judge)
            .unwrap();
        assert_eq!(first.admission_revision, 1);
        let refused = store.admit_skill_upgrade_row("tenant-a", "skill:demo", |_| {
            Err::<SkillAdmission, _>(
                SkillAdmissionRefusal::BreakingNoMigration
                    .code()
                    .to_string(),
            )
        });
        assert_eq!(refused.unwrap_err(), "SKILL_BREAKING_NO_MIGRATION");
        let second = store
            .admit_skill_upgrade_row("tenant-a", "skill:demo", judge)
            .unwrap();
        assert_eq!(second.admission_revision, 2);
        assert_eq!(
            store.skill_admission("tenant-a", "skill:demo", 1).unwrap(),
            Some(first)
        );
        assert_eq!(
            store.skill_admission("tenant-a", "skill:demo", 2).unwrap(),
            Some(second)
        );
        assert_eq!(
            store.skill_admission("tenant-a", "skill:demo", 3).unwrap(),
            None
        );
        assert_eq!(
            store.skill_admission("tenant-b", "skill:demo", 1).unwrap(),
            None
        );
    }
}
