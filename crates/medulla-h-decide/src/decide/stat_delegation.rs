//! The delegation quality gate behind `DecisionEval` op `delegation_quality`
//! (MEDULLA-DECISION-ENGINE-R157.3, R157.4).
//!
//! The submission is judged by the pure gate in
//! `medulla_decision_types::decision::delegation_quality`; the admission is
//! sealed as the scope's job artifact. The job is the only writer of that
//! artifact, and a new submission replaces the stored one by compare-and-set
//! on the window's `end_ms`: a window that ends before the stored one is
//! not written, and the answer is the stored artifact. A refused submission
//! returns before any write.

use medulla_decision_types::decision::delegation_quality::{
    admit_delegation, artifact_key, check_request, replaces_window, DelegationKnobs,
    DelegationQualityOutcome, DelegationQualityRequest, StoredDelegation,
};
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use medulla_types::contract::Digest256;

use super::stat_support::resolve_policy;

/// Judge `request` against the policy's `delegation.*` knobs at `now_ms`, seal
/// the admission, and answer with the scope's authoritative artifact.
pub(super) fn seal(
    store: &AgentLibraryStore,
    now_ms: u64,
    request: &DelegationQualityRequest,
) -> Result<DelegationQualityOutcome, String> {
    let policy = resolve_policy(store, &request.tenant_id, &request.policy)?;
    let knobs = DelegationKnobs::from_policy(&policy.policy);
    check_request(request, &knobs, now_ms)?;
    let admission = admit_delegation(
        &request.window,
        request.observations.as_slice(),
        request.threshold.as_ref(),
        &knobs,
    );
    let next = StoredDelegation {
        scope: request.scope.clone(),
        window: request.window,
        threshold: request.threshold.clone(),
        admission,
    };
    let key = artifact_key(&request.scope);
    let current = store.decision_artifact(&request.tenant_id, &key)?;
    let previous: Option<StoredDelegation> = current
        .as_deref()
        .map(|bytes| decode_artifact(bytes, "delegation quality artifact"))
        .transpose()?;
    if !replaces_window(previous.as_ref().map(|stored| &stored.window), &next.window) {
        let stored = previous.ok_or("CORRUPT_DECISION_ARTIFACT: delegation window lost")?;
        let artifact_digest = digest_of(&stored)?;
        return Ok(DelegationQualityOutcome {
            stored,
            artifact_digest,
            replaced: false,
        });
    }
    let bytes = encode_artifact(&next)?;
    store.swap_decision_artifact(&request.tenant_id, &key, current, bytes)?;
    let artifact_digest = digest_of(&next)?;
    Ok(DelegationQualityOutcome {
        stored: next,
        artifact_digest,
        replaced: true,
    })
}

/// The artifact digest: SHA-256 of the sealed artifact's stored bytes.
fn digest_of(stored: &StoredDelegation) -> Result<Digest256, String> {
    Ok(Digest256::sha256(&encode_artifact(stored)?))
}
