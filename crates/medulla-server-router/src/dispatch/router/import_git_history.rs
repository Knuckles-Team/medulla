//! `Method::ImportGitHistory` (MEDULLA-SCM-R010): the first-parent line of a git
//! history appended to a branch, one changeset per commit.
//!
//! The version-2 pack was uploaded first through the blob methods. The handler
//! reads it back under the caller's blob owner scope, plans the import in
//! `medulla_h_ingest::source_changeset::git` (a merge commit is one changeset
//! against its first parent that keeps its full parent list and a side pack),
//! checks write grants on every planned path, then appends each commit through
//! `append_changeset`, the same registry append `SubmitChangeset` uses. Every
//! commit is its own registry transaction: a refusal writes nothing for the
//! refused commit, the commits before it stay, and a retry resumes after them.

use super::*;

/// The wire field of `Method::ImportGitHistory`.
pub struct ImportGitHistoryFields {
    pub request: Box<medulla_source_types::source_changeset::ImportGitHistoryRequest>,
}

#[cfg(all(feature = "redb", feature = "blob"))]
mod served {
    use super::super::submit_changeset::{append_changeset, ChangesetAppend};
    use super::*;
    use medulla_h_graph::graph_ops::branch as ops;
    use medulla_h_ingest::source_changeset::git::{
        decode_pack, head_git, materialize, plan_import,
    };
    use medulla_h_ingest::source_changeset::ChunkStoreBlobs;
    use medulla_mutation_types::branch::BranchRef;
    use medulla_persistence::branch_registry_store::AppendError;
    use medulla_server_auth::access::path_grant;
    use medulla_server_blob::blob::engine_bodies::read_owned_blob;
    use medulla_server_blob::blob::store::ChunkStore;
    use medulla_server_blob::blob::BlobStateExt;
    use medulla_source_types::source_changeset::{
        Changeset, ChangesetRefusal, ImportGitHistoryReceipt, ImportGitHistoryRequest,
        SubmitRefusal,
    };
    use medulla_source_types::source_path_grant::PathAccess;

    fn refusal(code: ChangesetRefusal, detail: &str) -> String {
        SubmitRefusal::new(code, detail).message()
    }

    fn lower_hex(value: &str, len: usize) -> bool {
        value.len() == len
            && value
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    }

    fn receipt_response(req_id: u64, receipt: ImportGitHistoryReceipt) -> Response {
        match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::ImportGitHistory>(
            receipt,
        ) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }

    /// Check the request bounds before any store is read.
    fn validate(request: &ImportGitHistoryRequest) -> Result<(), String> {
        let id_len = medulla_compute::source_git_objects::id_len(request.object_format) * 2;
        if !lower_hex(&request.pack_digest, 64) {
            return Err(refusal(ChangesetRefusal::ChangesetInvalid, "pack_digest"));
        }
        if !lower_hex(&request.head, id_len) {
            return Err(refusal(ChangesetRefusal::ChangesetInvalid, "head"));
        }
        Ok(())
    }

