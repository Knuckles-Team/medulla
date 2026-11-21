//! The source changeset record (MEDULLA-SCM-R002): one commit-shaped change to a
//! repository, with its structured trailers (MEDULLA-SCM-R002.1).
//!
//! Pure data. Acceptance, rebase and git import and export live above this
//! crate; MEDULLA-SDLC derivation reads the typed trailer fields.

use serde::{Deserialize, Serialize};

/// What a changeset did to one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PathOp {
    Add,
    Modify,
    Delete,
    Rename,
}

/// One path that changed against the first parent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ChangedPath {
    pub path: String,
    pub op: PathOp,
    pub old_blob: Option<String>,
    pub new_blob: Option<String>,
    pub mode: u32,
}

/// One file version a changeset names (MEDULLA-SCM-R001). `digest` is the value
/// `BlobCommit` returned for the stored bytes; `size` counts the bytes from 0
/// to `changeset_max_blob_bytes`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SubmittedBlobRef {
    pub digest: String,
    pub size: u64,
}

/// Typed refusal of the source-changeset family; each variant serves one
/// stable refusal code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangesetRefusal {
    /// Declared and recomputed digests or sizes differ.
    BlobDigestMismatch,
    /// A declared blob exceeds `changeset_max_blob_bytes`.
    BlobTooLarge,
    /// The digest is unknown to the blob store, or unreadable.
    BlobNotFound,
    /// A submission with no paths (MEDULLA-SCM-R002).
    ChangesetEmpty,
    /// A bound or pairing of the request is broken (MEDULLA-SCM-R002).
    ChangesetInvalid,
    /// The writer's base is not a changeset of the branch (MEDULLA-SCM-R002).
    ChangesetParentNotOnBranch,
    /// An overlapping path whose three-way line merge does not merge cleanly
    /// (MEDULLA-BRANCH-MERGE-R010); the detail lists every conflicting path.
    SourceConflict,
    /// A blob of an overlapping path could not be read for the merge
    /// (MEDULLA-BRANCH-MERGE-R010).
    MergeBlobUnreadable,
    /// A tree read named a revision that is not a changeset of the branch
    /// chain (MEDULLA-SCM-R004).
    RevisionNotOnChain,
    /// The history index row of the branch head is absent, of another
    /// version, or names another commit (MEDULLA-SCM-R005).
    HistoryIndexStale,
    /// The blamed file is binary (MEDULLA-SCM-R005.2).
    BlameBinary,
    /// The blamed file has more lines than the blame index holds
    /// (MEDULLA-SCM-R005.2).
    BlameFileTooLarge,
    /// The blamed path is deleted or has no blame row at the revision
    /// (MEDULLA-SCM-R005.2).
    BlamePathAbsent,
    /// An agent request that names no run (MEDULLA-SCM-R012).
    AgentRunTraceMissing,
    /// A `run_trace_id` that names no `run_trace` node the caller can read
    /// (MEDULLA-SCM-R012).
    RunTraceUnknown,
}

impl ChangesetRefusal {
    /// The stable refusal code served on the wire.
    pub fn code(&self) -> &'static str {
        match self {
            Self::BlobDigestMismatch => "BLOB_DIGEST_MISMATCH",
            Self::BlobTooLarge => "BLOB_TOO_LARGE",
            Self::BlobNotFound => "BLOB_NOT_FOUND",
            Self::ChangesetEmpty => "CHANGESET_EMPTY",
            Self::ChangesetInvalid => "CHANGESET_INVALID",
            Self::ChangesetParentNotOnBranch => "CHANGESET_PARENT_NOT_ON_BRANCH",
            Self::SourceConflict => "SOURCE_CONFLICT",
            Self::MergeBlobUnreadable => "MERGE_BLOB_UNREADABLE",
            Self::RevisionNotOnChain => "REVISION_NOT_ON_CHAIN",
            Self::HistoryIndexStale => "HISTORY_INDEX_STALE",
            Self::BlameBinary => "BLAME_BINARY",
            Self::BlameFileTooLarge => "BLAME_FILE_TOO_LARGE",
            Self::BlamePathAbsent => "BLAME_PATH_ABSENT",
            Self::AgentRunTraceMissing => "AGENT_RUN_TRACE_MISSING",
            Self::RunTraceUnknown => "RUN_TRACE_UNKNOWN",
        }
    }
}

