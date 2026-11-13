//! Branch diff and time-travel reads (MEDULLA-BRANCH-MERGE-R006).
//!
//! A branch has ONE commit chain (R002) and every commit carries the record
//! that produced it as its payload: a branch write's mutation, or a source
//! changeset (MEDULLA-SCM-R002). The state of the branch at any version on
//! the chain is therefore the fold of the payloads up to that commit, and the
//! diff between two versions is the changes of the commits between them:
//!
//! - `BranchReadAt` folds the chain up to the commit whose `engine_version`
//!   is asked for and answers the nodes and edges at that point. A version no
//!   commit produced refuses `BRANCH_VERSION_NOT_ON_CHAIN`. Two commits at
//!   one version resolve to the newest.
//! - `BranchDiff` lists, per commit after `from_version` up to `to_version`,
//!   each entity change as added, updated or deleted against the state before
//!   it, bounded by the `branch.diff_page_size` knob.
//!
//! Both resolve the branch through [`super::resolve_target`], so a foreign
//! tenant refuses `BRANCH_TENANT_DENIED` before any chain read. Writes: none.

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::branch::{BranchCommit, BranchRef};
use medulla_types::contract::ServerErrorCode;
use medulla_types::source_changeset::{Changeset, PathOp};
use medulla_types::versioning::{
    page_of, BranchSnapshot, DiffEntity, DiffOperation, KgDiff, KgDiffEntry, KgTransaction,
    SnapshotEdge, SnapshotNode,
};
use tokio::sync::RwLock;

/// The wire fields of `Method::BranchDiff`.
#[derive(Debug, Clone)]
pub struct BranchDiffRequest {
    pub tenant: String,
    pub graph: String,
    pub branch: String,
    pub from_version: Option<u64>,
    pub to_version: Option<u64>,
    pub offset: u64,
}

/// The wire fields of `Method::BranchReadAt`.
#[derive(Debug, Clone)]
pub struct BranchReadAtRequest {
    pub tenant: String,
    pub graph: String,
    pub branch: String,
    pub engine_version: u64,
}

/// One chain commit with the payload it carries, oldest first.
pub type ChainEntry = (BranchCommit, Option<Vec<u8>>);

/// The nodes and edges of a branch at one point of its chain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionState {
    nodes: BTreeMap<String, Vec<u8>>,
    edges: BTreeMap<(String, String), Vec<u8>>,
}

fn change(entity: DiffEntity, operation: DiffOperation, image_ref: &str) -> KgDiffEntry {
    KgDiffEntry {
        entity,
        operation,
        image_ref: image_ref.to_string(),
    }
}

fn other(method: &str, image_ref: &str) -> Vec<KgDiffEntry> {
    vec![change(
        DiffEntity::Other {
            method: method.to_string(),
        },
        DiffOperation::Updated,
        image_ref,
    )]
}

impl VersionState {
    fn upsert_node(&mut self, id: &str, props: &[u8], image_ref: &str) -> KgDiffEntry {
        let operation = if self.nodes.insert(id.to_string(), props.to_vec()).is_some() {
            DiffOperation::Updated
        } else {
            DiffOperation::Added
        };
        change(
            DiffEntity::Node { id: id.to_string() },
            operation,
            image_ref,
        )
    }

    fn upsert_edge(
        &mut self,
        source: &str,
        target: &str,
        props: &[u8],
        image_ref: &str,
    ) -> KgDiffEntry {
        let key = (source.to_string(), target.to_string());
        let operation = if self.edges.insert(key, props.to_vec()).is_some() {
            DiffOperation::Updated
        } else {
            DiffOperation::Added
        };
        let entity = DiffEntity::Edge {
            source: source.to_string(),
            target: target.to_string(),
        };
        change(entity, operation, image_ref)
    }

    fn remove_node(&mut self, id: &str, image_ref: &str) -> Vec<KgDiffEntry> {
        if self.nodes.remove(id).is_none() {
            return Vec::new();
        }
        // A removed node takes its incident edges with it.
        let incident: Vec<(String, String)> = self
            .edges
            .keys()
            .filter(|(source, target)| source == id || target == id)
            .cloned()
            .collect();
        let mut entries = vec![change(
            DiffEntity::Node { id: id.to_string() },
            DiffOperation::Deleted,
            image_ref,
        )];
        for (source, target) in incident {
            self.edges.remove(&(source.clone(), target.clone()));
            entries.push(change(
                DiffEntity::Edge { source, target },
                DiffOperation::Deleted,
                image_ref,
            ));
        }
        entries
    }

