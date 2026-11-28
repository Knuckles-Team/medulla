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
    use medulla_compute_assemble::source_tree::TreeChain;
    use medulla_h_graph::graph_ops::branch as ops;
    use medulla_h_ingest::source_tree::read_tree;
    use medulla_mutation_types::branch::BranchRef;
    use medulla_persistence::branch_registry_store::BranchRegistry;
    use medulla_server_auth::access::path_grant;
    use medulla_source_types::source_changeset::{Changeset, ReadTreeReceipt};
    use medulla_source_types::source_path_grant::PathAccess;

    /// The branch chain as a tree read sees it.
    pub(in crate::dispatch::router) struct RegistryTree<'a> {
        pub(in crate::dispatch::router) registry: &'a BranchRegistry,
        pub(in crate::dispatch::router) branch: &'a BranchRef,
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
        // MEDULLA-SCM-R007: the grants are read inside the same blocking snapshot.
        let (tenant, principal) = (
            verified.tenant().to_string(),
            verified.agent_id().to_string(),
        );
        let answered = target
            .read_blocking(move |registry, branch| {
                let grants = registry.path_grants(&tenant, &request.repo, &principal)?;
                let prefix = request.prefix.strip_suffix('/').unwrap_or(&request.prefix);
                if !path_grant::any_allowed_at_or_below(&grants, prefix, PathAccess::Read) {
                    return Ok(Err(format!(
                        "{}: no read grant covers the prefix",
                        PathAccess::Read.denied().code()
                    )));
                }
                let answer = read_tree(&RegistryTree { registry, branch }, &request)?;
                Ok(answer
                    .map(|mut receipt| {
                        receipt.entries.retain(|entry| {
                            path_grant::allows(&grants, &entry.path, PathAccess::Read)
                        });
                        receipt
                    })
                    .map_err(|refusal| refusal.message()))
            })
            .await;
        match answered {
            Ok(Ok(Ok(receipt))) => receipt_response(req.id, receipt),
            Ok(Ok(Err(message))) => Response::err(req.id, message),
            Ok(Err(error)) | Err(error) => Response::err(req.id, error),
        }
    }
}

#[cfg(all(feature = "redb", feature = "blob"))]
pub use served::handle_read_tree;
#[cfg(all(feature = "redb", feature = "blob"))]
pub(in crate::dispatch::router) use served::RegistryTree;

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
