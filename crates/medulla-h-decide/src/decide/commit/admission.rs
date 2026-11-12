//! MEDULLA-DECISION-ENGINE-R036.103 and R036.104: the cross-owner admission of one
//! `DecisionCommit`.
//!
//! Before the decision record is written, the commit reserves the two owners it depends on:
//! the RBAC authorization image it was admitted under (`RbacStore::reserve`) and the Agent
//! Library component heads its candidates rest on (`AgentLibraryStore::reserve_admission`).
//! Both reservations are held until the commit decision is terminal and are then resolved
//! together with the same decision: COMMIT when the record is durable, ABORT when it is not.

use medulla_decision_types::decision::DecisionRecord;
use medulla_persistence_records::agent_library::admission::{
    AdmissionEvent, AdmissionIntent, ComponentHead,
};
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_server_api::authority_context::VerifiedRequestContext;

use super::super::SharedState;

/// The reservations one commit holds.
pub(super) struct Admitted {
    txn_id: String,
    tenant: String,
    #[cfg(feature = "security")]
    rbac: Option<(
        std::sync::Arc<dyn medulla_isolation::rbac_persist::RbacPolicyStore>,
        String,
    )>,
    library: bool,
}

impl Admitted {
    /// A commit that holds no reservation (an idempotent replay of a stored record).
    pub(super) fn none(tenant: &str) -> Self {
        Self {
            txn_id: String::new(),
            tenant: tenant.to_string(),
            #[cfg(feature = "security")]
            rbac: None,
            library: false,
        }
    }

    /// The sealed Agent Library intent of `record`.
    pub(super) fn intent(txn_id: &str, record: &DecisionRecord, profile: &str) -> AdmissionIntent {
        let mut heads: Vec<ComponentHead> = record
            .inputs
            .candidates
            .iter()
            .map(|candidate| ComponentHead {
                component_id: candidate.component_id.clone(),
                entry_revision: candidate.entry_revision,
                definition_digest: candidate.definition_digest.clone(),
            })
            .collect();
        heads.sort_by(|left, right| left.component_id.cmp(&right.component_id));
        AdmissionIntent {
            txn_id: txn_id.to_string(),
            profile_revision: profile.to_string(),
            component_heads: heads,
            visibility_digest: record.inputs.catalog_digest.clone(),
            write_key: format!("decision-record:{}", record.record_id),
        }
    }

    /// Reserve both owners for `record`. A refusal of either releases what was taken.
    pub(super) async fn reserve(
        state: &SharedState,
        store: &AgentLibraryStore,
        verified: &VerifiedRequestContext,
        record: &DecisionRecord,
        profile: &str,
    ) -> Result<Self, String> {
        let txn_id = format!(
            "decision-commit:{}:{}",
            record.record_id,
            verified.idempotency_key()
        );
        let mut held = Self::none(verified.tenant());
        held.txn_id = txn_id.clone();
        #[cfg(feature = "security")]
        {
            let policy_store = state.read().await.isolation.policy_store();
            if let Some(policy_store) = policy_store {
                let revision = policy_store
                    .authority_snapshot()
                    .map_err(|error| error.to_string())?
                    .revision();
                if let Some(record) = policy_store
                    .reserve_authority(revision, &txn_id)
                    .map_err(|error| error.to_string())?
                {
                    held.rbac = Some((policy_store, record.premise()));
                    state.write().await.isolation.adopt_reservation_revision()?;
                }
            }
        }
        #[cfg(not(feature = "security"))]
        let _ = state;
        let intent = Self::intent(&txn_id, record, profile);
        if let Err(error) = store.reserve_admission(verified.tenant(), &intent) {
            let _ = held.resolve(state, store, AdmissionEvent::Abort).await;
            return Err(error);
        }
        held.library = true;
        Ok(held)
    }

    /// Resolve every held reservation with the commit decision.
    pub(super) async fn resolve(
        self,
        state: &SharedState,
        store: &AgentLibraryStore,
        event: AdmissionEvent,
    ) -> Result<(), String> {
        let mut first_error = None;
        if self.library {
            if let Err(error) = store.resolve_admission(&self.tenant, &self.txn_id, event) {
                first_error = Some(error);
            }
        }
        #[cfg(feature = "security")]
        if let Some((policy_store, premise)) = &self.rbac {
            match policy_store.release_authority(&self.txn_id, Some(event)) {
                Ok(()) => {
                    tracing::info!(txn = %self.txn_id, %premise, "decision-commit rbac reservation resolved");
                    if let Err(error) = state.write().await.isolation.adopt_reservation_revision() {
                        first_error.get_or_insert(error);
                    }
                }
                Err(error) => {
                    first_error.get_or_insert(error.to_string());
                }
            }
        }
        #[cfg(not(feature = "security"))]
        let _ = state;
        first_error.map_or(Ok(()), Err)
    }
}
