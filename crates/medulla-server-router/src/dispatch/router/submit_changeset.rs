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
//! unchanged. A stale base whose paths overlap the changesets after it is
//! rebased per path through `merge_file` (MEDULLA-BRANCH-MERGE-R010) or
//! refused with `SOURCE_CONFLICT`.

use super::*;

/// The wire field of `Method::SubmitChangeset`.
pub struct SubmitChangesetFields {
    pub request: Box<medulla_source_types::source_changeset::SubmitChangesetRequest>,
}

#[cfg(all(feature = "redb", feature = "blob"))]
mod served {
    use super::*;
    use medulla_h_graph::graph_ops::branch as ops;
    use medulla_h_ingest::source_changeset::provenance::{self, RunTraceLookup};
    use medulla_h_ingest::source_changeset::{
        accept, verify_submission, AcceptContext, AcceptError, ChainView, ChunkStoreBlobs,
    };
    use medulla_h_ingest::source_history::index_rows;
    use medulla_h_ingest::source_tree::checkpoint_after;
    use medulla_persistence::branch_registry_store::{AppendError, CommitPlan, PayloadReader};
    use medulla_server_auth::access::path_grant;
    use medulla_server_blob::blob::store::ChunkStore;
    use medulla_server_blob::blob::BlobStateExt;
    use medulla_source_types::source_changeset::decode_changeset;
    use medulla_source_types::source_changeset::{Changeset, GitOrigin, SubmitChangesetReceipt};
    use medulla_source_types::source_path_grant::PathAccess;

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

    /// The caller's view of the repository graph (MEDULLA-SCM-R012): a node
    /// the caller may not see answers like an absent node.
    struct RepoGraph {
        core: Arc<medulla_graph_core::graph::GraphCore>,
        isolation: medulla_isolation::isolation::IsolationLayer,
        caller: String,
    }

    impl RunTraceLookup for RepoGraph {
        fn node_kind(&self, node_id: &str) -> Result<Option<String>, String> {
            let Some(blob) = self.core.get_node_properties(node_id) else {
                return Ok(None);
            };
            #[cfg(feature = "security")]
            if !self.isolation.can_see_row(
                &self.caller,
                &medulla_graph_core::rls::row_visibility(&blob),
            ) {
                return Ok(None);
            }
            #[cfg(not(feature = "security"))]
            let _ = (&self.isolation, &self.caller);
            let properties: serde_json::Value =
                rmp_serde::from_slice(&blob).map_err(|error| error.to_string())?;
            Ok(properties
                .get("kind")
                .and_then(|kind| kind.as_str())
                .map(str::to_string))
        }
    }

    /// A lookup for a repository graph that is not registered: no node exists.
    struct NoGraph;

    impl RunTraceLookup for NoGraph {
        fn node_kind(&self, _node_id: &str) -> Result<Option<String>, String> {
            Ok(None)
        }
    }

    /// Open the repository graph for the verified caller at read level, after
    /// `check_graph_access`. `None` when no graph of that name is registered.
    async fn open_repo_graph(
        state: &Arc<RwLock<ServerState>>,
        verified: &VerifiedRequestContext,
        repo: &str,
    ) -> Result<Option<RepoGraph>, String> {
        medulla_server_api::api::reentry::ensure_graph_resident(state, repo).await;
        let guard = state.read().await;
        let Some(entry) = guard.registry.get(repo) else {
            return Ok(None);
        };
        medulla_server_auth::access::check_graph_access(
            &guard.isolation,
            Some(verified.agent_id()),
            repo,
            entry.graph_type,
            entry.owner.as_deref(),
            entry.importing.as_deref(),
            medulla_isolation::isolation::AccessLevel::Read,
        )?;
        Ok(Some(RepoGraph {
            core: Arc::clone(&entry.core),
            isolation: guard.isolation.clone(),
            caller: verified.agent_id().to_string(),
        }))
    }

    /// What `append_changeset` needs besides the target and the batch id.
    pub struct ChangesetAppend {
        pub request: Arc<medulla_source_types::source_changeset::SubmitChangesetRequest>,
        /// The chunk store the merge and the blame rows read blobs through.
        pub store: Arc<dyn ChunkStore>,
        /// The blob owner scope a merged blob lands under.
        pub merge_owner: String,
        pub author: String,
        pub agent_identity: Option<String>,
        pub created_at_ms: u64,
        /// The git commit an imported changeset records (MEDULLA-SCM-R010);
        /// `None` for `SubmitChangeset`.
        pub git_origin: Option<GitOrigin>,
        /// The evidence of a served symbol edit (MEDULLA-SCM-R013.6); `None` otherwise.
        pub symbol_edit: Option<medulla_source_types::source_symbol_edit::SymbolEditEvidence>,
    }