/// Why a three-way file merge produced no merged blob (MEDULLA-BRANCH-MERGE-R010).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileMergeRefusal {
    /// The hunks overlap, or a blob is binary: the writer must resolve it.
    Conflict,
}

/// The three-way file merge `accept` calls on an overlapping path:
/// `(base, ours, theirs)` blob bytes to the merged bytes.
pub type FileMerger = fn(&[u8], &[u8], &[u8]) -> Result<Vec<u8>, FileMergeRefusal>;

/// The structured form of the `Spec:`, `Fixes:` and `Spec-Withdraw:` trailers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Trailers {
    pub spec_ids: Vec<String>,
    pub fixes: Vec<String>,
    pub withdraw: Vec<String>,
    pub withdraw_reason: Option<String>,
}

fn split_ids(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

impl Trailers {
    /// Parse the trailers from the last paragraph of a commit message. A
    /// `Spec: none (reason)` line cites no requirement.
    pub fn parse(message: &str) -> Self {
        let mut out = Self::default();
        let Some(block) = message.trim_end().rsplit("\n\n").next() else {
            return out;
        };
        for line in block.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "Spec" if !value.starts_with("none") => out.spec_ids.extend(split_ids(value)),
                "Fixes" => out.fixes.extend(split_ids(value)),
                "Spec-Withdraw" => {
                    let (ids, reason) = match value.split_once(" (") {
                        Some((ids, reason)) => (ids, reason.strip_suffix(')')),
                        None => (value, None),
                    };
                    out.withdraw.extend(split_ids(ids));
                    out.withdraw_reason = reason.map(str::to_owned);
                }
                _ => {}
            }
        }
        out
    }

    /// Render the trailer lines, in the order `Spec`, `Fixes`, `Spec-Withdraw`.
    pub fn render(&self) -> String {
        let mut lines = Vec::new();
        if !self.spec_ids.is_empty() {
            lines.push(format!("Spec: {}", self.spec_ids.join(", ")));
        }
        if !self.fixes.is_empty() {
            lines.push(format!("Fixes: {}", self.fixes.join(", ")));
        }
        if !self.withdraw.is_empty() {
            let reason = self
                .withdraw_reason
                .as_ref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default();
            lines.push(format!(
                "Spec-Withdraw: {}{reason}",
                self.withdraw.join(", ")
            ));
        }
        lines.join("\n")
    }
}

/// A changeset: the paths that changed against the first parent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Changeset {
    pub id: String,
    pub repo: String,
    pub parents: Vec<String>,
    pub author_principal: String,
    pub message: String,
    pub changed_paths: Vec<ChangedPath>,
    pub created_at: u64,
    pub trailers: Trailers,
    /// The agent that authored the change (MEDULLA-SCM-R012); `None` for a human.
    pub agent_identity: Option<String>,
    /// The run that produced the change (MEDULLA-SCM-R012).
    pub run_trace_id: Option<String>,
}

impl Changeset {
    /// The id of a changeset: the SHA-256 of the canonical named MessagePack
    /// of every other field, so the id commits to the whole record.
    pub fn canonical_id(&self) -> String {
        use sha2::{Digest, Sha256};
        let body = (
            &self.repo,
            &self.parents,
            &self.author_principal,
            &self.message,
            &self.changed_paths,
            self.created_at,
            &self.trailers,
            &self.agent_identity,
            &self.run_trace_id,
        );
        let bytes = rmp_serde::to_vec_named(&body).unwrap_or_default();
        hex::encode(Sha256::digest(bytes))
    }

    /// Fill `id` from the record content.
    pub fn sealed(mut self) -> Self {
        self.id = self.canonical_id();
        self
    }
}

