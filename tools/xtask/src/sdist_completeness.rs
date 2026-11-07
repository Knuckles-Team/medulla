//! MEDULLA-SDLC-R022: a source distribution is complete before the publication manifest freezes.
//!
//! `cargo xtask sdist-completeness <medulla-X.Y.Z.tar.gz>` derives the required source inventory
//! from the checkout (tracked files of the source directories, Cargo build inputs, the PEP 517
//! backend and the helpers it imports) and fails closed on a missing input, an unsafe or
//! duplicated archive member, or a name/version/`License-File` mismatch.

use flate2::read::GzDecoder;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use toml_edit::{DocumentMut, Item};

pub const NAME: &str = "sdist-completeness";
pub const SUMMARY: &str = "a source distribution holds every source input and matches its version";

const BASE_SOURCES: [&str; 5] = [
    "Cargo.toml",
    "Cargo.lock",
    "pyproject.toml",
    "rust-toolchain.toml",
    "README.md",
];
const SOURCE_DIRECTORIES: [&str; 2] = ["crates", "medulla"];
/// The root crate only belongs in the archive when maturin builds it.
const ROOT_CRATE_DIRECTORIES: [&str; 2] = ["src", ".cargo"];

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let [archive] = args else {
        return Err("usage: cargo xtask sdist-completeness <one source distribution>".into());
    };
    let version = package_version(root)?;
    validate(Path::new(archive), &required_sources(root)?, &version)?;
    println!("Complete source distribution: {archive}");
    Ok(())
}

fn parse_toml(path: &Path) -> Result<DocumentMut, String> {
    fs::read_to_string(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .parse::<DocumentMut>()
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn package_version(root: &Path) -> Result<String, String> {
    let doc = parse_toml(&root.join("Cargo.toml"))?;
    doc.get("package")
        .and_then(|p| p.get("version"))
        .and_then(Item::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "Cargo.toml: package.version is missing".to_string())
}

/// Lexical normalisation of `base/relative` as a path relative to `root`.
fn relative_to_root(root: &Path, base: &Path, relative: &str) -> Result<String, String> {
    let mut parts: Vec<String> = Vec::new();
    let joined = base.join(relative);
    let stripped = joined
        .strip_prefix(root)
        .map_err(|_| format!("{relative}: outside the repository"))?;
    for component in stripped.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => {
                parts
                    .pop()
                    .ok_or(format!("{relative}: outside the repository"))?;
            }
            _ => {}
        }
    }
    Ok(parts.join("/"))
}

/// Explicit Cargo build scripts, target paths and package file inputs of one manifest.
fn manifest_sources(manifest: &Path, root: &Path) -> Result<BTreeSet<String>, String> {
    let doc = parse_toml(manifest)?;
    let dir = manifest.parent().expect("manifest has a parent");
    let mut paths: Vec<String> = Vec::new();
    let package = doc.get("package");
    for key in ["build", "readme", "license-file"] {
        if let Some(path) = package.and_then(|p| p.get(key)).and_then(Item::as_str) {
            paths.push(path.to_owned());
        }
    }
    if package.is_some_and(|p| p.get("build").is_none()) && dir.join("build.rs").is_file() {
        paths.push("build.rs".into());
    }
    if let Some(path) = doc
        .get("lib")
        .and_then(|t| t.get("path"))
        .and_then(Item::as_str)
    {
        paths.push(path.to_owned());
    }
    for kind in ["bin", "example", "bench", "test"] {
        let Some(item) = doc.get(kind) else { continue };
        let tables: Vec<&dyn toml_edit::TableLike> = match item {
            Item::ArrayOfTables(a) => a.iter().map(|t| t as &dyn toml_edit::TableLike).collect(),
            Item::Value(v) => v
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e.as_inline_table())
                        .map(|t| t as &dyn toml_edit::TableLike)
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        for table in tables {
            if let Some(path) = table.get("path").and_then(Item::as_str) {
                paths.push(path.to_owned());
            }
        }
    }
    paths
        .iter()
        .map(|p| relative_to_root(root, dir, p))
        .collect()
}

fn builds_root_crate(root: &Path) -> Result<bool, String> {
    let pyproject = root.join("pyproject.toml");
    if !pyproject.is_file() {
        return Ok(true);
    }
    let doc = parse_toml(&pyproject)?;
    let manifest = doc
        .get("tool")
        .and_then(|t| t.get("maturin"))
        .and_then(|m| m.get("manifest-path"))
        .and_then(Item::as_str)
        .unwrap_or("Cargo.toml");
    Ok(relative_to_root(root, root, manifest)? == "Cargo.toml")
}

