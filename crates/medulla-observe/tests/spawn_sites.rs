//! A crate that holds a registered background job spawns it through `observe::spawn`.

use std::fs;
use std::path::{Path, PathBuf};

/// The crates that hold a registered background job (MEDULLA-OBSERVE-R003.3 to R003.6) and spawn
/// nothing else. A crate joins this list in the same change that registers its job.
const JOB_CRATES: [&str; 1] = ["medulla-persistence"];

/// The files, relative to `crates/`, that hold a registered background job in a crate that also
/// spawns work that is not a job (a connection handler, a request task): the file is scanned, the
/// crate is not. Code after the first `#[cfg(test)]` of a file is test code and is not scanned.
const JOB_FILES: [&str; 7] = [
    "medulla-h-admin/src/agent_library_projection.rs",
    "medulla-h-admin/src/namespace_migration.rs",
    "medulla-h-admin/src/boot_maintenance.rs",
    "medulla-h-admin/src/connector_pack_projection.rs",
    "medulla-h-retrieve/src/reasoning_projection.rs",
    "medulla-server-graph/src/semantic_activation.rs",
    "medulla-server/src/server_startup.rs",
];

/// Lines of `file` outside its test module that spawn without the observe helper.
fn bare_spawns(file: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for (number, line) in fs::read_to_string(file).unwrap().lines().enumerate() {
        if line.trim() == "#[cfg(test)]" {
            break;
        }
        let code = line.split("//").next().unwrap_or("");
        if code.contains("tokio::spawn(") || code.contains("task::spawn(") {
            found.push(format!(
                "{}:{}: {}",
                file.display(),
                number + 1,
                line.trim()
            ));
        }
    }
    found
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

// spec: MEDULLA-OBSERVE-R003.2
#[test]
fn registered_job_crates_hold_no_bare_tokio_spawn() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = Vec::new();
    for name in JOB_CRATES {
        let mut files = Vec::new();
        rust_files(&crates.join(name).join("src"), &mut files);
        assert!(!files.is_empty(), "{name} holds no source file");
        for file in files {
            for (number, line) in fs::read_to_string(&file).unwrap().lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                if code.contains("tokio::spawn(") || code.contains("task::spawn(") {
                    found.push(format!(
                        "{}:{}: {}",
                        file.display(),
                        number + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "spawn a registered job through medulla_observe::spawn::spawn: {found:#?}"
    );
}

// spec: MEDULLA-OBSERVE-R003.4
// spec: MEDULLA-OBSERVE-R003.5
// spec: MEDULLA-OBSERVE-R003.6
#[test]
fn registered_job_files_hold_no_bare_tokio_spawn() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = Vec::new();
    for relative in JOB_FILES {
        let file = crates.join(relative);
        assert!(
            file.is_file(),
            "{relative} moved or was deleted: update JOB_FILES"
        );
        found.extend(bare_spawns(&file));
        // A file on the list registers at least one job: an empty list is a drifted scan.
        let source = fs::read_to_string(&file).unwrap();
        assert!(
            source.contains("medulla_observe::spawn::spawn("),
            "{relative} no longer registers a job through observe::spawn"
        );
    }
    assert!(
        found.is_empty(),
        "spawn a registered job through medulla_observe::spawn::spawn: {found:#?}"
    );
}