    fn apply_mutation(&mut self, mutation: &Method, image_ref: &str) -> Vec<KgDiffEntry> {
        match mutation {
            Method::AddNode {
                node_id,
                properties_msgpack,
            } => vec![self.upsert_node(node_id, properties_msgpack, image_ref)],
            // An existing node is never overwritten: no change.
            Method::CreateNodeIfAbsent {
                node_id,
                properties_msgpack,
            } => {
                if self.nodes.contains_key(node_id) {
                    Vec::new()
                } else {
                    vec![self.upsert_node(node_id, properties_msgpack, image_ref)]
                }
            }
            Method::RemoveNode { node_id } => self.remove_node(node_id, image_ref),
            Method::AddEdge {
                source_id,
                target_id,
                properties_msgpack,
            }
            | Method::SupersedeEdge {
                source_id,
                target_id,
                properties_msgpack,
                ..
            } => vec![self.upsert_edge(source_id, target_id, properties_msgpack, image_ref)],
            Method::RemoveEdge {
                source_id,
                target_id,
            } => match self.edges.remove(&(source_id.clone(), target_id.clone())) {
                Some(_) => vec![change(
                    DiffEntity::Edge {
                        source: source_id.clone(),
                        target: target_id.clone(),
                    },
                    DiffOperation::Deleted,
                    image_ref,
                )],
                None => Vec::new(),
            },
            // Closes the windows of an edge; the edge stays, changed.
            Method::InvalidateEdge {
                source_id,
                target_id,
                ..
            } => {
                let key = (source_id.clone(), target_id.clone());
                if self.edges.contains_key(&key) {
                    vec![change(
                        DiffEntity::Edge {
                            source: source_id.clone(),
                            target: target_id.clone(),
                        },
                        DiffOperation::Updated,
                        image_ref,
                    )]
                } else {
                    Vec::new()
                }
            }
            // A mutation this feed does not enumerate is named, never hidden.
            _ => other(&method_name(mutation), image_ref),
        }
    }

    /// Fold one commit payload into the state, returning its changes in apply
    /// order. A source changeset lists its paths; a commit with no readable
    /// payload is named, so no commit is silently empty.
    pub fn apply_payload(&mut self, payload: Option<&[u8]>, image_ref: &str) -> Vec<KgDiffEntry> {
        let Some(bytes) = payload else {
            return other("unrecorded", image_ref);
        };
        if let Ok(mutation) = rmp_serde::from_slice::<Method>(bytes) {
            return self.apply_mutation(&mutation, image_ref);
        }
        if let Ok(changeset) = rmp_serde::from_slice::<Changeset>(bytes) {
            return changeset
                .changed_paths
                .iter()
                .map(|entry| {
                    let operation = match entry.op {
                        PathOp::Add | PathOp::Rename => DiffOperation::Added,
                        PathOp::Modify => DiffOperation::Updated,
                        PathOp::Delete => DiffOperation::Deleted,
                    };
                    change(
                        DiffEntity::Path {
                            path: entry.path.clone(),
                        },
                        operation,
                        image_ref,
                    )
                })
                .collect();
        }
        other("undecodable", image_ref)
    }

    fn snapshot(&self, branch: &str, commit: &BranchCommit) -> BranchSnapshot {
        BranchSnapshot {
            branch: branch.to_string(),
            engine_version: commit.engine_version,
            commit: commit.clone(),
            nodes: self
                .nodes
                .iter()
                .map(|(id, props)| SnapshotNode {
                    id: id.clone(),
                    properties_msgpack: props.clone(),
                })
                .collect(),
            edges: self
                .edges
                .iter()
                .map(|((source, target), props)| SnapshotEdge {
                    source: source.clone(),
                    target: target.clone(),
                    properties_msgpack: props.clone(),
                })
                .collect(),
        }
    }
}