/// The PEP 517 backend and the `scripts.*` helpers it imports: an sdist must be installable.
pub fn backend_sources(root: &Path) -> Result<BTreeSet<String>, String> {
    let pyproject = root.join("pyproject.toml");
    if !pyproject.is_file() {
        return Ok(BTreeSet::new());
    }
    let doc = parse_toml(&pyproject)?;
    let module = doc
        .get("build-system")
        .and_then(|b| b.get("build-backend"))
        .and_then(Item::as_str)
        .unwrap_or("");
    if module.is_empty() || !root.join(format!("{module}.py")).is_file() {
        return Ok(BTreeSet::new());
    }
    let import = Regex::new(r"^\s*from scripts\.(\w+) import").expect("static regex");
    let mut found = BTreeSet::new();
    let mut pending = vec![format!("{module}.py")];
    while let Some(name) = pending.pop() {
        if !found.insert(name.clone()) {
            continue;
        }
        let text = fs::read_to_string(root.join(&name)).map_err(|e| format!("{name}: {e}"))?;
        for line in text.lines() {
            if let Some(m) = import.captures(line) {
                if root.join("scripts").join(format!("{}.py", &m[1])).is_file() {
                    pending.push(format!("scripts/{}.py", &m[1]));
                }
            }
        }
    }
    Ok(found)
}

fn directories(root: &Path) -> Result<Vec<&'static str>, String> {
    let mut dirs = SOURCE_DIRECTORIES.to_vec();
    if builds_root_crate(root)? {
        dirs.extend(ROOT_CRATE_DIRECTORIES);
    }
    Ok(dirs)
}

/// The release checkout is the inventory authority; an archive or fixture has no Git metadata.
fn source_files(root: &Path) -> Result<BTreeSet<String>, String> {
    let dirs = directories(root)?;
    if root.join(".git").exists() {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["ls-files", "-z", "--"])
            .args(&dirs)
            .output()
            .map_err(|e| format!("git ls-files: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git ls-files failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        return Ok(String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect());
    }
    let mut files = BTreeSet::new();
    for dir in dirs {
        walk(&root.join(dir), &mut |p| {
            let rel = p.strip_prefix(root).expect("walked under root");
            let ignored = rel.components().any(|c| c.as_os_str() == "__pycache__")
                || matches!(p.extension().and_then(|e| e.to_str()), Some("pyc" | "pyo"));
            if !ignored {
                files.insert(rel.to_string_lossy().replace('\\', "/"));
            }
        })?;
    }
    Ok(files)
}

fn walk(dir: &Path, visit: &mut dyn FnMut(&Path)) -> Result<(), String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let path: PathBuf = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            walk(&path, visit)?;
        } else {
            visit(&path);
        }
    }
    Ok(())
}

/// Source files and manifest-declared build inputs.
pub fn required_sources(root: &Path) -> Result<BTreeSet<String>, String> {
    let mut required: BTreeSet<String> = BASE_SOURCES.iter().map(|s| (*s).to_owned()).collect();
    required.extend(backend_sources(root)?);
    let root_crate = builds_root_crate(root)?;
    if root_crate {
        required.insert("build.rs".into());
    }
    required.extend(source_files(root)?);
    let mut manifests = Vec::new();
    walk(&root.join("crates"), &mut |p| {
        if p.file_name().is_some_and(|n| n == "Cargo.toml") {
            manifests.push(p.to_path_buf());
        }
    })?;
    if root_crate {
        manifests.push(root.join("Cargo.toml"));
    }
    for manifest in manifests {
        required.extend(manifest_sources(&manifest, root)?);
    }
    Ok(required)
}

fn is_canonical_name(name: &str, is_dir: bool, prefix: &str) -> bool {
    if name.contains('\\') || name.split('/').any(|p| matches!(p, "" | "." | "..")) {
        return false;
    }
    name.starts_with(prefix) || (is_dir && name == &prefix[..prefix.len() - 1])
}

/// Header fields of a `PKG-INFO` document (RFC 822 style, folded lines joined).
fn headers(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            break;
        }
        if line.starts_with([' ', '\t']) {
            if let Some(last) = out.last_mut() {
                last.1.push('\n');
                last.1.push_str(line.trim());
            }
        } else if let Some((k, v)) = line.split_once(':') {
            out.push((k.to_owned(), v.trim().to_owned()));
        }
    }
    out
}

