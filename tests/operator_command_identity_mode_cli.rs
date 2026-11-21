//! MEDULLA-IDENTITY-R003.7: the offline `identity-mode set` command, exercised through the
//! real compiled binary. It is the only path out of an anonymous (`none`) store.
#![cfg(all(feature = "server", feature = "security"))]

use std::io::Write;
use std::process::{Command, Output, Stdio};

use medulla_identity_types::acl::{AgentIdentity, AgentRole};
use medulla_identity_types::identity::AuthMode;
use medulla_isolation::isolation::IsolationLayer;
use medulla_server::server_startup::{
    prepare_auth_store, ANONYMOUS_AGENT_ID, ANONYMOUS_MARKER_FILE,
};

fn server_bin() -> std::path::PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_medulla-server") {
        return path.into();
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent()
        .and_then(std::path::Path::parent)
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("medulla-server")
}

fn open_policy(dir: &std::path::Path) -> IsolationLayer {
    let authority = medulla_kernel::store_authority::process_authority();
    IsolationLayer::with_persist_dir(
        dir,
        authority.as_ref(),
        authority.principal(),
        &authority.proof(),
    )
    .expect("open the policy store")
}

/// An anonymous store as the engine leaves it: the marker file, an uninitialized identity
/// store and the registered System agent `anonymous`, beside one data file.
fn anonymous_store(dir: &std::path::Path) {
    prepare_auth_store("none", dir.to_str().expect("utf8 path")).expect("mark the store");
    let mut layer = open_policy(dir);
    layer
        .try_register_agent(AgentIdentity {
            agent_id: ANONYMOUS_AGENT_ID.into(),
            role: AgentRole::System,
            teams: Vec::new(),
            roles: Vec::new(),
        })
        .expect("register the anonymous agent");
    std::fs::write(dir.join("graph-data.bin"), b"kept").expect("write a data file");
}

fn run(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(server_bin())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn medulla-server");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write the password");
    child.wait_with_output().expect("wait for medulla-server")
}

// spec: MEDULLA-IDENTITY-R003.7
#[test]
fn none_store_converts_to_local_keeping_data() {
    let dir = tempfile::tempdir().expect("tempdir");
    anonymous_store(dir.path());
    let path = dir.path().to_str().expect("utf8 path");

    let output = run(
        &[
            "identity-mode",
            "set",
            path,
            "--to",
            "local",
            "--admin-username",
            "admin",
            "--confirm",
        ],
        "a long enough bootstrap password\n",
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!dir.path().join(ANONYMOUS_MARKER_FILE).exists());
    assert_eq!(
        std::fs::read(dir.path().join("graph-data.bin")).expect("data kept"),
        b"kept"
    );
    let layer = open_policy(dir.path());
    let config = layer
        .rbac()
        .identity_store()
        .config()
        .expect("the identity store is initialized");
    assert_eq!(config.mode, AuthMode::Local);
    assert_eq!(config.epoch, 1);
    assert!(!layer.is_registered(ANONYMOUS_AGENT_ID));
}

// spec: MEDULLA-IDENTITY-R003.7
#[test]
fn refuses_without_confirm_and_on_unmarked_store() {
    let marked = tempfile::tempdir().expect("tempdir");
    anonymous_store(marked.path());
    let marked_path = marked.path().to_str().expect("utf8 path");
    let output = run(
        &[
            "identity-mode",
            "set",
            marked_path,
            "--to",
            "local",
            "--admin-username",
            "admin",
        ],
        "a long enough bootstrap password\n",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("IDENTITY_MODE_NOT_CONFIRMED"));
    assert!(marked.path().join(ANONYMOUS_MARKER_FILE).exists());

    let unmarked = tempfile::tempdir().expect("tempdir");
    let unmarked_path = unmarked.path().to_str().expect("utf8 path");
    let output = run(
        &[
            "identity-mode",
            "set",
            unmarked_path,
            "--to",
            "local",
            "--admin-username",
            "admin",
            "--confirm",
        ],
        "a long enough bootstrap password\n",
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("IDENTITY_MODE_NOT_ANONYMOUS"));
    let entries: Vec<_> = std::fs::read_dir(unmarked.path())
        .expect("read the directory")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("engine.lock")]);
}
