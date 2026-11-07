//! MEDULLA-CONTRACT-R023.1: every tracked `tests/` file has an owner in the component registry.
//!
//! `cargo xtask registry-test-ownership [--root DIR]`. The registry's own layout checker only
//! validates the path strings a component declares; it never reads the filesystem, so a new file
//! under `tests/` that no component names is invisible to it. This check reads the real tracked
//! tree (`git ls-files -- tests`) and fails when a file is neither equal to nor nested under a
//! path any implementation component claims.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const NAME: &str = "registry-test-ownership";
pub const SUMMARY: &str = "every tracked tests/ file has an owner in the component registry";

const PATH_FIELDS: [&str; 6] = [
    "owned_source_roots",
    "public_contract_roots",
    "test_roots",
    "generated_roots",
    "shared_roots",
    "shared_files",
];

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let mut root: PathBuf = root.to_path_buf();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--root" {
            root = iter.next().ok_or("--root needs a directory")?.into();
        }
    }
    let registry = root.join("architecture/component-registry.yml");
    let (files, unowned) = unowned_test_files(&root, &registry)?;
    if unowned.is_empty() {
        println!(
            "registry-test-ownership: OK: all {} tracked tests/ file(s) are owned",
            files.len()
        );
        return Ok(());
    }
    let listing: String = unowned.iter().map(|f| format!("  {f}\n")).collect();
    Err(format!(
        "registry-test-ownership: {} of {} tracked tests/ file(s) have no owner in architecture/component-registry.yml:\n{listing}\n\
         Fix: add the exact path to the owning component's `test_roots` (or `owned_source_roots`, if it is not a test file) in \
         architecture/component-registry.yml. If it belongs under the shared root and no more specific component owns it, add it to \
         medulla.binary-composition's `test_roots` enumeration instead of widening that enumeration back into a directory glob \
         (a glob would re-overlap every per-component carve-out the layout checker requires).",
        unowned.len(),
        files.len()
    ))
}

/// The union of every path-bearing field across implementation components.
pub fn parse_claimed_paths(document: &Value) -> Vec<String> {
    let mut claimed = Vec::new();
    let components = document["components"].as_array().into_iter().flatten();
    for component in components {
        if component["component_kind"] != "implementation_component" {
            continue;
        }
        for field in PATH_FIELDS {
            let paths = component[field].as_array().into_iter().flatten();
            claimed.extend(paths.filter_map(Value::as_str).map(String::from));
        }
        let omissions = component["owned_source_roots_omissions"]
            .as_array()
            .into_iter()
            .flatten();
        claimed.extend(
            omissions
                .filter_map(|o| o["path"].as_str())
                .filter(|p| !p.is_empty())
                .map(String::from),
        );
    }
    claimed
}

pub fn is_owned(path: &str, claimed: &[String]) -> bool {
    claimed.iter().any(|claim| {
        let claim = claim.trim_end_matches('/');
        path == claim || path.strip_prefix(claim).is_some_and(|r| r.starts_with('/'))
    })
}

