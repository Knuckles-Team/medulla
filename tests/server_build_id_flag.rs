//! MEDULLA-CONTRACT-R060.5 -- `medulla-server --build-id` prints the running executable's
//! build identity, equal to what `scripts/read_build_id.py` reads from the same file, and
//! runs before any server setup.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn bin() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_medulla-server") {
        return path.into();
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."))
        .join("medulla-server")
}

// spec: MEDULLA-CONTRACT-R060.5
#[test]
fn build_id_flag_matches_the_artifact_reader() {
    let binary = bin();
    let flag = Command::new(&binary).arg("--build-id").output().unwrap();
    assert!(
        flag.status.success(),
        "{}",
        String::from_utf8_lossy(&flag.stderr)
    );
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/read_build_id.py");
    let reader = Command::new("python3")
        .arg(&script)
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        reader.status.success(),
        "{}",
        String::from_utf8_lossy(&reader.stderr)
    );
    assert_eq!(
        flag.stdout, reader.stdout,
        "the two readers return one identity"
    );
    let printed = String::from_utf8(flag.stdout).unwrap();
    let id = printed.strip_suffix('\n').expect("one trailing newline");
    assert!(!id.is_empty() && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
}

// spec: MEDULLA-CONTRACT-R060.5
#[test]
fn build_id_flag_runs_before_server_setup() {
    let dir = tempfile::tempdir().unwrap();
    let persist = dir.path().join("persist");
    let started = Instant::now();
    let output = Command::new(bin())
        .arg("--build-id")
        .env("GRAPH_SERVICE_PERSIST_DIR", &persist)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!persist.exists(), "no persist directory is created");
}
