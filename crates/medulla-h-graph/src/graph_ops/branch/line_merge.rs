//! Line-granularity file merge for source changesets
//! (MEDULLA-BRANCH-MERGE-R010).
//!
//! `merge_file` is a thin adapter over `merge_lines` of
//! MEDULLA-BRANCH-MERGE-R008 (`medulla_compute::line_merge`); it holds no
//! second diff. It is the [`FileMerger`] the accept path of
//! MEDULLA-SCM-R003 calls for each overlapping path: three blob byte slices
//! in, merged bytes out, or [`FileMergeRefusal::Conflict`] which `accept`
//! renders as `SOURCE_CONFLICT{paths}`. Pure and deterministic; it writes
//! nothing, the caller lands the merged blob.

use medulla_compute::line_merge::{merge_lines, LineMerge};
use medulla_types::source_changeset::{FileMergeRefusal, FileMerger};

/// Three-way text merge of one file over `base`, `ours` (the submission) and
/// `theirs` (the branch head). Disjoint hunks merge; overlapping hunks and
/// binary blobs conflict.
pub fn merge_file(base: &[u8], ours: &[u8], theirs: &[u8]) -> Result<Vec<u8>, FileMergeRefusal> {
    match merge_lines(base, ours, theirs) {
        LineMerge::Clean(bytes) => Ok(bytes),
        LineMerge::Conflict(_) => Err(FileMergeRefusal::Conflict),
    }
}

/// The merger the served accept path is wired with.
pub const FILE_MERGER: FileMerger = merge_file;

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-BRANCH-MERGE-R010
    #[test]
    fn merge_file_merges_disjoint_hunks_and_conflicts_on_overlap() {
        let base = b"one\ntwo\nthree\nfour\nfive\n";
        let ours = b"ONE\ntwo\nthree\nfour\nfive\n";
        let theirs = b"one\ntwo\nthree\nfour\nFIVE\n";
        assert_eq!(
            merge_file(base, ours, theirs).unwrap(),
            b"ONE\ntwo\nthree\nfour\nFIVE\n".to_vec()
        );
        let clash = b"one\ntwo\nTHREE\nfour\nfive\n";
        let other = b"one\ntwo\n3\nfour\nfive\n";
        assert_eq!(
            merge_file(base, clash, other),
            Err(FileMergeRefusal::Conflict)
        );
        // Deterministic.
        assert_eq!(
            merge_file(base, ours, theirs),
            merge_file(base, ours, theirs)
        );
        // A binary blob never merges.
        assert_eq!(
            merge_file(b"a\0b", b"a\0c", b"x\0b"),
            Err(FileMergeRefusal::Conflict)
        );
    }
}
