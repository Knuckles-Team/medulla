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
    use medulla_compute_assemble::source_tree::TreeChain;
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

    /// Store `pack` as one content-addressed blob and answer its reference: the pack is
    /// never carried in the reply (MEDULLA-SCM-R014).
    pub(super) fn pack_receipt(
        store: &dyn ChunkStore,
        owner: &str,
        object_format: medulla_source_types::source_changeset::GitObjectFormat,
        pack: &[u8],
    ) -> Result<GitExportPackReceipt, String> {
        let blob = medulla_server_blob::blob::stream::stream_blob_put_owned(
            store,
            std::io::Cursor::new(pack),
            0,
            owner,
        )?;
        Ok(GitExportPackReceipt {
            object_format,
            pack_ref: format!("cas:sha256:{}", blob.digest),
            pack_bytes: pack.len() as u64,
        })
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
        let (tenant, principal) = (
            verified.tenant().to_string(),
            verified.agent_id().to_string(),
        );
        // The pack lands under the caller's blob owner scope, the scope the caller's own
        // blob read of `pack_ref` uses.
        let owner = match medulla_server_api::carrier::CarrierAuthority::from_verified(verified) {
            Ok(authority) => authority.owner_scope().to_string(),
            Err(error) => return Response::err(req.id, error),
        };
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
                let resolved_haves: Vec<String> = resolve_revs(&chain, &request.haves)
                    .into_iter()
                    .flatten()
                    .collect();
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
                let blobs = ChunkStoreBlobs(store.as_ref(), &owner);
                let (object_format, pack) = match export_pack_with_format(
                    &tree,
                    &blobs,
                    &resolved_wants,
                    &resolved_haves,
                ) {
                    Ok(exported) => exported,
                    Err(refusal) => return Ok(Err(refusal.message())),
                };
                Ok(pack_receipt(store.as_ref(), &owner, object_format, &pack))
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

#[cfg(all(test, feature = "redb", feature = "blob"))]
mod tests {
    use super::served::pack_receipt;
    use medulla_server_blob::blob::store::RedbChunkStore;
    use medulla_server_blob::blob::stream::stream_blob_get;
    use medulla_source_types::source_changeset::GitObjectFormat;

    // spec: MEDULLA-SCM-R014
    #[test]
    fn receipt_carries_a_blob_ref_not_hex() {
        let dir = std::env::temp_dir().join(format!(
            "git-export-pack-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        let store = RedbChunkStore::open(dir.to_str().expect("utf-8 temp path")).expect("store");
        let pack = b"PACK\x00\x00\x00\x02 a pack body that is long enough to matter".to_vec();
        let receipt =
            pack_receipt(&store, "owner", GitObjectFormat::Sha1, &pack).expect("stores the pack");
        assert!(receipt.pack_ref.starts_with("cas:sha256:"));
        assert_eq!(receipt.pack_bytes, pack.len() as u64);
        let digest = receipt.pack_ref.trim_start_matches("cas:sha256:");
        let mut read = Vec::new();
        stream_blob_get(&store, digest, &mut read).expect("reads the blob back");
        assert_eq!(read, pack);
    }
}

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