/// One path of a submission (MEDULLA-SCM-R002). `blob` is `None` exactly when
/// `op` is `Delete`; `renamed_from` is `Some` exactly when `op` is `Rename`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SubmittedPath {
    pub path: String,
    pub op: PathOp,
    pub blob: Option<SubmittedBlobRef>,
    pub mode: u32,
    pub renamed_from: Option<String>,
}

/// The closed request of `Method::SubmitChangeset` (MEDULLA-SCM-R002).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SubmitChangesetRequest {
    pub repo: String,
    pub branch: String,
    pub base_head: Option<String>,
    pub message: String,
    pub paths: Vec<SubmittedPath>,
    #[serde(default)]
    pub trailers: Option<Trailers>,
    #[serde(default)]
    pub run_trace_id: Option<String>,
}

/// The served receipt of an accepted (or replayed) changeset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SubmitChangesetReceipt {
    pub changeset: Changeset,
    /// The branch head after the append: the id of the carrying branch commit.
    pub branch_head: String,
    pub replayed: bool,
}

/// A request refused with the stable code and a detail naming the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitRefusal {
    pub refusal: ChangesetRefusal,
    pub detail: String,
}

impl SubmitRefusal {
    pub fn new(refusal: ChangesetRefusal, detail: impl Into<String>) -> Self {
        Self {
            refusal,
            detail: detail.into(),
        }
    }

    /// The wire message: `CODE: detail`.
    pub fn message(&self) -> String {
        format!("{}: {}", self.refusal.code(), self.detail)
    }
}

/// Deepest `depth` a `ReadTree` accepts (MEDULLA-SCM-R004, `Fixed`).
pub const TREE_READ_MAX_DEPTH: u32 = 64;

/// One file of a materialized tree (MEDULLA-SCM-R004).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TreeEntry {
    pub path: String,
    pub blob_digest: String,
    pub mode: u32,
}

/// The closed request of `Method::ReadTree` (MEDULLA-SCM-R004): the tree of
/// `repo` as of the changeset commit `rev` on `branch`, restricted to `prefix`
/// and to `depth` path segments below it (`0` is unbounded).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReadTreeRequest {
    pub repo: String,
    pub branch: String,
    pub rev: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub depth: u32,
}

/// The `limit` a `ReadPathHistory` uses when the request leaves it 0
/// (MEDULLA-SCM-R005.1).
pub const PATH_HISTORY_DEFAULT_LIMIT: u32 = 100;
/// The largest `limit` a `ReadPathHistory` accepts (MEDULLA-SCM-R005.1, `Fixed`).
pub const PATH_HISTORY_MAX_LIMIT: u32 = 1000;

/// The closed request of `Method::ReadPathHistory` (MEDULLA-SCM-R005.1): the
/// changesets that touched `path`, newest first, at or below the changeset
/// commit `rev` of `branch`. `limit` 0 means 100.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReadPathHistoryRequest {
    pub repo: String,
    pub branch: String,
    pub rev: String,
    pub path: String,
    #[serde(default)]
    pub limit: u32,
}

/// One changeset of a path's history (MEDULLA-SCM-R005.1): `commit` is the
/// branch commit that carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PathHistoryEntry {
    pub commit: String,
    pub changeset_id: String,
    pub op: PathOp,
    pub author_principal: String,
    pub created_at: u64,
    pub message: String,
}

/// The served answer of `ReadPathHistory`: entries newest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReadPathHistoryReceipt {
    pub entries: Vec<PathHistoryEntry>,
}

impl ReadPathHistoryRequest {
    /// Check every bound of the request; the first failure names its field.
    /// Returns the effective limit. No store is read.
    pub fn validate(&self) -> Result<usize, SubmitRefusal> {
        validate_coordinates(&self.repo, &self.branch, &self.rev)?;
        if !valid_path(&self.path) {
            return Err(invalid("path"));
        }
        if self.limit > PATH_HISTORY_MAX_LIMIT {
            return Err(invalid("limit"));
        }
        let limit = if self.limit == 0 {
            PATH_HISTORY_DEFAULT_LIMIT
        } else {
            self.limit
        };
        Ok(limit as usize)
    }
}

