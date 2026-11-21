//! `DecisionEval` `CertifyLocalSkill` (MEDULLA-DECISION-ENGINE-R142.101): attest a local-model
//! skill certification and store the attestation by digest.
//!
//! `certify` (R142.100) checks the receipt against the request first. A workflow then needs one
//! stored attestation per named atomic dependency, and each must still be current: the
//! dependency, the model, the dataset and the criteria it attests are the ones the request pins,
//! and the skill is the revision currently published. The job is the write authority; a refusal
//! stores nothing.

use medulla_decision_types::decision::local_certification::{
    certify, LocalCertificationAttestation, LocalCertificationRequest, SkillClassification,
    CERT_DEPENDENCY_MISSING, CERT_DEPENDENCY_STALE,
};
use medulla_decision_types::decision::record::outcome::DecisionRefusal;
use medulla_decision_types::decision::DecisionJobOutput;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};

use super::stat_jobs::JobRun;
use super::stat_log::MAX_LOG_ROWS;

/// The decision-artifact key prefix of stored attestations.
const CERTIFICATION_PREFIX: &str = "local_certification/";

/// The key of the attestation with `digest`.
fn certification_key(digest: &str) -> String {
    format!("{CERTIFICATION_PREFIX}{digest}")
}

/// Every stored attestation of `tenant_id`.
fn stored_attestations(
    store: &AgentLibraryStore,
    tenant_id: &str,
) -> Result<Vec<LocalCertificationAttestation>, String> {
    store
        .decision_artifacts_with_prefix(tenant_id, CERTIFICATION_PREFIX, MAX_LOG_ROWS)?
        .into_iter()
        .map(|(_, bytes)| decode_artifact(&bytes, "local certification"))
        .collect()
}

/// Check that every atomic dependency of a workflow has a current stored attestation.
fn check_dependencies(
    store: &AgentLibraryStore,
    request: &LocalCertificationRequest,
) -> Result<(), String> {
    if request.dependencies.is_empty() {
        return Ok(());
    }
    let stored = stored_attestations(store, &request.tenant_id)?;
    for dependency in &request.dependencies {
        let own: Vec<&LocalCertificationAttestation> = stored
            .iter()
            .filter(|attested| {
                attested.body.classification == SkillClassification::Atomic
                    && attested.body.skill.component_id == dependency.component_id
            })
            .collect();
        if own.is_empty() {
            return Err(format!(
                "{}: {} has no stored attestation",
                CERT_DEPENDENCY_MISSING.as_str(),
                dependency.component_id
            ));
        }
        let published = store
            .component_revisions(&request.tenant_id, &dependency.component_id)?
            .last()
            .map(|entry| entry.definition_digest.clone());
        let current = published.as_deref() == Some(dependency.definition_digest.as_str());
        let fresh = own.iter().any(|attested| {
            attested.body.skill == *dependency
                && attested.body.pins.model == request.pins.model
                && attested.body.pins.criteria_policy == request.pins.criteria_policy
                && attested.body.pins.dataset == request.pins.dataset
        });
        if !current || !fresh {
            return Err(format!(
                "{}: the attestation of {} is not for the current skill, model, dataset and criteria",
                CERT_DEPENDENCY_STALE.as_str(),
                dependency.component_id
            ));
        }
    }
    Ok(())
}

/// Attest `request` and store the attestation by digest. A refusal stores nothing.
pub(super) fn run(store: &AgentLibraryStore, request: &LocalCertificationRequest) -> JobRun {
    let attestation = certify(request)
        .map_err(|refused| format!("{}: {}", refused.code().as_str(), refused.detail()))?;
    check_dependencies(store, request)?;
    let rows = vec![(
        certification_key(&attestation.digest),
        encode_artifact(&attestation)?,
    )];
    Ok((
        DecisionJobOutput::Certification {
            attestation: Box::new(attestation),
        },
        rows,
    ))
}
