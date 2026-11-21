//! `Method::PublishSkillPackage` (MEDULLA-DECISION-ENGINE-R160.3): publish a
//! rendered skill package over the run history `DecisionLog.learn` recorded.
//!
//! The handler is the single writer of `package_publication/` rows; the run
//! rows are written by `DecisionLog` `learn` (`LearningWrite::PackageRun`). The
//! request carries no tenant: the tenant is the verified one, so another
//! tenant's runs are never read. A refusal writes nothing and a replay answers
//! the stored receipt.

use std::sync::Arc;

use medulla_decision_types::decision::package_validation::{
    package_publication_key, package_run_prefix, validate_rendered_package, PublicationReceipt,
    PublishSkillPackageRequest, RunHistoryEntry, MAX_PACKAGE_RUNS, PACKAGE_DIGEST_MISMATCH,
    PACKAGE_RUN_SET_INVALID,
};
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_protocol_types::result_contract::coordination::PublishSkillPackage;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::Digest256;
use tokio::sync::RwLock;

/// Publish `request.package` when its recorded run history validates it.
pub async fn handle_publish_skill_package(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: PublishSkillPackageRequest,
) -> Response {
    let store = match medulla_persistence_records::agent_library::shared_agent_library(state).await
    {
        Ok(store) => store,
        Err(error) => return Response::err(req_id, error),
    };
    let tenant = verified.tenant().to_string();
    match publish(&store, &tenant, request) {
        Ok(receipt) => Response::ok(req_id, ResultPayload::of::<PublishSkillPackage>(receipt)),
        Err(error) => Response::err(req_id, error),
    }
}

fn publish(
    store: &medulla_persistence_records::agent_library::AgentLibraryStore,
    tenant: &str,
    request: PublishSkillPackageRequest,
) -> Result<PublicationReceipt, String> {
    // (1) The rendered digest must parse.
    let digest = Digest256::parse(&request.package.digest).map_err(|_| {
        format!(
            "{}: the rendered digest is not a digest",
            PACKAGE_DIGEST_MISMATCH.as_str()
        )
    })?;
    // (2) A stored receipt is the answer, and nothing is written.
    let key = package_publication_key(&digest);
    if let Some(stored) = store.decision_artifact(tenant, &key)? {
        return decode_artifact(&stored, "package publication receipt");
    }
    // (3) The tenant's own run history, in ascending run id order.
    let rows = store
        .decision_artifacts_with_prefix(tenant, &package_run_prefix(&digest), MAX_PACKAGE_RUNS)
        .map_err(|error| {
            format!(
                "{}: {error}",
                PACKAGE_RUN_SET_INVALID.as_str()
            )
        })?;
    let history = rows
        .iter()
        .map(|(_, bytes)| decode_artifact::<RunHistoryEntry>(bytes, "package run"))
        .collect::<Result<Vec<_>, String>>()?;
    // (4) The pure validation; a refusal returns its code and writes nothing.
    let validation =
        validate_rendered_package(&request.package, &history).map_err(|refusal| refusal.to_string())?;
    // (5) Mint the receipt and write it once.
    let receipt = PublicationReceipt {
        package_id: request.package_id,
        validation_result: validation,
        publish_timestamp_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
    };
    store.put_decision_artifacts(tenant, &[(key, encode_artifact(&receipt)?)])?;
    Ok(receipt)
}

