//! Fail closed when a tracked Rust file is not compiled by any cargo target.
//!
//! Every shape scanner in this repository (complexity, duplication, clippy, architecture lint)
//! measures source the compiler reads. A file with no `mod` declaration is read by none of them, so
//! it is short, simple, unduplicated, correctly layered and entirely absent from the binary.
//!
//! The property: for every cargo TARGET in the workspace (lib, bin, test, bench, example, build
//! script, and the fuzz targets of a nested fuzz package) walk the compiler-declared module closure
//! of its root file. The union of those closures is the set of files rustc reads; any tracked `.rs`
//! file outside it is unreachable. Nothing keys on a module NAME, a directory NAME or a line
//! number: a file that moves, is renamed, or is redeclared through `#[path]` stays reachable
//! because the walk follows the same declarations the compiler does.
//!
//! Traps handled: cargo AUTO-DISCOVERS targets in `tests/`, `benches/`, `examples/`, `src/bin/`
//! and `fuzz/fuzz_targets/` (suppressed per manifest by `autobins`/`autotests`/...); `mod`
//! declarations bind PER DIRECTORY; both module layouts, `#[path]` and `include!` are real
//! declarations; a `mod` behind any `#[cfg]` is reachable (the walk is test-inclusive).
//!
//! An orphan whose own module tree pulls in further files is an orphan ROOT; the files it would
//! have declared are collateral, so the count names distinct wiring gaps rather than file counts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use toml_edit::{DocumentMut, Item};

use crate::analysis::module_tree;

pub const NAME: &str = "orphan-modules";
pub const SUMMARY: &str = "every tracked Rust file is compiled by some cargo target";

pub fn run(root: &Path, _args: &[String]) -> Result<(), String> {
    let receipt = run_gate(root)?;
    if receipt.roots.is_empty() {
        println!(
            "orphan-module gate: OK: {} tracked Rust file(s) are compiled by {} cargo target(s)",
            receipt.reachable_files, receipt.targets
        );
        return Ok(());
    }
    let mut message = format!(
        "orphan-module gate: FAIL: {} orphan root(s), {} tracked Rust file(s) in total, are not compiled by any cargo target ({}/{} reachable)",
        receipt.roots.len(),
        receipt.orphan_files,
        receipt.reachable_files,
        receipt.tracked_files
    );
    for finding in &receipt.roots {
        message.push_str(&format!(
            "\n  {} ({} bytes) \u{2014} no `mod` declaration reaches it from any crate root",
            finding.path, finding.bytes
        ));
        for member in &finding.collateral {
            message.push_str(&format!("\n      also unreachable via it: {member}"));
        }
    }
    Err(message)
}

struct OrphanRoot {
    path: String,
    bytes: u64,
    collateral: Vec<String>,
}

struct Receipt {
    targets: usize,
    tracked_files: usize,
    reachable_files: usize,
    orphan_files: usize,
    roots: Vec<OrphanRoot>,
}

fn git_ls_files(root: &Path, pathspecs: &[&str]) -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "-z", "--"])
        .args(pathspecs)
        .output()
        .map_err(|e| format!("git ls-files failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|entry| !entry.is_empty() && !entry.starts_with("vendor/"))
        .map(str::to_string)
        .collect();
    files.sort();
    Ok(files)
}

/// `path` lexically normalized and made relative to the (canonical) repository root.
fn relative(root: &Path, path: &Path) -> Result<String, String> {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            other => normal.push(other),
        }
    }
    normal
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .map_err(|_| format!("cargo target escapes the repository: {}", path.display()))
}

fn declared(
    root: &Path,
    section: Option<&Item>,
    manifest_dir: &Path,
) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    let mut entry = |table: &dyn toml_edit::TableLike| -> Result<(), String> {
        if let Some(declared) = table.get("path").and_then(Item::as_str) {
            paths.push(relative(root, &manifest_dir.join(declared))?);
        }
        Ok(())
    };
    match section {
        Some(Item::ArrayOfTables(array)) => array.iter().try_for_each(|t| entry(t))?,
        Some(item) => {
            if let Some(table) = item.as_table_like() {
                entry(table)?;
            }
        }
        None => {}
    }
    Ok(paths)
}

/// cargo's auto-discovery rule for tests/, benches/, examples/, src/bin/: `<dir>/<name>.rs` or
/// `<dir>/<name>/main.rs`. Nothing deeper is a target.
fn auto_directory(root: &Path, directory: &Path) -> Result<Vec<String>, String> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Ok(vec![]);
    };
    let mut children: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    children.sort();
    let mut targets = Vec::new();
    for child in children {
        if child.is_file() && child.extension().is_some_and(|e| e == "rs") {
            targets.push(relative(root, &child)?);
        } else if child.is_dir() && child.join("main.rs").is_file() {
            targets.push(relative(root, &child.join("main.rs"))?);
        }
    }
    Ok(targets)
}

