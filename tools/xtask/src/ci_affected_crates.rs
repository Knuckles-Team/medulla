//! MEDULLA-CONTRACT-R053.7: select the workspace crates a pull request affects and print the cargo runs.
//!
//! `cargo xtask ci-affected-crates <cargo-metadata.json> [--list] < changed-paths`. The metadata
//! is `cargo metadata --format-version 1 --no-deps`; the changed paths (stdin, one per line) are
//! repository-relative. The affected set is every workspace package that owns a changed file plus
//! every package that depends on one of those (normal, dev or build edge, transitively); a change
//! to a workspace-wide input (`Cargo.lock`, the root manifest, the toolchain or `.cargo/`) affects
//! every package. Output is one cargo invocation per line, `{tool}` standing for `clippy` or
//! `nextest`, with the feature profile main CI already uses for that package: the root facade
//! `--features full`; the pyo3 crates every feature except `python`; the storage crate
//! `--all-features` with one test thread; every other package `--all-features`. Empty output means
//! no Rust package is affected.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const NAME: &str = "ci-affected-crates";
pub const SUMMARY: &str = "select the crates a pull request affects and print the cargo runs";

const WORKSPACE_WIDE: [&str; 6] = [
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    ".cargo/",
    "clippy.toml",
    ".config/nextest.toml",
];
const PYO3_FEATURES: &str = "medulla-numeric/analytics,medulla-numeric/contract,medulla-numeric/complex,\
medulla-numeric/decision,medulla-numeric/motif,medulla-pyengine/security,\
medulla-pyengine/ml-pipeline,medulla-pyengine/reasoning,medulla-pyengine/query,\
medulla-pyengine/cypher,medulla-pyengine/tsdb,medulla-pyengine/streaming,\
medulla-pyengine/wasm-udf,medulla-pyengine/rdf,medulla-pyengine/blob,medulla-pyengine/jobs,\
medulla-pyengine/statechart,medulla-pyengine/finance";
const PYO3_PACKAGES: [&str; 2] = ["medulla-numeric", "medulla-pyengine"];
const SERIAL_PACKAGES: [&str; 1] = ["medulla-storage"];
/// Tested only by their own job, as in the main workspace sweep: medulla-tts-piper needs a real
/// onnxruntime under --all-features (job `tts-piper-inference`).
const OWN_JOB_PACKAGES: [&str; 1] = ["medulla-tts-piper"];

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let list = args.iter().any(|a| a == "--list");
    let metadata_path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .ok_or("usage: cargo xtask ci-affected-crates <cargo-metadata.json> [--list] < changed-paths")?;
    let text = fs::read_to_string(metadata_path).map_err(|e| format!("{metadata_path}: {e}"))?;
    let metadata: Value = serde_json::from_str(&text).map_err(|e| format!("{metadata_path}: {e}"))?;
    let mut stdin = String::new();
    std::io::stdin()
        .read_to_string(&mut stdin)
        .map_err(|e| e.to_string())?;
    let changed: Vec<String> = stdin
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    let packages = packages(&metadata);
    let selected = affected(root, &metadata, &changed);
    let lines = if list {
        selected
    } else {
        commands(&packages, &selected)
    };
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

type Packages = BTreeMap<String, Vec<String>>;