    /// Stage two of `SubmitChangeset` and of `ImportGitHistory`: extend the chain
    /// inside the registry transaction through the one `accept`, with the
    /// checkpoint and the history rows of the changeset (MEDULLA-SCM-R002,
    /// MEDULLA-SCM-R004, MEDULLA-SCM-R005.1, MEDULLA-SCM-R010).
    pub async fn append_changeset(
        target: &ops::BranchTarget,
        tenant: &str,
        batch_id: &str,
        input: ChangesetAppend,
    ) -> Result<medulla_persistence::branch_registry_store::AppendedCommit, AppendError> {
        let ChangesetAppend {
            request,
            store,
            merge_owner,
            author,
            agent_identity,
            created_at_ms,
            git_origin,
            symbol_edit,
        } = input;
        let branch_ref = target.reference.clone();
        let build_batch = batch_id.to_string();
        target
            .append_with(tenant, batch_id, move |head, read, history| {
                let chain = RegistryChain { head, read };
                let blobs = ChunkStoreBlobs(store.as_ref(), &merge_owner);
                let ctx = AcceptContext {
                    author_principal: &author,
                    created_at_ms,
                    blobs: &blobs,
                    // MEDULLA-BRANCH-MERGE-R010: an overlapping path merges by line.
                    merge: ops::line_merge::FILE_MERGER,
                    agent_identity: agent_identity.as_deref(),
                    git_origin,
                    symbol_edit: symbol_edit.clone(),
                };
                let changeset = match accept(&chain, &request, &ctx) {
                    Ok(changeset) => changeset,
                    Err(AcceptError::Refused(refusal)) => return Err(refusal.message()),
                    Err(AcceptError::Store(error)) => return Err(error),
                };
                let payload =
                    rmp_serde::to_vec_named(&changeset).map_err(|error| error.to_string())?;
                // MEDULLA-SCM-R004: every `tree_checkpoint_interval`th changeset
                // commits a full tree snapshot with it.
                let checkpoint = checkpoint_after(
                    &chain,
                    &changeset,
                    medulla_work_types::scheduler_config::DEFAULT_TREE_CHECKPOINT_INTERVAL,
                )?;
                // MEDULLA-SCM-R005.1: the history rows commit with the changeset,
                // keyed by the id the registry gives the carrying commit.
                let commit_id = medulla_mutation_types::branch::branch_commit_id(
                    &branch_ref,
                    head,
                    &build_batch,
                    &changeset.id,
                );
                let history_rows = index_rows(history, &chain, &changeset, &commit_id, &blobs)?;
                Ok(CommitPlan {
                    engine_version: 0,
                    manifest_digest: changeset.id.clone(),
                    actor: author.clone(),
                    created_at_ms,
                    payload: Some(payload),
                    checkpoint,
                    history_rows,
                })
            })
            .await
    }

    fn receipt_response(req_id: u64, receipt: SubmitChangesetReceipt) -> Response {
        match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::SubmitChangeset>(
            receipt,
        ) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }

    /// Serve `Method::SubmitChangeset`. A request carries no evidence: it is server-derived.
    pub async fn handle_submit_changeset(
        state: &Arc<RwLock<ServerState>>,
        req: &DispatchHeader,
        verified: &VerifiedRequestContext,
        fields: SubmitChangesetFields,
    ) -> Response {
        submit_changeset_with(state, req, verified, fields, None).await
    }

