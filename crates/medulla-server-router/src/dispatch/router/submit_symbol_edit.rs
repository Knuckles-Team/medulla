//! `Method::SubmitSymbolEdit` (MEDULLA-SCM-R013.4): a graph-native code edit becomes one
//! changeset through the one accept path.
//!
//! Order: read access on the files, resolve each symbol over the code graph and the tree of
//! `base_rev`, `prepare` (project, then `verify_roundtrip`), then `SubmitChangeset`'s own handler
//! (write grants, blobs, accept, re-index). A refusal before the last step writes nothing.

use super::*;

/// The wire field of `Method::SubmitSymbolEdit`.
pub struct SubmitSymbolEditFields {
    pub request: Box<medulla_source_types::source_symbol_edit::SymbolEdit>,
}

#[cfg(all(feature = "redb", feature = "blob"))]
mod served {
    use super::super::read_tree::RegistryTree;
    use super::super::submit_changeset::{submit_changeset_with, SubmitChangesetFields};
    use super::*;
    use medulla_compute::source_symbol_edit::{ResolvedSymbol, SymbolResolver};
    use medulla_h_graph::graph_ops::branch as ops;
    use medulla_h_ingest::source_changeset::ChunkStoreBlobs;
    use medulla_h_ingest::source_symbol_edit::prepare;
    use medulla_h_ingest::source_tree::read_tree;
    use medulla_mutation_types::branch::BranchRef;
    use medulla_server_auth::access::path_grant;
    use medulla_server_blob::blob::BlobStateExt;
    use medulla_server_graph::dispatch::shared::{
        authorized_graph_read, ensure_graph_resident, timed_read,
    };
    use medulla_source_types::source_changeset::ReadTreeRequest;
    use medulla_source_types::source_path_grant::PathAccess;
    use serde_json::Value;
    use std::collections::{BTreeMap, BTreeSet};

    /// Symbols resolved before projection: the projection trait is synchronous, the reads are not.
    struct Prefetched(BTreeMap<String, ResolvedSymbol>);

    impl SymbolResolver for Prefetched {
        fn resolve(&self, _base_rev: &str, symbol_id: &str) -> Option<ResolvedSymbol> {
            self.0.get(symbol_id).cloned()
        }
    }