    /// Serve `Method::ImportGitHistory`.
    pub async fn handle_import_git_history(
        state: &Arc<RwLock<ServerState>>,
        req: &DispatchHeader,
        verified: &VerifiedRequestContext,
        fields: ImportGitHistoryFields,
    ) -> Response {
        let request: Arc<ImportGitHistoryRequest> = Arc::from(fields.request);
        if let Err(message) = validate(&request) {
            return Response::err(req.id, message);
        }
        let reference = BranchRef {
            tenant: verified.tenant().to_string(),
            graph: request.repo.clone(),
            name: request.branch.clone(),
        };
        let target = match ops::resolve_target(state, verified, reference).await {
            Ok(target) => target,
            Err(error) => return ops::target_error_response(req.id, error),
        };
        let store: Arc<dyn ChunkStore> = {
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
        let owner = match medulla_server_api::carrier::CarrierAuthority::from_verified(verified) {
            Ok(authority) => authority.owner_scope().to_string(),
            Err(error) => return Response::err(req.id, error),
        };

        // The branch head: the commit the new history must extend.
        let commits = match target.chain().await {
            Ok(commits) => commits,
            Err(error) => return Response::err(req.id, error),
        };
        let mut head_commit = commits.last().map(|commit| commit.id.clone());
        let head_changeset: Option<Changeset> = match &head_commit {
            Some(commit) => match target.payload(commit).await {
                Ok(Some(bytes)) => match rmp_serde::from_slice(&bytes) {
                    Ok(changeset) => Some(changeset),
                    Err(error) => return Response::err(req.id, error.to_string()),
                },
                Ok(None) => None,
                Err(error) => return Response::err(req.id, error),
            },
            None => None,
        };
        let branch_head = head_git(head_changeset.as_ref());
        // The head a new changeset extends: `None` while the chain holds no changeset.
        let mut chain_head: Option<String> = head_changeset.as_ref().and(head_commit.clone());

        // Read the pack and plan every commit before any write.
        let planned = {
            let request = Arc::clone(&request);
            let store = Arc::clone(&store);
            let owner = owner.clone();
            tokio::task::spawn_blocking(move || -> Result<_, String> {
                let bytes = read_owned_blob(
                    store.as_ref(),
                    &owner,
                    &format!("cas:sha256:{}", request.pack_digest),
                    medulla_work_types::scheduler_config::DEFAULT_CHANGESET_MAX_BLOB_BYTES
                        as usize,
                )
                .map_err(|error| {
                    refusal(ChangesetRefusal::BlobNotFound, &error)
                })?;
                let objects = decode_pack(request.object_format, &bytes)
                    .map_err(|error| error.message())?;
                plan_import(
                    request.object_format,
                    &objects,
                    &request.head,
                    &branch_head,
                    medulla_work_types::scheduler_config::HISTORY_KNOBS[0].2,
                )
                .map_err(|error| error.message())
            })
            .await
        };
        let plan = match planned {
            Ok(Ok(plan)) => plan,
            Ok(Err(message)) => return Response::err(req.id, message),
            Err(error) => return Response::err(req.id, error.to_string()),
        };

        // MEDULLA-SCM-R007: write access on every planned path, before any blob lands.
        let grants = match target
            .path_grants(verified.tenant(), &request.repo, verified.agent_id())
            .await
        {
            Ok(grants) => grants,
            Err(error) => return Response::err(req.id, error),
        };
        let touched: Vec<&str> = plan
            .commits
            .iter()
            .flat_map(|commit| commit.changes.iter().map(|change| change.path.as_str()))
            .collect();
        if let Err(denied) = path_grant::check(&grants, &touched, PathAccess::Write) {
            return Response::err(
                req.id,
                format!("{}: no write grant covers an imported path", denied.code()),
            );
        }

        let author = verified.principal().to_string();
        let agent_identity: Option<String> =
            (verified.agent_id() != verified.principal()).then(|| verified.agent_id().to_string());
        let mut imported = 0u32;
        let mut skipped = plan.skipped;
        for planned in plan.commits {
            let batch_id = medulla_server_api::batch_compile::branch_batch_id(
                verified.tenant(),
                &request.repo,
                &request.branch,
                Some(verified.principal()),
                &format!("git-import:{}", planned.origin.commit_id),
            );
            // A commit appended by an earlier attempt is skipped, not written twice.
            match target.replayed(&batch_id).await {
                Ok(Some(commit)) => {
                    skipped = skipped.saturating_add(1);
                    chain_head = Some(commit.id.clone());
                    head_commit = Some(commit.id);
                    continue;
                }
                Ok(None) => {}
                Err(error) => return Response::err(req.id, error),
            }
            let step = {
                let store = Arc::clone(&store);
                let owner = owner.clone();
                let request = Arc::clone(&request);
                let base_head = chain_head.clone();
                tokio::task::spawn_blocking(move || {
                    materialize(
                        &planned,
                        &ChunkStoreBlobs(store.as_ref(), &owner),
                        &request.repo,
                        &request.branch,
                        base_head,
                    )
                })
                .await
            };
            let step = match step {
                Ok(Ok(step)) => step,
                Ok(Err(error)) => return Response::err(req.id, error.message()),
                Err(error) => return Response::err(req.id, error.to_string()),
            };
            if let Err(error) = step.request.validate_import() {
                return Response::err(req.id, error.message());
            }
            let appended = append_changeset(
                &target,
                verified.tenant(),
                &batch_id,
                ChangesetAppend {
                    request: Arc::new(step.request),
                    store: Arc::clone(&store),
                    merge_owner: owner.clone(),
                    author: author.clone(),
                    agent_identity: agent_identity.clone(),
                    created_at_ms: step.created_at_ms,
                    git_origin: Some(step.origin),
                },
            )
            .await;
            match appended {
                Ok(done) => {
                    imported += 1;
                    chain_head = Some(done.commit.id.clone());
                    head_commit = Some(done.commit.id);
                }
                Err(AppendError::Rejected(text)) => return Response::err(req.id, text),
                Err(AppendError::Branch(error)) => {
                    return ops::append_error_response(req.id, error)
                }
            }
        }
        receipt_response(
            req.id,
            ImportGitHistoryReceipt {
                imported,
                skipped,
                branch_head: head_commit.unwrap_or_default(),
            },
        )
    }
}

#[cfg(all(feature = "redb", feature = "blob"))]
pub use served::handle_import_git_history;

/// `ImportGitHistory` needs the `redb` registry owner file and the blob store.
#[cfg(not(all(feature = "redb", feature = "blob")))]
pub async fn handle_import_git_history(
    _state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    _verified: &VerifiedRequestContext,
    _fields: ImportGitHistoryFields,
) -> Response {
    Response::err(
        req.id,
        "ImportGitHistory requires the `redb` and `blob` features",
    )
}
