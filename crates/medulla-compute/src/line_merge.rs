//! MEDULLA-BRANCH-MERGE-R008 — line-level three-way merge for file blobs.
//!
//! `merge_lines(base, ours, theirs)` runs a diff3 over `\n`-separated lines
//! and returns either a clean merged byte blob or the list of conflicting
//! hunks. The function is pure and holds no handler type, so both the accept
//! path (`MEDULLA-SCM-R003`) and `merge_file` of `MEDULLA-BRANCH-MERGE-R010`
//! call it. Binary blobs never merge: any disagreement is a whole-blob
//! conflict.

/// One edit region of a diff against `base`:
/// `chunk = (base_start, base_len, replacement_lines)`.
/// `base_len == 0` is a pure insertion at `base_start`.
pub type Chunk = (usize, usize, Vec<String>);

/// One non-clean region of a three-way merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictHunk {
    /// Zero-based index of the first base line the hunk covers.
    pub base_start: usize,
    /// Number of base lines the hunk covers (0 for pure insertions).
    pub base_len: usize,
    /// Lines from `ours` over the base region.
    pub ours: Vec<String>,
    /// Lines from `theirs` over the base region.
    pub theirs: Vec<String>,
}

/// Result of a three-way line merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineMerge {
    /// The merge is clean; the bytes are the merged blob.
    Clean(Vec<u8>),
    /// The merge conflicts; every non-clean hunk is listed.
    Conflict(Vec<ConflictHunk>),
}

/// MEDULLA-BRANCH-MERGE-R008 — a NUL byte means binary content.
fn is_binary(blob: &[u8]) -> bool {
    blob.contains(&0)
}

/// Split a blob into lines plus the trailing-newline flag; an interior
/// (non-trailing) newline never adds an empty invisible line.
fn split(blob: &[u8]) -> (Vec<String>, bool) {
    if blob.is_empty() {
        return (Vec::new(), false);
    }
    let trailing = blob.ends_with(b"\n");
    let text = String::from_utf8_lossy(blob);
    let body = text.trim_end_matches('\n');
    if body.is_empty() {
        return (vec![String::new()], trailing);
    }
    (body.split('\n').map(String::from).collect(), trailing)
}

fn join(lines: &[String], trailing: bool) -> Vec<u8> {
    let mut out = lines.join("\n").into_bytes();
    if trailing {
        out.push(b'\n');
    }
    out
}

/// LCS diff of one side against `base`, as ordered edit chunks.
fn diff_chunks(base: &[String], side: &[String]) -> Vec<Chunk> {
    let (n, m) = (base.len(), side.len());
    let mut table = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if base[i] == side[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut chunks: Vec<Chunk> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && base[i] == side[j] {
            i += 1;
            j += 1;
            continue;
        }
        let start = i;
        let mut replaced: Vec<String> = Vec::new();
        while i < n || j < m {
            if i < n && j < m && base[i] == side[j] {
                break;
            }
            if i < n && (j == m || table[i][j] == table[i + 1][j]) {
                i += 1;
            } else {
                replaced.push(side[j].clone());
                j += 1;
            }
        }
        chunks.push((start, i - start, replaced));
    }
    chunks
}