/// The closed request of `Method::ReadBlame` (MEDULLA-SCM-R005.2): the origin
/// changeset of every line of `path` at the changeset commit `rev` of `branch`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReadBlameRequest {
    pub repo: String,
    pub branch: String,
    pub rev: String,
    pub path: String,
}

/// A run of consecutive lines with one origin (MEDULLA-SCM-R005.2): 0-based
/// `start_line`, `commit` the branch commit and `changeset_id` the changeset
/// that introduced the lines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BlameRun {
    pub start_line: u32,
    pub line_count: u32,
    pub commit: String,
    pub changeset_id: String,
}

/// The served answer of `ReadBlame`: runs in line order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReadBlameReceipt {
    pub runs: Vec<BlameRun>,
}

impl ReadBlameRequest {
    /// Check every bound of the request; the first failure names its field.
    /// No store is read.
    pub fn validate(&self) -> Result<(), SubmitRefusal> {
        validate_coordinates(&self.repo, &self.branch, &self.rev)?;
        if !valid_path(&self.path) {
            return Err(invalid("path"));
        }
        Ok(())
    }
}

/// The served answer of `ReadTree`: entries sorted by path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReadTreeReceipt {
    pub entries: Vec<TreeEntry>,
}

impl ReadTreeRequest {
    /// Check every bound of the request; the first failure names its field.
    /// Returns the prefix without a trailing separator. No store is read.
    pub fn validate(&self) -> Result<String, SubmitRefusal> {
        validate_coordinates(&self.repo, &self.branch, &self.rev)?;
        if self.depth > TREE_READ_MAX_DEPTH {
            return Err(invalid("depth"));
        }
        let prefix = self.prefix.strip_suffix('/').unwrap_or(&self.prefix);
        if !prefix.is_empty() && !valid_path(prefix) {
            return Err(invalid("prefix"));
        }
        Ok(prefix.to_string())
    }
}

/// The bounds `ReadTree`, `ReadPathHistory` and `ReadBlame` share: `repo`,
/// `branch` and `rev` (the 64 lowercase hex id of a branch commit).
fn validate_coordinates(repo: &str, branch: &str, rev: &str) -> Result<(), SubmitRefusal> {
    if !valid_name(repo, 128) {
        return Err(invalid("repo"));
    }
    if !valid_name(branch, 255) {
        return Err(invalid("branch"));
    }
    if !is_hex64(rev) {
        return Err(invalid("rev"));
    }
    Ok(())
}