fn check_metadata(
    pkg_info: &str,
    cargo: &str,
    files: &BTreeSet<String>,
    prefix: &str,
    version: &str,
) -> Result<(), String> {
    let fields = headers(pkg_info);
    let first = |key: &str| {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    if first("Name") != Some("medulla") || first("Version") != Some(version) {
        return Err("sdist metadata identity differs".into());
    }
    // An extensions-only sdist carries a workspace-only manifest; its identity is PKG-INFO.
    let doc: DocumentMut = cargo
        .parse()
        .map_err(|_| "sdist Cargo manifest unreadable")?;
    if let Some(package) = doc.get("package") {
        if package.get("version").and_then(Item::as_str) != Some(version) {
            return Err("sdist Cargo version differs".into());
        }
    }
    // PyPI refuses (HTTP 400) an upload whose `License-File` is not in the archive.
    let absent: Vec<&str> = fields
        .iter()
        .filter(|(k, _)| k == "License-File")
        .map(|(_, v)| v.as_str())
        .filter(|name| !files.contains(&format!("{prefix}{name}")))
        .collect();
    if absent.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "declared License-File missing from sdist: {absent:?}"
        ))
    }
}

/// Validates canonical archive identities, the source inventory and the package version.
pub fn validate(path: &Path, required: &BTreeSet<String>, version: &str) -> Result<(), String> {
    let prefix = format!("medulla-{version}/");
    if path.file_name().and_then(|n| n.to_str()) != Some(&format!("medulla-{version}.tar.gz")) {
        return Err("sdist filename/version differs".into());
    }
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut archive = tar::Archive::new(GzDecoder::new(file));
    let mut files = BTreeSet::new();
    let mut members: BTreeMap<String, bool> = BTreeMap::new();
    let (mut pkg_info, mut cargo) = (String::new(), String::new());
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.header().entry_type();
        let is_dir = kind.is_dir();
        let raw = entry.path_bytes().into_owned();
        let mut name = String::from_utf8(raw).map_err(|_| "unsafe or noncanonical archive path")?;
        if is_dir {
            name = name.trim_end_matches('/').to_owned();
        }
        if !is_canonical_name(&name, is_dir, &prefix) {
            return Err("unsafe or noncanonical archive path".into());
        }
        if members.contains_key(&name) || !(kind.is_file() || is_dir) {
            return Err("linked, special or duplicate archive member".into());
        }
        members.insert(name.clone(), is_dir);
        if kind.is_file() {
            if name == format!("{prefix}PKG-INFO") {
                entry
                    .read_to_string(&mut pkg_info)
                    .map_err(|e| e.to_string())?;
            } else if name == format!("{prefix}Cargo.toml") {
                entry
                    .read_to_string(&mut cargo)
                    .map_err(|e| e.to_string())?;
            }
            files.insert(name);
        }
    }
    for name in members.keys() {
        let mut parent = name.as_str();
        while let Some((up, _)) = parent.rsplit_once('/') {
            if members.get(up) == Some(&false) {
                return Err("file/directory archive collision".into());
            }
            parent = up;
        }
    }
    let present: BTreeSet<&str> = files
        .iter()
        .filter_map(|n| n.strip_prefix(&prefix))
        .collect();
    let mut wanted = required.clone();
    wanted.extend(["Cargo.toml".to_owned(), "PKG-INFO".to_owned()]);
    let missing: Vec<&String> = wanted
        .iter()
        .filter(|n| !present.contains(n.as_str()))
        .collect();
    if !missing.is_empty() {
        return Err(format!("missing source inputs: {missing:?}"));
    }
    check_metadata(&pkg_info, &cargo, &files, &prefix, version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;
    use flate2::{write::GzEncoder, Compression};
    use tar::{EntryType, Header};

    const V: &str = "2.27.1";

    struct Member(&'static str, &'static str, EntryType);
    fn file(name: &'static str, text: &'static str) -> Member {
        Member(name, text, EntryType::Regular)
    }
    fn dir(name: &'static str) -> Member {
        Member(name, "", EntryType::Directory)
    }

    /// Writes `medulla-2.27.1/<name>` members with raw (unsanitised) names.
    fn archive(dir_path: &Path, members: &[Member]) -> PathBuf {
        let path = dir_path.join(format!("medulla-{V}.tar.gz"));
        let mut builder = tar::Builder::new(GzEncoder::new(
            fs::File::create(&path).unwrap(),
            Compression::fast(),
        ));
        for Member(name, text, kind) in members {
            let mut header = Header::new_gnu();
            let full = format!("medulla-{V}/{name}");
            header.as_old_mut().name[..full.len()].copy_from_slice(full.as_bytes());
            header.set_entry_type(*kind);
            header.set_size(if kind.is_file() { text.len() as u64 } else { 0 });
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, text.as_bytes()).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        path
    }

    fn base(omitted: &str, version: &str) -> Vec<Member> {
        let pkg: &'static str =
            Box::leak(format!("Name: medulla\nVersion: {version}\n").into_boxed_str());
        [
            file("Cargo.toml", "[package]\nversion = \"2.27.1\"\n"),
            file("PKG-INFO", pkg),
            file("crates/leaf/src/lib.rs", "pub fn leaf() {}"),
            file("build.rs", "fn main() {}"),
            file(
                "crates/leaf/assets/viz_interactive_client.html",
                "<main>client</main>",
            ),
        ]
        .into_iter()
        .filter(|m| m.0 != omitted)
        .collect()
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    fn err(result: Result<(), String>, needle: &str) {
        let message = result.expect_err("must refuse");
        assert!(message.contains(needle), "{message} lacks {needle}");
    }

    #[test]
    fn accepts_complete_sources() {
        let t = Tree::new(&[]);
        let a = archive(t.path(), &base("", V));
        validate(&a, &set(&["Cargo.toml", "crates/leaf/src/lib.rs"]), V).unwrap();
    }

    #[test]
    fn rejects_incomplete_or_misidentified_sources() {
        let t = Tree::new(&[]);
        let required = set(&["Cargo.toml", "crates/leaf/src/lib.rs"]);
        let a = archive(t.path(), &base("crates/leaf/src/lib.rs", V));
        err(validate(&a, &required, V), "missing source");
        let a = archive(t.path(), &base("", "2.27.0"));
        err(validate(&a, &required, V), "metadata identity");
    }

    #[test]
    fn rejects_unsafe_member_path() {
        let t = Tree::new(&[]);
        let mut members = base("", V);
        members.push(file("../escape", "outside"));
        err(
            validate(&archive(t.path(), &members), &set(&["Cargo.toml"]), V),
            "noncanonical",
        );
    }

    #[test]
    fn declared_license_file_must_ship() {
        let t = Tree::new(&[]);
        let info = file(
            "PKG-INFO",
            "Name: medulla\nVersion: 2.27.1\nLicense-File: LICENSE\n",
        );
        let cargo = file("Cargo.toml", "[package]\nversion = \"2.27.1\"\n");
        let a = archive(t.path(), &[cargo, info]);
        err(validate(&a, &set(&["Cargo.toml"]), V), "License-File");
        let info = file(
            "PKG-INFO",
            "Name: medulla\nVersion: 2.27.1\nLicense-File: LICENSE\n",
        );
        let cargo = file("Cargo.toml", "[package]\nversion = \"2.27.1\"\n");
        let a = archive(t.path(), &[cargo, info, file("LICENSE", "MIT")]);
        validate(&a, &set(&["Cargo.toml"]), V).unwrap();
    }

    #[test]
    fn rejects_noncanonical_member_aliases() {
        for alias in [
            "./Cargo.toml",
            "nested/../Cargo.toml",
            "nested//Cargo.toml",
            "nested/./Cargo.toml",
            "nested\\Cargo.toml",
            "/Cargo.toml",
            "",
        ] {
            let t = Tree::new(&[]);
            let mut members = base("", V);
            members.push(file(alias, "[package]\nversion = \"0.0.0\"\n"));
            err(
                validate(&archive(t.path(), &members), &set(&["Cargo.toml"]), V),
                "noncanonical",
            );
        }
    }

    #[test]
    fn rejects_directory_identity_collisions() {
        let cases: Vec<Vec<Member>> = vec![
            vec![dir("Cargo.toml")],
            vec![dir("folder"), dir("folder")],
            vec![dir("folder"), file("folder", "file")],
            vec![file("folder", "file"), file("folder/child", "child")],
            vec![file("folder/child", "child"), file("folder", "file")],
        ];
        for extras in cases {
            let t = Tree::new(&[]);
            let mut members = base("", V);
            members.extend(extras);
            let message = validate(&archive(t.path(), &members), &set(&["Cargo.toml"]), V)
                .expect_err("must refuse");
            assert!(
                message.contains("duplicate") || message.contains("collision"),
                "{message}"
            );
        }
    }

    #[test]
    fn accepts_canonical_directory_entries() {
        let t = Tree::new(&[]);
        let mut members = base("", V);
        members.extend([
            dir(""),
            dir("folder"),
            file("folder/source.rs", "pub fn source() {}"),
        ]);
        validate(
            &archive(t.path(), &members),
            &set(&["Cargo.toml", "folder/source.rs"]),
            V,
        )
        .unwrap();
    }

    #[test]
    fn requires_root_build_script() {
        let t = Tree::new(&[
            (
                "Cargo.toml",
                "[package]\nname = \"fixture\"\nversion = \"1.0.0\"\n",
            ),
            ("build.rs", "fn main() {}"),
        ]);
        assert!(required_sources(t.path()).unwrap().contains("build.rs"));
        let a = archive(t.path(), &base("build.rs", V));
        err(validate(&a, &set(&["build.rs"]), V), "build.rs");
    }

    #[test]
    fn extensions_only_inputs_are_the_real_build_inputs() {
        let root = crate::workspace_root();
        let required = required_sources(&root).unwrap();
        for absent in ["build.rs", "src/lib.rs", "src/main.rs"] {
            assert!(!required.contains(absent), "{absent}");
        }
        assert!(!required
            .iter()
            .any(|n| n.starts_with("benches/") || n.starts_with(".cargo/")));
        for present in ["build_backend.py", "crates/medulla-pyengine/Cargo.toml"] {
            assert!(required.contains(present), "{present}");
        }
        assert!(required.contains("scripts/inject_numeric_kernel.py"));
        assert!(!required.contains("scripts/inject_pyengine.py"));
    }

    #[test]
    fn inventory_includes_explicit_manifest_inputs() {
        let t = Tree::new(&[
            (
                "Cargo.toml",
                "[package]\nname = \"fixture\"\nversion = \"1.0.0\"\nbuild = \"tools/prepare.input\"\n\
                 readme = \"PACKAGE.txt\"\nlicense-file = \"LICENSE.txt\"\n\
                 [lib]\npath = \"library/entry.input\"\n\
                 [[bin]]\nname = \"server\"\npath = \"programs/server.input\"\n\
                 [[example]]\nname = \"example\"\npath = \"examples/entry.input\"\n\
                 [[bench]]\nname = \"bench\"\npath = \"benches/entry.input\"\n\
                 [[test]]\nname = \"test\"\npath = \"integration/entry.input\"\n",
            ),
            ("crates/leaf/Cargo.toml", "[package]\nname = \"leaf\"\nversion = \"1.0.0\"\n"),
            ("crates/leaf/build.rs", "fn main() {}"),
        ]);
        let required = required_sources(t.path()).unwrap();
        for expected in [
            "build.rs",
            "tools/prepare.input",
            "PACKAGE.txt",
            "LICENSE.txt",
            "library/entry.input",
            "programs/server.input",
            "examples/entry.input",
            "benches/entry.input",
            "integration/entry.input",
            "crates/leaf/build.rs",
        ] {
            assert!(required.contains(expected), "{expected}");
        }
    }

    #[test]
    fn requires_compiled_html_asset() {
        let asset = "crates/leaf/assets/viz_interactive_client.html";
        let t = Tree::new(&[
            (asset, "<main>client</main>"),
            (
                "crates/leaf/Cargo.toml",
                "[package]\nname = \"leaf\"\nversion = \"1.0.0\"\n",
            ),
            ("Cargo.toml", "[workspace]\n"),
        ]);
        assert!(required_sources(t.path()).unwrap().contains(asset));
        validate(&archive(t.path(), &base("", V)), &set(&[asset]), V).unwrap();
        err(
            validate(&archive(t.path(), &base(asset, V)), &set(&[asset]), V),
            "viz_interactive_client.html",
        );
    }

    #[test]
    fn inventory_has_no_native_asset_suffix_filter() {
        for asset in ["client.html", "shader.wgsl", "lookup.bin"] {
            let t = Tree::new(&[
                (
                    "Cargo.toml",
                    "[package]\nname = \"fixture\"\nversion = \"1.0.0\"\n",
                ),
                (&format!("src/{asset}"), "build input"),
            ]);
            assert!(required_sources(t.path())
                .unwrap()
                .contains(&format!("src/{asset}")));
        }
    }

    #[test]
    fn engine_composition_helper_must_ship() {
        let t = Tree::new(&[]);
        let a = archive(t.path(), &base("", V));
        err(
            validate(&a, &set(&["scripts/inject_numeric_kernel.py"]), V),
            "inject_numeric_kernel.py",
        );
    }

    #[test]
    fn backend_closure_follows_script_imports() {
        let t = Tree::new(&[
            (
                "pyproject.toml",
                "[build-system]\nbuild-backend = \"build_backend\"\n",
            ),
            ("build_backend.py", "from scripts.one import x\n"),
            ("scripts/one.py", "from scripts.two import y\n"),
            ("scripts/two.py", ""),
        ]);
        assert_eq!(
            backend_sources(t.path()).unwrap(),
            set(&["build_backend.py", "scripts/one.py", "scripts/two.py"])
        );
    }
}