/// Two half-open base ranges touch (an empty insertion range conflicts with
/// a range that starts at, or whose start lies inside, the other).
fn ranges_touch(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

/// MEDULLA-BRANCH-MERGE-R008 — three-way line merge. Deterministic and pure.
pub fn merge_lines(base: &[u8], ours: &[u8], theirs: &[u8]) -> LineMerge {
    // Identity fast paths: a side equal to the other or to base contributes
    // nothing, so the merge is that side's text.
    if ours == theirs {
        return LineMerge::Clean(ours.to_vec());
    }
    if base == ours {
        return LineMerge::Clean(theirs.to_vec());
    }
    if base == theirs {
        return LineMerge::Clean(ours.to_vec());
    }
    // Binary blobs never merge: any overlap is a conflict.
    if is_binary(base) || is_binary(ours) || is_binary(theirs) {
        return LineMerge::Conflict(vec![ConflictHunk {
            base_start: 0,
            base_len: base.len(),
            ours: Vec::new(),
            theirs: Vec::new(),
        }]);
    }
    let (base_lines, base_nl) = split(base);
    let (ours_lines, ours_nl) = split(ours);
    let (theirs_lines, theirs_nl) = split(theirs);
    let ours_chunks = diff_chunks(&base_lines, &ours_lines);
    let theirs_chunks = diff_chunks(&base_lines, &theirs_lines);

    // Overlap detection: any touching chunk pair (one from each side) whose
    // replacement texts still differ is a conflict.
    let mut conflict_hunks: Vec<ConflictHunk> = Vec::new();
    for a in &ours_chunks {
        let range_a = (a.0, a.0 + a.1);
        for b in &theirs_chunks {
            let range_b = (b.0, b.0 + b.1);
            if ranges_touch(range_a, range_b) && a.2 != b.2 {
                conflict_hunks.push(ConflictHunk {
                    base_start: range_a.0,
                    base_len: a.1,
                    ours: a.2.clone(),
                    theirs: b.2.clone(),
                });
            }
        }
    }
    if !conflict_hunks.is_empty() {
        // MEDULLA-BRANCH-MERGE-R008 — conflicts reported with both candidates.
        return LineMerge::Conflict(conflict_hunks);
    }

    // Clean merge: walk the base in order, applying each side's chunks.
    let mut merged: Vec<String> = Vec::new();
    let mut pos = 0usize;
    let (mut o, mut t) = (0usize, 0usize);
    while o < ours_chunks.len() || t < theirs_chunks.len() {
        let next = match (ours_chunks.get(o), theirs_chunks.get(t)) {
            (Some(a), Some(b)) => a.0.min(b.0),
            (Some(a), None) => a.0,
            (None, Some(b)) => b.0,
            (None, None) => break,
        };
        if next > pos {
            merged.extend(base_lines[pos..next].iter().cloned());
            // The chunk handlers below always advance `pos` past their region.
            let _ = pos;
        }
        if let Some(a) = ours_chunks.get(o).filter(|a| a.0 == next) {
            merged.extend(a.2.iter().cloned());
            pos = a.0 + a.1;
            o += 1;
            continue;
        }
        let b = theirs_chunks.get(t).expect("chunk boundary");
        merged.extend(b.2.iter().cloned());
        pos = b.0 + b.1;
        t += 1;
    }
    if pos < base_lines.len() {
        merged.extend(base_lines[pos..].iter().cloned());
    }
    let trailing = base_nl && ours_nl && theirs_nl;
    LineMerge::Clean(join(&merged, trailing))
}

// -- Bindings ---------------------------------------------------------------

// spec: MEDULLA-BRANCH-MERGE-R008
#[cfg(test)]
mod tests {
    use super::*;

    /// spec: MEDULLA-BRANCH-MERGE-R008
    #[test]
    fn clean_and_conflicting_hunks() {
        // Disjoint line edits merge clean.
        assert_eq!(
            merge_lines(b"a\nb\nc\n", b"A\nb\nc\n", b"a\nb\nC\n"),
            LineMerge::Clean(b"A\nb\nC\n".to_vec())
        );
        // Two edits to the same base line conflict with both candidates.
        match merge_lines(b"one\n", b"ours\n", b"theirs\n") {
            LineMerge::Conflict(hunks) => {
                assert_eq!(hunks.len(), 1);
                assert_eq!(hunks[0].ours, vec!["ours".to_string()]);
                assert_eq!(hunks[0].theirs, vec!["theirs".to_string()]);
            }
            LineMerge::Clean(bytes) => panic!("expected conflict, got {bytes:?}"),
        }
        // One side deletes; the other is untouched base: clean.
        assert_eq!(
            merge_lines(b"a\nb\nc\n", b"a\nb\n", b"a\nb\nc\n"),
            LineMerge::Clean(b"a\nb\n".to_vec())
        );
        // Identical sides collapse before any diff.
        assert_eq!(
            merge_lines(b"a\nb\n", b"x\n", b"x\n"),
            LineMerge::Clean(b"x\n".to_vec())
        );
        // Binary blobs never merge.
        assert!(matches!(
            merge_lines(&[0u8, 1, 2], b"ours", b"theirs"),
            LineMerge::Conflict(_)
        ));
    }
}