fn is_hex64(value: &str) -> bool {
    value.len() == 64
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

const NAME_SET: fn(char) -> bool =
    |c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-');
const FILE_MODES: [u32; 3] = [0o100644, 0o100755, 0o120000];

fn invalid(detail: impl Into<String>) -> SubmitRefusal {
    SubmitRefusal::new(ChangesetRefusal::ChangesetInvalid, detail)
}

fn valid_name(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.chars().count() <= max
        && value.chars().all(NAME_SET)
        && !value.split('/').any(|segment| segment == "..")
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains('\0')
        && !path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
}

impl SubmitChangesetRequest {
    /// Check every bound and pairing of the request; the first failure names
    /// its field. No store is read.
    pub fn validate(&self) -> Result<(), SubmitRefusal> {
        if self.paths.is_empty() {
            return Err(SubmitRefusal::new(
                ChangesetRefusal::ChangesetEmpty,
                "the submission has no paths",
            ));
        }
        if !valid_name(&self.repo, 128) {
            return Err(invalid("repo"));
        }
        if !valid_name(&self.branch, 255) {
            return Err(invalid("branch"));
        }
        if let Some(base) = &self.base_head {
            let hex64 = base.len() == 64
                && base
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
            if !hex64 {
                return Err(invalid("base_head"));
            }
        }
        if self.message.is_empty() || self.message.len() > 65536 {
            return Err(invalid("message"));
        }
        if self.paths.len() > 10000 {
            return Err(invalid("paths"));
        }
        if let Some(run) = &self.run_trace_id {
            if run.is_empty() || run.chars().count() > 128 {
                return Err(invalid("run_trace_id"));
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for entry in &self.paths {
            if !valid_path(&entry.path) {
                return Err(invalid(format!("paths.path {}", entry.path)));
            }
            if !seen.insert(entry.path.as_str()) {
                return Err(invalid(format!("paths.path repeated {}", entry.path)));
            }
            if (entry.op == PathOp::Delete) != entry.blob.is_none() {
                return Err(invalid(format!("paths.blob pairing for {}", entry.path)));
            }
            if (entry.op == PathOp::Rename) != entry.renamed_from.is_some() {
                return Err(invalid(format!(
                    "paths.renamed_from pairing for {}",
                    entry.path
                )));
            }
            if let Some(from) = &entry.renamed_from {
                if !valid_path(from) {
                    return Err(invalid(format!("paths.renamed_from {from}")));
                }
            }
            if !FILE_MODES.contains(&entry.mode) {
                return Err(invalid(format!("paths.mode for {}", entry.path)));
            }
        }
        for entry in &self.paths {
            if let Some(from) = &entry.renamed_from {
                if !seen.insert(from.as_str()) {
                    return Err(invalid(format!("paths.renamed_from repeated {from}")));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies MEDULLA-SCM-R002.1: Structured trailers on a changeset
    // spec: MEDULLA-SCM-R002.1
    #[test]
    fn trailers_parse_and_render_round_trip() {
        let message = "feat: x\n\nbody\n\nSpec: MEDULLA-A-R001, MEDULLA-A-R002.1\nFixes: DEF-7\nSpec-Withdraw: MEDULLA-A-R009 (superseded)";
        let trailers = Trailers::parse(message);
        assert_eq!(trailers.spec_ids, ["MEDULLA-A-R001", "MEDULLA-A-R002.1"]);
        assert_eq!(trailers.fixes, ["DEF-7"]);
        assert_eq!(trailers.withdraw, ["MEDULLA-A-R009"]);
        assert_eq!(trailers.withdraw_reason.as_deref(), Some("superseded"));
        assert_eq!(Trailers::parse(&trailers.render()), trailers);
        assert!(Trailers::parse("fix\n\nSpec: none (ci)")
            .spec_ids
            .is_empty());
    }

    /// Verifies MEDULLA-SCM-R001: the submitted blob ref is a closed wire
    /// shape, and the family's blob refusals carry their stable codes.
    // spec: MEDULLA-SCM-R001
    #[test]
    fn submitted_blob_ref_is_closed_and_refusal_codes_are_stable() {
        let submitted = SubmittedBlobRef {
            digest: "a".repeat(64),
            size: 134_217_728,
        };
        let encoded = serde_json::to_string(&submitted).unwrap();
        assert_eq!(
            serde_json::from_str::<SubmittedBlobRef>(&encoded).unwrap(),
            submitted
        );
        // The wire shape is closed: an unknown field is refused.
        assert!(
            serde_json::from_str::<SubmittedBlobRef>(r#"{"digest":"a","size":1,"extra":0}"#)
                .is_err()
        );
        assert_eq!(
            ChangesetRefusal::BlobDigestMismatch.code(),
            "BLOB_DIGEST_MISMATCH"
        );
        assert_eq!(ChangesetRefusal::BlobTooLarge.code(), "BLOB_TOO_LARGE");
        assert_eq!(ChangesetRefusal::BlobNotFound.code(), "BLOB_NOT_FOUND");
        assert_eq!(ChangesetRefusal::SourceConflict.code(), "SOURCE_CONFLICT");
        assert_eq!(
            ChangesetRefusal::MergeBlobUnreadable.code(),
            "MERGE_BLOB_UNREADABLE"
        );
    }
}
