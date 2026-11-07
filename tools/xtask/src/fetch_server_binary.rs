//! MEDULLA-CONTRACT-R053.9: record and verify the per-platform release server executable.
//!
//! `cargo xtask fetch-server-binary <record|verify> DIR`. `record` writes `<exe>.sha256` next to
//! the freshly built executable; `verify` requires exactly one executable whose bytes match that
//! checksum, so every folded-wheel pass of a platform folds the same bytes.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub const NAME: &str = "fetch-server-binary";
pub const SUMMARY: &str = "record or verify the release server executable checksum";

const SERVER_NAMES: [&str; 2] = ["medulla-server", "medulla-server.exe"];

pub fn run(_root: &Path, args: &[String]) -> Result<(), String> {
    let [action, directory] = args else {
        return Err("usage: cargo xtask fetch-server-binary <record|verify> DIR".into());
    };
    let digest = match action.as_str() {
        "record" => record(Path::new(directory))?,
        "verify" => verify(Path::new(directory))?,
        _ => return Err("usage: cargo xtask fetch-server-binary <record|verify> DIR".into()),
    };
    println!("{action} {digest}");
    Ok(())
}

fn executable(directory: &Path) -> Result<PathBuf, String> {
    let found: Vec<PathBuf> = SERVER_NAMES
        .iter()
        .map(|n| directory.join(n))
        .filter(|p| p.is_file())
        .collect();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(format!(
            "expected exactly one server executable in {}",
            directory.display()
        )),
    }
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn digest(path: &Path) -> Result<String, String> {
    let data = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if data.is_empty() {
        return Err(format!("server executable is empty: {}", name_of(path)));
    }
    Ok(Sha256::digest(&data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn sidecar(exe: &Path) -> PathBuf {
    PathBuf::from(format!("{}.sha256", exe.display()))
}

pub fn record(directory: &Path) -> Result<String, String> {
    let exe = executable(directory)?;
    let digest = digest(&exe)?;
    fs::write(sidecar(&exe), format!("{digest}  {}\n", name_of(&exe)))
        .map_err(|e| e.to_string())?;
    Ok(digest)
}

pub fn verify(directory: &Path) -> Result<String, String> {
    let exe = executable(directory)?;
    let path = sidecar(&exe);
    if !path.is_file() {
        return Err(format!("missing checksum for {}", name_of(&exe)));
    }
    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let (expected, name) = text.trim().split_once("  ").unwrap_or((text.trim(), ""));
    if name != name_of(&exe) || expected != digest(&exe)? {
        return Err(format!(
            "server executable does not match its checksum: {}",
            name_of(&exe)
        ));
    }
    Ok(expected.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;

    #[test]
    fn record_then_verify_round_trips_and_detects_tampering() {
        let tree = Tree::new(&[("bin/medulla-server", "server bytes")]);
        let dir = tree.path().join("bin");
        let recorded = record(&dir).expect("records");
        assert_eq!(verify(&dir).expect("verifies"), recorded);
        assert_eq!(
            fs::read_to_string(dir.join("medulla-server.sha256")).expect("sidecar"),
            format!("{recorded}  medulla-server\n")
        );
        fs::write(dir.join("medulla-server"), "tampered").expect("write");
        assert!(verify(&dir)
            .expect_err("tampered")
            .contains("does not match its checksum"));
    }

    #[test]
    fn exactly_one_non_empty_executable_with_a_checksum_is_required() {
        let tree = Tree::new(&[
            ("none/.keep", ""),
            ("empty/medulla-server", ""),
            ("two/medulla-server", "a"),
            ("two/medulla-server.exe", "b"),
        ]);
        let root = tree.path();
        assert!(record(&root.join("none")).is_err());
        assert!(record(&root.join("empty"))
            .expect_err("empty")
            .contains("is empty"));
        assert!(record(&root.join("two")).is_err());
        let tree = Tree::new(&[("bin/medulla-server", "x")]);
        assert!(verify(&tree.path().join("bin"))
            .expect_err("no sidecar")
            .contains("missing checksum"));
    }
}
