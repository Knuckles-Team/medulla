//! `Method::RevertChangeset` (MEDULLA-SCM-R009): the inverse of one changeset appended to its
//! branch as a new changeset, through the one accept path.
//!
//! Order: read access on the repository, the window of the chain from the changeset to the head
//! out of the branch history index, `plan_revert`, then `SubmitChangeset`'s own handler (write
//! grants, blobs, accept, re-index). A revert already on the chain answers its own receipt
//! without appending, so the request is idempotent. A refusal before the last step writes
//! nothing.

use super::*;

/// The wire field of `Method::RevertChangeset`.
pub struct RevertChangesetFields {
    pub request: Box<medulla_source_types::source_changeset::RevertChangesetRequest>,
}

#[cfg(all(feature = "redb", feature = "blob"))]
mod served {
    use super::super::read_path_history::RegistryHistory;
    use super::super::submit_changeset::{submit_changeset_with, SubmitChangesetFields};
    use super::*;
    use medulla_compute::source_history::{decode_head, head_key, id_key, HistoryIndex};
    use medulla_h_graph::graph_ops::branch as ops;
    use medulla_h_ingest::source_revert::{plan_revert, RevertPlan, MAX_REVERT_WINDOW};
    use medulla_mutation_types::branch::BranchRef;
    use medulla_server_auth::access::path_grant;
    use medulla_server_blob::blob::BlobStateExt;
    use medulla_source_types::source_changeset::{Changeset, SubmitChangesetReceipt};
    use medulla_source_types::source_path_grant::PathAccess;

    /// The chain from `target` to the head, oldest first, each with its carrying commit id.
    fn read_window(
        index: &dyn HistoryIndex,
        target: &str,
    ) -> Result<Result<Vec<(String, Changeset)>, String>, String> {
        let Some(head) = index.row(&head_key())? else {
            return Ok(Err(format!(
                "REVISION_NOT_ON_CHAIN: {target} is not a changeset of the branch"
            )));
        };
        let head = decode_head(&head)?;
        let mut newest_first = Vec::new();
        for seq in (1..=head.seq).rev().take(MAX_REVERT_WINDOW as usize) {
            let Some(bytes) = index.row(&id_key(seq))? else {
                return Err(format!("history index has no commit row at seq {seq}"));
            };
            let commit = String::from_utf8(bytes).map_err(|error| error.to_string())?;
            let changeset = index
                .changeset_at(&commit)?
                .ok_or_else(|| format!("commit {commit} carries no changeset"))?;
            let found = changeset.id == target;
            newest_first.push((commit, changeset));
            if found {
                newest_first.reverse();
                return Ok(Ok(newest_first));
            }
        }
        Ok(Err(format!(
            "REVISION_NOT_ON_CHAIN: {target} is not a changeset of the branch"
        )))
    }

    /// Serve `Method::RevertChangeset`.
    pub async fn handle_revert_changeset(
        state: &Arc<RwLock<ServerState>>,
        req: &DispatchHeader,
        verified: &VerifiedRequestContext,
        fields: RevertChangesetFields,
    ) -> Response {
        let request = fields.request;
        let reference = BranchRef {
            tenant: verified.tenant().to_string(),
            graph: request.repo.clone(),
            name: request.branch.clone(),
        };
        let target = match ops::resolve_target(state, verified, reference).await {
            Ok(target) => target,
            Err(error) => return ops::target_error_response(req.id, error),
        };
        let (tenant, principal) = (
            verified.tenant().to_string(),
            verified.agent_id().to_string(),
        );
        let (repo, changeset_id) = (request.repo.clone(), request.changeset.clone());
        let window = target
            .read_blocking(move |registry, branch| {
                // A revert reads the changeset it inverts: it needs read on the repository.
                let grants = registry.path_grants(&tenant, &repo, &principal)?;
                if !path_grant::allows(&grants, "", PathAccess::Read) {
                    return Ok(Err(format!(
                        "{}: no read grant covers the repository",
                        PathAccess::Read.denied().code()
                    )));
                }
                read_window(&RegistryHistory { registry, branch }, &changeset_id)
            })
            .await;
        let window = match window {
            Ok(Ok(Ok(window))) => window,
            Ok(Ok(Err(message))) => return Response::err(req.id, message),
            Ok(Err(error)) | Err(error) => return Response::err(req.id, error),
        };
        let store = {
            let s = state.read().await;
            match s.blob_cursors() {
                Some(cursors) => Arc::clone(&cursors.store),
                None => {
                    return Response::err(
                        req.id,
                        "Blob substrate disabled (no persist dir configured)",
                    )
                }
            }
        };
        let changesets: Vec<Changeset> = window
            .iter()
            .map(|(_, changeset)| changeset.clone())
            .collect();
        let planned = {
            let store = Arc::clone(&store);
            let changeset_id = request.changeset.clone();
            tokio::task::spawn_blocking(move || {
                let size_of = |digest: &str| match store.get_manifest(digest) {
                    Ok(Some(manifest)) => Some(manifest.len),
                    _ => None,
                };
                plan_revert(&changesets, &changeset_id, &size_of)
            })
            .await
        };
        match planned {
            Ok(Ok(RevertPlan::Existing(index))) => {
                let (commit, changeset) = window[index].clone();
                match ResultPayload::of::<
                    medulla_protocol_types::result_contract::cluster::RevertChangeset,
                >(SubmitChangesetReceipt {
                    changeset,
                    branch_head: commit,
                    replayed: true,
                    index: None,
                }) {
                    Ok(payload) => Response::ok(req.id, payload),
                    Err(error) => Response::err(req.id, error),
                }
            }
            Ok(Ok(RevertPlan::Submit(mut submit))) => {
                submit.branch = request.branch.clone();
                submit_changeset_with(
                    state,
                    req,
                    verified,
                    SubmitChangesetFields { request: submit },
                    None,
                )
                .await
            }
            Ok(Err(refusal)) => Response::err(req.id, refusal.message()),
            Err(error) => Response::err(req.id, error.to_string()),
        }
    }
}

#[cfg(all(feature = "redb", feature = "blob"))]
pub use served::handle_revert_changeset;

/// `RevertChangeset` needs the `redb` branch registry and the `blob` ingest handlers.
#[cfg(not(all(feature = "redb", feature = "blob")))]
pub async fn handle_revert_changeset(
    _state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    _verified: &VerifiedRequestContext,
    _fields: RevertChangesetFields,
) -> Response {
    Response::err(
        req.id,
        "RevertChangeset requires the `redb` and `blob` features",
    )
}
