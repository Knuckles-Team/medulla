//! `AgentComponent.admit_skill_upgrade`: judge a skill upgrade against the contracts its
//! consumers declared (MEDULLA-TYPED-PACKS-R100.101).
//!
//! The rule is the pure `admit_skill_upgrade` in the catalog layer. This handler converts
//! the wire request into its typed input, numbers the admission inside the store's write,
//! and answers a refusal with its registered code. A held admission is re-checked against
//! the package and contract now in force without taking a new one.

use medulla_agent_types::agent_component::{
    SkillAdmissionRequest, SkillContractFieldWire, SkillMigrationWire,
};
use medulla_types::contract::{Digest256, OpaqueId};
use {
    medulla_catalog_source_types::consumer_contract::ConsumerContractSchema,
    medulla_catalog_source_types::consumer_contract::ContractField,
};
use {
    medulla_catalog_source_types::skill_admission::admit_skill_upgrade,
    medulla_catalog_source_types::skill_admission::DeclaredSkillMigration,
    medulla_catalog_source_types::skill_admission::SkillAdmission,
    medulla_catalog_source_types::skill_admission::SkillAdmissionRefusal,
    medulla_catalog_source_types::skill_admission::SkillMigrationOutcome,
    medulla_catalog_source_types::skill_admission::SkillUpgradeAdmissionInput,
};

fn contract(fields: &[SkillContractFieldWire]) -> Result<ConsumerContractSchema, String> {
    ConsumerContractSchema::new(
        fields
            .iter()
            .map(|field| ContractField {
                name: field.name.clone(),
                type_name: field.type_name.clone(),
                required: field.required,
            })
            .collect(),
    )
}

fn migration(wire: &SkillMigrationWire) -> Result<DeclaredSkillMigration, String> {
    Ok(DeclaredSkillMigration {
        migration_id: OpaqueId::new(wire.migration_id.clone())?,
        migration_digest: Digest256::parse(&wire.migration_digest)?,
        previous_package_digest: Digest256::parse(&wire.previous_package_digest)?,
        new_package_digest: Digest256::parse(&wire.new_package_digest)?,
        consumer_contract_digest: Digest256::parse(&wire.consumer_contract_digest)?,
        outcome: if wire.receipt_passed {
            SkillMigrationOutcome::Passed
        } else {
            SkillMigrationOutcome::Failed
        },
    })
}

/// The typed input of one admission at `admission_revision`.
pub fn admission_input(
    request: &SkillAdmissionRequest,
    admission_revision: u64,
) -> Result<SkillUpgradeAdmissionInput, String> {
    Ok(SkillUpgradeAdmissionInput {
        previous_package_digest: Digest256::parse(&request.previous_package_digest)?,
        new_package_digest: Digest256::parse(&request.new_package_digest)?,
        previous_contract: contract(&request.previous_contract)?,
        new_contract: contract(&request.new_contract)?,
        cohort: request
            .cohort
            .iter()
            .map(|consumer| OpaqueId::new(consumer.clone()))
            .collect::<Result<_, _>>()?,
        migration: request.migration.as_ref().map(migration).transpose()?,
        admission_revision,
    })
}

/// A refusal as the wire string: its registered code, then a detail.
pub fn refusal_message(refusal: SkillAdmissionRefusal) -> String {
    let detail = match refusal {
        SkillAdmissionRefusal::BreakingNoMigration => {
            "the upgrade removes or retypes a field consumers declared and names no migration"
        }
        SkillAdmissionRefusal::ConsumerContractChanged => {
            "the consumer contract or package digest differs from the one admitted"
        }
        SkillAdmissionRefusal::MigrationReceiptInvalid => {
            "the migration is not bound to this exact change or its receipt did not pass"
        }
        // Not a registered refusal: a malformed input is answered as a plain
        // validation message, like every other request that fails `validate`.
        SkillAdmissionRefusal::InvalidInput => {
            return "the admission input is not well formed".into()
        }
    };
    format!("{}: {detail}", refusal.code())
}

