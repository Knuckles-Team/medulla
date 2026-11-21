//! Git import and export of the source-changeset chain (MEDULLA-SCM-R010).
//!
//! Import reads a version-2 pack (already decoded by
//! `medulla_compute::source_git_objects::read_pack`), walks the FIRST-PARENT
//! line from `head`, and plans one changeset per commit: the paths whose
//! content differs from the commit's first parent. A merge commit is therefore
//! one changeset whose `changed_paths` include everything the merged side
//! changed, its full parent list is stored in `GitOrigin.parent_commit_ids`,
//! and the objects its other parents bring in are kept in one stored side pack
//! (`GitOrigin.side_pack_digest`) so export stays a complete git history.
//!
//! The chain stays linear: history, blame and bisect walk the chain, which is
//! the first-parent line, and never the history behind a second parent.
//!
//! Export is [`export_pack`]: the one function the served `GitExportPack`
//! (MEDULLA-SCM-R014) calls.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use medulla_compute::source_git_objects::{
    build_commit, build_tree, object_id, parse_commit, parse_tree, read_pack, write_pack,
    GitObjectError, GitObjectKind, GitTreeEntry, MODE_SUBMODULE, MODE_TREE,
};
use medulla_compute::source_tree::{apply_scoped, Tree, TreeChain};
use medulla_source_types::source_changeset::{
    Changeset, ChangesetRefusal, GitObjectFormat, GitOrigin, PathOp, SubmitChangesetRequest,
    SubmitRefusal, SubmittedPath,
};

use super::BlobIo;

/// Objects of a read pack by id.
pub type PackObjects = BTreeMap<String, (GitObjectKind, Vec<u8>)>;

fn malformed(detail: impl Into<String>) -> SubmitRefusal {
    SubmitRefusal::new(ChangesetRefusal::GitGraphMalformed, detail)
}

fn read_failed(detail: impl Into<String>) -> SubmitRefusal {
    SubmitRefusal::new(ChangesetRefusal::GitExportTreeReadFailed, detail)
}

fn codec(error: GitObjectError) -> SubmitRefusal {
    match error {
        GitObjectError::ModeUnsupported(mode) => SubmitRefusal::new(
            ChangesetRefusal::GitModeUnsupported,
            format!("tree entry mode {mode:o}"),
        ),
        other => malformed(other.to_string()),
    }
}

/// Decode the pack bytes a git client sent.
pub fn decode_pack(format: GitObjectFormat, bytes: &[u8]) -> Result<PackObjects, SubmitRefusal> {
    read_pack(format, bytes).map_err(codec)
}

/// What the branch head holds, as the import sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchHeadGit {
    /// The branch has no changeset.
    Empty,
    /// The head changeset records this git commit.
    Commit(String),
    /// The head changeset is native: no git history can extend it.
    Native,
}

/// One path of a planned commit. `content` is `None` exactly for a delete; for
/// a submodule entry it is the hex commit id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedChange {
    pub path: String,
    pub op: PathOp,
    pub mode: u32,
    pub content: Option<Vec<u8>>,
}

/// One git commit planned as a changeset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedCommit {
    /// The origin with `side_pack_digest` still `None`; [`materialize`] fills it.
    pub origin: GitOrigin,
    pub message: String,
    pub committer_time_s: i64,
    pub changes: Vec<PlannedChange>,
    /// The version-2 side pack of a merge commit.
    pub side_pack: Option<Vec<u8>>,
}

/// The commits to append, oldest first, and how many first-parent commits the
/// branch already holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportPlan {
    pub commits: Vec<PlannedCommit>,
    pub skipped: u32,
}

type Flat = BTreeMap<String, (u32, String)>;

fn commit_of(
    objects: &PackObjects,
    id: &str,
) -> Result<medulla_compute::source_git_objects::ParsedCommit, SubmitRefusal> {
    match objects.get(id) {
        Some((GitObjectKind::Commit, body)) => parse_commit(body).map_err(codec),
        Some(_) => Err(malformed(format!("object {id} is not a commit"))),
        None => Err(malformed(format!("commit {id} is not in the pack"))),
    }
}

fn flatten_into(
    format: GitObjectFormat,
    objects: &PackObjects,
    tree_id: &str,
    prefix: &str,
    out: &mut Flat,
) -> Result<(), SubmitRefusal> {
    let body = match objects.get(tree_id) {
        Some((GitObjectKind::Tree, body)) => body,
        Some(_) => return Err(malformed(format!("object {tree_id} is not a tree"))),
        None => return Err(malformed(format!("tree {tree_id} is not in the pack"))),
    };
    for entry in parse_tree(format, body).map_err(codec)? {
        let path = if prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{prefix}/{}", entry.name)
        };
        if entry.mode == MODE_TREE {
            flatten_into(format, objects, &entry.id, &path, out)?;
        } else {
            out.insert(path, (entry.mode, entry.id));
        }
    }
    Ok(())
}

/// The files of a tree: path to `(mode, object id)`.
fn flatten(
    format: GitObjectFormat,
    objects: &PackObjects,
    tree_id: &str,
) -> Result<Flat, SubmitRefusal> {
    let mut out = Flat::new();
    flatten_into(format, objects, tree_id, "", &mut out)?;
    Ok(out)
}

