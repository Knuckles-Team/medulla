//! Activation of a held connector-pack release when its change request is applied
//! (MEDULLA-TYPED-PACKS-R107.6).
//!
//! `Stewardship.Apply` on a change request whose target is `pack:<connector>` lands here. The
//! held release and the active one are read from the connector's head row, the held one is
//! activated through `ContractHistory::approve` (the one activation path, which moves it
//! through the release machine and links `supersedes`), and the persistence owner stores the
//! result in one admitted write that clears the held slot.

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

/// The target prefix of a change request that holds a pack release.
pub const PACK_TARGET_PREFIX: &str = "pack:";

/// Activate the release held behind the change request that proposed `proposed_digest` for
/// `target`. `Ok(None)` when `target` is not a pack. Refuses `PACK_RELEASE_NOT_HELD` when no
/// release is held behind that request.
#[cfg(all(feature = "redb", feature = "shacl"))]
pub async fn activate_held_release(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    target: &str,
    proposed_digest: &str,
) -> Result<Option<String>, String> {
    use medulla_catalog_types::connector_pack::contract_version::PackRelease;
    use medulla_types::contract::ResourceId;

    use super::contract_diff::{ContractHistory, ReleaseContract};

    let Some(connector) = target.strip_prefix(PACK_TARGET_PREFIX) else {
        return Ok(None);
    };
    let connector = ResourceId::new(connector).map_err(|error| error.to_string())?;
    let tenant_pack_guard = super::tenant_pack_lock(verified.tenant()).await?;
    let store = super::agent_library_store(state).await?;
    let shell = medulla_agent_types::agent_library::AgentLibraryMutationContext {
        request_id: 0,
        principal: String::new(),
        caller_principal: String::new(),
        attempt_nonce: medulla_types::contract::Nonce::minted(),
        tenant_id: verified.tenant().to_string(),
        actor_scope: String::new(),
        purpose_id: String::new(),
        policy_revision: String::new(),
        policy_digest: String::new(),
        policy_decision_id: String::new(),
        idempotency_key: String::new(),
        expected_revision: None,
        trace_id: None,
        created_at_ms: 0,
    };
    let context = medulla_persistence_records::agent_library::context::bind_agent_library_context(
        &store,
        req_id,
        verified,
        shell,
        "connector-pack:release-activate",
        true,
    )?;
    let tenant = verified.tenant().to_string();
    let proposed_digest = proposed_digest.to_string();
    medulla_server_api::api::dispatch_support::blocking_task(
        "connector pack release activation",
        move || {
            let _held = tenant_pack_guard;
            let not_held = || {
                "PACK_RELEASE_NOT_HELD: no release is held behind this change request".to_string()
            };
            let pending = store
                .connector_pack_pending_release(&tenant, &connector)?
                .filter(|pending| pending.change_request.proposed_digest == proposed_digest)
                .ok_or_else(not_held)?;
            let active = store
                .connector_pack_release(&tenant, &connector)?
                .ok_or_else(not_held)?;
            // The head row keeps each release's summary; the ontology and shape text is not
            // needed to decide an activation, only the versions and their link.
            let contract_of = |release: &PackRelease| ReleaseContract {
                version: release.version.clone(),
                ontology_ttl: String::new(),
                shapes_ttl: None,
                fields: release.fields.clone(),
                mappings: release.mappings.clone(),
            };
            let version = pending.release.version.version.clone();
            let mut history = ContractHistory::with_held(
                contract_of(&active),
                contract_of(&pending.release),
                pending.breaks.clone(),
                pending.change_request.clone(),
            )
            .map_err(|error| error.to_string())?;
            history.approve(&version).map_err(|error| error.to_string())?;
            let activated = PackRelease {
                version: history
                    .by_version(&version)
                    .map(|stored| stored.contract.version.clone())
                    .ok_or_else(not_held)?,
                fields: pending.release.fields.clone(),
                mappings: pending.release.mappings.clone(),
                schemas: pending.release.schemas.clone(),
            };
            store.activate_connector_pack_release(
                context,
                &connector,
                &proposed_digest,
                activated,
            )?;
            Ok(Some(version))
        },
    )
    .await
}

/// Without the redb store or the release machine no release is held, so none activates.
#[cfg(not(all(feature = "redb", feature = "shacl")))]
pub async fn activate_held_release(
    _state: &Arc<RwLock<ServerState>>,
    _req_id: u64,
    _verified: &VerifiedRequestContext,
    _target: &str,
    _proposed_digest: &str,
) -> Result<Option<String>, String> {
    Ok(None)
}