/// Re-check the held admission `revision` against the package and contract in force.
#[cfg(feature = "redb")]
fn recheck(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    request: &SkillAdmissionRequest,
    revision: u64,
) -> Result<SkillAdmission, String> {
    let held = store
        .skill_admission(&request.tenant_id, &request.component_id, revision)?
        .ok_or_else(|| {
            format!(
                "no skill admission of {} at revision {revision}",
                request.component_id
            )
        })?;
    let package = Digest256::parse(&request.new_package_digest)?;
    let contract = contract(&request.new_contract)?;
    held.still_holds(&package, &contract)
        .map_err(refusal_message)?;
    Ok(held)
}

/// Serve one admission or re-check.
#[cfg(feature = "redb")]
pub fn handle_admit_skill_upgrade(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    req_id: u64,
    request: SkillAdmissionRequest,
) -> medulla_protocol_types::protocol::Response {
    use medulla_protocol_types::protocol::{Response, ResultPayload};

    let admitted = match request.held_admission_revision {
        Some(revision) => recheck(store, &request, revision),
        None => {
            store.admit_skill_upgrade_row(&request.tenant_id, &request.component_id, |revision| {
                let input = admission_input(&request, revision)?;
                admit_skill_upgrade(&input).map_err(refusal_message)
            })
        }
    };
    match admitted {
        Ok(admission) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentAdmitSkillUpgrade,
            >(&admission),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, type_name: &str, required: bool) -> SkillContractFieldWire {
        SkillContractFieldWire {
            name: name.into(),
            type_name: type_name.into(),
            required,
        }
    }

    fn request() -> SkillAdmissionRequest {
        SkillAdmissionRequest {
            tenant_id: "tenant".into(),
            component_id: "skill:demo".into(),
            previous_package_digest: "11".repeat(32),
            new_package_digest: "22".repeat(32),
            previous_contract: vec![field("a", "int", true)],
            new_contract: vec![field("a", "text", true)],
            cohort: vec!["consumer:one".into()],
            migration: None,
            held_admission_revision: None,
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R100.101
    #[test]
    fn a_breaking_wire_request_is_refused_with_its_registered_code_until_the_migration_binds() {
        let mut request = request();
        let input = admission_input(&request, 1).unwrap();
        let refusal = admit_skill_upgrade(&input).unwrap_err();
        assert!(refusal_message(refusal).starts_with("SKILL_BREAKING_NO_MIGRATION: "));

        let new_contract_digest = input.new_contract.digest().unwrap().to_hex();
        request.migration = Some(SkillMigrationWire {
            migration_id: "migration:v2".into(),
            migration_digest: "33".repeat(32),
            previous_package_digest: request.previous_package_digest.clone(),
            new_package_digest: request.new_package_digest.clone(),
            consumer_contract_digest: new_contract_digest,
            receipt_passed: true,
        });
        let admitted = admit_skill_upgrade(&admission_input(&request, 1).unwrap()).unwrap();
        assert_eq!(admitted.admission_revision, 1);

        request.migration.as_mut().unwrap().receipt_passed = false;
        let refusal = admit_skill_upgrade(&admission_input(&request, 1).unwrap()).unwrap_err();
        assert!(refusal_message(refusal).starts_with("SKILL_MIGRATION_RECEIPT_INVALID: "));
    }

    // spec: MEDULLA-TYPED-PACKS-R100.101
    #[test]
    fn a_malformed_digest_or_contract_is_refused_before_judgement() {
        let mut bad_digest = request();
        bad_digest.new_package_digest = "not-hex".into();
        assert!(admission_input(&bad_digest, 1).is_err());
        let mut duplicate = request();
        duplicate.new_contract = vec![field("a", "int", true), field("a", "int", true)];
        assert!(admission_input(&duplicate, 1).is_err());
    }
}