/// Every object reachable from `roots` that is not in `known` (which must be
/// closed under reachability). A reachable object missing from the pack, other
/// than the commit a submodule entry names, refuses `GIT_GRAPH_MALFORMED`.
fn closure(
    format: GitObjectFormat,
    objects: &PackObjects,
    roots: &[String],
    known: &BTreeSet<String>,
) -> Result<BTreeSet<String>, SubmitRefusal> {
    let mut found = BTreeSet::new();
    let mut stack: Vec<String> = roots.to_vec();
    while let Some(id) = stack.pop() {
        if known.contains(&id) || !found.insert(id.clone()) {
            continue;
        }
        let Some((kind, body)) = objects.get(&id) else {
            return Err(malformed(format!("object {id} is not in the pack")));
        };
        match kind {
            GitObjectKind::Commit => {
                let commit = parse_commit(body).map_err(codec)?;
                stack.push(commit.tree_id);
                stack.extend(commit.parents);
            }
            GitObjectKind::Tree => {
                for entry in parse_tree(format, body).map_err(codec)? {
                    if entry.mode != MODE_SUBMODULE {
                        stack.push(entry.id);
                    }
                }
            }
            GitObjectKind::Blob | GitObjectKind::Tag => {}
        }
    }
    Ok(found)
}

