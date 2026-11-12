//! MEDULLA-DURABLE-KERNEL-R007.1 — the offline `restore` point-in-time
//! boundary, exercised through the real compiled binary.
//!
//! The library-level unit tests cover `replay_to`'s typing on pure data
//! (`medulla-storage::recovery::archive`). Nothing before this file spawned the
//! actual `medulla-server` executable with a target-past-archive-end argv and
//! asserted that the refusal happens before any destination effect: a refused
//! replay must not move bytes into the persist-dir — no shard file, no
//! manifest — fail-closed at the CLI boundary, not just at the type level.
#![cfg(all(feature = "server", feature = "security", feature = "redb"))]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(feature = "security")]
use medulla_kernel::crypto;
#[allow(unused_imports)]
use medulla_server_api::api::persistence::PersistenceBackend;

fn server_bin() -> PathBuf {
    // `cargo test` defines this at compile time; `cargo check`/`clippy` may
    // not, so fall back on the layout cargo actually produces.
    if let Some(path) = option_env!("CARGO_BIN_EXE_medulla-server") {
        return path.into();
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent()
        .and_then(std::path::Path::parent)
        .unwrap_or_else(|| Path::new("."))
        .join("medulla-server")
}

fn run(args: &[&str]) -> Output {
    Command::new(server_bin())
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("spawn {} {args:?}: {error}", server_bin().display()))
}

/// A bundle re-derives its census from the shards it copies, so the manifest
/// the CLI test needs is one produced by the ONLINE backup path — the real
/// engine, not a synthesized file. A fresh K=1 store carries zero graphs and
/// zero retained xshard decisions, which IS the certified archive here: the
/// end-sequence bound of the manifest is 0, so `--to-seq 5` is past the
/// archive end by construction and `replay_to` must refuse it typed.
async fn bundle_of_fresh_k1_store(bundle_dir: &Path) {
    use medulla_persistence::redb_backend::RedbBackend;
    #[allow(unused_imports)]
    use medulla_server_api::api::persistence::PersistenceBackend;

    let host = bundle_dir
        .parent()
        .expect("bundle has a parent")
        .join("live");
    std::fs::create_dir_all(&host).expect("host dir");
    let backend = RedbBackend::open_with_shards(host.to_string_lossy().to_string(), 256, 1)
        .expect("open fresh K=1 store");
    let report = backend
        .backup(bundle_dir, "pit-r007.1-test", 1_700_000_000, "nightly", &[])
        .expect("backup");
    assert_eq!(report.shards, 1, "one shard file in the bundle");
    assert_eq!(
        report.xshard_decisions, 0,
        "a fresh store retains no xshard decisions: the archive end is sequence 0"
    );
    backend.shutdown();
}

// spec: MEDULLA-DURABLE-KERNEL-R007.1
#[tokio::test(flavor = "multi_thread")]
async fn target_past_archive_refuses_and_writes_nothing() {
    #[cfg(feature = "security")]
    let _env_lock = crypto::acquire_test_env_lock().await;

    let root = tempfile::tempdir().expect("tempdir");
    let bundle = root.path().join("bundle");
    let restored = root.path().join("restored");
    std::fs::create_dir_all(&bundle).expect("bundle dir");
    bundle_of_fresh_k1_store(&bundle).await;

    let bundle_s = bundle.to_string_lossy().to_string() + "/";
    let restored_s = restored.to_string_lossy().to_string();
    let output = run(&[
        "restore",
        "--bundle",
        &bundle_s,
        "--persist-dir",
        &restored_s,
        "--shards",
        "1",
        "--archive",
        &format!("directory://{bundle_s}"),
        "--to-seq",
        "5",
    ]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_ne!(
        output.status.code(),
        Some(0),
        "a target past the archive end must refuse; stdout: {stdout}; stderr: {stderr}"
    );
    // The refusal is typed (serialized ArchiveRefusal with a `code` tag), and
    // it names the boundary that refused: target 5 past archive end 0.
    assert!(
        stdout.contains(r#""code":"target_past_archive_end""#),
        "expected the typed ArchiveRefusal on stdout, got: {stdout}{stderr}"
    );
    assert!(
        stdout.contains(r#""target_sequence":5"#) && stdout.contains(r#""end_sequence":0"#),
        "the refusal must name the refused target and the certified archive end: {stdout}"
    );
    // Fail-closed: the refusal happened BEFORE any destination effect, so the
    // persist-dir was never populated (not even a partial shard file).
    let written: Vec<String> = std::fs::read_dir(&restored)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        written.is_empty(),
        "a refused restore must write NOTHING into the persist-dir, wrote: {written:?}"
    );
}