    fn text<'a>(node: &'a Value, key: &str) -> Option<&'a str> {
        node.get(key).and_then(Value::as_str)
    }

    fn number(node: &Value, key: &str) -> Option<usize> {
        match node.get(key)? {
            Value::String(text) => text.parse().ok(),
            other => other.as_u64().map(|n| n as usize),
        }
    }

    /// Serve `Method::SubmitSymbolEdit`.
    pub async fn handle_submit_symbol_edit(
        state: &Arc<RwLock<ServerState>>,
        req: &DispatchHeader,
        verified: &VerifiedRequestContext,
        fields: SubmitSymbolEditFields,
    ) -> Response {
        let edit = fields.request;
        let reference = BranchRef {
            tenant: verified.tenant().to_string(),
            graph: edit.repo.clone(),
            name: edit.branch.clone(),
        };
        let target = match ops::resolve_target(state, verified, reference).await {
            Ok(target) => target,
            Err(error) => return ops::target_error_response(req.id, error),
        };
        // The code graph: one node per symbol, with its file and byte span.
        ensure_graph_resident(state, &edit.repo).await;
        let (core, authority) = {
            let current = timed_read(state).await;
            match authorized_graph_read(
                &current,
                &edit.repo,
                verified,
                &format!("GRAPH_NOT_FOUND: {}", edit.repo),
            ) {
                Ok(read) => read,
                Err(error) => return Response::err(req.id, error),
            }
        };
        let mut located: BTreeMap<String, (String, usize, usize)> = BTreeMap::new();
        for op in &edit.ops {
            let node = core
                .get_node_properties(&op.symbol_id)
                .filter(|blob| authority.can_see_node(blob, false))
                .and_then(|blob| medulla_types::msgpack::decode_property_value(&blob).ok());
            let found = node.as_ref().and_then(|node| {
                Some((
                    text(node, "file_path")?.to_string(),
                    number(node, "start_byte")?,
                    number(node, "end_byte")?,
                ))
            });
            match found {
                Some(found) => {
                    located.insert(op.symbol_id.clone(), found);
                }
                None => {
                    return Response::err(req.id, format!("EDIT_SYMBOL_UNKNOWN: {}", op.symbol_id));
                }
            }
        }
        // The tree of `base_rev` under read grants: the blob and mode of each edited file.
        let paths: BTreeSet<String> = located.values().map(|(path, _, _)| path.clone()).collect();
        let (tenant, principal) = (
            verified.tenant().to_string(),
            verified.agent_id().to_string(),
        );
        let (repo, base_rev) = (edit.repo.clone(), edit.base_rev.clone());
        let wanted = paths.clone();
        let tree = target
            .read_blocking(move |registry, branch| {
                let grants = registry.path_grants(&tenant, &repo, &principal)?;
                let mut files = BTreeMap::new();
                for path in &wanted {
                    if let Err(refusal) =
                        path_grant::check(&grants, &[path.as_str()], PathAccess::Read)
                    {
                        return Ok(Err(format!(
                            "{}: no read grant covers the path",
                            refusal.code()
                        )));
                    }
                    let request = ReadTreeRequest {
                        repo: repo.clone(),
                        branch: branch.name.clone(),
                        rev: base_rev.clone(),
                        prefix: path.clone(),
                        depth: 0,
                    };
                    let receipt = match read_tree(&RegistryTree { registry, branch }, &request)? {
                        Ok(receipt) => receipt,
                        Err(refusal) => return Ok(Err(refusal.message())),
                    };
                    if let Some(entry) = receipt
                        .entries
                        .into_iter()
                        .find(|entry| &entry.path == path)
                    {
                        files.insert(path.clone(), (entry.blob_digest, entry.mode));
                    }
                }
                Ok(Ok(files))
            })
            .await;
        let files = match tree {
            Ok(Ok(Ok(files))) => files,
            Ok(Ok(Err(message))) => return Response::err(req.id, message),
            Ok(Err(error)) | Err(error) => return Response::err(req.id, error),
        };
        let mut resolved = BTreeMap::new();
        for (symbol_id, (path, start, end)) in located {
            // A file absent at `base_rev` is a symbol the graph holds for another revision.
            let Some((blob_digest, mode)) = files.get(&path).cloned() else {
                return Response::err(req.id, format!("EDIT_BASE_STALE: {symbol_id}"));
            };
            resolved.insert(
                symbol_id,
                ResolvedSymbol {
                    path,
                    blob_digest,
                    span: start..end,
                    body_span: None,
                    mode,
                },
            );
        }
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
        let owner = match medulla_server_api::carrier::CarrierAuthority::from_verified(verified) {
            Ok(authority) => authority.owner_scope().to_string(),
            Err(error) => return Response::err(req.id, error),
        };
        let author = verified.principal().to_string();
        let created_at = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
        let prepared = tokio::task::spawn_blocking(move || {
            let blobs = ChunkStoreBlobs(store.as_ref(), &owner);
            prepare(&edit, &author, created_at, &Prefetched(resolved), &blobs)
        })
        .await;
        match prepared {
            Ok(Ok(prepared)) => {
                submit_changeset_with(
                    state,
                    req,
                    verified,
                    SubmitChangesetFields {
                        request: Box::new(prepared.request),
                    },
                    Some(prepared.evidence),
                )
                .await
            }
            Ok(Err(refusal)) => Response::err(req.id, refusal.message()),
            Err(error) => Response::err(req.id, error.to_string()),
        }
    }
}

#[cfg(all(feature = "redb", feature = "blob"))]
pub use served::handle_submit_symbol_edit;

/// `SubmitSymbolEdit` needs the `redb` branch registry and the `blob` ingest handlers.
#[cfg(not(all(feature = "redb", feature = "blob")))]
pub async fn handle_submit_symbol_edit(
    _state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    _verified: &VerifiedRequestContext,
    _fields: SubmitSymbolEditFields,
) -> Response {
    Response::err(
        req.id,
        "SubmitSymbolEdit requires the `redb` and `blob` features",
    )
}