fn diff(old: &Flat, new: &Flat, objects: &PackObjects) -> Result<Vec<PlannedChange>, SubmitRefusal> {
    let content_of = |mode: u32, id: &str| -> Result<Vec<u8>, SubmitRefusal> {
        if mode == MODE_SUBMODULE {
            return Ok(id.as_bytes().to_vec());
        }
        match objects.get(id) {
            Some((GitObjectKind::Blob, body)) => Ok(body.clone()),
            Some(_) => Err(malformed(format!("object {id} is not a blob"))),
            None => Err(malformed(format!("blob {id} is not in the pack"))),
        }
    };
    let mut changes = Vec::new();
    for (path, (mode, id)) in new {
        match old.get(path) {
            Some(previous) if previous == &(*mode, id.clone()) => {}
            Some(_) => changes.push(PlannedChange {
                path: path.clone(),
                op: PathOp::Modify,
                mode: *mode,
                content: Some(content_of(*mode, id)?),
            }),
            None => changes.push(PlannedChange {
                path: path.clone(),
                op: PathOp::Add,
                mode: *mode,
                content: Some(content_of(*mode, id)?),
            }),
        }
    }
    for (path, (mode, _)) in old {
        if !new.contains_key(path) {
            changes.push(PlannedChange {
                path: path.clone(),
                op: PathOp::Delete,
                mode: *mode,
                content: None,
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

/// Plan the import of the first-parent line of `head` (MEDULLA-SCM-R010).
///
/// The walk runs from `head` along first parents until the commit the branch
/// head records (or a root commit when the branch is empty). A branch head that
/// is not on that line refuses `GIT_GRAPH_MALFORMED`, as does a commit, tree,
/// blob or parent the pack does not hold. More than `max_commits` new commits
/// refuse `CHANGESET_INVALID` before anything is planned. The commit the branch
/// head records must be in the pack, because the oldest new commit is diffed
/// against it.
pub fn plan_import(
    format: GitObjectFormat,
    objects: &PackObjects,
    head: &str,
    branch_head: &BranchHeadGit,
    max_commits: u64,
) -> Result<ImportPlan, SubmitRefusal> {
    // Newest first: the commits the branch does not hold yet.
    let mut line: Vec<(String, medulla_compute::source_git_objects::ParsedCommit)> = Vec::new();
    let mut stop: Option<String> = None;
    let mut cursor = head.to_string();
    loop {
        if matches!(branch_head, BranchHeadGit::Commit(held) if *held == cursor) {
            stop = Some(cursor);
            break;
        }
        let commit = commit_of(objects, &cursor)?;
        let next = commit.parents.first().cloned();
        line.push((cursor, commit));
        match next {
            Some(parent) => cursor = parent,
            None => break,
        }
    }
    match (branch_head, &stop) {
        (BranchHeadGit::Empty, _) | (BranchHeadGit::Commit(_), Some(_)) => {}
        (BranchHeadGit::Commit(_), None) | (BranchHeadGit::Native, _) => {
            return Err(malformed(
                "the branch head is not a commit on the first-parent line of head",
            ))
        }
    }
    // First-parent commits the branch already holds.
    let mut skipped = 0u32;
    if let Some(held) = &stop {
        let mut cursor = held.clone();
        while let Ok(commit) = commit_of(objects, &cursor) {
            skipped = skipped.saturating_add(1);
            match commit.parents.first() {
                Some(parent) => cursor = parent.clone(),
                None => break,
            }
        }
    }
    if line.len() as u64 > max_commits {
        return Err(SubmitRefusal::new(
            ChangesetRefusal::ChangesetInvalid,
            format!("{} new commits exceed the limit of {max_commits}", line.len()),
        ));
    }
    line.reverse();

    let has_merge = line.iter().any(|(_, commit)| commit.parents.len() > 1);
    // Objects reachable from the first parent of the commit being planned.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut previous: Option<Flat> = None;
    if let Some((_, oldest)) = line.first() {
        if let Some(parent) = oldest.parents.first() {
            let parent_commit = commit_of(objects, parent)?;
            previous = Some(flatten(format, objects, &parent_commit.tree_id)?);
            if has_merge {
                seen = closure(format, objects, std::slice::from_ref(parent), &BTreeSet::new())?;
            }
        }
    }
    let mut commits = Vec::with_capacity(line.len());
    for (commit_id, commit) in line {
        let flat = flatten(format, objects, &commit.tree_id)?;
        let empty = Flat::new();
        let changes = diff(previous.as_ref().unwrap_or(&empty), &flat, objects)?;
        let side_pack = if commit.parents.len() > 1 {
            let side_ids = closure(format, objects, &commit.parents[1..], &seen)?;
            let side_objects: Vec<(GitObjectKind, Vec<u8>)> = side_ids
                .iter()
                .filter_map(|id| objects.get(id).cloned())
                .collect();
            let mut pack = Vec::new();
            write_pack(format, &side_objects, &mut pack).map_err(codec)?;
            Some(pack)
        } else {
            None
        };
        if has_merge {
            seen.extend(closure(
                format,
                objects,
                std::slice::from_ref(&commit_id),
                &seen,
            )?);
        }
        commits.push(PlannedCommit {
            origin: GitOrigin {
                object_format: format,
                commit_id,
                tree_id: commit.tree_id,
                parent_commit_ids: commit.parents,
                side_pack_digest: None,
                raw_headers: commit.raw_headers,
            },
            message: commit.message,
            committer_time_s: commit.committer_time_s,
            changes,
            side_pack,
        });
        previous = Some(flat);
    }
    Ok(ImportPlan { commits, skipped })
}

/// One planned commit ready for `accept`: the request with its blobs stored,
/// the origin with its side pack stored, and the changeset time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportStep {
    pub request: SubmitChangesetRequest,
    pub origin: GitOrigin,
    pub created_at_ms: u64,
}

/// Store the blobs and the side pack of `planned` through `blobs` and build the
/// request that extends `base_head`. A store failure refuses
/// `CHANGESET_INVALID`.
pub fn materialize(
    planned: &PlannedCommit,
    blobs: &dyn BlobIo,
    repo: &str,
    branch: &str,
    base_head: Option<String>,
) -> Result<ImportStep, SubmitRefusal> {
    let store_failed = |detail: String| SubmitRefusal::new(ChangesetRefusal::ChangesetInvalid, detail);
    let mut paths = Vec::with_capacity(planned.changes.len());
    for change in &planned.changes {
        let blob = match &change.content {
            Some(bytes) => Some(blobs.store(bytes).map_err(&store_failed)?),
            None => None,
        };
        paths.push(SubmittedPath {
            path: change.path.clone(),
            op: change.op,
            blob,
            mode: change.mode,
            renamed_from: None,
        });
    }
    let mut origin = planned.origin.clone();
    if let Some(pack) = &planned.side_pack {
        origin.side_pack_digest = Some(blobs.store(pack).map_err(&store_failed)?.digest);
    }
    Ok(ImportStep {
        request: SubmitChangesetRequest {
            repo: repo.to_string(),
            branch: branch.to_string(),
            base_head,
            message: planned.message.clone(),
            paths,
            trailers: None,
            run_trace_id: None,
        },
        origin,
        created_at_ms: u64::try_from(planned.committer_time_s)
            .unwrap_or(0)
            .saturating_mul(1000),
    })
}

/// The git commit the changeset records, for the branch-head check of import.
pub fn head_git(head: Option<&Changeset>) -> BranchHeadGit {
    match head {
        None => BranchHeadGit::Empty,
        Some(changeset) => match &changeset.git {
            Some(origin) => BranchHeadGit::Commit(origin.commit_id.clone()),
            None => BranchHeadGit::Native,
        },
    }
}

#[derive(Default)]
struct Dir {
    files: Vec<GitTreeEntry>,
    dirs: BTreeMap<String, Dir>,
}

fn emit_dir(
    format: GitObjectFormat,
    dir: &Dir,
    out: &mut Vec<(String, Vec<u8>)>,
) -> Result<String, GitObjectError> {
    let mut entries = dir.files.clone();
    for (name, child) in &dir.dirs {
        let id = emit_dir(format, child, out)?;
        entries.push(GitTreeEntry {
            name: name.clone(),
            mode: MODE_TREE,
            id,
        });
    }
    let (id, body) = build_tree(format, &entries)?;
    out.push((id.clone(), body));
    Ok(id)
}

/// Build the tree objects of a flat file map (path to mode and object id)
/// bottom-up, one tree per directory. Returns the root tree id and every tree
/// as `(id, body)`.
pub fn build_tree_objects(
    format: GitObjectFormat,
    files: &BTreeMap<String, (u32, String)>,
) -> Result<(String, Vec<(String, Vec<u8>)>), GitObjectError> {
    let mut root = Dir::default();
    for (path, (mode, id)) in files {
        let parts: Vec<&str> = path.split('/').collect();
        let (name, dirs) = parts
            .split_last()
            .ok_or_else(|| GitObjectError::Malformed("empty path".into()))?;
        let mut node = &mut root;
        for part in dirs {
            node = node.dirs.entry((*part).to_string()).or_default();
        }
        node.files.push(GitTreeEntry {
            name: (*name).to_string(),
            mode: *mode,
            id: id.clone(),
        });
    }
    let mut out = Vec::new();
    let root_id = emit_dir(format, &root, &mut out)?;
    Ok((root_id, out))
}

/// Resolve git commit ids and branch commit ids to branch commit ids: `chain`
/// is the branch from the head back, as `(commit id, changeset)`. An id that
/// names no changeset resolves to `None`.
pub fn resolve_revs(chain: &[(String, Changeset)], ids: &[String]) -> Vec<Option<String>> {
    ids.iter()
        .map(|id| {
            chain
                .iter()
                .find(|(commit, changeset)| {
                    commit == id
                        || changeset
                            .git
                            .as_ref()
                            .is_some_and(|origin| &origin.commit_id == id)
                })
                .map(|(commit, _)| commit.clone())
        })
        .collect()
}

/// The git object id a file's blob has: the digest read through `blobs`, hashed
/// as a blob, or for a submodule entry the commit id the stored text holds.
fn file_id(
    format: GitObjectFormat,
    blobs: &dyn BlobIo,
    cache: &mut HashMap<(String, bool), String>,
    digest: &str,
    mode: u32,
) -> Result<String, SubmitRefusal> {
    let submodule = mode == MODE_SUBMODULE;
    if let Some(id) = cache.get(&(digest.to_string(), submodule)) {
        return Ok(id.clone());
    }
    let bytes = blobs
        .read(digest)
        .map_err(|error| read_failed(format!("blob {digest}: {error}")))?;
    let id = if submodule {
        String::from_utf8(bytes)
            .map_err(|_| read_failed(format!("submodule blob {digest} is not text")))?
            .trim()
            .to_string()
    } else {
        object_id(format, GitObjectKind::Blob, &bytes)
    };
    cache.insert((digest.to_string(), submodule), id.clone());
    Ok(id)
}

/// The commit body of a changeset: the stored headers of an imported one, or
/// headers synthesized from the principal and time of a native one.
fn commit_body(changeset: &Changeset, tree_id: &str, previous: Option<&str>) -> Vec<u8> {
    match &changeset.git {
        Some(origin) => build_commit(tree_id, &origin.raw_headers, &changeset.message),
        None => {
            let seconds = changeset.created_at / 1000;
            let who = &changeset.author_principal;
            let mut headers = String::new();
            if let Some(parent) = previous {
                headers.push_str(&format!("parent {parent}\n"));
            }
            headers.push_str(&format!(
                "author {who} <{who}> {seconds} +0000\ncommitter {who} <{who}> {seconds} +0000\n"
            ));
            build_commit(tree_id, &headers, &changeset.message)
        }
    }
}

/// The parents an origin's headers name, to be equal to `parent_commit_ids`.
fn header_parents(raw_headers: &str) -> Vec<String> {
    raw_headers
        .lines()
        .filter_map(|line| line.strip_prefix("parent "))
        .map(str::to_string)
        .collect()
}

/// [`export_pack`] that also returns the object format of the pack.
pub fn export_pack_with_format(
    chain: &dyn TreeChain,
    blobs: &dyn BlobIo,
    wants: &[String],
    haves: &[String],
) -> Result<(GitObjectFormat, Vec<u8>), SubmitRefusal> {
    // Newest first: the walk of each want back along first parents. The chain
    // is linear, so the longest walk holds every shorter one.
    let mut longest: Vec<(String, Changeset)> = Vec::new();
    for want in wants {
        let mut walked: Vec<(String, Changeset)> = Vec::new();
        let mut cursor = want.clone();
        while let Some(changeset) = chain.changeset_at(&cursor).map_err(read_failed)? {
            let next = changeset.parents.first().cloned().unwrap_or_default();
            walked.push((cursor, changeset));
            cursor = next;
        }
        if walked.is_empty() {
            return Err(SubmitRefusal::new(
                ChangesetRefusal::RevisionNotOnChain,
                format!("want {want} is not a changeset of the branch"),
            ));
        }
        if walked.len() > longest.len() {
            longest = walked;
        }
    }
    longest.reverse();
    // Everything up to and including the newest have is already held.
    let first_emitted = haves
        .iter()
        .filter_map(|have| longest.iter().position(|(rev, _)| rev == have))
        .max()
        .map_or(0, |index| index + 1);

    let mut formats: BTreeSet<&'static str> = BTreeSet::new();
    let mut format = GitObjectFormat::Sha1;
    for (_, changeset) in &longest {
        if let Some(origin) = &changeset.git {
            format = origin.object_format;
            formats.insert(match origin.object_format {
                GitObjectFormat::Sha1 => "sha1",
                GitObjectFormat::Sha256 => "sha256",
            });
        }
    }
    if formats.len() > 1 {
        return Err(malformed("the chain mixes git object formats"));
    }

    let mut objects: Vec<(GitObjectKind, Vec<u8>)> = Vec::new();
    // Ids already in the output or held by the haves.
    let mut done: HashSet<String> = HashSet::new();
    let mut cache: HashMap<(String, bool), String> = HashMap::new();
    let mut tree = Tree::new();
    let mut previous: Option<String> = None;
    for (index, (_, changeset)) in longest.iter().enumerate() {
        apply_scoped(&mut tree, changeset, "");
        let emit = index >= first_emitted;
        let mut files: BTreeMap<String, (u32, String)> = BTreeMap::new();
        let mut blob_digests: Vec<(String, String, u32)> = Vec::new();
        for (path, (digest, mode)) in &tree {
            let id = file_id(format, blobs, &mut cache, digest, *mode)?;
            if *mode != MODE_SUBMODULE {
                blob_digests.push((id.clone(), digest.clone(), *mode));
            }
            files.insert(path.clone(), (*mode, id));
        }
        let (tree_id, tree_objects) = build_tree_objects(format, &files).map_err(codec)?;
        let body = commit_body(changeset, &tree_id, previous.as_deref());
        let commit_id = object_id(format, GitObjectKind::Commit, &body);
        if let Some(origin) = &changeset.git {
            if origin.tree_id != tree_id
                || origin.commit_id != commit_id
                || header_parents(&origin.raw_headers) != origin.parent_commit_ids
            {
                return Err(malformed(format!(
                    "export of {} does not reproduce its commit and tree ids",
                    origin.commit_id
                )));
            }
        }
        if emit {
            for (id, digest, _) in &blob_digests {
                if done.insert(id.clone()) {
                    let bytes = blobs
                        .read(digest)
                        .map_err(|error| read_failed(format!("blob {digest}: {error}")))?;
                    objects.push((GitObjectKind::Blob, bytes));
                }
            }
            for (id, tree_body) in tree_objects {
                if done.insert(id) {
                    objects.push((GitObjectKind::Tree, tree_body));
                }
            }
            if done.insert(commit_id.clone()) {
                objects.push((GitObjectKind::Commit, body));
            }
            if let Some(digest) = changeset
                .git
                .as_ref()
                .and_then(|origin| origin.side_pack_digest.as_ref())
            {
                let bytes = blobs
                    .read(digest)
                    .map_err(|error| read_failed(format!("side pack {digest}: {error}")))?;
                let side = read_pack(format, &bytes)
                    .map_err(|error| read_failed(format!("side pack {digest}: {error}")))?;
                for (id, (kind, side_body)) in side {
                    if done.insert(id) {
                        objects.push((kind, side_body));
                    }
                }
            }
        } else {
            done.extend(blob_digests.into_iter().map(|(id, _, _)| id));
            done.extend(tree_objects.into_iter().map(|(id, _)| id));
            done.insert(commit_id.clone());
        }
        previous = Some(commit_id);
    }
    let mut pack = Vec::new();
    write_pack(format, &objects, &mut pack).map_err(codec)?;
    Ok((format, pack))
}

/// Export the changeset chain as a version-2 pack: every changeset from each
/// want back along first parents to, excluding, a have or the root. Trees are
/// built bottom-up per directory from the tree at each changeset (the replay of
/// R004), blobs are read through `blobs`, and an imported commit is rebuilt from
/// its stored headers so its commit and tree ids equal the origin's, else
/// `GIT_GRAPH_MALFORMED`. A merge changeset appends every object of its stored
/// side pack. A tree, blob or side pack that cannot be read refuses
/// `GIT_EXPORT_TREE_READ_FAILED` and returns no partial pack. Wants and haves
/// are branch commit ids; [`resolve_revs`] maps git commit ids to them.
pub fn export_pack(
    chain: &dyn TreeChain,
    blobs: &dyn BlobIo,
    wants: &[String],
    haves: &[String],
) -> Result<Vec<u8>, SubmitRefusal> {
    export_pack_with_format(chain, blobs, wants, haves).map(|(_, pack)| pack)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_changeset::{accept, AcceptContext, ChainView};
    use crate::source_history::{read_blame, read_path_history};
    use medulla_compute::source_git_objects::{MODE_EXECUTABLE, MODE_FILE, MODE_SYMLINK};
    use medulla_compute::source_history::{rebuild, HistoryIndex};
    use medulla_source_types::source_changeset::{
        FileMergeRefusal, ReadBlameRequest, ReadPathHistoryRequest, SubmittedBlobRef,
    };
    use sha2::{Digest, Sha256};
    use std::cell::RefCell;

    const REPO: &str = "repo";
    const BRANCH: &str = "branch";

    /// A content-addressed blob store in memory.
    #[derive(Default)]
    struct MemBlobs(RefCell<HashMap<String, Vec<u8>>>);

    impl BlobIo for MemBlobs {
        fn read(&self, digest: &str) -> Result<Vec<u8>, String> {
            self.0
                .borrow()
                .get(digest)
                .cloned()
                .ok_or_else(|| "absent".to_string())
        }
        fn store(&self, bytes: &[u8]) -> Result<SubmittedBlobRef, String> {
            let digest = hex::encode(Sha256::digest(bytes));
            self.0.borrow_mut().insert(digest.clone(), bytes.to_vec());
            Ok(SubmittedBlobRef {
                digest,
                size: bytes.len() as u64,
            })
        }
    }

    /// The branch chain: changesets by commit id, the commit id being the
    /// changeset id.
    struct MemChain {
        head: String,
        changesets: HashMap<String, Changeset>,
        order: Vec<String>,
    }

    impl MemChain {
        fn new() -> Self {
            Self {
                head: "0".repeat(64),
                changesets: HashMap::new(),
                order: Vec::new(),
            }
        }
        fn head_changeset(&self) -> Option<&Changeset> {
            self.changesets.get(&self.head)
        }
    }

    impl ChainView for MemChain {
        fn head(&self) -> &str {
            &self.head
        }
        fn changeset_at(&self, commit_id: &str) -> Result<Option<Changeset>, String> {
            Ok(self.changesets.get(commit_id).cloned())
        }
    }

    impl TreeChain for MemChain {
        fn changeset_at(&self, rev: &str) -> Result<Option<Changeset>, String> {
            Ok(self.changesets.get(rev).cloned())
        }
        fn checkpoint_at(&self, _rev: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
    }

    fn no_merge(_: &[u8], _: &[u8], _: &[u8]) -> Result<Vec<u8>, FileMergeRefusal> {
        Err(FileMergeRefusal::Conflict)
    }

    /// Append every planned commit through the real `accept`.
    fn apply(plan: &ImportPlan, chain: &mut MemChain, blobs: &MemBlobs) -> Vec<Changeset> {
        let mut appended = Vec::new();
        for planned in &plan.commits {
            let base_head = chain.order.last().cloned();
            let step = materialize(planned, blobs, REPO, BRANCH, base_head).unwrap();
            step.request.validate_import().unwrap();
            let ctx = AcceptContext {
                author_principal: "importer",
                created_at_ms: step.created_at_ms,
                blobs,
                merge: no_merge,
                agent_identity: None,
                git_origin: Some(step.origin),
            };
            let changeset = accept(&*chain, &step.request, &ctx).unwrap();
            chain.head = changeset.id.clone();
            chain.order.push(changeset.id.clone());
            chain.changesets.insert(changeset.id.clone(), changeset.clone());
            appended.push(changeset);
        }
        appended
    }

    type Files = BTreeMap<String, (u32, Vec<u8>)>;

    /// A generated git history: objects plus the commit ids by name.
    struct Fixture {
        format: GitObjectFormat,
        objects: Vec<(GitObjectKind, Vec<u8>)>,
        ids: BTreeMap<String, String>,
        tick: i64,
    }

    impl Fixture {
        fn commit(&mut self, name: &str, files: &Files, parents: &[&String], message: &str) -> String {
            let mut flat: BTreeMap<String, (u32, String)> = BTreeMap::new();
            for (path, (mode, content)) in files {
                let id = if *mode == MODE_SUBMODULE {
                    String::from_utf8(content.clone()).unwrap()
                } else {
                    self.objects.push((GitObjectKind::Blob, content.clone()));
                    object_id(self.format, GitObjectKind::Blob, content)
                };
                flat.insert(path.clone(), (*mode, id));
            }
            let (root, trees) = build_tree_objects(self.format, &flat).unwrap();
            for (_, body) in trees {
                self.objects.push((GitObjectKind::Tree, body));
            }
            self.tick += 1;
            let time = 1_700_000_000 + self.tick * 60;
            let mut headers = String::new();
            for parent in parents {
                headers.push_str(&format!("parent {parent}\n"));
            }
            headers.push_str(&format!(
                "author A <a@x> {time} +0000\ncommitter A <a@x> {time} +0000\n"
            ));
            let body = build_commit(&root, &headers, message);
            let id = object_id(self.format, GitObjectKind::Commit, &body);
            self.objects.push((GitObjectKind::Commit, body));
            self.ids.insert(name.to_string(), id.clone());
            id
        }

        fn pack(&self) -> PackObjects {
            let mut bytes = Vec::new();
            write_pack(self.format, &self.objects, &mut bytes).unwrap();
            decode_pack(self.format, &bytes).unwrap()
        }
    }

    fn put(files: &mut Files, path: &str, mode: u32, content: &str) {
        files.insert(path.to_string(), (mode, content.as_bytes().to_vec()));
    }

    /// Twenty commits: a first-parent line c1..c14, a side branch s1..s3 from
    /// c9, the merge `m` of c14 and s3 (second parent), then c16 and c17. The
    /// history holds a symlink, an executable file, a submodule entry, an empty
    /// commit (c5) and a delete.
    fn history(format: GitObjectFormat) -> Fixture {
        let mut fx = Fixture {
            format,
            objects: Vec::new(),
            ids: BTreeMap::new(),
            tick: 0,
        };
        let submodule = object_id(format, GitObjectKind::Commit, b"submodule head");
        let mut files = Files::new();
        put(&mut files, "a.txt", MODE_FILE, "alpha\n");
        put(&mut files, "notes.txt", MODE_FILE, "l1\nl2\nl3\n");
        put(&mut files, "link", MODE_SYMLINK, "a.txt");
        put(&mut files, "run.sh", MODE_EXECUTABLE, "#!/bin/sh\n");
        put(&mut files, "sub", MODE_SUBMODULE, &submodule);
        let mut last = fx.commit("c1", &files, &[], "c1\n");
        put(&mut files, "a.txt", MODE_FILE, "alpha2\n");
        last = fx.commit("c2", &files, &[&last], "c2\n");
        put(&mut files, "dir/x.txt", MODE_FILE, "x1\n");
        last = fx.commit("c3", &files, &[&last], "c3\n");
        put(&mut files, "dir/x.txt", MODE_FILE, "x2\n");
        last = fx.commit("c4", &files, &[&last], "c4\n");
        last = fx.commit("c5", &files, &[&last], "c5 empty\n");
        files.remove("dir/x.txt");
        last = fx.commit("c6", &files, &[&last], "c6 delete\n");
        for n in 7..=9 {
            put(&mut files, &format!("f{n}.txt"), MODE_FILE, &format!("f{n}\n"));
            last = fx.commit(&format!("c{n}"), &files, &[&last], &format!("c{n}\n"));
        }
        let fork = last.clone();
        let fork_files = files.clone();
        for n in 10..=14 {
            put(&mut files, &format!("f{n}.txt"), MODE_FILE, &format!("f{n}\n"));
            last = fx.commit(&format!("c{n}"), &files, &[&last], &format!("c{n}\n"));
        }
        let c14 = last.clone();
        // The side branch.
        let mut side = fork_files;
        put(&mut side, "notes.txt", MODE_FILE, "l1\nl2\nl3\nside1\n");
        let mut s = fx.commit("s1", &side, &[&fork], "side-1\n");
        put(&mut side, "side.txt", MODE_FILE, "s2\n");
        s = fx.commit("s2", &side, &[&s], "side-2\n");
        put(&mut side, "side.txt", MODE_FILE, "s3\n");
        s = fx.commit("s3", &side, &[&s], "side-3\n");
        // The merge: the first parent's tree plus what the side brought in.
        put(&mut files, "notes.txt", MODE_FILE, "l1\nl2\nl3\nside1\n");
        put(&mut files, "side.txt", MODE_FILE, "s3\n");
        last = fx.commit("m", &files, &[&c14, &s], "merge side\n");
        put(&mut files, "a.txt", MODE_FILE, "alpha3\n");
        last = fx.commit("c16", &files, &[&last], "c16\n");
        put(&mut files, "notes.txt", MODE_FILE, "l1\nl2\nl3\nside1\ntail\n");
        fx.commit("c17", &files, &[&last], "c17\n");
        fx
    }

    /// Verifies MEDULLA-SCM-R010: a generated twenty-commit history (symlink,
    /// executable file, submodule entry, empty commit and one merge of two
    /// parents whose second parent carries three commits) imports and exports
    /// with identical commit and tree ids, in both object formats; the merge
    /// changeset stores both parent ids, a side pack digest and its paths against
    /// the first parent; the export passes a connectivity check; a second import
    /// of the same head skips every first-parent commit.
    // spec: MEDULLA-SCM-R010
    #[test]
    fn export_import_preserves_commit_and_tree_ids() {
        for format in [GitObjectFormat::Sha1, GitObjectFormat::Sha256] {
            let fx = history(format);
            assert_eq!(fx.objects.iter().filter(|o| o.0 == GitObjectKind::Commit).count(), 20);
            let objects = fx.pack();
            let head = fx.ids["c17"].clone();
            let blobs = MemBlobs::default();
            let mut chain = MemChain::new();

            let plan = plan_import(format, &objects, &head, &BranchHeadGit::Empty, 100_000).unwrap();
            assert_eq!(plan.skipped, 0);
            assert_eq!(plan.commits.len(), 17);
            let appended = apply(&plan, &mut chain, &blobs);

            // The empty commit is an empty changeset.
            assert!(appended[4].changed_paths.is_empty());
            // The merge changeset.
            let merge = appended[14].git.as_ref().unwrap();
            assert_eq!(merge.commit_id, fx.ids["m"]);
            assert_eq!(merge.parent_commit_ids, vec![fx.ids["c14"].clone(), fx.ids["s3"].clone()]);
            assert!(merge.side_pack_digest.is_some());
            let merged_paths: Vec<&str> = appended[14]
                .changed_paths
                .iter()
                .map(|p| p.path.as_str())
                .collect();
            assert_eq!(merged_paths, ["notes.txt", "side.txt"]);
            // Ordinary commits keep one parent and no side pack.
            assert!(appended[3].git.as_ref().unwrap().side_pack_digest.is_none());
            assert_eq!(appended[0].git.as_ref().unwrap().parent_commit_ids.len(), 0);
            // The chain is the first-parent line.
            for pair in appended.windows(2) {
                assert_eq!(pair[1].parents, vec![pair[0].id.clone()]);
            }

            // Export every commit and tree id back.
            let exported = export_pack(&chain, &blobs, &[chain.head.clone()], &[]).unwrap();
            let read = read_pack(format, &exported).unwrap();
            for (name, id) in &fx.ids {
                assert_eq!(read[id].0, GitObjectKind::Commit, "commit {name}");
                assert_eq!(read[id].1, objects[id].1, "commit {name} body");
            }
            for changeset in &appended {
                let origin = changeset.git.as_ref().unwrap();
                let (_, body) = &read[&origin.commit_id];
                assert_eq!(parse_commit(body).unwrap().tree_id, origin.tree_id);
            }
            // Connectivity: everything the head commit reaches is in the pack,
            // including the whole side branch.
            let reached = closure(format, &read, &[head.clone()], &BTreeSet::new()).unwrap();
            for name in ["s1", "s2", "s3", "m", "c1", "c17"] {
                assert!(reached.contains(&fx.ids[name]), "{name} is reachable");
            }
            assert_eq!(read.len(), objects.len());

            // Haves: exporting past c14 sends no commit older than it.
            let have = appended[13].id.clone();
            let partial = export_pack(&chain, &blobs, &[chain.head.clone()], &[have]).unwrap();
            let partial = read_pack(format, &partial).unwrap();
            assert!(!partial.contains_key(&fx.ids["c14"]));
            assert!(partial.contains_key(&fx.ids["m"]));
            assert!(partial.contains_key(&fx.ids["s1"]));

            // A second import of the same head writes nothing.
            let again = plan_import(
                format,
                &objects,
                &head,
                &head_git(chain.head_changeset()),
                100_000,
            )
            .unwrap();
            assert!(again.commits.is_empty());
            assert_eq!(again.skipped, 17);

            // An incremental import resumes after the commits the branch holds.
            let mut resumed = MemChain::new();
            let first = plan_import(
                format,
                &objects,
                &fx.ids["c9"],
                &BranchHeadGit::Empty,
                100_000,
            )
            .unwrap();
            assert_eq!(first.commits.len(), 9);
            apply(&first, &mut resumed, &blobs);
            let rest = plan_import(
                format,
                &objects,
                &head,
                &head_git(resumed.head_changeset()),
                100_000,
            )
            .unwrap();
            assert_eq!(rest.skipped, 9);
            assert_eq!(rest.commits.len(), 8);
            assert_eq!(
                rest.commits[5].origin.parent_commit_ids,
                vec![fx.ids["c14"].clone(), fx.ids["s3"].clone()]
            );
            assert_eq!(rest.commits[5].origin.commit_id, fx.ids["m"]);
            apply(&rest, &mut resumed, &blobs);
            assert_eq!(
                resumed.head_changeset().unwrap().id,
                chain.head_changeset().unwrap().id
            );
        }
    }

    struct MemIndex {
        rows: BTreeMap<String, Vec<u8>>,
        changesets: HashMap<String, Changeset>,
    }

    impl HistoryIndex for MemIndex {
        fn row(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.rows.get(key).cloned())
        }
        fn scan_rev(
            &self,
            prefix: &str,
            upper: &str,
            limit: usize,
        ) -> Result<Vec<(String, Vec<u8>)>, String> {
            Ok(self
                .rows
                .range(prefix.to_string()..=upper.to_string())
                .rev()
                .filter(|(key, _)| key.starts_with(prefix))
                .take(limit)
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect())
        }
        fn changeset_at(&self, commit_id: &str) -> Result<Option<Changeset>, String> {
            Ok(self.changesets.get(commit_id).cloned())
        }
    }

    /// Verifies MEDULLA-SCM-R010 and MEDULLA-SCM-R005: path history and blame of
    /// a path the merged side changed answer with the merge changeset, with no
    /// side commit in the entries, because the chain is the first-parent line.
    // spec: MEDULLA-SCM-R010
    // spec: MEDULLA-SCM-R005
    #[test]
    fn history_and_blame_follow_first_parent_through_a_merge() {
        let fx = history(GitObjectFormat::Sha1);
        let objects = fx.pack();
        let blobs = MemBlobs::default();
        let mut chain = MemChain::new();
        let plan = plan_import(
            GitObjectFormat::Sha1,
            &objects,
            &fx.ids["c17"],
            &BranchHeadGit::Empty,
            100_000,
        )
        .unwrap();
        let appended = apply(&plan, &mut chain, &blobs);
        let (c1, merge, c17) = (&appended[0], &appended[14], &appended[16]);

        let oldest_first: Vec<(String, Changeset)> = appended
            .iter()
            .map(|changeset| (changeset.id.clone(), changeset.clone()))
            .collect();
        let rows = rebuild(&oldest_first, &|digest: &str| blobs.read(digest)).unwrap();
        let index = MemIndex {
            rows: rows.into_iter().collect(),
            changesets: chain.changesets.clone(),
        };
        let head = chain.head.clone();

        let history = read_path_history(
            &index,
            &head,
            &ReadPathHistoryRequest {
                repo: REPO.into(),
                branch: BRANCH.into(),
                rev: head.clone(),
                path: "notes.txt".into(),
                limit: 0,
            },
        )
        .unwrap()
        .unwrap();
        let ids: Vec<&str> = history.entries.iter().map(|e| e.changeset_id.as_str()).collect();
        assert_eq!(ids, [c17.id.as_str(), merge.id.as_str(), c1.id.as_str()]);
        assert!(history
            .entries
            .iter()
            .all(|entry| !entry.message.starts_with("side-")));

        // A path only the side branch changed is attributed to the merge.
        let side = read_path_history(
            &index,
            &head,
            &ReadPathHistoryRequest {
                repo: REPO.into(),
                branch: BRANCH.into(),
                rev: head.clone(),
                path: "side.txt".into(),
                limit: 0,
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(side.entries.len(), 1);
        assert_eq!(side.entries[0].changeset_id, merge.id);

        let blame = read_blame(
            &index,
            &head,
            &ReadBlameRequest {
                repo: REPO.into(),
                branch: BRANCH.into(),
                rev: head.clone(),
                path: "notes.txt".into(),
            },
        )
        .unwrap()
        .unwrap();
        let runs: Vec<(u32, u32, &str)> = blame
            .runs
            .iter()
            .map(|run| (run.start_line, run.line_count, run.changeset_id.as_str()))
            .collect();
        assert_eq!(
            runs,
            [
                (0, 3, c1.id.as_str()),
                (3, 1, merge.id.as_str()),
                (4, 1, c17.id.as_str())
            ]
        );
    }

    // spec: MEDULLA-SCM-R010
    #[test]
    fn a_broken_graph_refuses_before_anything_is_planned() {
        let format = GitObjectFormat::Sha1;
        let fx = history(format);
        let mut objects = fx.pack();
        // A commit the pack lacks.
        let missing = fx.ids["c3"].clone();
        objects.remove(&missing);
        let refusal = plan_import(format, &objects, &fx.ids["c5"], &BranchHeadGit::Empty, 100)
            .unwrap_err();
        assert_eq!(refusal.refusal, ChangesetRefusal::GitGraphMalformed);

        // A branch head that is not on the first-parent line of head.
        let objects = fx.pack();
        let refusal = plan_import(
            format,
            &objects,
            &fx.ids["c5"],
            &BranchHeadGit::Commit(fx.ids["s1"].clone()),
            100,
        )
        .unwrap_err();
        assert_eq!(refusal.refusal, ChangesetRefusal::GitGraphMalformed);
        let refusal = plan_import(format, &objects, &fx.ids["c5"], &BranchHeadGit::Native, 100)
            .unwrap_err();
        assert_eq!(refusal.refusal, ChangesetRefusal::GitGraphMalformed);

        // More new commits than the limit.
        let refusal = plan_import(format, &objects, &fx.ids["c17"], &BranchHeadGit::Empty, 5)
            .unwrap_err();
        assert_eq!(refusal.refusal, ChangesetRefusal::ChangesetInvalid);

        // A tree entry with a mode outside the four file modes.
        let blob = object_id(format, GitObjectKind::Blob, b"x");
        let mut tree = b"100664 f\0".to_vec();
        tree.extend(hex::decode(&blob).unwrap());
        let tree_id = object_id(format, GitObjectKind::Tree, &tree);
        let commit = build_commit(
            &tree_id,
            "author A <a@x> 1 +0000\ncommitter A <a@x> 1 +0000\n",
            "m\n",
        );
        let commit_id = object_id(format, GitObjectKind::Commit, &commit);
        let mut bad = PackObjects::new();
        bad.insert(blob, (GitObjectKind::Blob, b"x".to_vec()));
        bad.insert(tree_id, (GitObjectKind::Tree, tree));
        bad.insert(commit_id.clone(), (GitObjectKind::Commit, commit));
        let refusal = plan_import(format, &bad, &commit_id, &BranchHeadGit::Empty, 100).unwrap_err();
        assert_eq!(refusal.refusal, ChangesetRefusal::GitModeUnsupported);
    }
}
