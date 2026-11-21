//! A crate that holds a registered background job spawns it through `observe::spawn`.

use std::fs;
use std::path::{Path, PathBuf};

/// The crates that hold a registered background job (MEDULLA-OBSERVE-R003.3 to R003.6). A crate
/// joins this list in the same change that registers its job.
const JOB_CRATES: [&str; 1] = ["medulla-persistence"];

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
                    found.push(format!("{}:{}: {}", file.display(), number + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "spawn a registered job through medulla_observe::spawn::spawn: {found:#?}"
    );
}
