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
use medulla_raft_xshard::cross_shard_txn::certificate::OwnerDecision;
use medulla_server_api::authority_context::VerifiedRequestContext;

use super::super::SharedState;

/// The reservations one commit holds.
pub(in crate::decide) struct Admitted {
    txn_id: String,
    tenant: String,
    #[cfg(feature = "security")]
    rbac: Option<(
        std::sync::Arc<dyn medulla_isolation::rbac_persist::RbacPolicyStore>,
        String,
    )>,
    library: bool,
    /// MEDULLA-DECISION-ENGINE-R036.106: the `profile_grant:<id>` premises of the caller's
    /// active profile-use grants, read with the same authorization image the commit rests on.
    profile_premises: Vec<String>,
}

impl Admitted {
    /// A commit that holds no reservation (an idempotent replay of a stored record).
    pub(in crate::decide) fn none(tenant: &str) -> Self {
        Self {
            txn_id: String::new(),
            tenant: tenant.to_string(),
            #[cfg(feature = "security")]
            rbac: None,
            library: false,
            profile_premises: Vec::new(),
        }
    }

    /// The premises of the active profile-use grants the admission consulted.
    #[allow(dead_code)]
    pub(in crate::decide) fn profile_premises(&self) -> &[String] {
        &self.profile_premises
    }

    /// The sealed Agent Library intent of `record`.
    pub(in crate::decide) fn intent(
        txn_id: &str,
        record: &DecisionRecord,
        profile: &str,
    ) -> AdmissionIntent {
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
    pub(in crate::decide) async fn reserve(
        state: &SharedState,
        store: &AgentLibraryStore,
        verified: &VerifiedRequestContext,
        record: &DecisionRecord,
        profile: &str,
        txn_id: &str,
    ) -> Result<Self, String> {
        let txn_id = txn_id.to_string();
        let mut held = Self::none(verified.tenant());
        held.txn_id = txn_id.clone();
        #[cfg(feature = "security")]
        {
            let policy_store = state.read().await.isolation.policy_store();
            if let Some(policy_store) = policy_store {
                // Read before anything is reserved, so a refusal holds nothing.
                let profile_premises: Vec<String> = policy_store
                    .profile_use_grants()
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .filter(|grant| grant.is_active() && grant.principal == verified.principal())
                    .map(|grant| grant.premise())
                    .collect();
                let revision = policy_store
                    .authority_snapshot()
                    .map_err(|error| error.to_string())?
                    .revision();
                if let Some(record) = policy_store
                    .reserve_authority(revision, &txn_id)
                    .map_err(|error| error.to_string())?
                {
                    held.profile_premises = profile_premises;
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

    /// The reservations an earlier attempt of the same transaction took, adopted so the durable
    /// decision (COMMIT, already recorded) can resolve them.
    pub(in crate::decide) async fn resume(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        txn_id: &str,
    ) -> Self {
        let mut held = Self::none(verified.tenant());
        held.txn_id = txn_id.to_string();
        held.library = true;
        #[cfg(feature = "security")]
        {
            let policy_store = state.read().await.isolation.policy_store();
            held.rbac = policy_store.map(|store| (store, format!("resumed:{txn_id}")));
        }
        #[cfg(not(feature = "security"))]
        let _ = state;
        held
    }

    /// Resolve every held reservation with the durable coordinator decision.
    pub(in crate::decide) async fn resolve_decided(
        self,
        state: &SharedState,
        store: &AgentLibraryStore,
        decision: OwnerDecision,
    ) -> Result<(), String> {
        let event = match decision {
            OwnerDecision::Commit => AdmissionEvent::Commit,
            OwnerDecision::Abort => AdmissionEvent::Abort,
        };
        self.resolve(state, store, event).await
    }

    /// Resolve every held reservation with the commit decision.
    pub(in crate::decide) async fn resolve(
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
                    tracing::info!(txn = %self.txn_id, %premise, profile_grants = ?self.profile_premises, "decision-commit rbac reservation resolved");
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
