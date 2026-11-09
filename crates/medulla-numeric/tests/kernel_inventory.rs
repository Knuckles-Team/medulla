//! Shared-kernel inventory census (MEDULLA-FINANCE-PRIMITIVES-R002.1): every kernel has
//! exactly ONE implementation under `src/`, reached only through the shared authority.
//! A duplicate authority must fail qualification rather than shadow the shared kernel.
//!
//! One test per kernel row, added as each R002.1.x slice lands.

use std::fs;
use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir` (recursive, sorted for a stable census).
fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let entries =
        fs::read_dir(dir).unwrap_or_else(|error| panic!("census: reading {dir:?}: {error}"));
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(rs_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    out.sort();
    out
}

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.1.1
#[test]
fn rolling_z_score_one_implementation() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let src = Path::new(manifest_dir).join("src");
    let files = rs_files(&src);
    assert!(
        !files.is_empty(),
        "census scanned zero .rs files under src/; the inventory guard is blind"
    );

    // The kernel's one implementation: `fn zscore` in `src/series/kernel.rs`. A second
    // definition anywhere under `src/` is a duplicate authority and fails qualification.
    let implementations = files
        .iter()
        .map(|path| {
            let text = fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("census: reading {}: {error}", path.display()));
            text.matches("fn zscore(").count()
        })
        .sum::<usize>();
    assert_eq!(
        implementations,
        1,
        "rolling z-score kernel has {implementations} implementations across \
         {} .rs files under src/; the single authority is series/kernel.rs::zscore",
        files.len()
    );

    // The shared authority is the only route: `Spec::Rolling(Rolling::Zscore, w)`
    // dispatches to that one implementation, which keeps its typed outcome
    // (undefined while warming up, `None` on a non-finite window).
    let kernel = fs::read_to_string(Path::new(manifest_dir).join("src/series/kernel.rs"))
        .unwrap_or_else(|error| panic!("census: reading src/series/kernel.rs: {error}"));
    assert!(
        kernel.contains("Rolling::Zscore => zscore(m, x)"),
        "shared authority no longer dispatches Rolling::Zscore to the single implementation"
    );
}