/// The method's variant name: the leading identifier of its `Debug` form.
fn method_name(method: &Method) -> String {
    format!("{method:?}")
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

/// The image ref of a commit's changes.
fn image_ref(commit: &BranchCommit) -> String {
    format!("commit:{}", commit.id)
}

/// The index of the newest chain commit produced at `engine_version`.
fn index_of_version(chain: &[ChainEntry], engine_version: u64) -> Option<usize> {
    chain
        .iter()
        .rposition(|(commit, _)| commit.engine_version == engine_version)
}

/// The branch as of `engine_version`: the fold of every commit up to it.
pub fn read_at(
    branch: &str,
    chain: &[ChainEntry],
    engine_version: u64,
) -> Result<BranchSnapshot, ServerErrorCode> {
    let end =
        index_of_version(chain, engine_version).ok_or(ServerErrorCode::BranchVersionNotOnChain)?;
    let mut state = VersionState::default();
    for (commit, payload) in &chain[..=end] {
        state.apply_payload(payload.as_deref(), &image_ref(commit));
    }
    Ok(state.snapshot(branch, &chain[end].0))
}

/// The transactions after `from_version` up to `to_version` (`None` bounds are
/// the empty baseline and the head), oldest first.
pub fn transactions(
    chain: &[ChainEntry],
    from_version: Option<u64>,
    to_version: Option<u64>,
) -> Result<Vec<KgTransaction>, ServerErrorCode> {
    let start = match from_version {
        None => 0,
        Some(version) => {
            index_of_version(chain, version).ok_or(ServerErrorCode::BranchVersionNotOnChain)? + 1
        }
    };
    let end = match to_version {
        None => chain.len(),
        Some(version) => {
            index_of_version(chain, version).ok_or(ServerErrorCode::BranchVersionNotOnChain)? + 1
        }
    };
    if end < start {
        return Err(ServerErrorCode::BranchVersionNotOnChain);
    }
    let mut state = VersionState::default();
    let mut out = Vec::new();
    for (position, (commit, payload)) in chain[..end].iter().enumerate() {
        let changes = state.apply_payload(payload.as_deref(), &image_ref(commit));
        if position >= start {
            out.push(KgTransaction {
                commit: commit.clone(),
                changes,
            });
        }
    }
    Ok(out)
}

/// One page of the change feed between two versions.
pub fn diff(
    branch: &str,
    chain: &[ChainEntry],
    request: &BranchDiffRequest,
    page_size: u64,
) -> Result<KgDiff, ServerErrorCode> {
    let spanned = transactions(chain, request.from_version, request.to_version)?;
    let to_version = match (request.to_version, chain.last()) {
        (Some(version), _) => version,
        (None, Some((head, _))) => head.engine_version,
        (None, None) => 0,
    };
    let entries: Vec<KgDiffEntry> = spanned
        .iter()
        .flat_map(|transaction| transaction.changes.iter().cloned())
        .collect();
    let (page, next_offset) = page_of(&entries, request.offset, page_size);
    Ok(KgDiff {
        branch: branch.to_string(),
        from_version: request.from_version,
        to_version,
        commits: spanned.into_iter().map(|t| t.commit).collect(),
        entries: page,
        next_offset,
    })
}

fn refuse(req_id: u64, code: ServerErrorCode) -> Response {
    Response::err(req_id, format!("{}: branch read refused", code.as_str()))
}

/// The chain with each commit's payload, oldest first.
async fn load_chain(target: &super::BranchTarget) -> Result<Vec<ChainEntry>, String> {
    let mut entries = Vec::new();
    for commit in target.chain().await? {
        let payload = target.payload(&commit.id).await?;
        entries.push((commit, payload));
    }
    Ok(entries)
}

async fn resolve_chain(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    reference: BranchRef,
) -> Result<Vec<ChainEntry>, Response> {
    let target = super::resolve_target(state, verified, reference)
        .await
        .map_err(|error| super::target_error_response(req_id, error))?;
    load_chain(&target)
        .await
        .map_err(|error| Response::err(req_id, error))
}

/// `Method::BranchReadAt`: the branch as of one engine version on its chain.
pub async fn handle_branch_read_at(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: BranchReadAtRequest,
) -> Response {
    let reference = BranchRef {
        tenant: request.tenant,
        graph: request.graph,
        name: request.branch.clone(),
    };
    let chain = match resolve_chain(state, req_id, verified, reference).await {
        Ok(chain) => chain,
        Err(refused) => return refused,
    };
    match read_at(&request.branch, &chain, request.engine_version) {
        Ok(snapshot) => {
            match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::BranchReadAt>(
                snapshot,
            ) {
                Ok(payload) => Response::ok(req_id, payload),
                Err(error) => Response::err(req_id, error),
            }
        }
        Err(code) => refuse(req_id, code),
    }
}

/// `Method::BranchDiff`: the change feed between two versions of one chain.
pub async fn handle_branch_diff(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: BranchDiffRequest,
) -> Response {
    let reference = BranchRef {
        tenant: request.tenant.clone(),
        graph: request.graph.clone(),
        name: request.branch.clone(),
    };
    let chain = match resolve_chain(state, req_id, verified, reference).await {
        Ok(chain) => chain,
        Err(refused) => return refused,
    };
    let page_size = medulla_server_api::knob_config::knobs().branch_diff_page_size;
    match diff(&request.branch, &chain, &request, page_size) {
        Ok(feed) => {
            match ResultPayload::of::<medulla_protocol_types::result_contract::cluster::BranchDiff>(
                feed,
            ) {
                Ok(payload) => Response::ok(req_id, payload),
                Err(error) => Response::err(req_id, error),
            }
        }
        Err(code) => refuse(req_id, code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> Vec<u8> {
        let mutation = Method::AddNode {
            node_id: id.into(),
            properties_msgpack: vec![0x80],
        };
        rmp_serde::to_vec_named(&mutation).unwrap()
    }

    fn remove(id: &str) -> Vec<u8> {
        rmp_serde::to_vec_named(&Method::RemoveNode { node_id: id.into() }).unwrap()
    }

    fn edge(source: &str, target: &str) -> Vec<u8> {
        let mutation = Method::AddEdge {
            source_id: source.into(),
            target_id: target.into(),
            properties_msgpack: vec![0x80],
        };
        rmp_serde::to_vec_named(&mutation).unwrap()
    }

    /// A three-version chain: add a, add b plus an edge, remove a.
    fn chain() -> Vec<ChainEntry> {
        let payloads = [node("a"), node("b"), edge("a", "b"), remove("a")];
        let versions = [1u64, 2, 3, 4];
        let mut parent = String::from("base");
        let mut out = Vec::new();
        for (index, payload) in payloads.iter().enumerate() {
            let id = format!("c{index}");
            let commit = BranchCommit::single_parent_commit(
                id.clone(),
                "feature/x",
                parent.clone(),
                versions[index],
                "digest",
                "agent",
                index as u64,
            );
            parent = id;
            out.push((commit, Some(payload.clone())));
        }
        out
    }

    fn ids(snapshot: &BranchSnapshot) -> Vec<&str> {
        snapshot.nodes.iter().map(|n| n.id.as_str()).collect()
    }

    // spec: MEDULLA-BRANCH-MERGE-R006
    #[test]
    fn as_of_reads_each_version_and_refuses_a_version_off_the_chain() {
        let chain = chain();
        assert_eq!(ids(&read_at("feature/x", &chain, 1).unwrap()), ["a"]);
        let two = read_at("feature/x", &chain, 2).unwrap();
        assert_eq!(ids(&two), ["a", "b"]);
        assert!(two.edges.is_empty());
        let three = read_at("feature/x", &chain, 3).unwrap();
        assert_eq!(three.edges.len(), 1);
        let four = read_at("feature/x", &chain, 4).unwrap();
        assert_eq!(ids(&four), ["b"]);
        // The removed node took its edge with it.
        assert!(four.edges.is_empty());
        assert_eq!(four.commit.id, "c3");
        assert_eq!(
            read_at("feature/x", &chain, 9).unwrap_err(),
            ServerErrorCode::BranchVersionNotOnChain
        );
    }

    // spec: MEDULLA-BRANCH-MERGE-R006
    #[test]
    fn diff_equals_the_intervening_commits() {
        let chain = chain();
        let request = |from, to| BranchDiffRequest {
            tenant: "t".into(),
            graph: "g".into(),
            branch: "feature/x".into(),
            from_version: from,
            to_version: to,
            offset: 0,
        };
        let feed = diff("feature/x", &chain, &request(Some(1), Some(3)), 1000).unwrap();
        let spanned: Vec<&str> = feed.commits.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(spanned, ["c1", "c2"]);
        let shape: Vec<(DiffOperation, String)> = feed
            .entries
            .iter()
            .map(|e| (e.operation, e.image_ref.clone()))
            .collect();
        assert_eq!(
            shape,
            [
                (DiffOperation::Added, "commit:c1".to_string()),
                (DiffOperation::Added, "commit:c2".to_string())
            ]
        );
        // Head diff from the baseline lists every change, deletion included.
        let all = diff("feature/x", &chain, &request(None, None), 1000).unwrap();
        assert_eq!(all.to_version, 4);
        let ops: Vec<DiffOperation> = all.entries.iter().map(|e| e.operation).collect();
        assert_eq!(
            ops,
            [
                DiffOperation::Added,
                DiffOperation::Added,
                DiffOperation::Added,
                DiffOperation::Deleted,
                DiffOperation::Deleted
            ]
        );
        // Bounded pages chain by offset.
        let first = diff("feature/x", &chain, &request(None, None), 2).unwrap();
        assert_eq!((first.entries.len(), first.next_offset), (2, Some(2)));
        // An inverted or off-chain range refuses.
        assert_eq!(
            diff("feature/x", &chain, &request(Some(3), Some(1)), 10).unwrap_err(),
            ServerErrorCode::BranchVersionNotOnChain
        );
        assert_eq!(
            diff("feature/x", &chain, &request(Some(7), None), 10).unwrap_err(),
            ServerErrorCode::BranchVersionNotOnChain
        );
    }
}
