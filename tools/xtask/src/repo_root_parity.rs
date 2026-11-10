//! MEDULLA-CONTRACT-R008.1 / R008.2: a repository-parity check validates the resolved checkout.
//!
//! `verify_resolved_checkout` resolves the requested root and the actual checkout (symlinks
//! followed, made absolute) and fails closed when they are different directories.
//! `cargo xtask repo-root-parity <root>` runs it against the checkout the task runs from.

use std::fs;
use std::path::{Path, PathBuf};

pub const NAME: &str = "repo-root-parity";
pub const SUMMARY: &str = "the resolved checkout is the requested repository root";

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let requested = args
        .first()
        .ok_or("usage: cargo xtask repo-root-parity <repo-root>")?;
    verify_resolved_checkout(Path::new(requested), root).map(|_| ())
}

/// Fails closed unless `actual` is the same directory as `requested`; returns the resolved path.
pub fn verify_resolved_checkout(requested: &Path, actual: &Path) -> Result<PathBuf, String> {
    let wanted =
        fs::canonicalize(requested).map_err(|e| format!("{}: {e}", requested.display()))?;
    let found = fs::canonicalize(actual).map_err(|e| format!("{}: {e}", actual.display()))?;
    if wanted != found {
        return Err(format!(
            "resolved checkout {} does not match requested repository root {}; \
             refusing to compare against an unintended checkout",
            found.display(),
            wanted.display()
        ));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;

    // spec: MEDULLA-CONTRACT-R008.1
    // spec: MEDULLA-CONTRACT-R008.2
    #[test]
    fn mismatched_resolved_checkout_fails_closed() {
        let t = Tree::new(&[("requested/a", ""), ("other/b", "")]);
        let requested = t.path().join("requested");
        let checkout = t.path().join("other");
        assert!(verify_resolved_checkout(&requested, &checkout).is_err());
        // The command fails closed too: `main` calls `run` with the resolved
        // checkout and the requested root as its only argument.
        assert!(run(&checkout, &[requested.to_string_lossy().into()]).is_err());
    }

    #[test]
    fn matching_resolved_checkout_passes() {
        let t = Tree::new(&[("repo/a", "")]);
        let repo = t.path().join("repo");
        assert_eq!(
            verify_resolved_checkout(&repo, &repo).unwrap(),
            fs::canonicalize(&repo).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_checkout_of_the_same_repo_passes() {
        let t = Tree::new(&[("real/a", "")]);
        let link = t.path().join("link");
        std::os::unix::fs::symlink(t.path().join("real"), &link).unwrap();
        assert!(verify_resolved_checkout(&link, &t.path().join("real")).is_ok());
    }
}
