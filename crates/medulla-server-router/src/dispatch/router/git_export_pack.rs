//! `Method::GitExportPack` (MEDULLA-SCM-R014): the changeset chain of a branch
//! as a version-2 git pack.
//!
//! The branch resolves through the same tenant-checked target as every branch
//! read. Inside one registry snapshot the handler walks the chain, resolves the
//! wants and haves (git commit ids or branch commit ids), checks the caller's
//! read grants on every path of the exported chain (one denied path refuses the
//! whole request with `PATH_READ_DENIED`), and builds the pack with
//! `medulla_h_ingest::source_changeset::git::export_pack_with_format`, the one
//! export function of MEDULLA-SCM-R010. Reads only.

use super::*;

/// The wire field of `Method::GitExportPack`.
pub struct GitExportPackFields {
    pub request: Box<medulla_source_types::source_changeset::GitExportPackRequest>,
}

#[cfg(all(feature = "redb", feature = "blob"))]
mod served {
    use super::super::read_tree::RegistryTree;
    use super::*;
    use medulla_compute::source_tree::TreeChain;
    use medulla_h_graph::graph_ops::branch as ops;
    use medulla_h_ingest::source_changeset::git::{export_pack_with_format, resolve_revs};
    use medulla_h_ingest::source_changeset::ChunkStoreBlobs;
    use medulla_mutation_types::branch::BranchRef;
    use medulla_server_auth::access::path_grant;
    use medulla_server_blob::blob::store::ChunkStore;
    use medulla_server_blob::blob::BlobStateExt;
    use medulla_source_types::source_changeset::{
        Changeset, ChangesetRefusal, GitExportPackReceipt, SubmitRefusal,
    };
    use medulla_source_types::source_path_grant::PathAccess;

    /// Lowercase hex of `bytes`: the result contract is JSON, so the pack travels as text.
    fn to_hex(bytes: &[u8]) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            out.push(DIGITS[(byte >> 4) as usize] as char);
            out.push(DIGITS[(byte & 0x0f) as usize] as char);
        }
        out
    }

    fn receipt_response(req_id: u64, receipt: GitExportPackReceipt) -> Response {
        match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::GitExportPack>(
            receipt,
        ) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }

    /// Serve `Method::GitExportPack`.
    pub async fn handle_git_export_pack(
        state: &Arc<RwLock<ServerState>>,
        req: &DispatchHeader,
        verified: &VerifiedRequestContext,
        fields: GitExportPackFields,
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
        let (tenant, principal) = (verified.tenant().to_string(), verified.agent_id().to_string());
        let answered = target
            .read_blocking(move |registry, branch| {
                let grants = registry.path_grants(&tenant, &request.repo, &principal)?;
                let head = match registry.get(branch)? {
                    Some(row) => row.head_commit,
                    None => return Err("the resolved branch has no registry row".to_string()),
                };
                let tree = RegistryTree { registry, branch };
                // The chain from the head back, as `(branch commit id, changeset)`.
                let mut chain: Vec<(String, Changeset)> = Vec::new();
                let mut cursor = head;
                while let Some(changeset) = TreeChain::changeset_at(&tree, &cursor)? {
                    let next = changeset.parents.first().cloned().unwrap_or_default();
                    chain.push((cursor, changeset));
                    cursor = next;
                }
                let wants = resolve_revs(&chain, &request.wants);
                let mut resolved_wants = Vec::with_capacity(wants.len());
                for (want, rev) in request.wants.iter().zip(wants) {
                    match rev {
                        Some(rev) => resolved_wants.push(rev),
                        None => {
                            return Ok(Err(SubmitRefusal::new(
                                ChangesetRefusal::RevisionNotOnChain,
                                format!("want {want} is not a changeset of the branch"),
                            )
                            .message()))
                        }
                    }
                }
                // A have the chain does not know is not held by the chain; git
                // ignores such a have.
                let resolved_haves: Vec<String> =
                    resolve_revs(&chain, &request.haves).into_iter().flatten().collect();
                // MEDULLA-SCM-R007: read access on every path the export carries.
                let mut paths: Vec<&str> = chain
                    .iter()
                    .flat_map(|(_, changeset)| changeset.changed_paths.iter())
                    .map(|changed| changed.path.as_str())
                    .collect();
                paths.sort_unstable();
                paths.dedup();
                if let Err(denied) = path_grant::check(&grants, &paths, PathAccess::Read) {
                    return Ok(Err(format!(
                        "{}: no read grant covers an exported path",
                        denied.code()
                    )));
                }
                let blobs = ChunkStoreBlobs(store.as_ref(), "");
                Ok(
                    export_pack_with_format(&tree, &blobs, &resolved_wants, &resolved_haves)
                        .map(|(object_format, pack)| GitExportPackReceipt {
                            object_format,
                            pack_hex: to_hex(&pack),
                        })
                        .map_err(|refusal| refusal.message()),
                )
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
pub use served::handle_git_export_pack;

/// `GitExportPack` needs the `redb` branch registry and the `blob` store.
#[cfg(not(all(feature = "redb", feature = "blob")))]
pub async fn handle_git_export_pack(
    _state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    _verified: &VerifiedRequestContext,
    _fields: GitExportPackFields,
) -> Response {
    Response::err(
        req.id,
        "GitExportPack requires the `redb` and `blob` features",
    )
}
