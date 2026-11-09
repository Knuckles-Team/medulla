//! Kernel census guard: exactly one implementation of each rehomed finance
//! kernel under `crates/medulla-numeric/src/` (MEDULLA-FINANCE-PRIMITIVES
//! R002.1). A second implementation of the same kernel — a duplicate
//! authority — fails here, and an empty census universe (no files scanned)
//! fails the same way, so the guard cannot go quietly blind.
//!
//! Rows delivered so far are listed in [`KERNELS`]; later kernel rows add
//! their markers to the same table.

use std::path::{Path, PathBuf};

/// (spec row, kernel, markers of the kernel's one implementation under `src/`).
const KERNELS: &[(&str, &str, &[&str])] = &[
    (
        "MEDULLA-FINANCE-PRIMITIVES-R002.1.1",
        "rolling z-score",
        &["fn zscore(", "Rolling::Zscore => zscore(m, x)"],
    ),
    (
        "MEDULLA-FINANCE-PRIMITIVES-R002.1.2",
        "EWMA",
        &["struct EwmaState", "impl EwmaState"],
    ),
];

// spec: MEDULLA-FINANCE-PRIMITIVES-R002.1.1, MEDULLA-FINANCE-PRIMITIVES-R002.1.2
#[test]
fn one_implementation_per_kernel() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src, &mut files);
    assert!(
        !files.is_empty(),
        "empty census universe: no .rs files under {} — the scan paths drifted",
        src.display()
    );
    for (row, name, markers) in KERNELS {
        for marker in *markers {
            let mut hits = Vec::new();
            for file in &files {
                if std::fs::read_to_string(file)
                    .map(|text| text.contains(marker))
                    .unwrap_or(false)
                {
                    hits.push(file.strip_prefix(&src).unwrap().display().to_string());
                }
            }
            assert_eq!(
                hits.len(),
                1,
                "{row} ({name}): marker {marker:?} hit {}/{} scanned files, exactly one \
                 implementation is required — 0 means the scan drifted or the kernel was \
                 deleted, more than 1 means a duplicate authority ({})",
                hits.len(),
                files.len(),
                hits.join(", ")
            );
        }
    }
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().map(|ext| ext == "rs").unwrap_or(false) {
            out.push(path);
        }
    }
}
