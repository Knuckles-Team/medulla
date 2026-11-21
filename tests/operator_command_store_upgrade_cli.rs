//! MEDULLA-DURABLE-KERNEL-R007 — the host binary's `storage inspect`/`storage
//! upgrade` command surface, exercised through the real compiled binary.
//!
//! `src/operator_command.rs` and `src/server/persistence/store_upgrade.rs`
//! already carry library-level unit tests for the `inspect`/`apply`
//! functions. Nothing before this file spawned the actual
//! `medulla-server` executable and parsed real argv into the
//! `store-upgrade inspect`/`store-upgrade apply` subcommands, so the clap
//! wiring in `src/main.rs`/`src/operator_command.rs` (subcommand names, exit
//! status propagation) was untested at the CLI boundary R007 names.
#![cfg(all(feature = "server", feature = "security"))]

use std::process::{Command, Output};

fn server_bin() -> std::path::PathBuf {
    // `cargo test` defines this at compile time; `cargo check`/`clippy` may not, so
    // fall back to the binary cargo places next to this test's own executable.
    if let Some(path) = option_env!("CARGO_BIN_EXE_medulla-server") {
        return path.into();
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent()
        .and_then(std::path::Path::parent)
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("medulla-server")
}

fn run(args: &[&str]) -> Output {
    Command::new(server_bin())
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("spawn {} {args:?}: {error}", server_bin().display()))
}

// spec: MEDULLA-DURABLE-KERNEL-R007
#[test]
fn storage_inspect_reports_nothing_to_do_on_an_empty_data_directory() {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let data_dir = data_dir.path().to_str().expect("utf8 tempdir path");

    let output = run(&["store-upgrade", "inspect", data_dir]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "inspect on an empty data directory must report nothing to do; stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"command\":\"store-upgrade\""),
        "expected the single-line JSON summary in stdout, got: {stdout}"
    );
    assert!(
        stdout.contains("\"verb\":\"inspect\""),
        "expected the inspect verb recorded in the report: {stdout}"
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R007
#[test]
fn storage_apply_without_confirm_is_refused_and_opens_nothing() {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let data_dir = data_dir.path().to_str().expect("utf8 tempdir path");

    let output = run(&["store-upgrade", "apply", data_dir]);

    assert_ne!(
        output.status.code(),
        Some(0),
        "apply without --confirm must be refused, not silently accepted; stdout: {}",
        String::from_utf8_lossy(&output.stdout),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--confirm"),
        "the refusal must name the missing --confirm flag: {stderr}"
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R007
#[test]
fn storage_apply_with_confirm_reports_nothing_to_do_on_an_empty_data_directory() {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let data_dir = data_dir.path().to_str().expect("utf8 tempdir path");

    let output = run(&["store-upgrade", "apply", data_dir, "--confirm"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "apply --confirm on an empty data directory must report nothing to do; stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"verb\":\"apply\""),
        "expected the apply verb recorded in the report: {stdout}"
    );
}

fn shard_identity() -> medulla_storage::PhysicalStoreIdentity {
    medulla_storage::PhysicalStoreIdentity::new(
        medulla_store::redb_store::shard::SHARD_PHYSICAL_STORE,
    )
    .expect("shard physical identity")
}

fn open_shard(path: &std::path::Path) -> Result<medulla_storage::StorageKernel, String> {
    medulla_storage::StorageKernel::open_owner::<medulla_storage::GraphShardOwner>(
        path,
        shard_identity(),
        None,
    )
}

/// A source data directory holding one graph shard and a byte copy of it.
fn source_and_copy() -> (tempfile::TempDir, tempfile::TempDir) {
    let source = tempfile::tempdir().expect("source directory");
    let copy = tempfile::tempdir().expect("copy directory");
    drop(
        medulla_storage::StorageKernel::create_owner::<medulla_storage::GraphShardOwner>(
            &source.path().join("graph-0.redb"),
            shard_identity(),
            None,
        )
        .expect("create the source shard"),
    );
    std::fs::copy(
        source.path().join("graph-0.redb"),
        copy.path().join("graph-0.redb"),
    )
    .expect("copy the shard");
    (source, copy)
}

// spec: MEDULLA-DURABLE-KERNEL-R862.2
#[test]
fn rebind_copied_store_then_open() {
    let (source, copy) = source_and_copy();
    let copied = copy.path().join("graph-0.redb");
    let copy_dir = copy.path().to_str().expect("utf8 path");
    let source_dir = source.path().to_str().expect("utf8 path");

    assert!(
        open_shard(&copied).is_err(),
        "the ordinary open must refuse a copied store"
    );
    let before_source =
        medulla_storage::recovery_store_data_fingerprint(&source.path().join("graph-0.redb"))
            .expect("source fingerprint");
    let before_copy =
        medulla_storage::recovery_store_data_fingerprint(&copied).expect("copy fingerprint");
    assert_eq!(before_source, before_copy);

    let output = run(&[
        "store-upgrade",
        "rebind",
        copy_dir,
        "--copied-from",
        source_dir,
        "--confirm",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    drop(open_shard(&copied).expect("the rebound copy opens"));
    assert_eq!(
        medulla_storage::recovery_store_data_fingerprint(&copied).expect("fingerprint"),
        before_source,
        "the rebind changed data rows"
    );

    let again = run(&[
        "store-upgrade",
        "rebind",
        copy_dir,
        "--copied-from",
        source_dir,
        "--confirm",
    ]);
    assert_eq!(again.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&again.stdout).contains("nothing to do"),
        "a second run must report nothing to do: {}",
        String::from_utf8_lossy(&again.stdout)
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R862.2
#[test]
fn rebind_without_confirm_writes_nothing() {
    let (source, copy) = source_and_copy();
    let copied = copy.path().join("graph-0.redb");
    let before = std::fs::read(&copied).expect("read the copy");

    let output = run(&[
        "store-upgrade",
        "rebind",
        copy.path().to_str().expect("utf8 path"),
        "--copied-from",
        source.path().to_str().expect("utf8 path"),
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("REBIND_NOT_CONFIRMED"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read(&copied).expect("read the copy"), before);
    assert!(open_shard(&copied).is_err(), "the copy was rebound anyway");
}