fn tracked_test_files(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .args(["ls-files", "--", "tests"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("CANNOT RUN: git ls-files failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "CANNOT RUN: git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// `(all tracked tests/ files, the ones with no owner)`.
pub fn unowned_test_files(
    root: &Path,
    registry: &Path,
) -> Result<(Vec<String>, Vec<String>), String> {
    if !registry.is_file() {
        return Err(format!("CANNOT RUN: missing {}", registry.display()));
    }
    let text = fs::read_to_string(registry).map_err(|e| format!("CANNOT RUN: {e}"))?;
    let document: Value = serde_yaml_ng::from_str(&text)
        .map_err(|e| format!("CANNOT RUN: invalid YAML in {}: {e}", registry.display()))?;
    let claimed = parse_claimed_paths(&document);
    let files = tracked_test_files(root)?;
    if files.is_empty() {
        return Err(
            "CANNOT RUN: git ls-files -- tests returned no files; the registry claims the directory exists"
                .into(),
        );
    }
    let unowned = files
        .iter()
        .filter(|f| !is_owned(f, &claimed))
        .cloned()
        .collect();
    Ok((files, unowned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;
    use serde_json::json;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    fn component(overrides: Value) -> Value {
        let mut base = json!({
            "component_id": "medulla.example",
            "component_kind": "implementation_component",
            "owned_source_roots": [], "public_contract_roots": [], "test_roots": [],
            "generated_roots": [], "shared_roots": [], "shared_files": [],
        });
        for (k, v) in overrides.as_object().expect("object") {
            base[k] = v.clone();
        }
        base
    }

    /// A throwaway git repo with the registry and the named tracked files.
    fn repo(registry: Option<&Value>, tracked: &[&str]) -> Tree {
        let files: Vec<(&str, &str)> = tracked.iter().map(|p| (*p, "content\n")).collect();
        let tree = Tree::new(&files);
        let dir = tree.path();
        fs::create_dir_all(dir.join("architecture")).expect("mkdir");
        if let Some(registry) = registry {
            fs::write(
                dir.join("architecture/component-registry.yml"),
                serde_yaml_ng::to_string(registry).expect("yaml"),
            )
            .expect("write registry");
        }
        git(dir, &["init", "-q"]);
        git(dir, &["config", "user.name", "Registry Test"]);
        git(dir, &["config", "user.email", "registry@example.invalid"]);
        if !tracked.is_empty() {
            let mut args = vec!["add", "--"];
            args.extend(tracked);
            git(dir, &args);
        }
        tree
    }

    fn registry_of(components: Vec<Value>) -> Value {
        json!({ "components": components })
    }

    #[test]
    fn parse_claimed_paths_collects_every_path_field_from_impl_components_only() {
        let document = registry_of(vec![
            json!({"component_id": "medulla.seam", "component_kind": "layer_boundary_root_seam"}),
            component(json!({
                "owned_source_roots": ["src/a.rs"],
                "public_contract_roots": ["src/lib.rs"],
                "test_roots": ["tests/a.rs"],
                "owned_source_roots_omissions": [{"path": "src/model.rs", "reason": "x"}],
            })),
        ]);
        let mut claimed = parse_claimed_paths(&document);
        claimed.sort();
        assert_eq!(
            claimed,
            ["src/a.rs", "src/lib.rs", "src/model.rs", "tests/a.rs"]
        );
    }

    #[test]
    fn is_owned_matches_exact_path_and_directory_descendant() {
        let claimed = vec!["tests/parity".to_string(), "tests/exact.rs".to_string()];
        assert!(is_owned("tests/exact.rs", &claimed));
        assert!(is_owned("tests/parity/nested/file.py", &claimed));
        assert!(!is_owned("tests/parity_other.rs", &claimed));
        assert!(!is_owned("tests/unrelated.rs", &claimed));
    }

    #[test]
    fn planted_unowned_file_is_detected() {
        let registry = registry_of(vec![component(json!({"test_roots": ["tests/known.rs"]}))]);
        let tree = repo(
            Some(&registry),
            &["tests/known.rs", "tests/planted_unowned.rs"],
        );
        let registry_path = tree.path().join("architecture/component-registry.yml");
        let (files, unowned) = unowned_test_files(tree.path(), &registry_path).expect("runs");
        assert_eq!(files.len(), 2);
        assert_eq!(unowned, ["tests/planted_unowned.rs"]);
    }

    #[test]
    fn all_files_owned_reports_no_gap() {
        let registry = registry_of(vec![component(
            json!({"test_roots": ["tests/known.rs", "tests/subdir"]}),
        )]);
        let tree = repo(
            Some(&registry),
            &["tests/known.rs", "tests/subdir/nested.rs"],
        );
        let registry_path = tree.path().join("architecture/component-registry.yml");
        let (files, unowned) = unowned_test_files(tree.path(), &registry_path).expect("runs");
        assert_eq!(files.len(), 2);
        assert!(unowned.is_empty());
    }

    #[test]
    fn owned_source_roots_omission_counts_as_owned() {
        let registry = registry_of(vec![component(
            json!({"owned_source_roots_omissions": [{"path": "tests/model.rs", "reason": "x"}]}),
        )]);
        let tree = repo(Some(&registry), &["tests/model.rs"]);
        let registry_path = tree.path().join("architecture/component-registry.yml");
        let (_, unowned) = unowned_test_files(tree.path(), &registry_path).expect("runs");
        assert!(unowned.is_empty());
    }

    #[test]
    fn run_names_the_file_then_goes_green_when_it_is_removed() {
        let registry = registry_of(vec![component(json!({"test_roots": ["tests/known.rs"]}))]);
        let tree = repo(
            Some(&registry),
            &["tests/known.rs", "tests/planted_unowned.rs"],
        );
        let args = ["--root".to_string(), tree.path().display().to_string()];
        let failure = run(tree.path(), &args).expect_err("unowned file fails");
        assert!(failure.contains("tests/planted_unowned.rs"));
        assert!(failure.contains("1 of 2 tracked tests/"));

        git(
            tree.path(),
            &["rm", "-q", "-f", "--", "tests/planted_unowned.rs"],
        );
        run(tree.path(), &args).expect("clean tree passes");
    }

    #[test]
    fn missing_registry_fails_closed_not_silently() {
        let tree = repo(None, &["tests/known.rs"]);
        let args = ["--root".to_string(), tree.path().display().to_string()];
        let failure = run(tree.path(), &args).expect_err("no registry fails");
        assert!(failure.contains("CANNOT RUN"));
    }

    #[test]
    fn current_tracked_tests_tree_has_no_unowned_files() {
        let root = crate::workspace_root();
        if !root.join(".git").exists() {
            return; // a shipped source tree has no tracked-file list to read
        }
        let registry = root.join("architecture/component-registry.yml");
        let (_, unowned) = unowned_test_files(&root, &registry).expect("runs");
        assert!(unowned.is_empty(), "{unowned:?}");
    }
}
