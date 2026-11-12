//! `Method::SubmitChangeset` (MEDULLA-SCM-R002): one source changeset on a
//! branch.
//!
//! The request names its blobs by digest; they were uploaded first through
//! the blob methods. Stage one (`verify_submission`) checks the bounds and
//! every blob against the content-addressed store, outside any write
//! transaction. Stage two (`accept`) runs INSIDE the branch registry write
//! transaction, so the head it extends cannot move before the append: it walks
//! the changeset chain, checks the writer's base, fills `old_blob` from the
//! first parent and seals the record. The changeset is stored as the payload
//! of the one branch commit that carries it; the commit row, the payload row
//! and the head advance are one transaction. Every refusal leaves the chain
//! unchanged.

use super::*;

/// The wire field of `Method::SubmitChangeset`.
pub struct SubmitChangesetFields {
    pub request: Box<medulla_types::source_changeset::SubmitChangesetRequest>,
}

#[cfg(all(feature = "redb", feature = "blob"))]
mod served {
    use super::*;
    use medulla_h_graph::graph_ops::branch as ops;
    use medulla_h_ingest::source_changeset::{
        accept, verify_submission, AcceptContext, AcceptError, ChainView,
    };
    use medulla_persistence::branch_registry_store::{AppendError, CommitPlan, PayloadReader};
    use medulla_server_blob::blob::BlobStateExt;
    use medulla_types::source_changeset::{Changeset, SubmitChangesetReceipt};

    /// The chain as the registry transaction reads it.
    struct RegistryChain<'a> {
        head: &'a str,
        read: PayloadReader<'a>,
    }

    impl ChainView for RegistryChain<'_> {
        fn head(&self) -> &str {
            self.head
        }

        fn changeset_at(&self, commit_id: &str) -> Result<Option<Changeset>, String> {
            (self.read)(commit_id)?
                .map(|bytes| decode_changeset(&bytes))
                .transpose()
        }
    }

    fn decode_changeset(bytes: &[u8]) -> Result<Changeset, String> {
        rmp_serde::from_slice(bytes).map_err(|error| error.to_string())
    }

    fn receipt_response(req_id: u64, receipt: SubmitChangesetReceipt) -> Response {
        match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::SubmitChangeset>(
            receipt,
        ) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }

    /// Serve `Method::SubmitChangeset`.
    pub async fn handle_submit_changeset(
        state: &Arc<RwLock<ServerState>>,
        req: &DispatchHeader,
        verified: &VerifiedRequestContext,
        fields: SubmitChangesetFields,
    ) -> Response {
        let request: Arc<_> = Arc::from(fields.request);
        // The repository is the graph the branch belongs to.
        let reference = medulla_types::branch::BranchRef {
            tenant: verified.tenant().to_string(),
            graph: request.repo.clone(),
            name: request.branch.clone(),
        };
        let target = match ops::resolve_target(state, verified, reference).await {
            Ok(target) => target,
            Err(error) => return ops::target_error_response(req.id, error),
        };
        let batch_id = medulla_server_api::batch_compile::branch_batch_id(
            verified.tenant(),
            &request.repo,
            &request.branch,
            Some(verified.principal()),
            verified.idempotency_key(),
        );
        let replay = |commit: medulla_types::branch::BranchCommit, changeset: Changeset| {
            receipt_response(
                req.id,
                SubmitChangesetReceipt {
                    changeset,
                    branch_head: commit.id,
                    replayed: true,
                },
            )
        };
        match target.replayed(&batch_id).await {
            Ok(Some(commit)) => {
                return match target.payload(&commit.id).await {
                    Ok(Some(bytes)) => match decode_changeset(&bytes) {
                        Ok(changeset) => replay(commit, changeset),
                        Err(error) => Response::err(req.id, error),
                    },
                    Ok(None) => Response::err(req.id, "a replayed commit carries no changeset"),
                    Err(error) => Response::err(req.id, error),
                };
            }
            Ok(None) => {}
            Err(error) => return Response::err(req.id, error),
        }

        // Stage one: bounds, pairings and blobs, before any write.
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
        let checked = {
            let request = Arc::clone(&request);
            tokio::task::spawn_blocking(move || {
                verify_submission(
                    store.as_ref(),
                    &request,
                    medulla_types::scheduler_config::DEFAULT_CHANGESET_MAX_BLOB_BYTES,
                )
            })
            .await
        };
        match checked {
            Ok(Ok(())) => {}
            Ok(Err(refusal)) => return Response::err(req.id, refusal.message()),
            Err(error) => return Response::err(req.id, error.to_string()),
        }

        // Stage two: extend the chain inside the registry transaction.
        let author = verified.principal().to_string();
        let created_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        let build_request = Arc::clone(&request);
        let appended = target
            .append_with(verified.tenant(), &batch_id, move |head, read| {
                let chain = RegistryChain { head, read };
                let ctx = AcceptContext {
                    author_principal: &author,
                    created_at_ms,
                };
                let changeset = match accept(&chain, &build_request, &ctx) {
                    Ok(changeset) => changeset,
                    Err(AcceptError::Refused(refusal)) => return Err(refusal.message()),
                    Err(AcceptError::Store(error)) => return Err(error),
                };
                let payload =
                    rmp_serde::to_vec_named(&changeset).map_err(|error| error.to_string())?;
                Ok(CommitPlan {
                    engine_version: 0,
                    manifest_digest: changeset.id.clone(),
                    actor: author.clone(),
                    created_at_ms,
                    payload: Some(payload),
                })
            })
            .await;
        match appended {
            Ok(done) => match target.payload(&done.commit.id).await {
                Ok(Some(bytes)) => match decode_changeset(&bytes) {
                    Ok(changeset) => receipt_response(
                        req.id,
                        SubmitChangesetReceipt {
                            changeset,
                            branch_head: done.commit.id,
                            replayed: done.replayed,
                        },
                    ),
                    Err(error) => Response::err(req.id, error),
                },
                Ok(None) => Response::err(req.id, "an appended commit carries no changeset"),
                Err(error) => Response::err(req.id, error),
            },
            Err(AppendError::Rejected(text)) => Response::err(req.id, text),
            Err(AppendError::Branch(error)) => ops::append_error_response(req.id, error),
        }
    }
}

#[cfg(all(feature = "redb", feature = "blob"))]
pub use served::handle_submit_changeset;

/// `SubmitChangeset` needs the `redb` registry owner file and the blob store.
#[cfg(not(all(feature = "redb", feature = "blob")))]
pub async fn handle_submit_changeset(
    _state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    _verified: &VerifiedRequestContext,
    _fields: SubmitChangesetFields,
) -> Response {
    Response::err(
        req.id,
        "SubmitChangeset requires the `redb` and `blob` features",
    )
}