/// cargo-fuzz discovers every file under `fuzz_targets/` as its own binary.
fn fuzz_targets(root: &Path, manifest_dir: &Path) -> Result<Vec<String>, String> {
    let directory = manifest_dir.join("fuzz_targets");
    if !directory.is_dir() {
        return Ok(vec![]);
    }
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(&directory)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "rs"))
        .map(|e| e.into_path())
        .collect();
    files.sort();
    files.iter().map(|p| relative(root, p)).collect()
}

fn auto_enabled(package: &toml_edit::Table, key: &str) -> bool {
    package.get(key).and_then(Item::as_bool) != Some(false)
}

fn package_targets(root: &Path, manifest: &str) -> Result<Vec<String>, String> {
    let manifest_path = root.join(manifest);
    let manifest_dir = manifest_path
        .parent()
        .expect("manifest has a parent")
        .to_path_buf();
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("unreadable manifest {}: {e}", manifest_path.display()))?;
    let doc: DocumentMut = text
        .parse()
        .map_err(|e| format!("unreadable manifest {}: {e}", manifest_path.display()))?;
    let Some(package) = doc.get("package").and_then(Item::as_table) else {
        // A pure `[workspace]` manifest compiles nothing of its own.
        return Ok(vec![]);
    };

    let mut targets = declared(root, doc.get("lib"), &manifest_dir)?;
    if targets.is_empty() && manifest_dir.join("src/lib.rs").is_file() {
        targets.push(relative(root, &manifest_dir.join("src/lib.rs"))?);
    }
    match package.get("build") {
        Some(item) if item.as_str().is_some() => targets.push(relative(
            root,
            &manifest_dir.join(item.as_str().expect("checked")),
        )?),
        Some(item) if item.as_bool() == Some(false) => {}
        _ => {
            if manifest_dir.join("build.rs").is_file() {
                targets.push(relative(root, &manifest_dir.join("build.rs"))?);
            }
        }
    }
    targets.extend(declared(root, doc.get("bin"), &manifest_dir)?);
    if auto_enabled(package, "autobins") {
        targets.extend(auto_directory(root, &manifest_dir.join("src/bin"))?);
        if manifest_dir.join("src/main.rs").is_file() {
            targets.push(relative(root, &manifest_dir.join("src/main.rs"))?);
        }
    }
    for (kind, directory) in [
        ("test", "tests"),
        ("bench", "benches"),
        ("example", "examples"),
    ] {
        targets.extend(declared(root, doc.get(kind), &manifest_dir)?);
        if auto_enabled(package, &format!("auto{kind}s")) {
            targets.extend(auto_directory(root, &manifest_dir.join(directory))?);
        }
    }
    targets.extend(fuzz_targets(root, &manifest_dir)?);
    Ok(targets)
}

/// Every file the compiler reads when it compiles this root.
fn closure(root: &Path, relative_path: &str, crate_root: bool) -> Result<BTreeSet<String>, String> {
    let loaded = module_tree::load(root, relative_path, true, crate_root)
        .map_err(|e| format!("module walk failed for {relative_path}: {e}"))?;
    loaded.paths.iter().map(|p| relative(root, p)).collect()
}

