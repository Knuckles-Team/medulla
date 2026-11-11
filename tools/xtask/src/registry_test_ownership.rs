//! Fail closed when a tracked `tests/` file has no owner in the component registry.
//!
//! `architecture/component-registry.yml` is the repository's canonical owner manifest: each
//! component's `test_roots` owns a file or a directory prefix. Every tracked test file must
//! match one entry exactly, or sit inside a directory-prefix entry, so a new test file needs
//! an owner entry in the same change. The check reads the registry once and the tracked
//! `tests/` tree from `git ls-files`; it reports every unowned path and fails closed on any.

use std::path::Path;
use std::process::Command;

use serde_yaml_ng::Value;

pub const NAME: &str = "registry-test-ownership";
pub const SUMMARY: &str = "every tracked test file has a component-registry owner";

pub fn run(root: &Path, _args: &[String]) -> Result<(), String> {
    let entries = load_test_roots(root)?;
    if entries.is_empty() {
        return Err(
            "registry-test-ownership: FAIL: architecture/component-registry.yml declares no test_roots entries"
                .to_string(),
        );
    }
    let tracked = git_tracked_tests(root)?;
    let unowned = unowned_test_files(&tracked, &entries);
    if unowned.is_empty() {
        println!(
            "registry-test-ownership: OK: all {} tracked tests/ file(s) have a component-registry owner ({} test_roots entr(y)ies)",
            tracked.len(),
            entries.len()
        );
        return Ok(());
    }
    let mut message = format!(
        "registry-test-ownership: FAIL: {} tracked tests/ file(s) have no component-registry owner ({} of {} owned)",
        unowned.len(),
        tracked.len() - unowned.len(),
        tracked.len()
    );
    for path in &unowned {
        message.push_str(&format!(
            "\n  {path} — no test_roots entry of any component matches it; add one in the same change"
        ));
    }
    Err(message)
}

/// Every `test_roots` entry of every component, in file order.
fn load_test_roots(root: &Path) -> Result<Vec<String>, String> {
    let registry = root.join("architecture/component-registry.yml");
    let text = std::fs::read_to_string(&registry)
        .map_err(|e| format!("cannot read architecture/component-registry.yml: {e}"))?;
    let doc: Value = serde_yaml_ng::from_str(&text)
        .map_err(|e| format!("cannot parse architecture/component-registry.yml: {e}"))?;
    let components = doc
        .get("components")
        .and_then(Value::as_sequence)
        .ok_or("architecture/component-registry.yml has no components list")?;
    let mut entries = Vec::new();
    for component in components {
        let id = component
            .get("component_id")
            .and_then(Value::as_str)
            .unwrap_or("<unnamed>")
            .to_string();
        let Some(list) = component.get("test_roots").and_then(Value::as_sequence) else {
            continue;
        };
        for entry in list {
            match entry.as_str() {
                Some(path) => entries.push(path.to_string()),
                None => {
                    return Err(format!(
                        "component {id} declares a non-string test_roots entry"
                    ))
                }
            }
        }
    }
    Ok(entries)
}

/// The tracked files under the root `tests/` tree.
fn git_tracked_tests(root: &Path) -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "-z", "--", "tests/"])
        .output()
        .map_err(|e| format!("git ls-files failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect())
}

/// A tracked path is owned when it equals an entry exactly or lies beneath a directory-prefix
/// entry; the registry writes directory entries without a trailing slash.
fn owns(entry: &str, path: &str) -> bool {
    if entry == path {
        return true;
    }
    path.starts_with(format!("{}/", entry.trim_end_matches('/')).as_str())
}

/// The tracked paths no `test_roots` entry owns, in tracked order.
fn unowned_test_files(tracked: &[String], entries: &[String]) -> Vec<String> {
    tracked
        .iter()
        .filter(|path| entries.iter().all(|entry| !owns(entry, path)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;

    #[test]
    fn directory_prefix_entries_own_the_files_beneath_them() {
        assert!(owns("tests/common", "tests/common"));
        assert!(owns("tests/common", "tests/common/mod.rs"));
        assert!(!owns("tests/common", "tests/commode.rs"));
        assert!(!owns("tests/commode.rs", "tests/common/mod.rs"));
    }

    #[test]
    fn a_planted_unowned_test_is_the_only_finding() {
        let t = Tree::new(&[(
            "architecture/component-registry.yml",
            "components:\n  - component_id: medulla.query-plan\n    test_roots:\n      - \"tests/nl_query_bounded_steps.rs\"\n      - \"tests/common\"\n  - component_id: medulla.types\n    test_roots: []\n",
        )]);
        let entries = load_test_roots(t.path()).expect("registry parses");
        assert_eq!(entries.len(), 2);
        let tracked = vec![
            "tests/nl_query_bounded_steps.rs".to_string(),
            "tests/common/mod.rs".to_string(),
            "tests/planted_unowned_test_zzz.py".to_string(),
        ];
        assert_eq!(
            unowned_test_files(&tracked, &entries),
            vec!["tests/planted_unowned_test_zzz.py".to_string()]
        );
    }

    // spec: MEDULLA-CONTRACT-R023.1
    #[test]
    fn current_tracked_tests_tree_has_no_unowned_files() {
        let root = crate::workspace_root();
        if !root.join(".git").exists() {
            return; // a shipped source tree has no tracked-file list to read
        }
        run(&root, &[]).expect("every tracked tests/ file must have a component-registry owner");
    }
}
