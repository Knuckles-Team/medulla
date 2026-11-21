//! `Method::ReadBlame` (MEDULLA-SCM-R005.2): the origin changeset of every
//! line of one path at a revision of a branch.
//!
//! The branch is resolved through the same tenant-checked target as every
//! branch read; the answer comes from the blame rows the accept wrote in the
//! branch registry. Reads only.

use super::*;

/// The wire field of `Method::ReadBlame`.
pub struct ReadBlameFields {
    pub request: Box<medulla_source_types::source_changeset::ReadBlameRequest>,
}

#[cfg(all(feature = "redb", feature = "blob"))]
mod served {
    use super::*;
    use medulla_h_graph::graph_ops::branch as ops;
    use medulla_h_ingest::source_history::read_blame;
    use medulla_mutation_types::branch::BranchRef;
    use super::super::read_path_history::RegistryHistory;
    use medulla_source_types::source_changeset::ReadBlameReceipt;

    fn receipt_response(req_id: u64, receipt: ReadBlameReceipt) -> Response {
        match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::ReadBlame>(
            receipt,
        ) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }

    /// Serve `Method::ReadBlame`.
    pub async fn handle_read_blame(
        state: &Arc<RwLock<ServerState>>,
        req: &DispatchHeader,
        verified: &VerifiedRequestContext,
        fields: ReadBlameFields,
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
                let head = match registry.get(branch)? {
                    Some(row) => row.head_commit,
                    None => return Err("the resolved branch has no registry row".to_string()),
                };
                read_blame(&RegistryHistory { registry, branch }, &head, &request)
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
pub use served::handle_read_blame;

/// `ReadBlame` needs the `redb` branch registry and the `blob` ingest handlers.
#[cfg(not(all(feature = "redb", feature = "blob")))]
pub async fn handle_read_blame(
    _state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    _verified: &VerifiedRequestContext,
    _fields: ReadBlameFields,
) -> Response {
    Response::err(
        req.id,
        "ReadBlame requires the `redb` and `blob` features",
    )
}