/// Workspace package name to its directory components relative to the workspace root.
fn packages(metadata: &Value) -> Packages {
    let root = metadata["workspace_root"].as_str().unwrap_or("");
    let members: BTreeSet<&str> = metadata["workspace_members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut out = Packages::new();
    for pkg in metadata["packages"].as_array().into_iter().flatten() {
        let id = pkg["id"].as_str().unwrap_or("");
        if !members.is_empty() && !members.contains(id) {
            continue;
        }
        let manifest = Path::new(pkg["manifest_path"].as_str().unwrap_or(""));
        let dir = manifest.parent().unwrap_or(Path::new(""));
        let relative = dir.strip_prefix(root).unwrap_or(dir);
        let parts = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        out.insert(pkg["name"].as_str().unwrap_or("").to_string(), parts);
    }
    out
}

fn reverse_edges(metadata: &Value, names: &BTreeSet<&String>) -> BTreeMap<String, BTreeSet<String>> {
    let mut rdeps: BTreeMap<String, BTreeSet<String>> =
        names.iter().map(|n| ((*n).clone(), BTreeSet::new())).collect();
    for pkg in metadata["packages"].as_array().into_iter().flatten() {
        let name = pkg["name"].as_str().unwrap_or("");
        if !names.iter().any(|n| n.as_str() == name) {
            continue;
        }
        for dep in pkg["dependencies"].as_array().into_iter().flatten() {
            let target = dep["name"].as_str().unwrap_or("");
            let has_path = dep["path"].as_str().is_some_and(|p| !p.is_empty());
            if has_path && target != name {
                if let Some(set) = rdeps.get_mut(target) {
                    set.insert(name.to_string());
                }
            }
        }
    }
    rdeps
}

/// The package whose directory is the longest prefix of `path`.
fn owner(path: &str, packages: &Packages) -> Option<String> {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let mut best: Option<(usize, &String)> = None;
    for (name, dir) in packages {
        let n = dir.len();
        if parts.len() >= n
            && parts[..n].iter().zip(dir).all(|(a, b)| a == b)
            && best.is_none_or(|(len, _)| n > len)
        {
            best = Some((n, name));
        }
    }
    best.map(|(_, name)| name.clone())
}

/// The concatenated Rust sources under a package directory.
fn sources(root: &Path, dir: &[String]) -> String {
    let mut files = Vec::new();
    if dir.is_empty() {
        // The root package compiles only these; member crates are scanned as their own packages.
        for d in ["src", "tests", "benches", "examples"] {
            collect_rs(&root.join(d), &mut files);
        }
        files.sort();
        let build = root.join("build.rs");
        if build.is_file() {
            files.push(build);
        }
    } else {
        collect_rs(&root.join(dir.join("/")), &mut files);
        files.sort();
    }
    files
        .iter()
        .map(|f| String::from_utf8_lossy(&fs::read(f).unwrap_or_default()).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.' || c == '-'
}

/// Packages whose Rust sources name `path` or a directory that holds it. A test reads a
/// non-crate file through `include_str!`, `read_to_string` or a path join, so its source holds the
/// literal `specs/<dir>` (or the full path). Single-segment directories are too broad.
fn readers(root: &Path, path: &str, packages: &Packages, cache: &mut BTreeMap<String, String>) -> BTreeSet<String> {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let literals: Vec<String> = (2..=parts.len()).map(|n| parts[..n].join("/")).collect();
    let mut out = BTreeSet::new();
    for (name, dir) in packages {
        let text = cache
            .entry(dir.join("/"))
            .or_insert_with(|| sources(root, dir));
        let hit = literals.iter().any(|lit| {
            text.match_indices(lit.as_str()).any(|(at, _)| {
                !text[at + lit.len()..].chars().next().is_some_and(is_word)
            })
        });
        if hit {
            out.insert(name.clone());
        }
    }
    out
}

fn is_root_package_source(path: &str) -> bool {
    path == "build.rs"
        || path.starts_with("src/")
        || (["tests/", "benches/", "examples/"].iter().any(|d| path.starts_with(d))
            && path.ends_with(".rs"))
}

/// Owning packages of the changed files plus their reverse dependencies.
pub fn affected(root: &Path, metadata: &Value, changed: &[String]) -> Vec<String> {
    let packages = packages(metadata);
    let every = || packages.keys().cloned().collect::<Vec<_>>();
    if changed.iter().any(|p| {
        WORKSPACE_WIDE
            .iter()
            .any(|w| p == w || (w.ends_with('/') && p.starts_with(w)))
    }) {
        return every();
    }
    let root_pkg = owner("Cargo.toml", &packages);
    let mut cache = BTreeMap::new();
    let mut seeds: BTreeSet<String> = BTreeSet::new();
    for path in changed {
        if path == "Cargo.toml" {
            // The root manifest holds [workspace] and shared dependency versions.
            return every();
        }
        match owner(path, &packages) {
            Some(name) if !(Some(&name) == root_pkg.as_ref() && !is_root_package_source(path)) => {
                seeds.insert(name);
            }
            _ => seeds.extend(readers(root, path, &packages, &mut cache)),
        }
    }
    let names: BTreeSet<&String> = packages.keys().collect();
    let rdeps = reverse_edges(metadata, &names);
    let mut out = seeds.clone();
    let mut stack: Vec<String> = seeds.into_iter().collect();
    while let Some(next) = stack.pop() {
        for parent in rdeps.get(&next).into_iter().flatten() {
            if out.insert(parent.clone()) {
                stack.push(parent.clone());
            }
        }
    }
    out.into_iter().collect()
}

/// The pyo3 feature profile, limited to the selected packages' features.
pub fn pyo3_features(packages: &[&str]) -> String {
    PYO3_FEATURES
        .split(',')
        .filter(|f| packages.contains(&f.split('/').next().unwrap_or("")))
        .collect::<Vec<_>>()
        .join(",")
}

pub fn commands(packages: &Packages, selected: &[String]) -> Vec<String> {
    let selected: Vec<&str> = selected
        .iter()
        .map(String::as_str)
        .filter(|n| !OWN_JOB_PACKAGES.contains(n))
        .collect();
    let root_pkg = owner("Cargo.toml", packages);
    let default: Vec<&str> = selected
        .iter()
        .copied()
        .filter(|n| {
            Some(*n) != root_pkg.as_deref()
                && !PYO3_PACKAGES.contains(n)
                && !SERIAL_PACKAGES.contains(n)
        })
        .collect();
    let pyo3: Vec<&str> = selected.iter().copied().filter(|n| PYO3_PACKAGES.contains(n)).collect();
    let serial: Vec<&str> = selected.iter().copied().filter(|n| SERIAL_PACKAGES.contains(n)).collect();
    let flags = |names: &[&str]| {
        names
            .iter()
            .map(|n| format!("-p {n}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut runs = Vec::new();
    if !default.is_empty() {
        runs.push(format!("{{tool}} {} --all-features", flags(&default)));
    }
    for name in serial {
        runs.push(format!("{{tool}} -p {name} --all-features {{serial}}"));
    }
    if !pyo3.is_empty() {
        runs.push(format!("{{tool}} {} --features {}", flags(&pyo3), pyo3_features(&pyo3)));
    }
    if let Some(root_pkg) = root_pkg.filter(|r| selected.contains(&r.as_str())) {
        runs.push(format!("{{tool}} -p {root_pkg} --features full"));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;
    use serde_json::json;

    fn meta() -> Value {
        let pkg = |name: &str, path: &str, deps: &[&str]| {
            let mut dependencies: Vec<Value> =
                deps.iter().map(|d| json!({"name": d, "path": "/x"})).collect();
            dependencies.push(json!({"name": "serde"}));
            json!({"id": name, "name": name, "manifest_path": format!("/ws/{path}Cargo.toml"), "dependencies": dependencies})
        };
        let pkgs = vec![
            pkg("medulla-types", "crates/medulla-types/", &[]),
            pkg("medulla-storage", "crates/medulla-storage/", &["medulla-types"]),
            pkg("medulla-viz", "crates/medulla-viz/", &[]),
            pkg("medulla-numeric", "crates/medulla-numeric/", &["medulla-types"]),
            pkg("facade", "", &["medulla-storage", "medulla-viz", "medulla-numeric"]),
        ];
        let ids: Vec<Value> = pkgs.iter().map(|p| p["id"].clone()).collect();
        json!({"workspace_root": "/ws", "packages": pkgs, "workspace_members": ids})
    }

    fn sel(root: &Path, changed: &[&str]) -> Vec<String> {
        let changed: Vec<String> = changed.iter().map(|c| c.to_string()).collect();
        affected(root, &meta(), &changed)
    }

    #[test]
    fn affected_is_owners_plus_reverse_dependencies() {
        let none = Tree::new(&[]);
        let r = none.path();
        let all = ["facade", "medulla-numeric", "medulla-storage", "medulla-types", "medulla-viz"];
        assert!(sel(r, &["docs/a.md", "specs/x/requirements.md", "tests/test_a.py"]).is_empty());
        assert_eq!(sel(r, &["crates/medulla-viz/src/lib.rs"]), ["facade", "medulla-viz"]);
        assert_eq!(
            sel(r, &["crates/medulla-types/src/lib.rs"]),
            ["facade", "medulla-numeric", "medulla-storage", "medulla-types"]
        );
        assert_eq!(sel(r, &["src/main.rs"]), ["facade"]);
        assert_eq!(sel(r, &["Cargo.lock"]), all);
        assert_eq!(sel(r, &["Cargo.toml"]), all);
    }

    #[test]
    fn runs_keep_main_feature_profiles() {
        let selected: Vec<String> = ["facade", "medulla-numeric", "medulla-storage", "medulla-types"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let packages = packages(&meta());
        assert_eq!(
            commands(&packages, &selected),
            [
                "{tool} -p medulla-types --all-features".to_string(),
                "{tool} -p medulla-storage --all-features {serial}".to_string(),
                format!("{{tool}} -p medulla-numeric --features {}", pyo3_features(&["medulla-numeric"])),
                "{tool} -p facade --features full".to_string(),
            ]
        );
        assert!(commands(&packages, &[]).is_empty());
    }

    #[test]
    fn own_job_packages_are_left_to_their_own_job() {
        let packages = packages(&meta());
        let two = ["medulla-types".to_string(), "medulla-tts-piper".to_string()];
        assert_eq!(commands(&packages, &two), ["{tool} -p medulla-types --all-features"]);
        assert!(commands(&packages, &["medulla-tts-piper".to_string()]).is_empty());
    }

    #[test]
    fn pyo3_features_name_only_selected_packages() {
        let features = pyo3_features(&["medulla-pyengine"]);
        assert!(!features.is_empty() && features.split(',').all(|f| f.starts_with("medulla-pyengine/")));
        assert_eq!(pyo3_features(&["medulla-numeric", "medulla-pyengine"]), PYO3_FEATURES);
    }

    #[test]
    fn spec_file_read_by_a_test_selects_its_package() {
        let tree = Tree::new(&[
            (
                "crates/medulla-types/tests/engine_capabilities.rs",
                "let plan = include_str!(\"../../../specs/engine-parity/plan.md\");\n",
            ),
            ("crates/medulla-viz/src/lib.rs", "// specs/engine-parity-other\n"),
        ]);
        let got = sel(tree.path(), &["specs/engine-parity/plan.md"]);
        assert!(got.contains(&"medulla-types".to_string()));
        assert!(!got.contains(&"medulla-viz".to_string()));
        assert!(sel(tree.path(), &["specs/unread/plan.md"]).is_empty());
    }
}
