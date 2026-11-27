//! `Method::GitRefsList` (MEDULLA-SCM-R851): the refs of a repository with the git commit
//! id of each head.
//!
//! The refs are the live branches of the verified tenant's repository, read from the one
//! branch registry. Inside one blocking read the handler walks each branch chain, checks the
//! caller's read grants on every path of every chain (one denied path refuses the whole
//! request with `PATH_READ_DENIED`, the check `GitExportPack` makes) and hashes the head
//! commit with `medulla_h_ingest::source_changeset::git::head_commit_id`, the same commit
//! bytes the export builds. A branch with no changeset has no head object and is not listed.
//! Reads only.

use super::*;

/// The wire field of `Method::GitRefsList`.
pub struct GitRefsListFields {
    pub request: Box<medulla_source_types::source_changeset::GitRefsListRequest>,
}

#[cfg(all(feature = "redb", feature = "blob"))]
mod served {
    use super::super::read_tree::RegistryTree;
    use super::*;
    use medulla_compute::source_tree::TreeChain;
    use medulla_h_ingest::source_changeset::git::head_commit_id;
    use medulla_h_ingest::source_changeset::ChunkStoreBlobs;
    use medulla_mutation_types::branch::BranchRef;
    use medulla_persistence::branch_registry_store::{shared_branch_registry, BranchRegistry};
    use medulla_server_auth::access::path_grant;
    use medulla_server_blob::blob::store::ChunkStore;
    use medulla_server_blob::blob::BlobStateExt;
    use medulla_source_types::source_changeset::{git_ref_name, GitRef, GitRefsListRequest};
    use medulla_source_types::source_path_grant::{PathAccess, PathGrant};

    /// Every path the branch chain from `head` back to the root changed.
    fn chain_paths(tree: &RegistryTree<'_>, head: &str) -> Result<Vec<String>, String> {
        let mut paths = Vec::new();
        let mut cursor = head.to_string();
        while let Some(changeset) = TreeChain::changeset_at(tree, &cursor)? {
            let next = changeset.parents.first().cloned().unwrap_or_default();
            paths.extend(
                changeset
                    .changed_paths
                    .iter()
                    .map(|changed| changed.path.clone()),
            );
            cursor = next;
        }
        Ok(paths)
    }

    /// The refs of `request.repo` under `grants`: an `Err` text is one refusal, `Ok` the list.
    pub(super) fn refs_of(
        registry: &BranchRegistry,
        blobs: &ChunkStoreBlobs<'_>,
        tenant: &str,
        request: &GitRefsListRequest,
        grants: &[PathGrant],
    ) -> Result<Result<Vec<GitRef>, String>, String> {
        let mut refs = Vec::new();
        for row in registry.live_branches_of(tenant, &request.repo)? {
            let reference = BranchRef {
                tenant: tenant.to_string(),
                graph: request.repo.clone(),
                name: row.name.clone(),
            };
            let tree = RegistryTree {
                registry,
                branch: &reference,
            };
            let paths = chain_paths(&tree, &row.head_commit)?;
            let wanted: Vec<&str> = paths.iter().map(String::as_str).collect();
            if let Err(denied) = path_grant::check(grants, &wanted, PathAccess::Read) {
                return Ok(Err(format!(
                    "{}: no read grant covers a path of branch {}",
                    denied.code(),
                    row.name
                )));
            }
            match head_commit_id(&tree, blobs, &row.head_commit) {
                Ok(Some((_, object_id))) => refs.push(GitRef {
                    ref_name: git_ref_name(&row.name),
                    object_id,
                }),
                Ok(None) => {}
                Err(refusal) => return Ok(Err(refusal.message())),
            }
        }
        Ok(Ok(refs))
    }

    fn refs_response(req_id: u64, refs: Vec<GitRef>) -> Response {
        match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::GitRefsList>(
            refs,
        ) {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }

    /// Serve `Method::GitRefsList`.
    pub async fn handle_git_refs_list(
        state: &Arc<RwLock<ServerState>>,
        req: &DispatchHeader,
        verified: &VerifiedRequestContext,
        fields: GitRefsListFields,
    ) -> Response {
        let request = fields.request;
        let (registry, store): (Arc<BranchRegistry>, Arc<dyn ChunkStore>) = {
            let s = state.read().await;
            let registry = match shared_branch_registry(&s) {
                Ok(registry) => registry,
                Err(error) => return Response::err(req.id, error),
            };
            match s.blob_cursors() {
                Some(cursors) => (registry, Arc::clone(&cursors.store)),
                None => {
                    return Response::err(
                        req.id,
                        "Blob substrate disabled (no persist dir configured)",
                    )
                }
            }
        };
        let (tenant, principal) = (verified.tenant().to_string(), verified.agent_id().to_string());
        let owner = match medulla_server_api::carrier::CarrierAuthority::from_verified(verified) {
            Ok(authority) => authority.owner_scope().to_string(),
            Err(error) => return Response::err(req.id, error),
        };
        let answered = tokio::task::spawn_blocking(move || {
            let grants = registry.path_grants(&tenant, &request.repo, &principal)?;
            let blobs = ChunkStoreBlobs(store.as_ref(), &owner);
            refs_of(&registry, &blobs, &tenant, &request, &grants)
        })
        .await
        .map_err(|error| error.to_string());
        match answered {
            Ok(Ok(Ok(refs))) => refs_response(req.id, refs),
            Ok(Ok(Err(message))) => Response::err(req.id, message),
            Ok(Err(error)) | Err(error) => Response::err(req.id, error),
        }
    }
}

#[cfg(all(feature = "redb", feature = "blob"))]
pub use served::handle_git_refs_list;

/// `GitRefsList` needs the `redb` branch registry and the `blob` store.
#[cfg(not(all(feature = "redb", feature = "blob")))]
pub async fn handle_git_refs_list(
    _state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    _verified: &VerifiedRequestContext,
    _fields: GitRefsListFields,
) -> Response {
    Response::err(
        req.id,
        "GitRefsList requires the `redb` and `blob` features",
    )
}
