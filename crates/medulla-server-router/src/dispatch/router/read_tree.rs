//! `Method::ReadTree` (MEDULLA-SCM-R004): the files of a repository under a
//! prefix at one changeset revision of a branch.
//!
//! The branch is resolved through the same tenant-checked target as every
//! branch read; the chain is then read from the branch registry and replayed
//! from the nearest stored tree checkpoint by `read_tree`. Reads only.

use super::*;

/// The wire field of `Method::ReadTree`.
pub struct ReadTreeFields {
    pub request: Box<medulla_source_types::source_changeset::ReadTreeRequest>,
}

#[cfg(all(feature = "redb", feature = "blob"))]
mod served {
    use super::*;
    use medulla_compute::source_tree::TreeChain;
    use medulla_h_graph::graph_ops::branch as ops;
    use medulla_h_ingest::source_tree::read_tree;
    use medulla_mutation_types::branch::BranchRef;
    use medulla_persistence::branch_registry_store::BranchRegistry;
    use medulla_source_types::source_changeset::{Changeset, ReadTreeReceipt};

    /// The branch chain as a tree read sees it.
    struct RegistryTree<'a> {
        registry: &'a BranchRegistry,
        branch: &'a BranchRef,
    }

    impl TreeChain for RegistryTree<'_> {
        fn changeset_at(&self, rev: &str) -> Result<Option<Changeset>, String> {
            self.registry
                .commit_payload(self.branch, rev)?
                .map(|bytes| rmp_serde::from_slice(&bytes).map_err(|error| error.to_string()))
                .transpose()
        }

        fn checkpoint_at(&self, rev: &str) -> Result<Option<Vec<u8>>, String> {
            self.registry.commit_checkpoint(self.branch, rev)
        }
    }

    fn receipt_response(req_id: u64, receipt: ReadTreeReceipt) -> Response {
        match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::ReadTree>(
            receipt,
        ) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }

    /// Serve `Method::ReadTree`.
    pub async fn handle_read_tree(
        state: &Arc<RwLock<ServerState>>,
        req: &DispatchHeader,
        verified: &VerifiedRequestContext,
        fields: ReadTreeFields,
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
        let answered = target
            .read_blocking(move |registry, branch| {
                read_tree(&RegistryTree { registry, branch }, &request)
            })
            .await;
        match answered {
            Ok(Ok(Ok(receipt))) => receipt_response(req.id, receipt),
            Ok(Ok(Err(refusal))) => Response::err(req.id, refusal.message()),
            Ok(Err(error)) | Err(error) => Response::err(req.id, error),
        }
    }
}

#[cfg(all(feature = "redb", feature = "blob"))]
pub use served::handle_read_tree;

/// `ReadTree` needs the `redb` branch registry and the `blob` ingest handlers.
#[cfg(not(all(feature = "redb", feature = "blob")))]
pub async fn handle_read_tree(
    _state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    _verified: &VerifiedRequestContext,
    _fields: ReadTreeFields,
) -> Response {
    Response::err(req.id, "ReadTree requires the `redb` and `blob` features")
}
