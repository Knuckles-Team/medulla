//! `cargo xtask crate-size-budget`: no workspace crate holds more than 25,000 lines of Rust.
//!
//! A cold build compiles one crate as one unit, so a very large crate serializes the build.
//! The check counts the physical lines (`wc -l` semantics) of every `.rs` file under each
//! workspace member's `src/` and fails listing every member above the budget, with its count
//! and its largest module. Tests under `tests/`, `benches/` and `examples/` are not counted,
//! and `vendor/` is not a member. There is no allow-list, no baseline and no override.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

pub const NAME: &str = "crate-size-budget";
pub const SUMMARY: &str = "no workspace crate holds more than 25,000 lines of Rust under src/";

/// The most physical lines of Rust a crate may hold under `src/`.
pub const BUDGET_LINES: u64 = 25_000;

/// One member's size and its largest module.
pub struct Count {
    pub name: String,
    pub lines: u64,
    pub largest_module: String,
    pub largest_lines: u64,
}

/// Counts the newline-terminated lines of every `.rs` file under `dir`.
pub fn count_lines(dir: &Path) -> (u64, String, u64) {
    let mut total = 0;
    let mut largest = (String::new(), 0);
    for entry in WalkDir::new(dir).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !entry.file_type().is_file() || path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let lines = bytes.iter().filter(|b| **b == b'\n').count() as u64;
        total += lines;
        if lines > largest.1 {
            let rel = path.strip_prefix(dir).unwrap_or(path);
            largest = (rel.display().to_string(), lines);
        }
    }
    (total, largest.0, largest.1)
}

/// The members above the budget, largest first.
pub fn over_budget(counts: &[Count]) -> Vec<&Count> {
    let mut over: Vec<&Count> = counts.iter().filter(|c| c.lines > BUDGET_LINES).collect();
    over.sort_by(|a, b| b.lines.cmp(&a.lines).then_with(|| a.name.cmp(&b.name)));
    over
}

fn members(root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let out = Command::new("cargo")
        .args(["metadata", "--no-deps", "--locked", "--format-version", "1"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("CANNOT RUN: cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "CANNOT RUN: cargo metadata failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let meta: Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("CANNOT RUN: cargo metadata output: {e}"))?;
    let packages = meta["packages"]
        .as_array()
        .ok_or("CANNOT RUN: cargo metadata has no packages")?;
    let mut found: Vec<(String, PathBuf)> = packages
        .iter()
        .filter_map(|p| {
            let name = p["name"].as_str()?.to_owned();
            let manifest = PathBuf::from(p["manifest_path"].as_str()?);
            Some((name, manifest.parent()?.join("src")))
        })
        .collect();
    found.sort();
    Ok(found)
}

/// Checks the given `(crate name, src dir)` members against the budget.
fn check(members: &[(String, PathBuf)]) -> Result<usize, String> {
    if members.is_empty() {
        return Err("CANNOT RUN: the workspace lists no members".to_owned());
    }
    let counts: Vec<Count> = members
        .iter()
        .map(|(name, src)| {
            let (lines, largest_module, largest_lines) = count_lines(src);
            Count {
                name: name.clone(),
                lines,
                largest_module,
                largest_lines,
            }
        })
        .collect();
    let over = over_budget(&counts);
    if over.is_empty() {
        return Ok(counts.len());
    }
    let mut report = format!(
        "  CRATE_OVER_BUDGET: {} of {} crate(s) hold more than {BUDGET_LINES} lines under src/:",
        over.len(),
        counts.len()
    );
    for c in over {
        report.push_str(&format!(
            "\n  {}: {} lines; largest module {} ({} lines)",
            c.name, c.lines, c.largest_module, c.largest_lines
        ));
    }
    Err(report)
}

pub fn run(root: &Path, _args: &[String]) -> Result<(), String> {
    let members = members(root)?;
    let checked = check(&members)?;
    println!("crate-size-budget: PASS: {checked} crate(s) hold at most {BUDGET_LINES} lines");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempdir::Tree;

    fn lines(n: usize) -> String {
        "x\n".repeat(n)
    }

    // spec: MEDULLA-CONTRACT-R067.1
    #[test]
    fn a_crate_of_25001_lines_fails_and_25000_passes() {
        let big = lines(25_001);
        let exact = lines(25_000);
        let tree = Tree::new(&[
            ("big/src/lib.rs", &big),
            ("exact/src/lib.rs", &exact),
            // Not counted: tests, benches and examples sit outside src/.
            ("exact/tests/huge.rs", &big),
            ("exact/benches/huge.rs", &big),
        ]);
        let (total, _, _) = count_lines(&tree.path().join("exact/src"));
        assert_eq!(total, 25_000);
        let members = vec![
            ("big".to_owned(), tree.path().join("big/src")),
            ("exact".to_owned(), tree.path().join("exact/src")),
        ];
        let err = check(&members).expect_err("25,001 lines is over the budget");
        assert!(err.contains("CRATE_OVER_BUDGET"), "{err}");
        assert!(err.contains("big: 25001 lines"), "{err}");
        assert!(err.contains("lib.rs"), "{err}");
        assert!(!err.contains("exact:"), "{err}");
        assert_eq!(check(&members[1..]).expect("25,000 lines passes"), 1);
    }

    // spec: MEDULLA-CONTRACT-R067.14
    #[test]
    fn crate_size_budget_holds_for_this_repository() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("tools/xtask sits two levels below the workspace root");
        let members = members(root).expect("the workspace lists its members");
        check(&members).expect("every workspace crate holds at most 25,000 lines");
        let registered = crate::CHECKS
            .iter()
            .find(|c| c.name == NAME)
            .expect("crate-size-budget is a registered check");
        assert!(!registered.standalone, "`cargo xtask checks` must run it");
    }

    #[test]
    fn counts_every_rs_file_under_src_and_names_the_largest() {
        let a = lines(3);
        let b = lines(5);
        let tree = Tree::new(&[
            ("c/src/lib.rs", &a),
            ("c/src/sub/deep.rs", &b),
            ("c/src/notes.txt", &b),
        ]);
        let (total, largest, n) = count_lines(&tree.path().join("c/src"));
        assert_eq!((total, n), (8, 5));
        assert!(largest.ends_with("deep.rs"), "{largest}");
    }

    #[test]
    fn an_empty_member_list_cannot_pass() {
        assert!(check(&[]).is_err());
    }
}
