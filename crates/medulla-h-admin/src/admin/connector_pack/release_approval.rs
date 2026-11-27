//! Activation of a held connector-pack release when its change request is applied
//! (MEDULLA-TYPED-PACKS-R107.6).
//!
//! `Stewardship.Apply` on a change request whose target is `pack:<connector>` lands here. The
//! release was staged with its whole pack when it was held, so activation reads the staged
//! import back and runs it through the one import path with the release approved: the release
//! machine's `ContractHistory::approve` is the activation, and the import commit installs the
//! full pack (members, ontology, shapes) as the connector's head and clears the held slot.

use medulla_server_blob::blob::BlobStateExt;
use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

/// The target prefix of a change request that holds a pack release.
pub const PACK_TARGET_PREFIX: &str = "pack:";

/// Activate the release held behind the change request that proposed `proposed_digest` for
/// `target`. `Ok(None)` when `target` is not a pack. Refuses `PACK_RELEASE_NOT_HELD` when no
/// release is held behind that request.
#[cfg(all(feature = "redb", feature = "shacl", feature = "blob"))]
pub async fn activate_held_release(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    target: &str,
    proposed_digest: &str,
) -> Result<Option<String>, String> {
    use medulla_types::contract::ResourceId;

    let Some(connector) = target.strip_prefix(PACK_TARGET_PREFIX) else {
        return Ok(None);
    };
    let connector = ResourceId::new(connector).map_err(|error| error.to_string())?;
    let tenant_pack_guard = super::tenant_pack_lock(verified.tenant()).await?;
    let store = super::agent_library_store(state).await?;
    let blob = state
        .read()
        .await
        .blob_cursors()
        .cloned()
        .ok_or_else(|| "BODY_MISSING: Blob substrate disabled".to_string())?;
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
        "connector-pack:import",
        true,
    )?;
    let tenant = verified.tenant().to_string();
    let proposed_digest = proposed_digest.to_string();
    let cancel = medulla_server_api::request_scope::current();
    let usage = super::import::release::GraphClassUsage {
        state: Arc::clone(state),
    };
    medulla_server_api::api::dispatch_support::blocking_task(
        "connector pack release activation",
        move || {
            let _held = tenant_pack_guard;
            let held = store
                .connector_pack_pending_release(&tenant, &connector)?
                .filter(|held| held.change_request.proposed_digest == proposed_digest)
                .ok_or_else(|| {
                    "PACK_RELEASE_NOT_HELD: no release is held behind this change request"
                        .to_string()
                })?;
            let version = held.release.version.version.clone();
            super::import::commit_staged(&store, &*blob.store, context, &held, &cancel, &usage)?;
            Ok(Some(version))
        },
    )
    .await
}

/// Without the redb store, the release machine or the blob store no release is held, so none
/// activates.
#[cfg(not(all(feature = "redb", feature = "shacl", feature = "blob")))]
pub async fn activate_held_release(
    _state: &Arc<RwLock<ServerState>>,
    _req_id: u64,
    _verified: &VerifiedRequestContext,
    _target: &str,
    _proposed_digest: &str,
) -> Result<Option<String>, String> {
    Ok(None)
}