fn run_gate(root: &Path) -> Result<Receipt, String> {
    let root = &root.canonicalize().map_err(|e| e.to_string())?;
    let tracked = git_ls_files(root, &["*.rs"])?;
    if tracked.is_empty() {
        return Err("no tracked Rust source: the gate has no universe".into());
    }
    let manifests = git_ls_files(root, &["Cargo.toml", "*/Cargo.toml"])?;
    if manifests.is_empty() {
        return Err("no tracked Cargo manifest: the gate has no universe".into());
    }
    let mut targets = BTreeSet::new();
    for manifest in &manifests {
        targets.extend(package_targets(root, manifest)?);
    }
    if targets.is_empty() {
        return Err("no cargo target resolved: the gate has no universe".into());
    }
    if let Some(missing) = targets.iter().find(|t| !root.join(t).is_file()) {
        return Err(format!("declared cargo target is absent: {missing}"));
    }

    let mut reachable = BTreeSet::new();
    for target in &targets {
        reachable.extend(closure(root, target, true)?);
    }
    let tracked_set: BTreeSet<String> = tracked.iter().cloned().collect();
    let unreachable: Vec<&String> = tracked_set.difference(&reachable).collect();

    // One `mod` statement wires a whole unreachable sub-tree, so that sub-tree is ONE finding.
    let mut collateral: BTreeMap<&String, BTreeSet<String>> = BTreeMap::new();
    for path in &unreachable {
        let members = closure(root, path, false)?;
        collateral.insert(
            path,
            members
                .into_iter()
                .filter(|m| m != *path && tracked_set.contains(m))
                .collect(),
        );
    }
    let claimed: BTreeSet<&String> = collateral.values().flatten().collect();
    let roots = unreachable
        .iter()
        .filter(|path| !claimed.contains(**path))
        .map(|path| OrphanRoot {
            path: (*path).clone(),
            bytes: std::fs::metadata(root.join(path))
                .map(|m| m.len())
                .unwrap_or(0),
            collateral: collateral[path].iter().cloned().collect(),
        })
        .collect();
    Ok(Receipt {
        targets: targets.len(),
        tracked_files: tracked.len(),
        reachable_files: tracked_set.intersection(&reachable).count(),
        orphan_files: unreachable.len(),
        roots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;

    fn git_tracked(files: &[(&str, &str)]) -> Tree {
        let tree = Tree::new(files);
        for args in [&["init", "-q"][..], &["add", "-A"][..]] {
            assert!(Command::new("git")
                .current_dir(tree.path())
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        tree
    }

    const MANIFEST: &str = "[package]\nname = \"c\"\nversion = \"0\"\n";

    #[test]
    fn declared_and_auto_discovered_roots_are_reachable() {
        let t = git_tracked(&[
            ("Cargo.toml", MANIFEST),
            ("src/lib.rs", "mod a;\n#[cfg(test)]\nmod b;\n"),
            ("src/a.rs", ""),
            ("src/b.rs", ""),
            ("tests/it.rs", "mod common;\n"),
            ("tests/common/mod.rs", ""),
            ("src/bin/tool.rs", ""),
        ]);
        let r = run_gate(t.path()).unwrap();
        assert!(
            r.roots.is_empty(),
            "{:?}",
            r.roots.iter().map(|o| &o.path).collect::<Vec<_>>()
        );
        assert_eq!(r.targets, 3);
    }

    #[test]
    fn an_undeclared_file_is_an_orphan_root_and_its_subtree_is_collateral() {
        let t = git_tracked(&[
            ("Cargo.toml", MANIFEST),
            ("src/lib.rs", "mod a;\n"),
            ("src/a.rs", ""),
            ("src/orphan.rs", "mod inner;\n"),
            ("src/orphan/inner.rs", ""),
        ]);
        let r = run_gate(t.path()).unwrap();
        assert_eq!(r.orphan_files, 2);
        assert_eq!(r.roots.len(), 1);
        assert_eq!(r.roots[0].path, "src/orphan.rs");
        assert_eq!(r.roots[0].collateral, ["src/orphan/inner.rs"]);
        assert!(run(t.path(), &[])
            .unwrap_err()
            .contains("1 orphan root(s), 2 tracked Rust file(s)"));
    }

    #[test]
    fn declarations_bind_per_directory_not_by_name() {
        let t = git_tracked(&[
            ("Cargo.toml", MANIFEST),
            ("src/lib.rs", "mod shared;\n"),
            ("src/shared.rs", ""),
            ("src/other/shared.rs", ""),
        ]);
        let r = run_gate(t.path()).unwrap();
        assert_eq!(
            r.roots.iter().map(|o| o.path.as_str()).collect::<Vec<_>>(),
            ["src/other/shared.rs"]
        );
    }

    #[test]
    fn autodiscovery_is_suppressed_by_the_manifest() {
        let t = git_tracked(&[
            ("Cargo.toml", &format!("{MANIFEST}autotests = false\n")),
            ("src/lib.rs", ""),
            ("tests/it.rs", ""),
        ]);
        assert_eq!(run_gate(t.path()).unwrap().roots[0].path, "tests/it.rs");
    }

    #[test]
    fn a_path_override_keeps_a_file_reachable() {
        let t = git_tracked(&[
            ("Cargo.toml", MANIFEST),
            ("src/lib.rs", "#[path = \"elsewhere/x.rs\"]\nmod x;\n"),
            ("src/elsewhere/x.rs", ""),
        ]);
        assert!(run_gate(t.path()).unwrap().roots.is_empty());
    }

    #[test]
    fn the_unwired_topic_log_and_agent_peer_code_is_gone() {
        let root = crate::workspace_root();
        let receipt = run_gate(&root).unwrap();
        assert!(
            receipt.roots.is_empty(),
            "{:?}",
            receipt.roots.iter().map(|o| &o.path).collect::<Vec<_>>()
        );
        for path in [
            "crates/medulla-durable/src/topic.rs",
            "crates/medulla-agent-types/src/agent_peer.rs",
        ] {
            assert!(!root.join(path).exists(), "{path}");
        }
        let grep = Command::new("git")
            .current_dir(&root)
            .args([
                "grep",
                "-n",
                r"TopicEntry\|AgentPeer",
                "--",
                "src",
                "crates",
            ])
            .output()
            .unwrap();
        // The test file itself is tracked under tools/, outside the searched trees.
        assert_eq!(
            grep.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&grep.stdout)
        );
    }
}