    /// The one accept path: `SubmitChangeset` passes no evidence, `SubmitSymbolEdit` passes the
    /// evidence it derived (MEDULLA-SCM-R013.6).
    pub async fn submit_changeset_with(
        state: &Arc<RwLock<ServerState>>,
        req: &DispatchHeader,
        verified: &VerifiedRequestContext,
        fields: SubmitChangesetFields,
        symbol_edit: Option<medulla_source_types::source_symbol_edit::SymbolEditEvidence>,
    ) -> Response {
        let request: Arc<medulla_source_types::source_changeset::SubmitChangesetRequest> =
            Arc::from(fields.request);
        // The repository is the graph the branch belongs to.
        let reference = medulla_mutation_types::branch::BranchRef {
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
        let replay = |commit: medulla_mutation_types::branch::BranchCommit,
                      changeset: Changeset| {
            receipt_response(
                req.id,
                SubmitChangesetReceipt {
                    changeset,
                    branch_head: commit.id,
                    replayed: true,
                    index: None,
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

        // MEDULLA-SCM-R007: write access on every path and rename source, before
        // any blob is read.
        let grants = match target
            .path_grants(verified.tenant(), &request.repo, verified.agent_id())
            .await
        {
            Ok(grants) => grants,
            Err(error) => return Response::err(req.id, error),
        };
        let touched: Vec<&str> = request
            .paths
            .iter()
            .flat_map(|entry| {
                std::iter::once(entry.path.as_str()).chain(entry.renamed_from.as_deref())
            })
            .collect();
        if let Err(refusal) = path_grant::check(&grants, &touched, PathAccess::Write) {
            return Response::err(
                req.id,
                format!("{}: no write grant covers a submitted path", refusal.code()),
            );
        }

        // MEDULLA-SDLC-R017.4: a loop-authored edit of the spec tree lands only as the delivery
        // of an approved proposal, before any blob is read and before any write.
        if let Err(error) = super::super::sdlc::check_spec_edit_provenance(
            state,
            verified,
            req.id,
            &request.repo,
            &touched,
            &request.message,
        )
        .await
        {
            return Response::err(req.id, error);
        }

        // Stage one: bounds and pairings, then the agent provenance of
        // MEDULLA-SCM-R012, before any blob is read and before any write.
        if let Err(refusal) = request.validate() {
            return Response::err(req.id, refusal.message());
        }
        let agent_identity: Option<String> =
            (verified.agent_id() != verified.principal()).then(|| verified.agent_id().to_string());
        let graph = match request.run_trace_id {
            Some(_) => match open_repo_graph(state, verified, &request.repo).await {
                Ok(graph) => graph,
                Err(error) => return Response::err(req.id, error),
            },
            None => None,
        };
        let lookup: &dyn RunTraceLookup = match &graph {
            Some(graph) => graph,
            None => &NoGraph,
        };
        match provenance::check(
            agent_identity.as_deref(),
            request.run_trace_id.as_deref(),
            lookup,
        ) {
            Ok(()) => {}
            Err(AcceptError::Refused(refusal)) => return Response::err(req.id, refusal.message()),
            Err(AcceptError::Store(error)) => return Response::err(req.id, error),
        }

        // Stage one: blobs, before any write.
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
        // A merged blob lands under the submitter's blob owner scope, the scope its
        // own fetch of the landed bytes reads (MEDULLA-BRANCH-MERGE-R010).
        let merge_owner =
            match medulla_server_api::carrier::CarrierAuthority::from_verified(verified) {
                Ok(authority) => authority.owner_scope().to_string(),
                Err(error) => return Response::err(req.id, error),
            };
        let checked = {
            let request = Arc::clone(&request);
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                verify_submission(
                    store.as_ref(),
                    &request,
                    medulla_work_types::scheduler_config::DEFAULT_CHANGESET_MAX_BLOB_BYTES,
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
        let appended = append_changeset(
            &target,
            verified.tenant(),
            &batch_id,
            ChangesetAppend {
                request: Arc::clone(&request),
                store: Arc::clone(&store),
                merge_owner: merge_owner.clone(),
                author,
                agent_identity,
                created_at_ms,
                git_origin: None,
                symbol_edit,
            },
        )
        .await;
        match appended {
            Ok(done) => match target.payload(&done.commit.id).await {
                Ok(Some(bytes)) => match decode_changeset(&bytes) {
                    Ok(changeset) => {
                        // MEDULLA-SCM-R006: the code graph follows the accepted
                        // changeset before the receipt is built; a replay indexes
                        // nothing.
                        #[cfg(feature = "ast")]
                        let index = if done.replayed {
                            None
                        } else {
                            super::changeset_index::reindex(
                                state,
                                verified,
                                Arc::clone(&store),
                                merge_owner.clone(),
                                changeset.clone(),
                                done.commit.id.clone(),
                                req.id,
                            )
                            .await
                        };
                        #[cfg(not(feature = "ast"))]
                        let index = None;
                        // MEDULLA-SDLC-R001.3: the changeset node and its derived lifecycle
                        // edges are written in one batch after the code graph follows the
                        // changeset. A replay runs it again; the write is idempotent.
                        #[cfg(feature = "ast")]
                        {
                            let commit_id = done.commit.id.clone();
                            let seq = match target
                                .read_blocking(move |registry, branch| {
                                    registry.history_row(
                                        branch,
                                        &medulla_compute::source_history::seq_key(&commit_id),
                                    )
                                })
                                .await
                            {
                                Ok(Ok(Some(bytes))) => {
                                    medulla_compute::source_history::decode_seq(&bytes)
                                }
                                Ok(Ok(None)) => {
                                    Err("the history index holds no seq for the accepted changeset"
                                        .to_string())
                                }
                                Ok(Err(error)) | Err(error) => Err(error),
                            };
                            let accepted = match seq {
                                Ok(seq) => {
                                    medulla_server_graph::dispatch::changeset_lifecycle::accept_edges(
                                        state,
                                        verified,
                                        req.id,
                                        Arc::clone(&store),
                                        merge_owner,
                                        medulla_server_graph::dispatch::changeset_lifecycle::AcceptedChangeset {
                                            changeset: &changeset,
                                            branch: &request.branch,
                                            seq,
                                            chain: medulla_server_graph::dispatch::changeset_lifecycle::BranchChain::new(
                                                target.registry(),
                                                target.reference.clone(),
                                            ),
                                        },
                                    )
                                    .await
                                }
                                Err(error) => Err(error),
                            };
                            if let Err(error) = accepted {
                                return Response::err(req.id, error);
                            }
                        }
                        receipt_response(
                            req.id,
                            SubmitChangesetReceipt {
                                changeset,
                                branch_head: done.commit.id,
                                replayed: done.replayed,
                                index,
                            },
                        )
                    }
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
#[cfg(all(feature = "redb", feature = "blob"))]
pub(in crate::dispatch::router) use served::submit_changeset_with;
#[cfg(all(feature = "redb", feature = "blob"))]
pub(in crate::dispatch::router) use served::{append_changeset, ChangesetAppend};

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
