//! `Method::ReadPathHistory` (MEDULLA-SCM-R005.1): the changesets that touched
//! one path at or below a revision of a branch, newest first.
//!
//! The branch is resolved through the same tenant-checked target as every
//! branch read; the answer comes from the history index rows the accept wrote
//! in the branch registry. Reads only.

use super::*;

/// The wire field of `Method::ReadPathHistory`.
pub struct ReadPathHistoryFields {
    pub request: Box<medulla_source_types::source_changeset::ReadPathHistoryRequest>,
}

#[cfg(all(feature = "redb", feature = "blob"))]
mod served {
    use super::*;
    use medulla_compute::source_history::HistoryIndex;
    use medulla_h_graph::graph_ops::branch as ops;
    use medulla_h_ingest::source_history::read_path_history;
    use medulla_mutation_types::branch::BranchRef;
    use medulla_persistence::branch_registry_store::BranchRegistry;
    use medulla_server_auth::access::path_grant;
    use medulla_source_types::source_path_grant::PathAccess;
    use medulla_source_types::source_changeset::{Changeset, ReadPathHistoryReceipt};

    /// The history index rows of one branch as a read sees them.
    pub(in crate::dispatch::router) struct RegistryHistory<'a> {
        pub(in crate::dispatch::router) registry: &'a BranchRegistry,
        pub(in crate::dispatch::router) branch: &'a BranchRef,
    }

    impl HistoryIndex for RegistryHistory<'_> {
        fn row(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
            self.registry.history_row(self.branch, key)
        }

        fn scan_rev(
            &self,
            prefix: &str,
            upper: &str,
            limit: usize,
        ) -> Result<Vec<(String, Vec<u8>)>, String> {
            self.registry
                .history_scan_rev(self.branch, prefix, upper, limit)
        }

        fn changeset_at(&self, commit_id: &str) -> Result<Option<Changeset>, String> {
            self.registry
                .commit_payload(self.branch, commit_id)?
                .map(|bytes| rmp_serde::from_slice(&bytes).map_err(|error| error.to_string()))
                .transpose()
        }
    }

    fn receipt_response(req_id: u64, receipt: ReadPathHistoryReceipt) -> Response {
        match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::ReadPathHistory>(
            receipt,
        ) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }

    /// Serve `Method::ReadPathHistory`.
    pub async fn handle_read_path_history(
        state: &Arc<RwLock<ServerState>>,
        req: &DispatchHeader,
        verified: &VerifiedRequestContext,
        fields: ReadPathHistoryFields,
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
        // MEDULLA-SCM-R007: `read` on the path, from grants read inside the same snapshot.
        let (tenant, principal) = (verified.tenant().to_string(), verified.agent_id().to_string());
        let answered = target
            .read_blocking(move |registry, branch| {
                let grants = registry.path_grants(&tenant, &request.repo, &principal)?;
                if let Err(refusal) = path_grant::check(&grants, &[request.path.as_str()], PathAccess::Read) {
                    return Ok(Err(format!("{}: no read grant covers the path", refusal.code())));
                }
                let head = match registry.get(branch)? {
                    Some(row) => row.head_commit,
                    None => return Err("the resolved branch has no registry row".to_string()),
                };
                Ok(read_path_history(&RegistryHistory { registry, branch }, &head, &request)?
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
pub use served::handle_read_path_history;
#[cfg(all(feature = "redb", feature = "blob"))]
pub(in crate::dispatch::router) use served::RegistryHistory;

/// `ReadPathHistory` needs the `redb` branch registry and the `blob` ingest handlers.
#[cfg(not(all(feature = "redb", feature = "blob")))]
pub async fn handle_read_path_history(
    _state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    _verified: &VerifiedRequestContext,
    _fields: ReadPathHistoryFields,
) -> Response {
    Response::err(
        req.id,
        "ReadPathHistory requires the `redb` and `blob` features",
    )
}
